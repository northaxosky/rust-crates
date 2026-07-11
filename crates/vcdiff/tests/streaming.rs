//! Streaming API, bounded I/O, overlap, limit, checksum, and failure tests

mod common;

use std::error::Error as StdError;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use common::{Op, build, join_windows, varint};
use vcdiff_rs::{DecodeError, DecodeOptions, IoOperation, SectionKind, decode, decode_to};

const IO_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Default)]
struct IoStats {
    read_calls: usize,
    write_calls: usize,
    seek_calls: usize,
    flush_calls: usize,
    read_bytes: u64,
    write_bytes: u64,
    max_read_request: usize,
    max_write_request: usize,
}

#[derive(Default)]
struct FailAt {
    read: Option<usize>,
    write: Option<usize>,
    seek: Option<usize>,
    flush: Option<usize>,
}

struct TrackedIo {
    inner: Cursor<Vec<u8>>,
    stats: IoStats,
    fail: FailAt,
}

impl TrackedIo {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
            stats: IoStats::default(),
            fail: FailAt::default(),
        }
    }

    fn contents(&self) -> &[u8] {
        self.inner.get_ref()
    }
}

impl Read for TrackedIo {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.stats.read_calls += 1;
        self.stats.max_read_request = self.stats.max_read_request.max(output.len());
        if self.fail.read == Some(self.stats.read_calls) {
            return Err(injected("read"));
        }
        let count = self.inner.read(output)?;
        self.stats.read_bytes += count as u64;
        Ok(count)
    }
}

impl Write for TrackedIo {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.stats.write_calls += 1;
        self.stats.max_write_request = self.stats.max_write_request.max(input.len());
        if self.fail.write == Some(self.stats.write_calls) {
            return Err(injected("write"));
        }
        let count = self.inner.write(input)?;
        self.stats.write_bytes += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stats.flush_calls += 1;
        if self.fail.flush == Some(self.stats.flush_calls) {
            return Err(injected("flush"));
        }
        self.inner.flush()
    }
}

impl Seek for TrackedIo {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.stats.seek_calls += 1;
        if self.fail.seek == Some(self.stats.seek_calls) {
            return Err(injected("seek"));
        }
        self.inner.seek(position)
    }
}

fn injected(operation: &str) -> io::Error {
    io::Error::other(format!("injected {operation}"))
}

fn options_with_target_limit(limit: u64) -> DecodeOptions {
    let mut options = DecodeOptions::default();
    options.max_target_size = limit;
    options
}

#[allow(clippy::too_many_arguments)]
fn raw_delta(
    window_indicator: u8,
    segment: Option<(u64, u64)>,
    target_size: u64,
    delta_indicator: u8,
    data: &[u8],
    instructions: &[u8],
    addresses: &[u8],
    checksum: Option<u32>,
) -> Vec<u8> {
    let mut encoding = Vec::new();
    encoding.extend(varint(target_size));
    encoding.push(delta_indicator);
    encoding.extend(varint(data.len() as u64));
    encoding.extend(varint(instructions.len() as u64));
    encoding.extend(varint(addresses.len() as u64));
    if let Some(checksum) = checksum {
        encoding.extend(checksum.to_be_bytes());
    }
    encoding.extend_from_slice(data);
    encoding.extend_from_slice(instructions);
    encoding.extend_from_slice(addresses);

    let mut delta = vec![0xD6, 0xC3, 0xC4, 0x00, 0x00, window_indicator];
    if let Some((len, start)) = segment {
        delta.extend(varint(len));
        delta.extend(varint(start));
    }
    delta.extend(varint(encoding.len() as u64));
    delta.extend(encoding);
    delta
}

fn with_compressor(mut delta: Vec<u8>, compressor_id: u8) -> Vec<u8> {
    delta[4] = 0x01;
    delta.insert(5, compressor_id);
    delta
}

