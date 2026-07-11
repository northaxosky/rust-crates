//! Bounded target output, target-backed COPY, and incremental Adler32

use std::cmp;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem;

use crate::error::{ByteRange, DecodeContext, DecodeError, IoOperation};

pub(crate) const IO_BUFFER_SIZE: usize = 64 * 1024;

/// Incremental buffered output over a readable seekable target
pub(crate) struct TargetWriter<'a, T> {
    target: &'a mut T,
    pending: Vec<u8>,
    scratch: Vec<u8>,
    logical_len: u64,
    physical_pos: u64,
    window: Option<u64>,
    context: DecodeContext,
    adler: Adler32,
}

impl<'a, T: Read + Write + Seek> TargetWriter<'a, T> {
    /// Create target output over an empty stream positioned at zero
    pub(crate) fn new(target: &'a mut T) -> Result<Self, DecodeError> {
        let context = DecodeContext::new(0, None, None);
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(IO_BUFFER_SIZE)
            .map_err(|_| DecodeError::AllocationFailed {
                requested: IO_BUFFER_SIZE as u64,
                context,
            })?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(IO_BUFFER_SIZE)
            .map_err(|_| DecodeError::AllocationFailed {
                requested: IO_BUFFER_SIZE as u64,
                context,
            })?;
        scratch.resize(IO_BUFFER_SIZE, 0);
        Ok(Self {
            target,
            pending,
            scratch,
            logical_len: 0,
            physical_pos: 0,
            window: None,
            context,
            adler: Adler32::new(),
        })
    }

    /// Start checksum accounting for one target window
    pub(crate) fn begin_window(&mut self, window: u64, context: DecodeContext) {
        self.window = Some(window);
        self.context = context;
        self.adler = Adler32::new();
    }

    /// Return the logical cumulative target length
    pub(crate) const fn len(&self) -> u64 {
        self.logical_len
    }

    /// Return the current window checksum
    pub(crate) const fn checksum(&self) -> u32 {
        self.adler.finish()
    }

    /// Persist buffered bytes after one window has passed validation
    pub(crate) fn finish_window(&mut self) -> Result<(), DecodeError> {
        self.drain_pending()
    }

    /// Buffer emitted bytes while updating the current checksum
    pub(crate) fn emit(
        &mut self,
        mut bytes: &[u8],
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        while !bytes.is_empty() {
            if self.pending.len() == IO_BUFFER_SIZE {
                self.drain_pending()?;
            }
            let count = cmp::min(IO_BUFFER_SIZE - self.pending.len(), bytes.len());
            let chunk = &bytes[..count];
            let new_len = self.logical_len.checked_add(count as u64).ok_or(
                DecodeError::ArithmeticOverflow {
                    context: self.context,
                },
            )?;
            self.adler.update(chunk);
            self.pending.extend_from_slice(chunk);
            self.logical_len = new_len;
            bytes = &bytes[count..];
        }
        Ok(())
    }

