//! End-to-end decode tests: raw golden vectors, builder round-trips, error paths, and properties.

mod common;

use common::{Op, build, join_windows};
use proptest::prelude::*;
use vcdiff_rs::{DecodeError, decode};

// Golden vectors below are hand-encoded from the RFC 3284 grammar and the default code table, so they
// validate the decoder against externally-specified bytes rather than our own encoder.

#[test]
fn golden_add_only() {
    // header, no-source window, ADD size 3 of "abc"
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x09, 0x03, 0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63,
        0x04,
    ];
    assert_eq!(decode(b"", &delta).unwrap(), b"abc");
}

#[test]
fn golden_run_only() {
    // RUN of 'z' four times
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x08, 0x04, 0x00, 0x01, 0x02, 0x00, 0x7A, 0x00, 0x04,
    ];
    assert_eq!(decode(b"", &delta).unwrap(), b"zzzz");
}

#[test]
fn golden_copy_from_source() {
    // VCD_SOURCE window copying the whole 3-byte source
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x01, 0x03, 0x00, 0x08, 0x03, 0x00, 0x00, 0x02, 0x01, 0x13,
        0x03, 0x00,
    ];
    assert_eq!(decode(b"abc", &delta).unwrap(), b"abc");
}

#[test]
fn golden_overlapping_copy() {
    // ADD 'a' then COPY 5 via HERE mode overlapping the just-written byte -> "aaaaaa"
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x08, 0x06, 0x00, 0x01, 0x01, 0x01, 0x61, 0xB0, 0x01,
    ];
    assert_eq!(decode(b"", &delta).unwrap(), b"aaaaaa");
}

#[test]
fn golden_adler32_verified() {
    // VCD_ADLER32 window with the correct checksum of "abc" (0x024D0127)
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x04, 0x0D, 0x03, 0x00, 0x03, 0x01, 0x00, 0x02, 0x4D, 0x01,
        0x27, 0x61, 0x62, 0x63, 0x04,
    ];
    assert_eq!(decode(b"", &delta).unwrap(), b"abc");
}

#[test]
fn golden_adler32_mismatch_is_rejected() {
    // Same window with a corrupted checksum byte
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x04, 0x0D, 0x03, 0x00, 0x03, 0x01, 0x00, 0x02, 0x4D, 0x01,
        0x28, 0x61, 0x62, 0x63, 0x04,
    ];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::ChecksumMismatch)
    ));
}

#[test]
fn golden_vcd_target_window() {
    // Window 1 writes "abc"; window 2 copies it from a VCD_TARGET segment -> "abcabc"
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, // header
        0x00, 0x09, 0x03, 0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63, 0x04, // window 1
        0x02, 0x03, 0x00, 0x08, 0x03, 0x00, 0x00, 0x02, 0x01, 0x13, 0x03, 0x00, // window 2
    ];
    assert_eq!(decode(b"", &delta).unwrap(), b"abcabc");
}

#[test]
fn builder_add_run_copy_round_trip() {
    let source = b"Hello, world!";
    let ops = vec![
        Op::Copy(0, 5),              // "Hello" from source
        Op::Add(b" there".to_vec()), // literal
        Op::Run(b'!', 3),            // "!!!"
        Op::Copy(5, 8),              // ", world!" from source
    ];
    let decoded = decode(source, &build(source.len(), &ops)).unwrap();
    assert_eq!(decoded, b"Hello there!!!, world!");
}

#[test]
fn builder_overlapping_run_via_copy() {
    // No source: ADD one byte, then COPY it forward to make a run
    let ops = vec![Op::Add(b"x".to_vec()), Op::Copy(0, 7)];
    assert_eq!(decode(b"", &build(0, &ops)).unwrap(), b"xxxxxxxx");
}

#[test]
fn builder_multi_window() {
    let w1 = build(0, &[Op::Add(b"ab".to_vec())]);
    let w2 = build(0, &[Op::Add(b"cd".to_vec())]);
    assert_eq!(decode(b"", &join_windows(&[w1, w2])).unwrap(), b"abcd");
}

#[test]
fn empty_target_is_valid() {
    let delta = build(0, &[]);
    assert_eq!(decode(b"", &delta).unwrap(), b"");
}