fn adler32(bytes: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    let mut s1 = 1_u32;
    let mut s2 = 0_u32;
    for &byte in bytes {
        s1 = (s1 + u32::from(byte)) % MODULUS;
        s2 = (s2 + s1) % MODULUS;
    }
    (s2 << 16) | s1
}

fn checksum_delta(checksum: u32) -> Vec<u8> {
    raw_delta(
        0x04,
        None,
        10,
        0,
        b"abc",
        &[1, 2, 0, 3, 19, 5],
        &[0],
        Some(checksum),
    )
}

fn declared_empty_window(target_size: u64) -> Vec<u8> {
    raw_delta(0, None, target_size, 0, &[], &[], &[], None)
}

fn assert_injected_source(error: &DecodeError, expected: &str) {
    let source = StdError::source(error).expect("I/O error should retain its source");
    assert_eq!(source.to_string(), format!("injected {expected}"));
}

#[test]
fn decode_to_matches_decode() {
    let source_bytes = b"source bytes".to_vec();
    let delta_bytes = build(
        source_bytes.len(),
        &[
            Op::Copy(0, 6),
            Op::Add(b" stream".to_vec()),
            Op::Run(b'!', 3),
        ],
    );
    let expected = decode(&source_bytes, &delta_bytes).unwrap();

    let mut source = Cursor::new(source_bytes);
    let mut delta = Cursor::new(delta_bytes);
    let mut target = Cursor::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();

    assert_eq!(target.into_inner(), expected);
}

#[test]
fn stream_positions_are_ignored_and_target_finishes_at_end() {
    let source_bytes = b"abcdef".to_vec();
    let delta_bytes = build(source_bytes.len(), &[Op::Copy(1, 4)]);
    let mut source = Cursor::new(source_bytes);
    let mut delta = Cursor::new(delta_bytes);
    let mut target = Cursor::new(Vec::new());
    source.set_position(5);
    delta.set_position(7);
    target.set_position(99);

    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();

    assert_eq!(target.get_ref(), b"bcde");
    assert_eq!(target.position(), 4);
}

#[test]
fn nonempty_target_is_rejected() {
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(build(0, &[Op::Add(b"x".to_vec())]));
    let mut target = Cursor::new(vec![9]);
    assert!(matches!(
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &DecodeOptions::default()
        ),
        Err(DecodeError::TargetNotEmpty { len: 1 })
    ));
}

#[test]
fn stream_target_limit_is_independent_of_wrapper_guard() {
    let oversized = declared_empty_window((1_u64 << 30) + 1);
    assert!(matches!(
        decode(b"", &oversized),
        Err(DecodeError::TargetSizeLimit {
            attempted: 1_073_741_825,
            limit: 1_073_741_824,
            ..
        })
    ));

    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(oversized);
    let mut target = Cursor::new(Vec::new());
    assert!(matches!(
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &DecodeOptions::default()
        ),
        Err(DecodeError::TargetSizeMismatch {
            expected: 1_073_741_825,
            actual: 0,
            ..
        })
    ));

    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(build(0, &[Op::Add(b"abc".to_vec())]));
    let mut target = Cursor::new(Vec::new());
    assert!(matches!(
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &options_with_target_limit(2)
        ),
        Err(DecodeError::TargetSizeLimit {
            attempted: 3,
            limit: 2,
            ..
        })
    ));
}

#[test]
fn wrapper_target_grows_incrementally_below_its_guard() {
    let output = decode(b"", &build(0, &[Op::Run(0xA5, 200 * 1024)])).unwrap();
    assert_eq!(output.len(), 200 * 1024);
    assert!(output.iter().all(|&byte| byte == 0xA5));
}

#[test]
fn incremental_adler32_accepts_and_rejects_streamed_output() {
    let expected = b"abcccabccc";
    let checksum = adler32(expected);
    let delta_bytes = checksum_delta(checksum);
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(delta_bytes);
    let mut target = Cursor::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(target.into_inner(), expected);

    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(checksum_delta(checksum ^ 1));
    let mut target = Cursor::new(Vec::new());
    assert!(matches!(
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &DecodeOptions::default()
        ),
        Err(DecodeError::ChecksumMismatch {
            window: 0,
            expected,
            actual,
            ..
        }) if expected == checksum ^ 1 && actual == checksum
    ));
}

