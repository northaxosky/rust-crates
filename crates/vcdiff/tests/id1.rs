//! ID-1 routing and integration tests

mod common;

use std::error::Error as _;
use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use common::varint;
use vcdiff_rs::{DecodeError, DecodeOptions, IoOperation, SectionKind, decode, decode_to};

const VCD_DATACOMP: u8 = 0x01;
const VCD_INSTCOMP: u8 = 0x02;
const VCD_ADDRCOMP: u8 = 0x04;
const ONE_PAYLOAD: &[u8] = include_bytes!("fixtures/xdelta/djw-one-xdelta-3.2.0.payload.bin");
const ONE_RAW: &[u8] = include_bytes!("fixtures/xdelta/djw-one-xdelta-3.2.0.raw.bin");
const MULTI_PAYLOAD: &[u8] = include_bytes!("fixtures/xdelta/djw-multi-xdelta-3.2.0.payload.bin");
const MULTI_RAW: &[u8] = include_bytes!("fixtures/xdelta/djw-multi-xdelta-3.2.0.raw.bin");

fn compressed_section(decoded: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut section = varint(decoded.len() as u64);
    section.extend_from_slice(payload);
    section
}

fn add_instruction(size: usize) -> Vec<u8> {
    let mut instructions = vec![1];
    instructions.extend(varint(size as u64));
    instructions
}

fn id1_delta(
    source_len: usize,
    target_size: usize,
    indicator: u8,
    data: &[u8],
    instructions: &[u8],
    addresses: &[u8],
) -> Vec<u8> {
    let mut encoding = varint(target_size as u64);
    encoding.push(indicator);
    encoding.extend(varint(data.len() as u64));
    encoding.extend(varint(instructions.len() as u64));
    encoding.extend(varint(addresses.len() as u64));
    encoding.extend_from_slice(data);
    encoding.extend_from_slice(instructions);
    encoding.extend_from_slice(addresses);

    let mut delta = vec![0xD6, 0xC3, 0xC4, 0x00, 0x01, 0x01];
    delta.push(u8::from(source_len != 0));
    if source_len != 0 {
        delta.extend(varint(source_len as u64));
        delta.extend(varint(0));
    }
    delta.extend(varint(encoding.len() as u64));
    delta.extend(encoding);
    delta
}

fn assert_both_apis(source: &[u8], delta: &[u8], expected: &[u8]) {
    assert_eq!(decode(source, delta).unwrap(), expected);

    let mut source = Cursor::new(source);
    let mut delta = Cursor::new(delta);
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

fn source_bytes() -> Vec<u8> {
    (0..=u8::MAX).collect()
}

fn instruction_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let source = source_bytes();
    let mut expected = Vec::new();
    for &opcode in ONE_RAW {
        let slot = usize::from(opcode - 19) % 16;
        assert_ne!(slot, 0);
        expected.extend_from_slice(&source[..slot + 3]);
    }
    let instructions = compressed_section(ONE_RAW, ONE_PAYLOAD);
    let addresses = vec![0; ONE_RAW.len()];
    let delta = id1_delta(
        source.len(),
        expected.len(),
        VCD_INSTCOMP,
        &[],
        &instructions,
        &addresses,
    );
    (source, delta, expected)
}

fn address_expected(source: &[u8]) -> Vec<u8> {
    let mut expected = Vec::with_capacity(ONE_RAW.len() * 4);
    for &address in ONE_RAW {
        let address = usize::from(address);
        expected.extend_from_slice(&source[address..address + 4]);
    }
    expected
}

fn data_fixture() -> Vec<u8> {
    id1_delta(
        0,
        MULTI_RAW.len(),
        VCD_DATACOMP,
        &compressed_section(MULTI_RAW, MULTI_PAYLOAD),
        &add_instruction(MULTI_RAW.len()),
        &[],
    )
}

#[test]
fn external_multi_payload_routes_only_data_through_both_apis() {
    let delta = data_fixture();
    assert_both_apis(&[], &delta, MULTI_RAW);

    let mut options = DecodeOptions::default();
    options.max_secondary_dictionary_size = 0;
    let mut source = Cursor::new([]);
    let mut delta = Cursor::new(delta);
    let mut target = Cursor::new(Vec::new());
    decode_to(&mut source, &mut delta, &mut target, &options).unwrap();
    assert_eq!(target.into_inner(), MULTI_RAW);
}

#[test]
fn external_one_payload_routes_only_instructions_through_both_apis() {
    let (source, delta, expected) = instruction_fixture();
    assert_both_apis(&source, &delta, &expected);
}

#[test]
fn external_one_payload_routes_only_addresses_through_both_apis() {
    let source = source_bytes();
    let expected = address_expected(&source);
    let instructions = vec![20; ONE_RAW.len()];
    let delta = id1_delta(
        source.len(),
        expected.len(),
        VCD_ADDRCOMP,
        &[],
        &instructions,
        &compressed_section(ONE_RAW, ONE_PAYLOAD),
    );
    assert_both_apis(&source, &delta, &expected);
}

#[test]
fn mixed_data_and_address_flags_decode_independently() {
    let source = source_bytes();
    let mut expected = MULTI_RAW.to_vec();
    expected.extend(address_expected(&source));
    let mut instructions = add_instruction(MULTI_RAW.len());
    instructions.extend(std::iter::repeat_n(20, ONE_RAW.len()));
    let delta = id1_delta(
        source.len(),
        expected.len(),
        VCD_DATACOMP | VCD_ADDRCOMP,
        &compressed_section(MULTI_RAW, MULTI_PAYLOAD),
        &instructions,
        &compressed_section(ONE_RAW, ONE_PAYLOAD),
    );
    assert_both_apis(&source, &delta, &expected);
}

