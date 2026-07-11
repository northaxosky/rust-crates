//! Hermetic interoperability tests for the external xdelta producer matrix

use std::io::Cursor;

use sha2::{Digest, Sha256};
use vcdiff_rs::{DecodeOptions, decode, decode_to};
use xz4rust::{DICT_SIZE_MIN, XzDecoder};

const SOURCE: &[u8] = include_bytes!("fixtures/xdelta/source.bin");
const TARGET: &[u8] = include_bytes!("fixtures/xdelta/target.bin");
const CONFIG_31: &str = include_str!("fixtures/xdelta/producers/xdelta-3.1.0-config.txt");
const CONFIG_32: &str = include_str!("fixtures/xdelta/producers/xdelta-3.2.0-config.txt");
const VERSION_31: &str = include_str!("fixtures/xdelta/producers/xdelta-3.1.0-version.txt");
const VERSION_32: &str = include_str!("fixtures/xdelta/producers/xdelta-3.2.0-version.txt");

#[derive(Clone, Copy, PartialEq, Eq)]
enum FixtureKind {
    None,
    Lzma,
    Default,
}

struct Fixture {
    name: &'static str,
    producer: &'static str,
    kind: FixtureKind,
    delta: &'static [u8],
    size: usize,
    crc32: u32,
    sha256: &'static str,
    headers: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "xdelta-3.1.0-none",
        producer: "3.1.0",
        kind: FixtureKind::None,
        delta: include_bytes!("fixtures/xdelta/xdelta-3.1.0-none.vcdiff"),
        size: 98_435,
        crc32: 0xC7B4_B3E1,
        sha256: "5e12b575e3ea2c78e85eb03e68f3f154dbcaecf2cdef136c55396128cfb7da61",
        headers: include_str!("fixtures/xdelta/printhdrs/xdelta-3.1.0-none.txt"),
    },
    Fixture {
        name: "xdelta-3.1.0-lzma",
        producer: "3.1.0",
        kind: FixtureKind::Lzma,
        delta: include_bytes!("fixtures/xdelta/xdelta-3.1.0-lzma.vcdiff"),
        size: 16_714,
        crc32: 0x29BE_B5D8,
        sha256: "f40d8e39994dfd7460cf63883764159cd4fae8285d3cc8c4f8ef231a969f007c",
        headers: include_str!("fixtures/xdelta/printhdrs/xdelta-3.1.0-lzma.txt"),
    },
    Fixture {
        name: "xdelta-3.2.0-none",
        producer: "3.2.0",
        kind: FixtureKind::None,
        delta: include_bytes!("fixtures/xdelta/xdelta-3.2.0-none.vcdiff"),
        size: 98_435,
        crc32: 0xC7B4_B3E1,
        sha256: "5e12b575e3ea2c78e85eb03e68f3f154dbcaecf2cdef136c55396128cfb7da61",
        headers: include_str!("fixtures/xdelta/printhdrs/xdelta-3.2.0-none.txt"),
    },
    Fixture {
        name: "xdelta-3.2.0-lzma",
        producer: "3.2.0",
        kind: FixtureKind::Lzma,
        delta: include_bytes!("fixtures/xdelta/xdelta-3.2.0-lzma.vcdiff"),
        size: 16_714,
        crc32: 0x29BE_B5D8,
        sha256: "f40d8e39994dfd7460cf63883764159cd4fae8285d3cc8c4f8ef231a969f007c",
        headers: include_str!("fixtures/xdelta/printhdrs/xdelta-3.2.0-lzma.txt"),
    },
    Fixture {
        name: "xdelta-3.2.0-default",
        producer: "3.2.0",
        kind: FixtureKind::Default,
        delta: include_bytes!("fixtures/xdelta/xdelta-3.2.0-default.vcdiff"),
        size: 16_842,
        crc32: 0x20EA_56BE,
        sha256: "cd9db261e9cc08ad036a933314fcfc1c24922762ccb8078145469bca000a1ad5",
        headers: include_str!("fixtures/xdelta/printhdrs/xdelta-3.2.0-default.txt"),
    },
];