#[test]
fn vcd_target_and_current_overlap_use_seekable_target() {
    let vcd_target = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x09, 0x03, 0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63,
        0x04, 0x02, 0x03, 0x00, 0x08, 0x03, 0x00, 0x00, 0x02, 0x01, 0x13, 0x03, 0x00,
    ];
    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(vcd_target.to_vec());
    let mut target = TrackedIo::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(target.contents(), b"abcabc");
    assert!(target.stats.read_calls > 0);
    assert!(target.stats.flush_calls > 0);

    let overlap = build(0, &[Op::Add(b"xy".to_vec()), Op::Copy(0, 10)]);
    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(overlap);
    let mut target = TrackedIo::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(target.contents(), b"xyxyxyxyxyxy");
    assert_eq!(target.stats.read_calls, 1);
}

#[test]
fn overlap_boundary_distances_are_correct_and_bounded() {
    for distance in [1_usize, 2, 63 * 1024, 64 * 1024, 64 * 1024 + 1] {
        let seed: Vec<u8> = (0..distance).map(|index| (index % 251) as u8).collect();
        let copy_size = distance as u64 + 12_345;
        let delta_bytes = build(0, &[Op::Add(seed.clone()), Op::Copy(0, copy_size)]);
        let mut expected = seed.clone();
        expected.extend((0..copy_size as usize).map(|index| seed[index % distance]));

        let mut source = TrackedIo::new(Vec::new());
        let mut delta = TrackedIo::new(delta_bytes);
        let mut target = TrackedIo::new(Vec::new());
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &DecodeOptions::default(),
        )
        .unwrap();

        if target.contents() != expected {
            let mismatch = target
                .contents()
                .iter()
                .zip(&expected)
                .position(|(actual, expected)| actual != expected);
            panic!(
                "distance {distance} mismatch at {mismatch:?}, lengths {} and {}",
                target.contents().len(),
                expected.len()
            );
        }
        assert!(target.stats.max_read_request <= IO_BUFFER_SIZE);
        assert!(target.stats.max_write_request <= IO_BUFFER_SIZE);
        if distance <= IO_BUFFER_SIZE {
            assert_eq!(target.stats.read_calls, 1, "distance {distance}");
        }
    }
}

#[test]
fn large_distance_dependent_overlap_is_chunked() {
    let distance = 100 * 1024;
    let copy_size = 1024 * 1024_u64;
    let seed: Vec<u8> = (0..distance).map(|index| (index % 239) as u8).collect();
    let delta_bytes = build(0, &[Op::Add(seed.clone()), Op::Copy(0, copy_size)]);
    let mut expected = seed.clone();
    expected.extend((0..copy_size as usize).map(|index| seed[index % distance]));

    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(delta_bytes);
    let mut target = TrackedIo::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();

    if target.contents() != expected {
        let mismatch = target
            .contents()
            .iter()
            .zip(&expected)
            .position(|(actual, expected)| actual != expected);
        panic!(
            "large overlap mismatch at {mismatch:?}, lengths {} and {}",
            target.contents().len(),
            expected.len()
        );
    }
    assert!(target.stats.read_calls > 1);
    assert!(target.stats.read_calls <= 20);
    assert!(target.stats.seek_calls <= 40);
    assert!(target.stats.max_read_request <= IO_BUFFER_SIZE);
    assert!(target.stats.max_write_request <= IO_BUFFER_SIZE);
}