    /// Emit one byte repeatedly without allocating for the instruction size
    pub(crate) fn emit_run(
        &mut self,
        byte: u8,
        mut size: u64,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        self.scratch.fill(byte);
        while size > 0 {
            let count_u64 = size.min(IO_BUFFER_SIZE as u64);
            let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: count_u64,
                context,
            })?;
            self.emit_scratch(count, context)?;
            size -= count_u64;
        }
        Ok(())
    }

    /// Copy one validated source range with a single seek and bounded sequential reads
    pub(crate) fn copy_from_source<S: Read + Seek>(
        &mut self,
        source: &mut S,
        start: u64,
        size: u64,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        let range = ByteRange::new(start, size);
        let actual =
            source
                .seek(SeekFrom::Start(start))
                .map_err(|source| DecodeError::SourceIo {
                    operation: IoOperation::Seek,
                    range: Some(range),
                    window: self.window,
                    source,
                })?;
        if actual != start {
            return Err(DecodeError::SourceIo {
                operation: IoOperation::Seek,
                range: Some(range),
                window: self.window,
                source: wrong_seek_error("source"),
            });
        }

        let mut copied = 0_u64;
        while copied < size {
            let count_u64 = (size - copied).min(IO_BUFFER_SIZE as u64);
            let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: count_u64,
                context,
            })?;
            let chunk_start = start
                .checked_add(copied)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
            let mut scratch = mem::take(&mut self.scratch);
            let result = read_source_exact(
                source,
                &mut scratch[..count],
                ByteRange::new(chunk_start, count_u64),
                self.window,
            );
            self.scratch = scratch;
            result?;
            self.emit_scratch(count, context)?;
            copied = copied
                .checked_add(count_u64)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
        }
        Ok(())
    }

    /// Copy a validated prior-target range in bounded chunks
    pub(crate) fn copy_from_target(
        &mut self,
        start: u64,
        size: u64,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        let mut copied = 0_u64;
        while copied < size {
            let count_u64 = (size - copied).min(IO_BUFFER_SIZE as u64);
            let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: count_u64,
                context,
            })?;
            let chunk_start = start
                .checked_add(copied)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
            self.read_scratch(chunk_start, count, context)?;
            self.emit_scratch(count, context)?;
            copied = copied
                .checked_add(count_u64)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
        }
        Ok(())
    }

    /// Copy from the current target window with bounded overlap handling
    pub(crate) fn copy_from_current(
        &mut self,
        start: u64,
        size: u64,
        distance: u64,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        if size > distance && distance <= IO_BUFFER_SIZE as u64 {
            let seed_len =
                usize::try_from(distance).map_err(|_| DecodeError::PlatformSizeLimit {
                    value: distance,
                    context,
                })?;
            self.read_scratch(start, seed_len, context)?;
            return self.emit_repeated_scratch(seed_len, size, context);
        }

        let mut copied = 0_u64;
        while copied < size {
            let count_u64 = (size - copied).min(IO_BUFFER_SIZE as u64);
            let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: count_u64,
                context,
            })?;
            let chunk_start = start
                .checked_add(copied)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
            self.read_scratch(chunk_start, count, context)?;
            self.emit_scratch(count, context)?;
            copied = copied
                .checked_add(count_u64)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
        }
        Ok(())
    }

    /// Drain, flush, and leave the target at the decoded length
    pub(crate) fn finish(mut self) -> Result<(), DecodeError> {
        self.drain_pending()?;
        self.flush_target()?;
        self.seek_target(self.logical_len, None)
    }

    fn emit_scratch(&mut self, len: usize, context: DecodeContext) -> Result<(), DecodeError> {
        let scratch = mem::take(&mut self.scratch);
        let result = self.emit(&scratch[..len], context);
        self.scratch = scratch;
        result
    }

    fn emit_repeated_scratch(
        &mut self,
        seed_len: usize,
        size: u64,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        let mut emitted = 0_u64;
        while emitted < size {
            if self.pending.len() == IO_BUFFER_SIZE {
                self.drain_pending()?;
            }
            let available = IO_BUFFER_SIZE - self.pending.len();
            let count_u64 = (size - emitted).min(available as u64);
            let count = usize::try_from(count_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: count_u64,
                context,
            })?;
            let phase_u64 = emitted % seed_len as u64;
            let phase = usize::try_from(phase_u64).map_err(|_| DecodeError::PlatformSizeLimit {
                value: phase_u64,
                context,
            })?;
            let pending_start = self.pending.len();
            self.pending
                .extend((0..count).map(|index| self.scratch[(phase + index) % seed_len]));
            self.adler.update(&self.pending[pending_start..]);
            self.logical_len =
                self.logical_len
                    .checked_add(count_u64)
                    .ok_or(DecodeError::ArithmeticOverflow {
                        context: self.context,
                    })?;
            emitted = emitted
                .checked_add(count_u64)
                .ok_or(DecodeError::ArithmeticOverflow { context })?;
        }
        Ok(())
    }

    fn read_scratch(
        &mut self,
        start: u64,
        len: usize,
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        let mut scratch = mem::take(&mut self.scratch);
        let result = self.read_target(start, &mut scratch[..len], context);
        self.scratch = scratch;
        result
    }

    fn read_target(
        &mut self,
        start: u64,
        output: &mut [u8],
        context: DecodeContext,
    ) -> Result<(), DecodeError> {
        self.context = context;
        self.drain_pending()?;
        self.flush_target()?;
        let len = output.len() as u64;
        let range = ByteRange::new(start, len);
        if self.physical_pos != start {
            self.seek_target(start, Some(range))?;
        }

        let mut read = 0;
        while read < output.len() {
            match self.target.read(&mut output[read..]) {
                Ok(0) => {
                    return Err(DecodeError::TargetIo {
                        operation: IoOperation::Read,
                        range: Some(range),
                        window: self.window,
                        source: io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "target range ended early",
                        ),
                    });
                }
                Ok(count) => {
                    read += count;
                    self.physical_pos = self.physical_pos.checked_add(count as u64).ok_or(
                        DecodeError::ArithmeticOverflow {
                            context: self.context,
                        },
                    )?;
                }
                Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => {
                    return Err(DecodeError::TargetIo {
                        operation: IoOperation::Read,
                        range: Some(range),
                        window: self.window,
                        source,
                    });
                }
            }
        }
        if self.physical_pos != self.logical_len {
            self.seek_target(self.logical_len, Some(range))?;
        }
        Ok(())
    }

    fn drain_pending(&mut self) -> Result<(), DecodeError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending_len = self.pending.len() as u64;
        let start =
            self.logical_len
                .checked_sub(pending_len)
                .ok_or(DecodeError::ArithmeticOverflow {
                    context: self.context,
                })?;
        let range = ByteRange::new(start, pending_len);
        if self.physical_pos != start {
            self.seek_target(start, Some(range))?;
        }

        let mut written = 0;
        while written < self.pending.len() {
            match self.target.write(&self.pending[written..]) {
                Ok(0) => {
                    return Err(DecodeError::TargetIo {
                        operation: IoOperation::Write,
                        range: Some(range),
                        window: self.window,
                        source: io::Error::new(
                            io::ErrorKind::WriteZero,
                            "target accepted no bytes",
                        ),
                    });
                }
                Ok(count) => {
                    written += count;
                    self.physical_pos = self.physical_pos.checked_add(count as u64).ok_or(
                        DecodeError::ArithmeticOverflow {
                            context: self.context,
                        },
                    )?;
                }
                Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => {
                    return Err(DecodeError::TargetIo {
                        operation: IoOperation::Write,
                        range: Some(range),
                        window: self.window,
                        source,
                    });
                }
            }
        }
        self.pending.clear();
        Ok(())
    }

    fn flush_target(&mut self) -> Result<(), DecodeError> {
        self.target.flush().map_err(|source| DecodeError::TargetIo {
            operation: IoOperation::Flush,
            range: None,
            window: self.window,
            source,
        })
    }

    fn seek_target(&mut self, position: u64, range: Option<ByteRange>) -> Result<(), DecodeError> {
        let actual = self
            .target
            .seek(SeekFrom::Start(position))
            .map_err(|source| DecodeError::TargetIo {
                operation: IoOperation::Seek,
                range,
                window: self.window,
                source,
            })?;
        if actual != position {
            return Err(DecodeError::TargetIo {
                operation: IoOperation::Seek,
                range,
                window: self.window,
                source: wrong_seek_error("target"),
            });
        }
        self.physical_pos = position;
        Ok(())
    }
}

