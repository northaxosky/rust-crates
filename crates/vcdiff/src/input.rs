//! Bounded streaming delta input and section cursors

use std::cmp;
use std::io::{self, Read, Seek, SeekFrom};

use crate::error::{DecodeContext, DecodeError, IoOperation, SectionKind};

const INPUT_BUFFER_SIZE: usize = 64 * 1024;

/// A fixed-buffer streaming reader with absolute delta offsets
pub(crate) struct DeltaInput<'a, D> {
    reader: &'a mut D,
    len: u64,
    pos: u64,
    buffer: Vec<u8>,
    buffer_pos: usize,
    buffer_len: usize,
    window: Option<u64>,
    section: Option<SectionKind>,
}

impl<'a, D: Read + Seek> DeltaInput<'a, D> {
    /// Create a reader over a measured stream positioned at zero
    pub(crate) fn new(reader: &'a mut D, len: u64) -> Result<Self, DecodeError> {
        let context = DecodeContext::new(0, None, None);
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(INPUT_BUFFER_SIZE)
            .map_err(|_| DecodeError::AllocationFailed {
                requested: INPUT_BUFFER_SIZE as u64,
                context,
            })?;
        buffer.resize(INPUT_BUFFER_SIZE, 0);
        Ok(Self {
            reader,
            len,
            pos: 0,
            buffer,
            buffer_pos: 0,
            buffer_len: 0,
            window: None,
            section: None,
        })
    }

    /// Set the active target-window index
    pub(crate) fn set_window(&mut self, window: Option<u64>) {
        self.window = window;
    }

    /// Set the active VCDIFF section
    pub(crate) fn set_section(&mut self, section: Option<SectionKind>) {
        self.section = section;
    }

    /// Return the current absolute delta offset
    pub(crate) const fn position(&self) -> u64 {
        self.pos
    }

    /// Return the measured delta length
    pub(crate) const fn len(&self) -> u64 {
        self.len
    }

    /// Whether the complete measured stream has been consumed
    pub(crate) fn is_empty(&self) -> bool {
        self.pos == self.len
    }

    /// Build context at the current absolute offset
    pub(crate) const fn context(&self) -> DecodeContext {
        DecodeContext::new(self.pos, self.window, self.section)
    }

    /// Read one byte
    pub(crate) fn read_u8(&mut self) -> Result<u8, DecodeError> {
        if self.buffer_pos == self.buffer_len {
            self.fill()?;
        }
        let byte = self.buffer[self.buffer_pos];
        self.buffer_pos += 1;
        self.pos = self
            .pos
            .checked_add(1)
            .ok_or_else(|| DecodeError::ArithmeticOverflow {
                context: self.context(),
            })?;
        Ok(byte)
    }

    /// Read a big-endian four-byte integer
    pub(crate) fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        let mut bytes = [0; 4];
        self.read_exact(&mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    /// Decode an RFC 3284 base-128 big-endian integer
    pub(crate) fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let start = self.context();
        let mut value = 0_u64;
        for _ in 0..10 {
            let byte = self.read_u8()?;
            value = value
                .checked_mul(128)
                .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
                .ok_or(DecodeError::VarintOverflow { context: start })?;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::VarintOverflow { context: start })
    }

    /// Read exactly the requested bytes without crossing the measured stream end
    pub(crate) fn read_exact(&mut self, output: &mut [u8]) -> Result<(), DecodeError> {
        let output_len =
            u64::try_from(output.len()).map_err(|_| DecodeError::PlatformSizeLimit {
                value: u64::MAX,
                context: self.context(),
            })?;
        let end =
            self.pos
                .checked_add(output_len)
                .ok_or_else(|| DecodeError::ArithmeticOverflow {
                    context: self.context(),
                })?;
        if end > self.len {
            return Err(DecodeError::TruncatedDelta {
                context: self.context(),
            });
        }

        let mut written = 0;
        while written < output.len() {
            if self.buffer_pos == self.buffer_len {
                self.fill()?;
            }
            let available = self.buffer_len - self.buffer_pos;
            let count = cmp::min(available, output.len() - written);
            output[written..written + count]
                .copy_from_slice(&self.buffer[self.buffer_pos..self.buffer_pos + count]);
            self.buffer_pos += count;
            written += count;
            self.pos = self.pos.checked_add(count as u64).ok_or_else(|| {
                DecodeError::ArithmeticOverflow {
                    context: self.context(),
                }
            })?;
        }
        Ok(())
    }

