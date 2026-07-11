//! Ignored bounded-I/O acceptance test for multi-gigabyte targets

use std::env;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Instant;

use vcdiff_rs::{DecodeOptions, decode_to};

const WINDOW_SIZE: u64 = 524_288;
const LARGE_WINDOWS: u64 = 4_097;
const MAX_IO_SIZE: usize = 64 * 1024;

struct VirtualZeroSource {
    len: u64,
    pos: u64,
    read_calls: u64,
    seek_calls: u64,
    bytes_read: u64,
    max_read_request: usize,
}

impl VirtualZeroSource {
    const fn new(len: u64) -> Self {
        Self {
            len,
            pos: 0,
            read_calls: 0,
            seek_calls: 0,
            bytes_read: 0,
            max_read_request: 0,
        }
    }

    const fn retained_payload_bytes(&self) -> usize {
        0
    }
}

impl Read for VirtualZeroSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.read_calls += 1;
        self.max_read_request = self.max_read_request.max(output.len());
        let remaining = self.len.saturating_sub(self.pos);
        let count = usize::try_from(remaining.min(output.len() as u64))
            .map_err(|_| io::Error::other("source read exceeds usize"))?;
        output[..count].fill(0);
        self.pos = self
            .pos
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("source position overflow"))?;
        self.bytes_read = self
            .bytes_read
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("source byte count overflow"))?;
        Ok(count)
    }
}

impl Seek for VirtualZeroSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.seek_calls += 1;
        self.pos = seek_position(self.len, self.pos, position)?;
        Ok(self.pos)
    }
}

struct CountingZeroTarget {
    len: u64,
    pos: u64,
    read_calls: u64,
    write_calls: u64,
    seek_calls: u64,
    flush_calls: u64,
    bytes_written: u64,
    max_read_request: usize,
    max_write_request: usize,
}

impl CountingZeroTarget {
    const fn new() -> Self {
        Self {
            len: 0,
            pos: 0,
            read_calls: 0,
            write_calls: 0,
            seek_calls: 0,
            flush_calls: 0,
            bytes_written: 0,
            max_read_request: 0,
            max_write_request: 0,
        }
    }

    const fn retained_payload_bytes(&self) -> usize {
        0
    }
}

impl Read for CountingZeroTarget {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.read_calls += 1;
        self.max_read_request = self.max_read_request.max(output.len());
        let remaining = self.len.saturating_sub(self.pos);
        let count = usize::try_from(remaining.min(output.len() as u64))
            .map_err(|_| io::Error::other("target read exceeds usize"))?;
        output[..count].fill(0);
        self.pos = self
            .pos
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("target read position overflow"))?;
        Ok(count)
    }
}

impl Write for CountingZeroTarget {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.write_calls += 1;
        self.max_write_request = self.max_write_request.max(input.len());
        if input.iter().any(|&byte| byte != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "target received a nonzero byte",
            ));
        }
        let count = input.len() as u64;
        self.pos = self
            .pos
            .checked_add(count)
            .ok_or_else(|| io::Error::other("target write position overflow"))?;
        self.len = self.len.max(self.pos);
        self.bytes_written = self
            .bytes_written
            .checked_add(count)
            .ok_or_else(|| io::Error::other("target byte count overflow"))?;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_calls += 1;
        Ok(())
    }
}

impl Seek for CountingZeroTarget {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.seek_calls += 1;
        self.pos = seek_position(self.len, self.pos, position)?;
        Ok(self.pos)
    }
}

fn seek_position(len: u64, current: u64, position: SeekFrom) -> io::Result<u64> {
    let next = match position {
        SeekFrom::Start(position) => Some(position),
        SeekFrom::End(offset) => signed_offset(len, offset),
        SeekFrom::Current(offset) => signed_offset(current, offset),
    };
    next.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek"))
}

fn signed_offset(base: u64, offset: i64) -> Option<u64> {
    if offset >= 0 {
        base.checked_add(offset as u64)
    } else {
        base.checked_sub(offset.unsigned_abs())
    }
}

fn append_varint(mut value: u64, output: &mut Vec<u8>) {
    let mut encoded = [0_u8; 10];
    let mut index = encoded.len();
    index -= 1;
    encoded[index] = (value & 0x7f) as u8;
    value >>= 7;
    while value != 0 {
        index -= 1;
        encoded[index] = 0x80 | (value & 0x7f) as u8;
        value >>= 7;
    }
    output.extend_from_slice(&encoded[index..]);
}

fn build_delta(windows: u64) -> Vec<u8> {
    let mut instructions = vec![19];
    append_varint(WINDOW_SIZE, &mut instructions);

    let mut encoding = Vec::new();
    append_varint(WINDOW_SIZE, &mut encoding);
    encoding.push(0);
    append_varint(0, &mut encoding);
    append_varint(instructions.len() as u64, &mut encoding);
    append_varint(1, &mut encoding);
    encoding.extend_from_slice(&instructions);
    encoding.push(0);

    let mut delta = vec![0xD6, 0xC3, 0xC4, 0, 0];
    for window in 0..windows {
        delta.push(1);
        append_varint(WINDOW_SIZE, &mut delta);
        append_varint(window * WINDOW_SIZE, &mut delta);
        append_varint(encoding.len() as u64, &mut delta);
        delta.extend_from_slice(&encoding);
    }
    delta
}

fn configured_windows() -> u64 {
    env::var("VCDIFF_STRESS_WINDOWS")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("VCDIFF_STRESS_WINDOWS must be a positive integer")
        })
        .unwrap_or(LARGE_WINDOWS)
}

#[test]
#[ignore = "release-only 2+ GiB bounded-I/O acceptance"]
fn stress_over_2gib() {
    let windows = configured_windows();
    assert!(windows > 0);
    let target_size = windows.checked_mul(WINDOW_SIZE).unwrap();
    let delta_bytes = build_delta(windows);
    assert!(delta_bytes.len() < 256 * 1024);

    let mut source = VirtualZeroSource::new(target_size);
    let mut delta = io::Cursor::new(delta_bytes);
    let mut target = CountingZeroTarget::new();
    let mut options = DecodeOptions::default();
    options.max_target_size = target_size;
    let started = Instant::now();
    decode_to(&mut source, &mut delta, &mut target, &options).unwrap();
    let elapsed = started.elapsed();

    let expected_io_calls = windows * (WINDOW_SIZE / MAX_IO_SIZE as u64);
    assert_eq!(target_size, windows * WINDOW_SIZE);
    assert_eq!(target.len, target_size);
    assert_eq!(target.pos, target_size);
    assert_eq!(source.bytes_read, target_size);
    assert_eq!(target.bytes_written, target_size);
    assert_eq!(source.read_calls, expected_io_calls);
    assert_eq!(target.write_calls, expected_io_calls);
    assert_eq!(source.seek_calls, windows + 2);
    assert_eq!(target.seek_calls, 3);
    assert_eq!(target.read_calls, 0);
    assert_eq!(target.flush_calls, 1);
    assert!(source.max_read_request <= MAX_IO_SIZE);
    assert!(target.max_write_request <= MAX_IO_SIZE);
    assert_eq!(source.retained_payload_bytes(), 0);
    assert_eq!(target.retained_payload_bytes(), 0);
    println!(
        "stress windows={windows} target_bytes={target_size} delta_bytes={} duration_ms={} source_reads={} target_writes={} peak_request={}",
        delta.get_ref().len(),
        elapsed.as_millis(),
        source.read_calls,
        target.write_calls,
        source.max_read_request.max(target.max_write_request)
    );
}