fn read_source_exact<S: Read>(
    source: &mut S,
    output: &mut [u8],
    range: ByteRange,
    window: Option<u64>,
) -> Result<(), DecodeError> {
    let mut read = 0;
    while read < output.len() {
        match source.read(&mut output[read..]) {
            Ok(0) => {
                return Err(DecodeError::SourceIo {
                    operation: IoOperation::Read,
                    range: Some(range),
                    window,
                    source: io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "source range ended early",
                    ),
                });
            }
            Ok(count) => read += count,
            Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(DecodeError::SourceIo {
                    operation: IoOperation::Read,
                    range: Some(range),
                    window,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn wrong_seek_error(stream: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{stream} seek returned wrong offset"),
    )
}

/// Incremental Adler32 state for one target window
#[derive(Clone, Copy)]
struct Adler32 {
    s1: u32,
    s2: u32,
}

impl Adler32 {
    const MODULUS: u64 = 65_521;
    const REDUCTION_CHUNK: usize = 5_552;

    const fn new() -> Self {
        Self { s1: 1, s2: 0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(Self::REDUCTION_CHUNK) {
            let mut s1 = u64::from(self.s1);
            let mut s2 = u64::from(self.s2);
            for &byte in chunk {
                s1 += u64::from(byte);
                s2 += s1;
            }
            self.s1 = (s1 % Self::MODULUS) as u32;
            self.s2 = (s2 % Self::MODULUS) as u32;
        }
    }

    const fn finish(self) -> u32 {
        (self.s2 << 16) | self.s1
    }
}

/// A fallibly growing in-memory target for the convenience API
pub(crate) struct FallibleMemoryTarget {
    bytes: Vec<u8>,
    pos: u64,
}

impl FallibleMemoryTarget {
    /// Create an empty in-memory target
    pub(crate) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            pos: 0,
        }
    }

    /// Consume the target and return its bytes
    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Read for FallibleMemoryTarget {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let Ok(start) = usize::try_from(self.pos) else {
            return Ok(0);
        };
        if start >= self.bytes.len() {
            return Ok(0);
        }
        let count = cmp::min(output.len(), self.bytes.len() - start);
        output[..count].copy_from_slice(&self.bytes[start..start + count]);
        self.pos = self
            .pos
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("memory target position overflow"))?;
        Ok(count)
    }
}

impl Write for FallibleMemoryTarget {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let input_len = u64::try_from(input.len())
            .map_err(|_| io::Error::other("memory target write exceeds u64"))?;
        let end = self
            .pos
            .checked_add(input_len)
            .ok_or_else(|| io::Error::other("memory target position overflow"))?;
        let start = usize::try_from(self.pos)
            .map_err(|_| io::Error::other("memory target position exceeds usize"))?;
        let end_index = usize::try_from(end)
            .map_err(|_| io::Error::other("memory target length exceeds usize"))?;
        if end_index > self.bytes.len() {
            let additional = end_index - self.bytes.len();
            self.bytes
                .try_reserve(additional)
                .map_err(io::Error::other)?;
            self.bytes.resize(end_index, 0);
        }
        self.bytes[start..end_index].copy_from_slice(input);
        self.pos = end;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for FallibleMemoryTarget {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(offset) => checked_signed_offset(self.bytes.len() as u64, offset),
            SeekFrom::Current(offset) => checked_signed_offset(self.pos, offset),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid memory target seek"))?;
        self.pos = next;
        Ok(next)
    }
}

fn checked_signed_offset(base: u64, offset: i64) -> Option<u64> {
    if offset >= 0 {
        base.checked_add(offset as u64)
    } else {
        base.checked_sub(offset.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_matches_known_values_incrementally() {
        let mut adler = Adler32::new();
        adler.update(b"a");
        adler.update(b"bc");
        assert_eq!(adler.finish(), 0x024D_0127);
    }

    #[test]
    fn memory_target_reads_writes_and_seeks() {
        let mut target = FallibleMemoryTarget::new();
        target.write_all(b"abcdef").unwrap();
        target.seek(SeekFrom::Start(2)).unwrap();
        let mut bytes = [0; 3];
        target.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"cde");
    }
}
