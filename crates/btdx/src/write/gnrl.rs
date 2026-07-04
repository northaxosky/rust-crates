//! Writer for Fallout 4 and Starfield GNRL (`BTDX`) general archives.
//!
//! Emits general archives with correct CRC hashes and either compressed or stored files, so the
//! engine indexes and loads their contents. The [`Ba2Format`] selects the version and codec: Fallout
//! 4 version 1 (zlib, the default), Starfield version 2 (zlib), or Starfield version 3 (LZ4). This
//! writer never emits DX10 textures.

use super::{Ba2Format, CHUNK_SENTINEL, MAX_PATH_LEN, compress, lossy, write_header};
use crate::error::WriteError;
use crate::hashing::{FileHash, hash_file};
use std::collections::HashMap;
use std::io::Write;

const GNRL: &[u8; 4] = b"GNRL";
const RECORD_SIZE: u64 = 36;
const GNRL_FILE_HEADER_SIZE: u16 = 0x0010;

struct Entry {
    name: Vec<u8>,
    hash: FileHash,
    data: Vec<u8>,
    compress: bool,
}

/// A compressed-or-stored payload with the sizes recorded in its file record
struct Block {
    packed: u32,
    unpacked: u32,
    bytes: Vec<u8>,
}

/// Builds a GNRL BA2 archive in memory
pub struct GnrlWriter {
    entries: Vec<Entry>,
    seen: HashMap<FileHash, Vec<u8>>,
    write_names: bool,
    format: Ba2Format,
}

impl GnrlWriter {
    /// Create an empty writer that emits a Fallout 4 v1 GNRL archive with a name table
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: HashMap::new(),
            write_names: true,
            format: Ba2Format::Fo4,
        }
    }

    /// Enable or disable the trailing name table (default=on)
    pub fn names(&mut self, enabled: bool) -> &mut Self {
        self.write_names = enabled;
        self
    }

    /// Select the output format: version and archive-wide codec (default [`Ba2Format::Fo4`])
    pub fn format(&mut self, format: Ba2Format) -> &mut Self {
        self.format = format;
        self
    }

    /// Add a file whose data is zlib-compressed in the archive
    pub fn add_file(
        &mut self,
        path: impl AsRef<[u8]>,
        data: impl Into<Vec<u8>>,
    ) -> Result<(), WriteError> {
        self.push(path.as_ref(), data.into(), true)
    }

    /// Add a file whose data is stored uncompressed in the archive
    pub fn add_file_stored(
        &mut self,
        path: impl AsRef<[u8]>,
        data: impl Into<Vec<u8>>,
    ) -> Result<(), WriteError> {
        self.push(path.as_ref(), data.into(), false)
    }

    /// Serialize the archive into a new byte buffer
    pub fn to_vec(&self) -> Result<Vec<u8>, WriteError> {
        let file_count = u32::try_from(self.entries.len())
            .map_err(|_| WriteError::TooManyFiles(self.entries.len()))?;
        let mut blocks: Vec<Block> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let unpacked =
                u32::try_from(entry.data.len()).map_err(|_| WriteError::FileTooLarge {
                    path: lossy(&entry.name),
                    size: entry.data.len(),
                })?;

            let (packed, bytes) = if entry.compress {
                let compressed = compress(self.format, &entry.data).map_err(|source| {
                    WriteError::ZlibCompress {
                        path: lossy(&entry.name),
                        source,
                    }
                })?;
                let packed =
                    u32::try_from(compressed.len()).map_err(|_| WriteError::FileTooLarge {
                        path: lossy(&entry.name),
                        size: compressed.len(),
                    })?;
                (packed, compressed)
            } else {
                (0, entry.data.clone())
            };
            blocks.push(Block {
                packed,
                unpacked,
                bytes,
            });
        }

        let records = RECORD_SIZE
            .checked_mul(self.entries.len() as u64)
            .ok_or(WriteError::OffsetOverflow)?;
        let data_start = self
            .format
            .header_size()
            .checked_add(records)
            .ok_or(WriteError::OffsetOverflow)?;

        let mut data_len = 0u64;
        for block in &blocks {
            data_len = data_len
                .checked_add(block.bytes.len() as u64)
                .ok_or(WriteError::OffsetOverflow)?;
        }

        let names_offset = if self.write_names && !self.entries.is_empty() {
            data_start
                .checked_add(data_len)
                .ok_or(WriteError::OffsetOverflow)?
        } else {
            0
        };

        let mut out = Vec::new();
        write_header(&mut out, self.format, GNRL, file_count, names_offset);

        let mut data_off = data_start;
        for (entry, block) in self.entries.iter().zip(&blocks) {
            out.extend_from_slice(&entry.hash.file.to_le_bytes());
            out.extend_from_slice(&entry.hash.extension.to_le_bytes());
            out.extend_from_slice(&entry.hash.directory.to_le_bytes());
            out.push(0);
            out.push(1);
            out.extend_from_slice(&GNRL_FILE_HEADER_SIZE.to_le_bytes());
            out.extend_from_slice(&data_off.to_le_bytes());
            out.extend_from_slice(&block.packed.to_le_bytes());
            out.extend_from_slice(&block.unpacked.to_le_bytes());
            out.extend_from_slice(&CHUNK_SENTINEL.to_le_bytes());
            data_off = data_off
                .checked_add(block.bytes.len() as u64)
                .ok_or(WriteError::OffsetOverflow)?;
        }

        for block in &blocks {
            out.extend_from_slice(&block.bytes);
        }

        if self.write_names && !self.entries.is_empty() {
            for entry in &self.entries {
                out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
                out.extend_from_slice(&entry.name);
            }
        }
        Ok(out)
    }

    /// Serialize the archive and write it `out`: an I/O error may leave a partial write
    pub fn write<W: Write>(&self, out: &mut W) -> Result<(), WriteError> {
        let bytes = self.to_vec()?;
        out.write_all(&bytes).map_err(WriteError::Io)
    }

    /// Normalize, validate, and record one file
    fn push(&mut self, path: &[u8], data: Vec<u8>, compress: bool) -> Result<(), WriteError> {
        let (name, hash) = hash_file(path);
        if name.is_empty() {
            return Err(WriteError::InvalidPath {
                reason: "path is empty or only separator",
            });
        }
        if name.len() >= MAX_PATH_LEN {
            return Err(WriteError::InvalidPath {
                reason: "path is 260 bytes or longer",
            });
        }
        if data.len() > u32::MAX as usize {
            return Err(WriteError::FileTooLarge {
                path: lossy(&name),
                size: data.len(),
            });
        }
        if self.entries.len() >= u32::MAX as usize {
            return Err(WriteError::TooManyFiles(self.entries.len() + 1));
        }
        match self.seen.get(&hash) {
            Some(existing) if existing == &name => {
                return Err(WriteError::DuplicatePath(lossy(&name)));
            }
            Some(existing) => {
                return Err(WriteError::HashCollision {
                    first: lossy(existing),
                    second: lossy(&name),
                });
            }
            None => {}
        }
        self.seen.insert(hash, name.clone());
        self.entries.push(Entry {
            name,
            hash,
            data,
            compress,
        });
        Ok(())
    }
}

