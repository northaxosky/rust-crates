//! Integration round-trip tests: write with each writer/format, read back through the public
//! `Archive` API, and confirm the extracted bytes match. These use only the public crate surface.

#[allow(dead_code)]
mod common;

use btdx::{Archive, Ba2Format, Compression, Dx10Writer, Entries, GnrlWriter};
use common::dx10_dds;
use proptest::prelude::*;

const FORMATS: [Ba2Format; 3] = [
    Ba2Format::Fo4,
    Ba2Format::StarfieldV2,
    Ba2Format::StarfieldV3Lz4,
];

#[test]
fn gnrl_round_trips_in_every_format() {
    for format in FORMATS {
        let compressed = b"the quick brown fox jumps".repeat(4);
        let mut w = GnrlWriter::new();
        w.format(format);
        w.add_file("Meshes\\a.nif", compressed.clone()).unwrap();
        w.add_file_stored("Scripts\\b.pex", b"stored".to_vec())
            .unwrap();
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        let Entries::General(files) = archive.entries() else {
            panic!("expected general for {format:?}");
        };
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path.as_deref(), Some("meshes\\a.nif"));
        assert_eq!(archive.extract(&files[0]).unwrap(), compressed);
        assert_eq!(files[1].packed_size, 0);
        assert_eq!(archive.extract(&files[1]).unwrap(), b"stored");
    }
}

#[test]
fn dx10_round_trips_in_every_format() {
    for format in FORMATS {
        let dds = dx10_dds(64, 64, 7, 71, false);
        let mut w = Dx10Writer::new();
        w.format(format);
        w.add_texture("Textures\\t.dds", dds.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        let Entries::Texture(texs) = archive.entries() else {
            panic!("expected texture for {format:?}");
        };
        assert_eq!(archive.extract_texture(&texs[0]).unwrap(), dds);
    }
}

#[test]
fn header_reports_the_written_format() {
    let cases = [
        (Ba2Format::Fo4, 1u32, Compression::Zlib),
        (Ba2Format::StarfieldV2, 2, Compression::Zlib),
        (Ba2Format::StarfieldV3Lz4, 3, Compression::Lz4),
    ];
    for (format, version, compression) in cases {
        let mut w = GnrlWriter::new();
        w.format(format);
        w.add_file("a.bin", vec![1u8; 32]).unwrap();
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        assert_eq!(archive.header().version, version);
        assert_eq!(archive.header().compression, compression);
    }
}

#[test]
fn round_trips_without_a_name_table() {
    let mut w = GnrlWriter::new();
    w.names(false);
    w.format(Ba2Format::StarfieldV2);
    w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
    let img = w.to_vec().unwrap();
    let archive = Archive::read(&img).unwrap();
    let Entries::General(files) = archive.entries() else {
        panic!("expected general");
    };
    assert_eq!(files[0].path, None);
    assert_eq!(archive.extract(&files[0]).unwrap(), b"hi");
}

#[test]
fn round_trips_a_cubemap() {
    let dds = dx10_dds(16, 16, 1, 71, true);
    let mut w = Dx10Writer::new();
    w.format(Ba2Format::StarfieldV3Lz4);
    w.add_texture("Textures\\cube.dds", dds.clone()).unwrap();
    let img = w.to_vec().unwrap();
    let archive = Archive::read(&img).unwrap();
    let Entries::Texture(texs) = archive.entries() else {
        panic!("expected texture");
    };
    assert_eq!(texs[0].flags & 1, 1);
    assert_eq!(archive.extract_texture(&texs[0]).unwrap(), dds);
}

#[test]
fn round_trips_many_textures() {
    let dds = [
        dx10_dds(32, 32, 1, 71, false),
        dx10_dds(64, 16, 1, 83, false),
        dx10_dds(8, 8, 1, 98, false),
    ];
    let mut w = Dx10Writer::new();
    w.format(Ba2Format::StarfieldV3Lz4);
    for (i, d) in dds.iter().enumerate() {
        let path = format!("Textures\\t{i}.dds");
        w.add_texture(path.as_bytes(), d.clone()).unwrap();
    }
    let img = w.to_vec().unwrap();
    let archive = Archive::read(&img).unwrap();
    let Entries::Texture(texs) = archive.entries() else {
        panic!("expected texture");
    };
    for (i, t) in texs.iter().enumerate() {
        assert_eq!(archive.extract_texture(t).unwrap(), dds[i]);
    }
}

#[test]
fn empty_archives_round_trip_in_every_format() {
    for format in FORMATS {
        let mut w = GnrlWriter::new();
        w.format(format);
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        assert_eq!(archive.header().file_count, 0);
        let Entries::General(files) = archive.entries() else {
            panic!("expected general");
        };
        assert!(files.is_empty());
    }
}

#[test]
fn writers_are_deterministic_in_every_format() {
    for format in FORMATS {
        let build = || {
            let mut w = GnrlWriter::new();
            w.format(format);
            w.add_file("Data\\a.bin", vec![9u8; 200]).unwrap();
            w.add_file_stored("Data\\b.bin", vec![1, 2, 3]).unwrap();
            w.to_vec().unwrap()
        };
        assert_eq!(build(), build());
    }
}

proptest! {
    #[test]
    fn any_gnrl_files_round_trip(
        files in proptest::collection::vec(
            (proptest::collection::vec(any::<u8>(), 0..96), any::<bool>()),
            0..10,
        ),
        fmt_idx in 0usize..3,
    ) {
        let format = FORMATS[fmt_idx];
        let mut w = GnrlWriter::new();
        w.format(format);
        for (i, (data, compress)) in files.iter().enumerate() {
            let path = format!("Data\\f{i}.bin");
            if *compress {
                w.add_file(path.as_bytes(), data.clone()).unwrap();
            } else {
                w.add_file_stored(path.as_bytes(), data.clone()).unwrap();
            }
        }
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        let Entries::General(entries) = archive.entries() else {
            panic!("expected general");
        };
        prop_assert_eq!(entries.len(), files.len());
        for (entry, (data, _)) in entries.iter().zip(files.iter()) {
            prop_assert_eq!(&archive.extract(entry).unwrap(), data);
        }
    }

    #[test]
    fn any_texture_round_trips(
        dim_exp in 2u32..8,
        dxgi in prop::sample::select(vec![71u32, 74, 77, 80, 83, 98]),
        fmt_idx in 0usize..3,
    ) {
        let side = 1u32 << dim_exp;
        let mips = 32 - side.leading_zeros();
        let dds = dx10_dds(side, side, mips, dxgi, false);
        let format = FORMATS[fmt_idx];
        let mut w = Dx10Writer::new();
        w.format(format);
        w.add_texture("Textures\\z.dds", dds.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let archive = Archive::read(&img).unwrap();
        let Entries::Texture(texs) = archive.entries() else {
            panic!("expected texture");
        };
        prop_assert_eq!(archive.extract_texture(&texs[0]).unwrap(), dds);
    }
}