struct RecordedFile {
    name: &'static str,
    bytes: &'static [u8],
    sha256: &'static str,
}

const RECORDED_FILES: &[RecordedFile] = &[
    RecordedFile {
        name: ".gitattributes",
        bytes: include_bytes!("fixtures/xdelta/.gitattributes"),
        sha256: "b7230e83b7c121aa5aad13a26df837e6cbdfd6a7b109581ffc09cb68008d8e93",
    },
    RecordedFile {
        name: "README.md",
        bytes: include_bytes!("fixtures/xdelta/README.md"),
        sha256: "e51eb374715f4cdb11ee26ea334c5ec84f627b79e5c034f11b4603192ddd0a3c",
    },
    RecordedFile {
        name: "generate.py",
        bytes: include_bytes!("fixtures/xdelta/generate.py"),
        sha256: "34243c4f0723865f9fd33668a9a4b5e3c808068789f1675d5046159df5836c74",
    },
    RecordedFile {
        name: "producers/xdelta-3.1.0-config.txt",
        bytes: include_bytes!("fixtures/xdelta/producers/xdelta-3.1.0-config.txt"),
        sha256: "f1bfce0a241fbff696db10d5e684c99dd4c90c41ff211406c50a1b8495cd2024",
    },
    RecordedFile {
        name: "producers/xdelta-3.1.0-version.txt",
        bytes: include_bytes!("fixtures/xdelta/producers/xdelta-3.1.0-version.txt"),
        sha256: "e825be975740782a973e89fca74cc6dca89b77291bbf74bab24daff124bbedca",
    },
    RecordedFile {
        name: "producers/xdelta-3.2.0-config.txt",
        bytes: include_bytes!("fixtures/xdelta/producers/xdelta-3.2.0-config.txt"),
        sha256: "84ed345b78a876978bdea9f82f0fcfdf6501c75db3e95a0a70c1ef1894b69789",
    },
    RecordedFile {
        name: "producers/xdelta-3.2.0-version.txt",
        bytes: include_bytes!("fixtures/xdelta/producers/xdelta-3.2.0-version.txt"),
        sha256: "09fb457e33f39cc51ee190f99ec0e7b3211a2453b53eafbe12b9083a55ebb589",
    },
    RecordedFile {
        name: "printhdrs/xdelta-3.1.0-lzma.txt",
        bytes: include_bytes!("fixtures/xdelta/printhdrs/xdelta-3.1.0-lzma.txt"),
        sha256: "9c4f90d28c6cfec30f119a9cc467822544619abc1af23310f5c46c47647dea64",
    },
    RecordedFile {
        name: "printhdrs/xdelta-3.1.0-none.txt",
        bytes: include_bytes!("fixtures/xdelta/printhdrs/xdelta-3.1.0-none.txt"),
        sha256: "2d042a6db33e3860c8f2dfe7acceb9088f7a2c8e24309f9ed471675e2267d6c1",
    },
    RecordedFile {
        name: "printhdrs/xdelta-3.2.0-default.txt",
        bytes: include_bytes!("fixtures/xdelta/printhdrs/xdelta-3.2.0-default.txt"),
        sha256: "de05c2872af4361b07d6b0b315fc706b863fa0d90dad5ae49e872fe370ba6e75",
    },
    RecordedFile {
        name: "printhdrs/xdelta-3.2.0-lzma.txt",
        bytes: include_bytes!("fixtures/xdelta/printhdrs/xdelta-3.2.0-lzma.txt"),
        sha256: "9c4f90d28c6cfec30f119a9cc467822544619abc1af23310f5c46c47647dea64",
    },
    RecordedFile {
        name: "printhdrs/xdelta-3.2.0-none.txt",
        bytes: include_bytes!("fixtures/xdelta/printhdrs/xdelta-3.2.0-none.txt"),
        sha256: "2d042a6db33e3860c8f2dfe7acceb9088f7a2c8e24309f9ed471675e2267d6c1",
    },
];