#[test]
fn active_window_memory_limit_precedes_section_allocation() {
    let delta_bytes = build(0, &[Op::Add(vec![7; 1024])]);
    let mut options = DecodeOptions::default();
    options.max_window_memory = 100;
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(delta_bytes);
    let mut target = Cursor::new(Vec::new());

    assert!(matches!(
        decode_to(&mut source, &mut delta, &mut target, &options),
        Err(DecodeError::WindowMemoryLimit {
            attempted,
            limit: 100,
            ..
        }) if attempted >= 1024
    ));
    assert!(target.get_ref().is_empty());
}

#[test]
fn output_limit_after_a_completed_window_keeps_prior_output() {
    let first = build(0, &[Op::Add(b"abc".to_vec())]);
    let second = build(0, &[Op::Add(b"def".to_vec())]);
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(join_windows(&[first, second]));
    let mut target = Cursor::new(Vec::new());

    assert!(matches!(
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &options_with_target_limit(5)
        ),
        Err(DecodeError::TargetSizeLimit {
            attempted: 6,
            limit: 5,
            ..
        })
    ));
    assert_eq!(target.get_ref(), b"abc");
}

#[test]
fn app_header_is_skipped_without_reading_its_payload() {
    let app_len = 2 * 1024 * 1024;
    let window = build(0, &[Op::Add(b"ok".to_vec())]);
    let mut delta_bytes = vec![0xD6, 0xC3, 0xC4, 0x00, 0x04];
    delta_bytes.extend(varint(app_len));
    delta_bytes.resize(delta_bytes.len() + app_len as usize, 0xA5);
    delta_bytes.extend_from_slice(&window[5..]);

    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(delta_bytes);
    let mut target = TrackedIo::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();

    assert_eq!(target.contents(), b"ok");
    assert!(delta.stats.read_bytes < 128 * 1024);
    assert!(delta.stats.seek_calls >= 3);
}

#[test]
fn input_and_output_requests_are_bounded_and_source_is_not_prefetched() {
    let source_bytes: Vec<u8> = (0..256 * 1024).map(|index| (index % 251) as u8).collect();
    let literal = vec![0x5A; 200 * 1024];
    let delta_bytes = build(
        source_bytes.len(),
        &[Op::Copy(100, 70 * 1024), Op::Add(literal)],
    );
    let mut source = TrackedIo::new(source_bytes);
    let mut delta = TrackedIo::new(delta_bytes);
    let mut target = TrackedIo::new(Vec::new());

    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();

    assert_eq!(source.stats.read_bytes, 70 * 1024);
    assert!(source.stats.read_bytes < source.contents().len() as u64);
    assert!(source.stats.max_read_request <= IO_BUFFER_SIZE);
    assert!(delta.stats.max_read_request <= IO_BUFFER_SIZE);
    assert!(target.stats.max_write_request <= IO_BUFFER_SIZE);
}

#[test]
fn compressor_and_reserved_flag_errors_are_specific() {
    let raw = build(0, &[Op::Add(b"x".to_vec())]);
    assert_eq!(decode(b"", &with_compressor(raw.clone(), 2)).unwrap(), b"x");

    assert!(matches!(
        decode(b"", &[0xD6, 0xC3, 0xC4, 0x00, 0x01, 17]),
        Err(DecodeError::UnsupportedSecondaryCompressor { id: 17, .. })
    ));

    for (bit, expected_section) in [
        (0x01, SectionKind::Data),
        (0x02, SectionKind::Instructions),
        (0x04, SectionKind::Addresses),
    ] {
        let mut compressed = raw.clone();
        compressed[8] = bit;
        match decode(b"", &compressed) {
            Err(DecodeError::CompressedSectionWithoutCompressor { section, .. }) => {
                assert_eq!(section, expected_section);
            }
            result => panic!("unexpected compression result: {result:?}"),
        }
        match decode(b"", &with_compressor(compressed, 2)) {
            Err(
                DecodeError::SecondaryDecompression { context, .. }
                | DecodeError::InvalidSecondarySize { context, .. }
                | DecodeError::SecondarySizeMismatch { context, .. }
                | DecodeError::SecondaryInputMismatch { context, .. }
                | DecodeError::MalformedSecondarySection { context, .. },
            ) => assert_eq!(context.section, Some(expected_section)),
            result => panic!("unexpected compressor result: {result:?}"),
        }
    }

    assert!(matches!(
        decode(b"", &[0xD6, 0xC3, 0xC4, 0x00, 0x08]),
        Err(DecodeError::InvalidHeaderIndicator {
            indicator: 0x08,
            ..
        })
    ));
    assert!(matches!(
        decode(b"", &[0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x08]),
        Err(DecodeError::InvalidWindowIndicator {
            indicator: 0x08,
            ..
        })
    ));
    assert!(matches!(
        decode(b"", &[0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x03]),
        Err(DecodeError::InvalidWindowIndicator {
            indicator: 0x03,
            ..
        })
    ));
    let invalid_delta = raw_delta(0, None, 0, 0x08, &[], &[], &[], None);
    assert!(matches!(
        decode(b"", &invalid_delta),
        Err(DecodeError::InvalidDeltaIndicator {
            indicator: 0x08,
            ..
        })
    ));
}

