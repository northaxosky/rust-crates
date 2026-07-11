//! End-to-end xdelta ID-2 fixture coverage

use std::io::Cursor;

use vcdiff_rs::{DecodeError, DecodeOptions, SectionKind, decode, decode_to};

const DELTA: &[u8] = include_bytes!("fixtures/xdelta/xdelta-3.2.0-lzma.vcdiff");
const TARGET: &[u8] = include_bytes!("fixtures/xdelta/target.bin");

#[test]
fn real_six_window_id2_fixture_decodes_through_both_apis() {
    assert_eq!(decode(b"", DELTA).unwrap(), TARGET);

    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(DELTA);
    let mut target = Cursor::new(Vec::new());
    decode_to(
        &mut source,
        &mut delta,
        &mut target,
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(target.position(), TARGET.len() as u64);
    assert_eq!(target.into_inner(), TARGET);
}

#[test]
fn id2_dictionary_limit_is_contextual() {
    let mut options = DecodeOptions::default();
    options.max_secondary_dictionary_size = 4 * 1024;
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(DELTA);
    let mut target = Cursor::new(Vec::new());
    match decode_to(&mut source, &mut delta, &mut target, &options) {
        Err(DecodeError::SecondaryDictionaryLimit {
            required,
            limit: 4_096,
            context,
        }) => {
            assert!(required > 4_096);
            assert_eq!(context.window, Some(0));
            assert_eq!(context.section, Some(SectionKind::Data));
        }
        result => panic!("unexpected dictionary-limit result: {result:?}"),
    }
}

#[test]
fn unlimited_dictionary_policy_decodes_the_fixture() {
    let mut options = DecodeOptions::default();
    options.max_secondary_dictionary_size = u64::MAX;
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(DELTA);
    let mut target = Cursor::new(Vec::new());
    decode_to(&mut source, &mut delta, &mut target, &options).unwrap();
    assert_eq!(target.into_inner(), TARGET);
}

#[test]
fn id2_decoded_window_memory_limit_is_contextual() {
    let mut options = DecodeOptions::default();
    options.max_window_memory = 16_384;
    let mut source = Cursor::new(Vec::<u8>::new());
    let mut delta = Cursor::new(DELTA);
    let mut target = Cursor::new(Vec::new());
    assert!(matches!(
        decode_to(&mut source, &mut delta, &mut target, &options),
        Err(DecodeError::WindowMemoryLimit {
            attempted: 16_388,
            limit: 16_384,
            context,
        }) if context.window == Some(0)
            && context.section == Some(SectionKind::Instructions)
    ));
}

#[test]
fn id2_adler32_rejects_a_corrupted_checksum() {
    assert_eq!(&DELTA[17..21], &[0x49, 0xE1, 0xC9, 0xAA]);
    let mut corrupted = DELTA.to_vec();
    corrupted[20] ^= 1;
    assert!(matches!(
        decode(b"", &corrupted),
        Err(DecodeError::ChecksumMismatch {
            window: 0,
            expected: 0x49E1_C9AB,
            actual: 0x49E1_C9AA,
            delta_offset: 17,
        })
    ));
}