#[test]
fn bad_magic_is_rejected() {
    assert!(matches!(decode(b"", b"XXXX"), Err(DecodeError::BadMagic)));
}

#[test]
fn truncated_header_is_rejected() {
    assert!(matches!(
        decode(b"", &[0xD6, 0xC3]),
        Err(DecodeError::UnexpectedEof)
    ));
}

#[test]
fn unsupported_version_is_rejected() {
    let delta = [0xD6, 0xC3, 0xC4, 0x01, 0x00];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::UnsupportedVersion(1))
    ));
}

#[test]
fn secondary_compressor_is_rejected() {
    let delta = [0xD6, 0xC3, 0xC4, 0x00, 0x01, 0x00];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::UnsupportedSecondaryCompressor)
    ));
}

#[test]
fn custom_code_table_is_rejected() {
    let delta = [0xD6, 0xC3, 0xC4, 0x00, 0x02];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::UnsupportedCodeTable)
    ));
}

#[test]
fn delta_length_mismatch_is_rejected() {
    // golden_add_only with the delta-encoding length bumped from 9 to 10
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x0A, 0x03, 0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63,
        0x04,
    ];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::DeltaLengthMismatch)
    ));
}

#[test]
fn target_size_mismatch_is_rejected() {
    // golden_add_only with the target size inflated from 3 to 4
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x09, 0x04, 0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63,
        0x04,
    ];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::TargetSizeMismatch)
    ));
}

#[test]
fn address_out_of_bounds_is_rejected() {
    // COPY from address 5 with nothing produced and no source
    let ops = vec![Op::Copy(5, 1)];
    assert!(matches!(
        decode(b"", &build(0, &ops)),
        Err(DecodeError::AddressOutOfBounds)
    ));
}

#[test]
fn copy_crossing_source_target_is_rejected() {
    // Source of length 4, copy starting at 3 for length 3 would cross into the target
    let ops = vec![Op::Copy(3, 3)];
    assert!(matches!(
        decode(b"abcd", &build(4, &ops)),
        Err(DecodeError::CopyCrossesSourceTarget)
    ));
}

#[test]
fn segment_out_of_bounds_is_rejected() {
    // VCD_SOURCE segment size 10 against a 3-byte source
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x01, 0x0A, 0x00, 0x08, 0x03, 0x00, 0x00, 0x02, 0x01, 0x13,
        0x03, 0x00,
    ];
    assert!(matches!(
        decode(b"abc", &delta),
        Err(DecodeError::SegmentOutOfBounds)
    ));
}

#[test]
fn oversized_target_is_rejected_without_allocating() {
    // A tiny window declaring a 4 GiB RUN target must be capped, not allocated
    let delta = [
        0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x10, 0x90, 0x80, 0x80, 0x80, 0x00, 0x00, 0x01, 0x06,
        0x00, 0x41, 0x00, 0x90, 0x80, 0x80, 0x80, 0x00,
    ];
    assert!(matches!(
        decode(b"", &delta),
        Err(DecodeError::SizeLimitExceeded)
    ));
}

proptest! {
    #[test]
    fn add_run_and_source_copies_round_trip(
        source in prop::collection::vec(any::<u8>(), 1..48),
        seeds in prop::collection::vec((0u8..3, any::<u8>(), any::<u16>(), any::<u16>()), 0..24),
    ) {
        let mut ops = Vec::new();
        let mut expected: Vec<u8> = Vec::new();
        for (kind, byte, a, b) in &seeds {
            match kind {
                0 => {
                    let len = (*b as usize % 8) + 1;
                    let bytes = vec![*byte; len];
                    expected.extend_from_slice(&bytes);
                    ops.push(Op::Add(bytes));
                }
                1 => {
                    let len = (*b as u64 % 8) + 1;
                    expected.extend(std::iter::repeat_n(*byte, len as usize));
                    ops.push(Op::Run(*byte, len));
                }
                _ => {
                    let start = *a as usize % source.len();
                    let len = (*b as usize % (source.len() - start)) + 1;
                    expected.extend_from_slice(&source[start..start + len]);
                    ops.push(Op::Copy(start as u64, len as u64));
                }
            }
        }
        let decoded = decode(&source, &build(source.len(), &ops)).unwrap();
        prop_assert_eq!(decoded, expected);
    }
}
