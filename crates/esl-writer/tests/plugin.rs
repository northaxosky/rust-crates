//! Integration tests for the public esl-writer API.

use esl_writer::{Game, Plugin, carrier_plugin};

/// Read a little-endian u16 at `off`
fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// Read a little-endian u32 at `off`
fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Read a little-endian f32 at `off`
fn f32le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Return the payload of the first field with `sig`, scanning the TES4 record body
fn find_field(bytes: &[u8], sig: &[u8; 4]) -> Option<Vec<u8>> {
    let mut off = 24;
    while off + 6 <= bytes.len() {
        let this = &bytes[off..off + 4];
        let size = u16le(bytes, off + 4) as usize;
        let start = off + 6;
        if start + size > bytes.len() {
            break;
        }
        if this == sig {
            return Some(bytes[start..start + size].to_vec());
        }
        off = start + size;
    }
    None
}

#[test]
fn carrier_has_a_valid_tes4_header_for_every_game() {
    for game in [Game::SkyrimSe, Game::Fallout4, Game::Starfield] {
        let bytes = carrier_plugin(game);
        assert_eq!(&bytes[0..4], b"TES4");
        let data_size = u32le(&bytes, 4) as usize;
        assert_eq!(data_size, bytes.len() - 24);
        assert_eq!(u32le(&bytes, 12), 0);
        assert_eq!(&bytes[24..28], b"HEDR");
        assert_eq!(u16le(&bytes, 28), 12);
        assert_eq!(u32le(&bytes, 34), 0);
    }
}

#[test]
fn carrier_flags_and_version_are_per_game() {
    let fo4 = carrier_plugin(Game::Fallout4);
    assert_eq!(u32le(&fo4, 8), 0x201);
    assert_eq!(f32le(&fo4, 30), 1.0);

    let sf = carrier_plugin(Game::Starfield);
    assert_eq!(u32le(&sf, 8), 0x101);
    assert_eq!(f32le(&sf, 30), 0.96);

    let sse = carrier_plugin(Game::SkyrimSe);
    assert_eq!(u32le(&sse, 8), 0x201);
    assert_eq!(f32le(&sse, 30), 1.71);
}

#[test]
fn builder_writes_author_and_master() {
    let bytes = Plugin::new(Game::Fallout4)
        .author("muteptr")
        .master("Fallout4.esm")
        .to_bytes()
        .unwrap();
    assert_eq!(find_field(&bytes, b"CNAM"), Some(b"muteptr\0".to_vec()));
    assert_eq!(
        find_field(&bytes, b"MAST"),
        Some(b"Fallout4.esm\0".to_vec())
    );
    assert!(find_field(&bytes, b"DATA").is_some());
}

#[test]
fn starfield_masters_have_no_data() {
    let bytes = Plugin::new(Game::Starfield)
        .master("Starfield.esm")
        .to_bytes()
        .unwrap();
    assert!(find_field(&bytes, b"MAST").is_some());
    assert!(find_field(&bytes, b"DATA").is_none());
}