    /// Skip bytes with an absolute seek and no payload allocation
    pub(crate) fn skip(&mut self, len: u64) -> Result<(), DecodeError> {
        let context = self.context();
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        if end > self.len {
            return Err(DecodeError::TruncatedDelta { context });
        }
        let actual =
            self.reader
                .seek(SeekFrom::Start(end))
                .map_err(|source| DecodeError::DeltaIo {
                    operation: IoOperation::Seek,
                    context,
                    source,
                })?;
        if actual != end {
            return Err(DecodeError::DeltaIo {
                operation: IoOperation::Seek,
                context,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "delta seek returned wrong offset",
                ),
            });
        }
        self.pos = end;
        self.buffer_pos = 0;
        self.buffer_len = 0;
        Ok(())
    }

    fn fill(&mut self) -> Result<(), DecodeError> {
        if self.pos >= self.len {
            return Err(DecodeError::TruncatedDelta {
                context: self.context(),
            });
        }
        let remaining = self.len - self.pos;
        let request_u64 = remaining.min(INPUT_BUFFER_SIZE as u64);
        let request = usize::try_from(request_u64).map_err(|_| DecodeError::PlatformSizeLimit {
            value: request_u64,
            context: self.context(),
        })?;
        loop {
            match self.reader.read(&mut self.buffer[..request]) {
                Ok(0) => {
                    return Err(DecodeError::TruncatedDelta {
                        context: self.context(),
                    });
                }
                Ok(count) => {
                    self.buffer_pos = 0;
                    self.buffer_len = count;
                    return Ok(());
                }
                Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => {
                    return Err(DecodeError::DeltaIo {
                        operation: IoOperation::Read,
                        context: self.context(),
                        source,
                    });
                }
            }
        }
    }
}

/// A u64-positioned cursor over one resident window section
pub(crate) struct SliceCursor<'a> {
    bytes: &'a [u8],
    pos: u64,
    base_offset: u64,
    window: u64,
    section: SectionKind,
}

impl<'a> SliceCursor<'a> {
    /// Wrap one resident section
    pub(crate) const fn new(
        bytes: &'a [u8],
        base_offset: u64,
        window: u64,
        section: SectionKind,
    ) -> Self {
        Self {
            bytes,
            pos: 0,
            base_offset,
            window,
            section,
        }
    }

    /// Whether every section byte has been consumed
    pub(crate) fn is_empty(&self) -> bool {
        self.pos == self.bytes.len() as u64
    }

    /// Number of unconsumed section bytes
    pub(crate) fn remaining(&self) -> u64 {
        self.bytes.len() as u64 - self.pos
    }

    /// Build context at the current section position
    pub(crate) fn context(&self) -> DecodeContext {
        DecodeContext::new(
            self.base_offset.saturating_add(self.pos),
            Some(self.window),
            Some(self.section),
        )
    }

    /// Read one section byte
    pub(crate) fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_slice(1)?[0])
    }

    /// Borrow and advance over the requested section bytes
    pub(crate) fn read_slice(&mut self, len: u64) -> Result<&'a [u8], DecodeError> {
        let context = self.context();
        let remaining = self.remaining();
        if len > remaining {
            return Err(DecodeError::SectionOutOfBounds {
                section: self.section,
                requested: len,
                remaining,
                context,
            });
        }
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::ArithmeticOverflow { context })?;
        let start_index =
            usize::try_from(self.pos).map_err(|_| DecodeError::PlatformSizeLimit {
                value: self.pos,
                context,
            })?;
        let end_index = usize::try_from(end).map_err(|_| DecodeError::PlatformSizeLimit {
            value: end,
            context,
        })?;
        self.pos = end;
        Ok(&self.bytes[start_index..end_index])
    }

    /// Decode an RFC 3284 base-128 big-endian integer
    pub(crate) fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let context = self.context();
        let mut value = 0_u64;
        for _ in 0..10 {
            let byte = self.read_u8()?;
            value = value
                .checked_mul(128)
                .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
                .ok_or(DecodeError::VarintOverflow { context })?;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::VarintOverflow { context })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn delta_input_reads_and_skips_with_absolute_offsets() {
        let mut stream = Cursor::new(vec![1, 2, 3, 4, 5, 6]);
        let mut input = DeltaInput::new(&mut stream, 6).unwrap();
        assert_eq!(input.read_u8().unwrap(), 1);
        input.skip(3).unwrap();
        assert_eq!(input.position(), 4);
        let mut tail = [0; 2];
        input.read_exact(&mut tail).unwrap();
        assert_eq!(tail, [5, 6]);
        assert!(input.is_empty());
    }

    #[test]
    fn slice_cursor_decodes_varints() {
        let mut cursor =
            SliceCursor::new(&[0xBA, 0xEF, 0x9A, 0x15], 20, 2, SectionKind::Instructions);
        assert_eq!(cursor.read_varint().unwrap(), 123_456_789);
        assert!(cursor.is_empty());
    }

    #[test]
    fn slice_cursor_reports_section_exhaustion() {
        let mut cursor = SliceCursor::new(&[1], 20, 2, SectionKind::Data);
        assert!(matches!(
            cursor.read_slice(2),
            Err(DecodeError::SectionOutOfBounds {
                section: SectionKind::Data,
                requested: 2,
                remaining: 1,
                ..
            })
        ));
    }
}