struct Window<'a> {
    target_size: u64,
    delta_indicator: u8,
    data: &'a [u8],
    instructions: &'a [u8],
    addresses: &'a [u8],
}

struct ParsedDelta<'a> {
    header_indicator: u8,
    compressor_id: Option<u8>,
    app_header: Option<&'a [u8]>,
    windows: Vec<Window<'a>>,
}

struct Armor<'a> {
    target_name: &'a str,
    target_digest: &'a str,
    source_name: &'a str,
    source_digest: &'a str,
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn byte(&mut self) -> Result<u8, String> {
        let byte = self
            .bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| format!("truncated byte at {}", self.pos))?;
        self.pos += 1;
        Ok(byte)
    }

    fn take(&mut self, len: u64) -> Result<&'a [u8], String> {
        let len = usize::try_from(len).map_err(|_| format!("length {len} exceeds usize"))?;
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "fixture offset overflow".to_owned())?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| format!("truncated range {}+{len}", self.pos))?;
        self.pos = end;
        Ok(bytes)
    }

    fn varint(&mut self) -> Result<u64, String> {
        let start = self.pos;
        let mut value = 0_u64;
        for _ in 0..10 {
            let byte = self.byte()?;
            value = value
                .checked_mul(128)
                .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
                .ok_or_else(|| format!("varint overflow at {start}"))?;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(format!("unterminated varint at {start}"))
    }
}

fn parse_delta(bytes: &[u8]) -> Result<ParsedDelta<'_>, String> {
    let mut cursor = ByteCursor::new(bytes);
    if cursor.take(3)? != [0xD6, 0xC3, 0xC4] {
        return Err("bad VCDIFF magic".to_owned());
    }
    if cursor.byte()? != 0 {
        return Err("fixture is not VCDIFF version zero".to_owned());
    }
    let header_indicator = cursor.byte()?;
    let compressor_id = if header_indicator & 0x01 != 0 {
        Some(cursor.byte()?)
    } else {
        None
    };
    if header_indicator & 0x02 != 0 {
        let len = cursor.varint()?;
        cursor.take(len)?;
    }
    let app_header = if header_indicator & 0x04 != 0 {
        let len = cursor.varint()?;
        Some(cursor.take(len)?)
    } else {
        None
    };

    let mut windows = Vec::new();
    while cursor.pos < bytes.len() {
        let window_indicator = cursor.byte()?;
        if window_indicator & 0x03 != 0 {
            cursor.varint()?;
            cursor.varint()?;
        }
        let encoding_len = cursor.varint()?;
        let encoding_start = cursor.pos;
        let encoding_end = encoding_start
            .checked_add(
                usize::try_from(encoding_len)
                    .map_err(|_| format!("encoding length {encoding_len} exceeds usize"))?,
            )
            .ok_or_else(|| "encoding endpoint overflow".to_owned())?;
        let target_size = cursor.varint()?;
        let delta_indicator = cursor.byte()?;
        let data_len = cursor.varint()?;
        let instructions_len = cursor.varint()?;
        let addresses_len = cursor.varint()?;
        if window_indicator & 0x04 != 0 {
            cursor.take(4)?;
        }
        let data = cursor.take(data_len)?;
        let instructions = cursor.take(instructions_len)?;
        let addresses = cursor.take(addresses_len)?;
        if cursor.pos != encoding_end {
            return Err(format!(
                "window {} ends at {}, expected {encoding_end}",
                windows.len(),
                cursor.pos
            ));
        }
        windows.push(Window {
            target_size,
            delta_indicator,
            data,
            instructions,
            addresses,
        });
    }
    Ok(ParsedDelta {
        header_indicator,
        compressor_id,
        app_header,
        windows,
    })
}

fn split_compressed(payload: &[u8]) -> Result<(u64, &[u8]), String> {
    let mut cursor = ByteCursor::new(payload);
    let decoded_size = cursor.varint()?;
    Ok((decoded_size, &payload[cursor.pos..]))
}

