//! Integration tests for the reader's error handling and public accessors, driven only through the
//! public API. Archives are built with the writers, then read back or deliberately corrupted.

#[allow(dead_code)]
mod common;

use btdx::{Archive, Ba2Format, Dx10Writer, Entries, GnrlWriter, ReadError};
use common::dx10_dds;

fn valid_gnrl() -> Vec<u8> {
    let mut w = GnrlWriter::new();
    w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
    w.to_vec().unwrap()
}

#[test]
fn rejects_bad_magic() {
    let mut img = valid_gnrl();
    img[0] = b'X';
    assert!(matches!(Archive::read(&img), Err(ReadError::BadMagic)));
}

#[test]
fn rejects_a_truncated_header() {
    assert!(matches!(
        Archive::read(b"BTDX"),
        Err(ReadError::TooShort { .. })
    ));
}

#[test]
fn rejects_an_unknown_version() {
    let mut img = valid_gnrl();
    img[4] = 9;
    assert!(matches!(
        Archive::read(&img),
        Err(ReadError::UnsupportedVersion(9))
    ));
}

#[test]
fn rejects_an_unknown_compression_method() {
    let mut w = GnrlWriter::new();
    w.format(Ba2Format::StarfieldV3Lz4);
    w.add_file("a.bin", vec![1u8; 16]).unwrap();
    let mut img = w.to_vec().unwrap();
    img[32] = 7;
    assert!(matches!(
        Archive::read(&img),
        Err(ReadError::UnsupportedCompression(7))
    ));
}

#[test]
fn entries_accessors_match_archive_kind() {
    let img = valid_gnrl();
    let archive = Archive::read(&img).unwrap();
    assert!(archive.entries().general().is_some());
    assert!(archive.entries().texture().is_none());

    let dds = dx10_dds(8, 8, 1, 71, false);
    let mut w = Dx10Writer::new();
    w.add_texture("t.dds", dds).unwrap();
    let img = w.to_vec().unwrap();
    let archive = Archive::read(&img).unwrap();
    assert!(archive.entries().texture().is_some());
    assert!(archive.entries().general().is_none());
}

#[test]
fn a_corrupt_zlib_chunk_errors_without_panicking() {
    let mut w = GnrlWriter::new();
    w.add_file("a.bin", b"the quick brown fox".repeat(8))
        .unwrap();
    let mut img = w.to_vec().unwrap();
    for b in &mut img[60..66] {
        *b ^= 0xFF;
    }
    let archive = Archive::read(&img).unwrap();
    let Entries::General(files) = archive.entries() else {
        panic!("expected general");
    };
    assert!(archive.extract(&files[0]).is_err());
}

#[test]
fn a_corrupt_lz4_chunk_errors_without_panicking() {
    let mut w = GnrlWriter::new();
    w.format(Ba2Format::StarfieldV3Lz4);
    w.add_file("a.bin", b"the quick brown fox".repeat(8))
        .unwrap();
    let mut img = w.to_vec().unwrap();
    for b in &mut img[72..80] {
        *b ^= 0xFF;
    }
    let archive = Archive::read(&img).unwrap();
    let Entries::General(files) = archive.entries() else {
        panic!("expected general");
    };
    let _ = archive.extract(&files[0]);
}

#[test]
fn a_truncated_archive_errors_without_panicking() {
    let img = valid_gnrl();
    for cut in 0..img.len() {
        let _ = Archive::read(&img[..cut]);
    }
}