#[test]
fn later_windows_start_with_fresh_djw_state() {
    let first = data_fixture();
    let second = data_fixture();
    let mut joined = first;
    joined.extend_from_slice(&second[6..]);
    let mut expected = MULTI_RAW.to_vec();
    expected.extend_from_slice(MULTI_RAW);
    assert_both_apis(&[], &joined, &expected);
}

#[test]
fn malformed_djw_reports_codec_and_failure_context() {
    let data = [1, 0];
    let delta = id1_delta(0, 1, VCD_DATACOMP, &data, &[2], &[]);
    let error = decode(&[], &delta).unwrap_err();
    match &error {
        DecodeError::SecondaryDecompression {
            compressor_id,
            context,
            source,
        } => {
            assert_eq!(*compressor_id, 1);
            assert_eq!(context.window, Some(0));
            assert_eq!(context.section, Some(SectionKind::Data));
            assert!(context.delta_offset > 0);
            assert!(source.to_string().contains("DJW"));
        }
        other => panic!("unexpected malformed error: {other:?}"),
    }
    assert!(error.source().is_some());
}

#[test]
fn extra_whole_djw_byte_is_an_input_mismatch() {
    let mut data = compressed_section(ONE_RAW, ONE_PAYLOAD);
    data.push(0xa5);
    let delta = id1_delta(
        0,
        ONE_RAW.len(),
        VCD_DATACOMP,
        &data,
        &add_instruction(ONE_RAW.len()),
        &[],
    );
    assert!(matches!(
        decode(&[], &delta),
        Err(DecodeError::SecondaryInputMismatch {
            expected,
            actual,
            context,
        }) if expected == ONE_PAYLOAD.len() as u64 + 1
            && actual == ONE_PAYLOAD.len() as u64
            && context.window == Some(0)
            && context.section == Some(SectionKind::Data)
    ));
}

#[test]
fn zero_decoded_size_is_rejected_before_djw() {
    let delta = id1_delta(0, 0, VCD_DATACOMP, &[0], &[], &[]);
    assert!(matches!(
        decode(&[], &delta),
        Err(DecodeError::InvalidSecondarySize {
            value: 0,
            context,
        }) if context.window == Some(0) && context.section == Some(SectionKind::Data)
    ));
}

#[test]
fn selector_peak_counts_a_prior_resident_section() {
    let prior = vec![0xaa; 123];
    let instructions = compressed_section(MULTI_RAW, MULTI_PAYLOAD);
    let delta = id1_delta(0, 0, VCD_INSTCOMP, &prior, &instructions, &[]);
    let active_after_output = prior.len() as u64 + MULTI_RAW.len() as u64;
    let selector_bytes = 200_u64;
    let limit = active_after_output + selector_bytes - 1;
    let mut options = DecodeOptions::default();
    options.max_window_memory = limit;
    let mut source = Cursor::new([]);
    let mut delta = Cursor::new(delta);
    let mut target = Cursor::new(Vec::new());
    assert!(matches!(
        decode_to(&mut source, &mut delta, &mut target, &options),
        Err(DecodeError::WindowMemoryLimit {
            attempted,
            limit: actual_limit,
            context,
        }) if attempted == active_after_output + selector_bytes
            && actual_limit == limit
            && context.window == Some(0)
            && context.section == Some(SectionKind::Instructions)
    ));
}

#[test]
fn raw_id1_header_is_accepted_and_unknown_ids_stay_specific() {
    let delta = id1_delta(0, 1, 0, b"x", &[2], &[]);
    assert_eq!(decode(&[], &delta).unwrap(), b"x");
    assert!(matches!(
        decode(&[], &[0xD6, 0xC3, 0xC4, 0x00, 0x01, 3]),
        Err(DecodeError::UnsupportedSecondaryCompressor { id: 3, .. })
    ));
}

#[derive(Debug)]
struct ReadMarker;

impl fmt::Display for ReadMarker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ID-1 injected read")
    }
}

impl std::error::Error for ReadMarker {}

struct OffsetFailReader {
    inner: Cursor<Vec<u8>>,
    fail_at: u64,
}

impl Read for OffsetFailReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let position = self.inner.position();
        if position >= self.fail_at {
            return Err(io::Error::other(ReadMarker));
        }
        let available = usize::try_from(self.fail_at - position).unwrap();
        let count = output.len().min(available);
        self.inner.read(&mut output[..count])
    }
}

impl Seek for OffsetFailReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[test]
fn integrated_djw_delta_io_preserves_the_original_source() {
    let delta = id1_delta(
        0,
        ONE_RAW.len(),
        VCD_DATACOMP,
        &compressed_section(ONE_RAW, ONE_PAYLOAD),
        &add_instruction(ONE_RAW.len()),
        &[],
    );
    let payload_start = delta
        .windows(ONE_PAYLOAD.len())
        .position(|window| window == ONE_PAYLOAD)
        .unwrap() as u64;
    let mut source = Cursor::new([]);
    let mut delta = OffsetFailReader {
        inner: Cursor::new(delta),
        fail_at: payload_start,
    };
    let mut target = Cursor::new(Vec::new());
    let error = decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap_err();
    match error {
        DecodeError::DeltaIo {
            operation: IoOperation::Read,
            context,
            source,
        } => {
            assert_eq!(context.window, Some(0));
            assert_eq!(context.section, Some(SectionKind::Data));
            assert!(
                source
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<ReadMarker>())
                    .is_some()
            );
        }
        other => panic!("unexpected I/O error: {other:?}"),
    }
}