#[test]
fn source_failures_retain_operation_and_origin() {
    let delta_bytes = build(4, &[Op::Copy(0, 4)]);
    let mut source = TrackedIo::new(b"abcd".to_vec());
    source.fail.read = Some(1);
    let mut delta = TrackedIo::new(delta_bytes.clone());
    let mut target = TrackedIo::new(Vec::new());
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::SourceIo {
            operation: IoOperation::Read,
            ..
        }
    ));
    assert_injected_source(&error, "read");

    let mut source = TrackedIo::new(b"abcd".to_vec());
    source.fail.seek = Some(3);
    let mut delta = TrackedIo::new(delta_bytes);
    let mut target = TrackedIo::new(Vec::new());
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::SourceIo {
            operation: IoOperation::Seek,
            ..
        }
    ));
    assert_injected_source(&error, "seek");
}

#[test]
fn delta_failures_retain_operation_and_origin() {
    let delta_bytes = build(0, &[Op::Add(b"x".to_vec())]);
    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(delta_bytes.clone());
    delta.fail.read = Some(1);
    let mut target = TrackedIo::new(Vec::new());
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::DeltaIo {
            operation: IoOperation::Read,
            ..
        }
    ));
    assert_injected_source(&error, "read");

    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(delta_bytes);
    delta.fail.seek = Some(1);
    let mut target = TrackedIo::new(Vec::new());
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::DeltaIo {
            operation: IoOperation::Length,
            ..
        }
    ));
    assert_injected_source(&error, "seek");
}

#[test]
fn target_failures_retain_operation_and_origin() {
    let add = build(0, &[Op::Add(b"x".to_vec())]);
    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(add.clone());
    let mut target = TrackedIo::new(Vec::new());
    target.fail.write = Some(1);
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::TargetIo {
            operation: IoOperation::Write,
            ..
        }
    ));
    assert_injected_source(&error, "write");

    let overlap = build(0, &[Op::Add(b"x".to_vec()), Op::Copy(0, 3)]);
    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(overlap.clone());
    let mut target = TrackedIo::new(Vec::new());
    target.fail.read = Some(1);
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::TargetIo {
            operation: IoOperation::Read,
            ..
        }
    ));
    assert_injected_source(&error, "read");

    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(overlap);
    let mut target = TrackedIo::new(Vec::new());
    target.fail.seek = Some(3);
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::TargetIo {
            operation: IoOperation::Seek,
            ..
        }
    ));
    assert_injected_source(&error, "seek");

    let mut source = TrackedIo::new(Vec::new());
    let mut delta = TrackedIo::new(add);
    let mut target = TrackedIo::new(Vec::new());
    target.fail.flush = Some(1);
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DecodeError::TargetIo {
            operation: IoOperation::Flush,
            ..
        }
    ));
    assert_injected_source(&error, "flush");
}