impl Default for GnrlWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Archive, Ba2Format, Compression, Entries, GnrlEntry};
    use proptest::prelude::*;

    // Thin adapters over the Archive container, so the round-trip tests read unchanged.
    fn read(bytes: &[u8]) -> Result<(crate::Header, Entries), crate::ReadError> {
        let a = Archive::read(bytes)?;
        Ok((*a.header(), a.entries().clone()))
    }
    fn extract(bytes: &[u8], entry: &GnrlEntry) -> Result<Vec<u8>, crate::ReadError> {
        Archive::read(bytes)?.extract(entry)
    }

    fn u16le(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn u32le(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn u64le(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }

    #[test]
    fn round_trips_a_stored_file() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("Meshes\\a.nif", b"hello world".to_vec())
            .unwrap();
        let img = w.to_vec().unwrap();
        let (header, entries) = read(&img).unwrap();
        assert_eq!(header.version, 1);
        assert_eq!(header.file_count, 1);
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files[0].path.as_deref(), Some("meshes\\a.nif"));
        assert_eq!(files[0].packed_size, 0);
        assert_eq!(extract(&img, &files[0]).unwrap(), b"hello world");
    }

    #[test]
    fn round_trips_a_compressed_file() {
        let payload = b"the quick brown fox jumps over the lazy dog".repeat(16);
        let mut w = GnrlWriter::new();
        w.add_file("Scripts\\b.pex", payload.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_ne!(files[0].packed_size, 0);
        assert_eq!(extract(&img, &files[0]).unwrap(), payload);
    }

    #[test]
    fn round_trips_multiple_files_in_order() {
        let mut w = GnrlWriter::new();
        w.add_file("Meshes\\a.nif", b"aaaa".to_vec()).unwrap();
        w.add_file_stored("Sound\\b.xwm", b"bbbbbb".to_vec())
            .unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path.as_deref(), Some("meshes\\a.nif"));
        assert_eq!(files[1].path.as_deref(), Some("sound\\b.xwm"));
        assert_eq!(extract(&img, &files[0]).unwrap(), b"aaaa");
        assert_eq!(extract(&img, &files[1]).unwrap(), b"bbbbbb");
    }

    // Assert the on-disk layout field by field, independent of the reader.
    #[test]
    fn writes_the_documented_byte_layout() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        let img = w.to_vec().unwrap();

        assert_eq!(&img[0..4], b"BTDX");
        assert_eq!(u32le(&img, 4), 1);
        assert_eq!(&img[8..12], b"GNRL");
        assert_eq!(u32le(&img, 12), 1);

        let data_offset = 24 + 36;
        let name_offset = data_offset + 2;
        assert_eq!(u64le(&img, 16), name_offset as u64);

        let (_name, hash) = hash_file(b"a.txt");
        assert_eq!(u32le(&img, 24), hash.file);
        assert_eq!(u32le(&img, 28), hash.extension);
        assert_eq!(u32le(&img, 32), hash.directory);
        assert_eq!(img[36], 0);
        assert_eq!(img[37], 1);
        assert_eq!(u16le(&img, 38), 0x0010);
        assert_eq!(u64le(&img, 40), data_offset as u64);
        assert_eq!(u32le(&img, 48), 0);
        assert_eq!(u32le(&img, 52), 2);
        assert_eq!(u32le(&img, 56), 0xBAAD_F00D);

        assert_eq!(&img[data_offset..data_offset + 2], b"hi");
        assert_eq!(u16le(&img, name_offset), 5);
        assert_eq!(&img[name_offset + 2..name_offset + 7], b"a.txt");
        assert_eq!(img.len(), name_offset + 2 + 5);
    }

    #[test]
    fn omits_the_name_table_when_disabled() {
        let mut w = GnrlWriter::new();
        w.names(false);
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        let img = w.to_vec().unwrap();
        assert_eq!(u64le(&img, 16), 0);
        assert_eq!(img.len(), 24 + 36 + 2);
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files[0].path, None);
    }

    #[test]
    fn rejects_a_duplicate_path() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("Meshes\\a.nif", b"x".to_vec()).unwrap();
        let err = w
            .add_file_stored("meshes/A.NIF", b"y".to_vec())
            .unwrap_err();
        assert!(matches!(err, WriteError::DuplicatePath(_)));
    }

    #[test]
    fn rejects_a_hash_collision() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("dir\\file.abcd1", b"x".to_vec()).unwrap();
        let err = w
            .add_file_stored("dir\\file.abcd2", b"y".to_vec())
            .unwrap_err();
        assert!(matches!(err, WriteError::HashCollision { .. }));
    }

    #[test]
    fn rejects_invalid_paths() {
        let mut w = GnrlWriter::new();
        assert!(matches!(
            w.add_file_stored("", b"x".to_vec()),
            Err(WriteError::InvalidPath { .. })
        ));
        assert!(matches!(
            w.add_file_stored("\\\\", b"x".to_vec()),
            Err(WriteError::InvalidPath { .. })
        ));
    }

    #[test]
    fn handles_empty_files_and_empty_archive() {
        let empty = GnrlWriter::new().to_vec().unwrap();
        assert_eq!(u32le(&empty, 12), 0);
        assert_eq!(u64le(&empty, 16), 0);
        assert_eq!(empty.len(), 24);
        let (h, entries) = read(&empty).unwrap();
        assert_eq!(h.file_count, 0);
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert!(files.is_empty());

        let mut w = GnrlWriter::new();
        w.add_file_stored("a.bin", Vec::new()).unwrap();
        w.add_file("b.bin", Vec::new()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(extract(&img, &files[0]).unwrap(), b"");
        assert_eq!(extract(&img, &files[1]).unwrap(), b"");
    }

    #[test]
    fn writes_to_a_generic_sink() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        let mut sink = Vec::new();
        w.write(&mut sink).unwrap();
        assert_eq!(sink, w.to_vec().unwrap());
    }

    struct FailingSink;
    impl std::io::Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("sink is closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_propagates_a_sink_error() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        assert!(matches!(w.write(&mut FailingSink), Err(WriteError::Io(_))));
    }

    #[test]
    fn writes_are_deterministic() {
        let build = || {
            let mut w = GnrlWriter::new();
            w.add_file_stored("Data\\a.bin", vec![1, 2, 3]).unwrap();
            w.add_file("Data\\b.bin", vec![4u8; 64]).unwrap();
            w.to_vec().unwrap()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn writes_a_starfield_v2_header() {
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV2);
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        let img = w.to_vec().unwrap();
        assert_eq!(u32le(&img, 4), 2);
        assert_eq!(u64le(&img, 24), 1);
        let data_offset = 32 + 36;
        assert_eq!(u64le(&img, 16), (data_offset + 2) as u64);
        assert_eq!(u64le(&img, 32 + 16), data_offset as u64);
        let (header, _entries) = read(&img).unwrap();
        assert_eq!(header.version, 2);
        assert_eq!(header.compression, Compression::Zlib);
    }

    #[test]
    fn writes_a_starfield_v3_lz4_header() {
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV3Lz4);
        w.add_file("a.bin", vec![7u8; 64]).unwrap();
        let img = w.to_vec().unwrap();
        assert_eq!(u32le(&img, 4), 3);
        assert_eq!(u64le(&img, 24), 1);
        assert_eq!(u32le(&img, 32), 3);
        let (header, _entries) = read(&img).unwrap();
        assert_eq!(header.version, 3);
        assert_eq!(header.compression, Compression::Lz4);
    }

    #[test]
    fn v3_lz4_round_trips_via_reader() {
        let payload = b"the quick brown fox".repeat(8);
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV3Lz4);
        w.add_file("Scripts\\q.pex", payload.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_ne!(files[0].packed_size, 0);
        assert_eq!(extract(&img, &files[0]).unwrap(), payload);
    }

    #[test]
    fn v3_lz4_stored_entry_round_trips() {
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV3Lz4);
        w.add_file_stored("a.bin", b"stored".to_vec()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files[0].packed_size, 0);
        assert_eq!(extract(&img, &files[0]).unwrap(), b"stored");
    }

    #[test]
    fn format_is_applied_at_serialization() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        w.format(Ba2Format::StarfieldV2);
        assert_eq!(u32le(&w.to_vec().unwrap(), 4), 2);
    }

    #[test]
    fn empty_starfield_archives_are_header_only() {
        let mut v2 = GnrlWriter::new();
        v2.format(Ba2Format::StarfieldV2);
        let v2 = v2.to_vec().unwrap();
        assert_eq!(v2.len(), 32);
        assert_eq!(u32le(&v2, 4), 2);
        assert_eq!(u64le(&v2, 16), 0);

        let mut v3 = GnrlWriter::new();
        v3.format(Ba2Format::StarfieldV3Lz4);
        let v3 = v3.to_vec().unwrap();
        assert_eq!(v3.len(), 36);
        assert_eq!(u32le(&v3, 4), 3);
        assert_eq!(u32le(&v3, 32), 3);
        assert_eq!(u64le(&v3, 16), 0);
    }

    #[test]
    fn lz4_round_trips_an_empty_file() {
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV3Lz4);
        w.add_file("a.bin", Vec::new()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(extract(&img, &files[0]).unwrap(), b"");
    }

    #[test]
    fn lz4_round_trips_incompressible_data() {
        let payload: Vec<u8> = (0..1000u32)
            .map(|i| i.wrapping_mul(2654435761) as u8)
            .collect();
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV3Lz4);
        w.add_file("a.bin", payload.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(extract(&img, &files[0]).unwrap(), payload);
    }

    #[test]
    fn names_disabled_under_starfield_v2() {
        let mut w = GnrlWriter::new();
        w.format(Ba2Format::StarfieldV2).names(false);
        w.add_file_stored("a.txt", b"hi".to_vec()).unwrap();
        let img = w.to_vec().unwrap();
        assert_eq!(u64le(&img, 16), 0);
        assert_eq!(img.len(), 32 + 36 + 2);
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files[0].path, None);
    }

    proptest! {
        // Any set of unique files survives a write -> read -> extract round trip, in every format.
        #[test]
        fn gnrl_round_trips(
            files in proptest::collection::vec(
                (proptest::collection::vec(any::<u8>(), 0..64), any::<bool>()),
                0..8,
            ),
            format in prop::sample::select(vec![
                Ba2Format::Fo4,
                Ba2Format::StarfieldV2,
                Ba2Format::StarfieldV3Lz4,
            ]),
        ) {
            let mut w = GnrlWriter::new();
            w.format(format);
            for (i, (data, compress)) in files.iter().enumerate() {
                let path = format!("Data\\file{i}.bin");
                if *compress {
                    w.add_file(&path, data.clone()).unwrap();
                } else {
                    w.add_file_stored(&path, data.clone()).unwrap();
                }
            }
            let img = w.to_vec().unwrap();
            let (_h, entries) = read(&img).unwrap();
            let Entries::General(entries) = entries else {
                panic!("expected general");
            };
            prop_assert_eq!(entries.len(), files.len());
            for (entry, (data, _)) in entries.iter().zip(files.iter()) {
                prop_assert_eq!(&extract(&img, entry).unwrap(), data);
            }
        }
    }
}