fn parse_armor_entry(entry: &str) -> Result<(&str, &str), String> {
    let (name, digest) = entry
        .split_once('#')
        .ok_or_else(|| format!("armor entry lacks digest: {entry}"))?;
    if name.is_empty() || digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!("invalid armor entry: {entry}"));
    }
    Ok((name, digest))
}

fn parse_armor(header: &[u8]) -> Result<Armor<'_>, String> {
    let header =
        std::str::from_utf8(header).map_err(|_| "application header is not ASCII".to_owned())?;
    let body = header
        .strip_suffix('/')
        .ok_or_else(|| "application header lacks its terminator".to_owned())?;
    let mut entries = body.split("//");
    let target = entries
        .next()
        .ok_or_else(|| "application header lacks target armor".to_owned())?;
    let source = entries
        .next()
        .ok_or_else(|| "application header lacks source armor".to_owned())?;
    if entries.next().is_some() {
        return Err("application header has trailing armor entries".to_owned());
    }

    let (target_name, target_digest) = parse_armor_entry(target)?;
    let (source_name, source_digest) = parse_armor_entry(source)?;
    Ok(Armor {
        target_name,
        target_digest,
        source_name,
        source_digest,
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn producer_metadata_and_fixture_identities_are_stable() {
    assert_eq!(SOURCE.len(), 131_072);
    assert_eq!(crc32(SOURCE), 0x5AE9_2CB3);
    assert_eq!(
        sha256(SOURCE),
        "4ec1e45ab24b6c8ff8ac75692496692cc5908a67492c2d6497310268e69e70ee"
    );
    assert_eq!(TARGET.len(), 98_304);
    assert_eq!(crc32(TARGET), 0x4231_8550);
    assert_eq!(
        sha256(TARGET),
        "c21ff467100a57e3495cf97bd025a9c903c32a85fd927f5d13b559d2b197daae"
    );
    assert!(VERSION_31.contains("Xdelta version 3.1.0"));
    assert!(VERSION_32.contains("Xdelta version 3.2.0"));
    assert!(CONFIG_31.contains("SECONDARY_LZMA=1"));
    assert!(CONFIG_32.contains("SECONDARY_LZMA=1"));

    for fixture in FIXTURES {
        let version = match fixture.producer {
            "3.1.0" => VERSION_31,
            "3.2.0" => VERSION_32,
            producer => panic!("unexpected producer {producer}"),
        };
        assert!(version.contains(&format!("Xdelta version {}", fixture.producer)));
        assert_eq!(fixture.delta.len(), fixture.size, "{} size", fixture.name);
        assert_eq!(
            crc32(fixture.delta),
            fixture.crc32,
            "{} CRC32",
            fixture.name
        );
        assert_eq!(
            sha256(fixture.delta),
            fixture.sha256,
            "{} SHA-256",
            fixture.name
        );
        assert!(
            fixture.headers.contains("VCDIFF version:               0"),
            "{} printhdrs format",
            fixture.name
        );
    }

    for file in RECORDED_FILES {
        assert_eq!(sha256(file.bytes), file.sha256, "{} SHA-256", file.name);
    }
}

#[test]
fn all_external_matrix_fixtures_decode_exactly() {
    for fixture in FIXTURES {
        let mut source = Cursor::new(SOURCE);
        let mut delta = Cursor::new(fixture.delta);
        let mut target = Cursor::new(Vec::new());
        decode_to(
            &mut source,
            &mut delta,
            &mut target,
            &DecodeOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{} failed: {error}", fixture.name));
        assert_eq!(
            target.position(),
            TARGET.len() as u64,
            "{} position",
            fixture.name
        );
        assert_eq!(target.into_inner(), TARGET, "{} output", fixture.name);
    }
}

#[test]
fn matrix_structure_matches_the_recorded_generation_modes() {
    for fixture in FIXTURES {
        let parsed =
            parse_delta(fixture.delta).unwrap_or_else(|error| panic!("{}: {error}", fixture.name));
        match fixture.kind {
            FixtureKind::None => {
                assert_eq!(parsed.compressor_id, None, "{} compressor", fixture.name);
                assert_eq!(parsed.header_indicator, 0, "{} header", fixture.name);
                assert_eq!(parsed.windows.len(), 6, "{} windows", fixture.name);
                assert!(
                    parsed.windows.iter().all(|window| {
                        window.target_size == 16_384
                            && window.delta_indicator == 0
                            && window.data.len() == 16_384
                            && window.instructions.len() == 4
                            && window.addresses.is_empty()
                    }),
                    "{} raw sections",
                    fixture.name
                );
            }
            FixtureKind::Lzma => {
                assert_eq!(parsed.compressor_id, Some(2), "{} compressor", fixture.name);
                assert_eq!(parsed.header_indicator, 1, "{} header", fixture.name);
                assert_eq!(parsed.app_header, None, "{} app header", fixture.name);
                assert!(parsed.windows.len() >= 3, "{} windows", fixture.name);
                assert!(
                    parsed
                        .windows
                        .iter()
                        .skip(1)
                        .all(|window| window.delta_indicator & 0x01 != 0),
                    "{} later DATA compression",
                    fixture.name
                );
                assert!(
                    parsed
                        .windows
                        .iter()
                        .all(|window| window.delta_indicator == 0x01),
                    "{} mixed section flags",
                    fixture.name
                );
            }
            FixtureKind::Default => {
                assert_eq!(parsed.compressor_id, Some(2), "{} compressor", fixture.name);
                assert_eq!(parsed.header_indicator, 5, "{} header", fixture.name);
                let app_header = parsed.app_header.expect("default fixture needs app header");
                let armor = parse_armor(app_header).expect("default armor should parse completely");
                assert_eq!(armor.target_name, "target.bin");
                assert_eq!(armor.source_name, "source.bin");
                let target_digest = blake3::hash(TARGET).to_hex();
                let source_digest = blake3::hash(SOURCE).to_hex();
                assert_eq!(armor.target_digest, target_digest.as_str());
                assert_eq!(armor.source_digest, source_digest.as_str());
                assert_eq!(parsed.windows.len(), 1);
                assert_eq!(parsed.windows[0].target_size, 98_304);
                assert_eq!(parsed.windows[0].delta_indicator, 0x07);
            }
        }
    }
}

#[test]
fn explicit_lzma_corpus_proves_later_window_continuation() {
    for fixture in FIXTURES
        .iter()
        .filter(|fixture| fixture.kind == FixtureKind::Lzma)
    {
        let parsed = parse_delta(fixture.delta).unwrap();
        let mut decoder = XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, 64 * 1024 * 1024);
        let mut target_offset = 0_usize;
        for window in parsed.windows.iter().take(2) {
            let (decoded_size, fragment) = split_compressed(window.data).unwrap();
            assert_eq!(decoded_size, window.target_size);
            let mut output = vec![0; decoded_size as usize];
            let result = decoder.decode(fragment, &mut output).unwrap();
            assert!(!result.is_end_of_stream());
            assert_eq!(result.input_consumed(), fragment.len());
            assert_eq!(result.output_produced(), output.len());
            assert!(decoder.is_lzma2_chunk_boundary());
            let target_end = target_offset + output.len();
            assert_eq!(
                output,
                TARGET[target_offset..target_end],
                "{}",
                fixture.name
            );
            target_offset = target_end;
        }

        let second = &parsed.windows[1];
        let (decoded_size, fragment) = split_compressed(second.data).unwrap();
        let mut fresh = XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, 64 * 1024 * 1024);
        let mut output = vec![0; decoded_size as usize];
        let reproduced = fresh.decode(fragment, &mut output).is_ok_and(|result| {
            result.output_produced() == output.len()
                && output == TARGET[16_384..32_768]
                && fresh.is_lzma2_chunk_boundary()
        });
        assert!(!reproduced, "{} fresh later fragment", fixture.name);
        assert_eq!(
            decode(SOURCE, fixture.delta).unwrap(),
            TARGET,
            "{}",
            fixture.name
        );
    }
}
