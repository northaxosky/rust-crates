//! Writer for Fallout 4 version-1 GNRL (`BTDX`) archives.
//!
//! Emits Old-Gen (`version = 1`) general archives with correct CRC hashes and either zlib-compressed
//! or stored files, so the Fallout 4 engine indexes and loads their contents (both Old-Gen and
//! Next-Gen load version 1). This writer only ever emits version-1 GNRL with zlib/stored data; it
//! never writes DX10/GNMF textures, LZ4 compression, or the v2/v3/v7/v8 header variants.

use crate::dds::{self, ParsedTexture};
use crate::error::Ba2WriteError;
use crate::hashing::{FileHash, hash_file};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::collections::HashMap;
use std::io::Write;

const MAGIC: &[u8; 4] = b"BTDX";
const GNRL: &[u8; 4] = b"GNRL";
const HEADER_SIZE: u64 = 24;
const RECORD_SIZE: u64 = 36;
const GNRL_FILE_HEADER_SIZE: u16 = 0x0010;
const CHUNK_SENTINEL: u32 = 0xBAAD_F00D;
const MAX_PATH_LEN: usize = 260;

/// One pending file: its on-disk name, lookup key, payload, and whether to compress it
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

/// Builds a version-1 GNRL BA2 archive in memory
pub struct GnrlWriter {
    entries: Vec<Entry>,
    seen: HashMap<FileHash, Vec<u8>>,
    write_names: bool,
}

impl GnrlWriter {
    /// Create an empty writer that emits a v1 GNRL archive with a name table
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: HashMap::new(),
            write_names: true,
        }
    }

    /// Enable or disable the trailing name table (default=on)
    pub fn names(&mut self, enabled: bool) -> &mut Self {
        self.write_names = enabled;
        self
    }

    /// Add a file whose data is zlib-compressed in the archive
    pub fn add_file(
        &mut self,
        path: impl AsRef<[u8]>,
        data: impl Into<Vec<u8>>,
    ) -> Result<(), Ba2WriteError> {
        self.push(path.as_ref(), data.into(), true)
    }

    /// Add a file whose data is stored uncompressed in the archive
    pub fn add_file_stored(
        &mut self,
        path: impl AsRef<[u8]>,
        data: impl Into<Vec<u8>>,
    ) -> Result<(), Ba2WriteError> {
        self.push(path.as_ref(), data.into(), false)
    }

    /// Serialize the archive into a new byte buffer
    pub fn to_vec(&self) -> Result<Vec<u8>, Ba2WriteError> {
        let file_count = u32::try_from(self.entries.len())
            .map_err(|_| Ba2WriteError::TooManyFiles(self.entries.len()))?;
        let mut blocks: Vec<Block> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let unpacked =
                u32::try_from(entry.data.len()).map_err(|_| Ba2WriteError::FileTooLarge {
                    path: lossy(&entry.name),
                    size: entry.data.len(),
                })?;

            let (packed, bytes) = if entry.compress {
                let compressed =
                    zlib_compress(&entry.data).map_err(|source| Ba2WriteError::ZlibCompress {
                        path: lossy(&entry.name),
                        source,
                    })?;
                let packed =
                    u32::try_from(compressed.len()).map_err(|_| Ba2WriteError::FileTooLarge {
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
            .ok_or(Ba2WriteError::OffsetOverflow)?;
        let data_start = HEADER_SIZE
            .checked_add(records)
            .ok_or(Ba2WriteError::OffsetOverflow)?;

        let mut data_len = 0u64;
        for block in &blocks {
            data_len = data_len
                .checked_add(block.bytes.len() as u64)
                .ok_or(Ba2WriteError::OffsetOverflow)?;
        }

        let names_offset = if self.write_names && !self.entries.is_empty() {
            data_start
                .checked_add(data_len)
                .ok_or(Ba2WriteError::OffsetOverflow)?
        } else {
            0
        };

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(GNRL);
        out.extend_from_slice(&file_count.to_le_bytes());
        out.extend_from_slice(&names_offset.to_le_bytes());

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
                .ok_or(Ba2WriteError::OffsetOverflow)?;
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
    pub fn write<W: Write>(&self, out: &mut W) -> Result<(), Ba2WriteError> {
        let bytes = self.to_vec()?;
        out.write_all(&bytes).map_err(Ba2WriteError::Io)
    }

    /// Normalize, validate, and record one file
    fn push(&mut self, path: &[u8], data: Vec<u8>, compress: bool) -> Result<(), Ba2WriteError> {
        let (name, hash) = hash_file(path);
        if name.is_empty() {
            return Err(Ba2WriteError::InvalidPath {
                reason: "path is empty or only separator",
            });
        }
        if name.len() >= MAX_PATH_LEN {
            return Err(Ba2WriteError::InvalidPath {
                reason: "path is 260 bytes or longer",
            });
        }
        if data.len() > u32::MAX as usize {
            return Err(Ba2WriteError::FileTooLarge {
                path: lossy(&name),
                size: data.len(),
            });
        }
        if self.entries.len() >= u32::MAX as usize {
            return Err(Ba2WriteError::TooManyFiles(self.entries.len() + 1));
        }
        match self.seen.get(&hash) {
            Some(existing) if existing == &name => {
                return Err(Ba2WriteError::DuplicatePath(lossy(&name)));
            }
            Some(existing) => {
                return Err(Ba2WriteError::HashCollision {
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

/// Render path bytes as a lossy string for error messages
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Compress `data` int oa standalone zlib stream at the default level
fn zlib_compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

const DX10: &[u8; 4] = b"DX10";
const DX10_FILE_HEADER_SIZE: u64 = 24;
const DX10_CHUNK_SIZE: u64 = 24;
const DX10_CHUNK_RECORD_SIZE: u16 = 24;
const MIP_CHUNK_DIM: u32 = 512;
const MAX_DX10_CHUNKS: usize = 4;

/// One prepared texture: its name, key, DX10 header fields, and split chunks
struct TextureEntry {
    name: Vec<u8>,
    hash: FileHash,
    height: u16,
    width: u16,
    mip_count: u8,
    format: u8,
    cubemap: bool,
    chunks: Vec<RawChunk>,
}

/// One texture chunk before compression: its inclusive mip range and raw bytes
struct RawChunk {
    start_mip: u16,
    end_mip: u16,
    data: Vec<u8>,
}

/// One texture chunk after compression, ready to serialize into a chunk record
struct PreparedChunk {
    start_mip: u16,
    end_mip: u16,
    packed: u32,
    unpacked: u32,
    bytes: Vec<u8>,
}

/// Builds a version-1 DX10 texture BA2 archive from DDS files
pub struct Dx10Writer {
    entries: Vec<TextureEntry>,
    seen: HashMap<FileHash, Vec<u8>>,
    write_names: bool,
}

impl Dx10Writer {
    /// Create an empty writer that emits a version-1 DX10 archive with a name table
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: HashMap::new(),
            write_names: true,
        }
    }

    /// Enable or disable the trailing name table (on by default)
    pub fn names(&mut self, enabled: bool) -> &mut Self {
        self.write_names = enabled;
        self
    }

    /// Add a `.dds` texture, splitting its mips into chunks and zlib-compressing them on write
    pub fn add_texture(
        &mut self,
        path: impl AsRef<[u8]>,
        dds: impl Into<Vec<u8>>,
    ) -> Result<(), Ba2WriteError> {
        self.push(path.as_ref(), dds.into())
    }

    /// Serialize the archive into a new byte buffer
    pub fn to_vec(&self) -> Result<Vec<u8>, Ba2WriteError> {
        let file_count = u32::try_from(self.entries.len())
            .map_err(|_| Ba2WriteError::TooManyFiles(self.entries.len()))?;

        let mut prepared: Vec<Vec<PreparedChunk>> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let mut chunks = Vec::with_capacity(entry.chunks.len());
            for chunk in &entry.chunks {
                let unpacked =
                    u32::try_from(chunk.data.len()).map_err(|_| Ba2WriteError::FileTooLarge {
                        path: lossy(&entry.name),
                        size: chunk.data.len(),
                    })?;
                let compressed =
                    zlib_compress(&chunk.data).map_err(|source| Ba2WriteError::ZlibCompress {
                        path: lossy(&entry.name),
                        source,
                    })?;
                let packed =
                    u32::try_from(compressed.len()).map_err(|_| Ba2WriteError::FileTooLarge {
                        path: lossy(&entry.name),
                        size: compressed.len(),
                    })?;
                chunks.push(PreparedChunk {
                    start_mip: chunk.start_mip,
                    end_mip: chunk.end_mip,
                    packed,
                    unpacked,
                    bytes: compressed,
                });
            }
            prepared.push(chunks);
        }

        let mut records = 0u64;
        for chunks in &prepared {
            let per = DX10_CHUNK_SIZE
                .checked_mul(chunks.len() as u64)
                .and_then(|c| c.checked_add(DX10_FILE_HEADER_SIZE))
                .ok_or(Ba2WriteError::OffsetOverflow)?;
            records = records
                .checked_add(per)
                .ok_or(Ba2WriteError::OffsetOverflow)?;
        }
        let data_start = HEADER_SIZE
            .checked_add(records)
            .ok_or(Ba2WriteError::OffsetOverflow)?;

        let mut data_len = 0u64;
        for chunks in &prepared {
            for chunk in chunks {
                data_len = data_len
                    .checked_add(chunk.bytes.len() as u64)
                    .ok_or(Ba2WriteError::OffsetOverflow)?;
            }
        }
        let names_offset = if self.write_names && !self.entries.is_empty() {
            data_start
                .checked_add(data_len)
                .ok_or(Ba2WriteError::OffsetOverflow)?
        } else {
            0
        };

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(DX10);
        out.extend_from_slice(&file_count.to_le_bytes());
        out.extend_from_slice(&names_offset.to_le_bytes());

        let mut data_off = data_start;
        for (entry, chunks) in self.entries.iter().zip(&prepared) {
            let chunk_count = u8::try_from(chunks.len())
                .map_err(|_| Ba2WriteError::TooManyFiles(chunks.len()))?;
            out.extend_from_slice(&entry.hash.file.to_le_bytes());
            out.extend_from_slice(&entry.hash.extension.to_le_bytes());
            out.extend_from_slice(&entry.hash.directory.to_le_bytes());
            out.push(0);
            out.push(chunk_count);
            out.extend_from_slice(&DX10_CHUNK_RECORD_SIZE.to_le_bytes());
            out.extend_from_slice(&entry.height.to_le_bytes());
            out.extend_from_slice(&entry.width.to_le_bytes());
            out.push(entry.mip_count);
            out.push(entry.format);
            out.push(u8::from(entry.cubemap));
            out.push(8);
            for chunk in chunks {
                out.extend_from_slice(&data_off.to_le_bytes());
                out.extend_from_slice(&chunk.packed.to_le_bytes());
                out.extend_from_slice(&chunk.unpacked.to_le_bytes());
                out.extend_from_slice(&chunk.start_mip.to_le_bytes());
                out.extend_from_slice(&chunk.end_mip.to_le_bytes());
                out.extend_from_slice(&CHUNK_SENTINEL.to_le_bytes());
                data_off = data_off
                    .checked_add(chunk.bytes.len() as u64)
                    .ok_or(Ba2WriteError::OffsetOverflow)?;
            }
        }

        for chunks in &prepared {
            for chunk in chunks {
                out.extend_from_slice(&chunk.bytes);
            }
        }

        if self.write_names && !self.entries.is_empty() {
            for entry in &self.entries {
                out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
                out.extend_from_slice(&entry.name);
            }
        }

        Ok(out)
    }

    /// Serialize the archive and write it to `out`; an I/O error may leave a partial write
    pub fn write<W: Write>(&self, out: &mut W) -> Result<(), Ba2WriteError> {
        let bytes = self.to_vec()?;
        out.write_all(&bytes).map_err(Ba2WriteError::Io)
    }

    /// Parse, validate, split, and record one DDS texture
    fn push(&mut self, path: &[u8], dds: Vec<u8>) -> Result<(), Ba2WriteError> {
        let (name, hash) = hash_file(path);
        if name.is_empty() {
            return Err(Ba2WriteError::InvalidPath {
                reason: "path is empty or only separators",
            });
        }
        if name.len() >= MAX_PATH_LEN {
            return Err(Ba2WriteError::InvalidPath {
                reason: "path is 260 bytes or longer",
            });
        }
        if self.entries.len() >= u32::MAX as usize {
            return Err(Ba2WriteError::TooManyFiles(self.entries.len() + 1));
        }
        match self.seen.get(&hash) {
            Some(existing) if existing == &name => {
                return Err(Ba2WriteError::DuplicatePath(lossy(&name)));
            }
            Some(existing) => {
                return Err(Ba2WriteError::HashCollision {
                    first: lossy(existing),
                    second: lossy(&name),
                });
            }
            None => {}
        }
        let texture = dds::parse(&dds).map_err(|source| Ba2WriteError::Dds {
            path: lossy(&name),
            source,
        })?;
        let chunks = split_chunks(&texture);
        self.seen.insert(hash, name.clone());
        self.entries.push(TextureEntry {
            name,
            hash,
            height: texture.height,
            width: texture.width,
            mip_count: texture.mip_count,
            format: texture.dxgi,
            cubemap: texture.cubemap,
            chunks,
        });
        Ok(())
    }
}

impl Default for Dx10Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// Split a texture's mips into chunks per the Archive2 512x512 rule; cubemaps stay one chunk
fn split_chunks(texture: &ParsedTexture<'_>) -> Vec<RawChunk> {
    let mip_count = u32::from(texture.mip_count);
    if texture.cubemap {
        return vec![RawChunk {
            start_mip: 0,
            end_mip: texture.mip_count.saturating_sub(1).into(),
            data: texture.data.to_vec(),
        }];
    }

    let width = u32::from(texture.width);
    let height = u32::from(texture.height);
    let dxgi = u32::from(texture.dxgi);

    let mut sizes = Vec::with_capacity(texture.mip_count as usize);
    let mut offsets = Vec::with_capacity(texture.mip_count as usize + 1);
    let mut acc = 0usize;
    offsets.push(0usize);
    for m in 0..mip_count {
        let mw = (width >> m).max(1);
        let mh = (height >> m).max(1);
        let size = dds::mip_size(mw, mh, dxgi).unwrap_or(0);
        sizes.push(size);
        acc += size as usize;
        offsets.push(acc);
    }

    let threshold = dds::mip_size(MIP_CHUNK_DIM, MIP_CHUNK_DIM, dxgi).unwrap_or(u64::MAX);
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut start = 0u32;
    let mut size = 0u64;
    let mut m = 0u32;
    while m < mip_count {
        let ms = sizes[m as usize];
        if size == 0 || size + ms < threshold {
            size += ms;
        } else {
            ranges.push((start, m - 1));
            start = m;
            size = ms;
            if ranges.len() == MAX_DX10_CHUNKS - 1 {
                break;
            }
        }
        m += 1;
    }
    ranges.push((start, mip_count - 1));

    ranges
        .into_iter()
        .map(|(s, e)| RawChunk {
            start_mip: s as u16,
            end_mip: e as u16,
            data: texture.data[offsets[s as usize]..offsets[e as usize + 1]].to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entries, extract, extract_texture, read};

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
        assert!(matches!(err, Ba2WriteError::DuplicatePath(_)));
    }

    #[test]
    fn rejects_a_hash_collision() {
        let mut w = GnrlWriter::new();
        w.add_file_stored("dir\\file.abcd1", b"x".to_vec()).unwrap();
        let err = w
            .add_file_stored("dir\\file.abcd2", b"y".to_vec())
            .unwrap_err();
        assert!(matches!(err, Ba2WriteError::HashCollision { .. }));
    }

    #[test]
    fn rejects_invalid_paths() {
        let mut w = GnrlWriter::new();
        assert!(matches!(
            w.add_file_stored("", b"x".to_vec()),
            Err(Ba2WriteError::InvalidPath { .. })
        ));
        assert!(matches!(
            w.add_file_stored("\\\\", b"x".to_vec()),
            Err(Ba2WriteError::InvalidPath { .. })
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

    // Build a DX10 DDS (extension header) with `len` bytes of non-trivial pixel data.
    fn dx10_dds(
        width: u32,
        height: u32,
        mips: u32,
        dxgi: u32,
        cubemap: bool,
        len: usize,
    ) -> Vec<u8> {
        let mut dds = dds::header(width, height, mips, dxgi, cubemap);
        dds.extend((0u32..len as u32).map(|i| i as u8));
        dds
    }

    #[test]
    fn round_trips_a_texture() {
        let dds = dx10_dds(64, 64, 3, 71, false, 0x800 + 0x200 + 0x80);
        let mut w = Dx10Writer::new();
        w.add_texture("Textures\\a.dds", dds.clone()).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert_eq!(textures.len(), 1);
        assert_eq!(textures[0].path.as_deref(), Some("textures\\a.dds"));
        assert_eq!(extract_texture(&img, &textures[0]).unwrap(), dds);
    }

    // The bsa-rs worked example: 1024x1024 BC1, 11 mips, splits into 0..0, 1..1, 2..10.
    #[test]
    fn splits_mips_like_archive2() {
        let dds = dx10_dds(1024, 1024, 11, 71, false, 0x80000 + 0x20000 + 0xAAB8);
        let mut w = Dx10Writer::new();
        w.add_texture("t.dds", dds).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        let chunks = &textures[0].chunks;
        assert_eq!(chunks.len(), 3);
        let range = |c: &crate::Dx10Chunk| (c.start_mip, c.end_mip, c.unpacked_size);
        assert_eq!(range(&chunks[0]), (0, 0, 0x80000));
        assert_eq!(range(&chunks[1]), (1, 1, 0x20000));
        assert_eq!(range(&chunks[2]), (2, 10, 0xAAB8));
    }

    #[test]
    fn packs_a_cubemap_as_one_chunk() {
        let dds = dx10_dds(8, 8, 1, 71, true, 6 * 0x20);
        let mut w = Dx10Writer::new();
        w.add_texture("cube.dds", dds).unwrap();
        let img = w.to_vec().unwrap();
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert_eq!(textures[0].flags, 1);
        assert_eq!(textures[0].chunks.len(), 1);
        assert_eq!(
            (
                textures[0].chunks[0].start_mip,
                textures[0].chunks[0].end_mip
            ),
            (0, 0)
        );
    }

    // Assert the DX10 header and chunk record layout for a one-chunk texture.
    #[test]
    fn writes_the_dx10_record() {
        let dds = dx10_dds(4, 4, 1, 71, false, 8);
        let mut w = Dx10Writer::new();
        w.names(false);
        w.add_texture("a.dds", dds).unwrap();
        let img = w.to_vec().unwrap();

        assert_eq!(&img[0..4], b"BTDX");
        assert_eq!(u32le(&img, 4), 1);
        assert_eq!(&img[8..12], b"DX10");
        assert_eq!(u32le(&img, 12), 1);
        assert_eq!(u64le(&img, 16), 0);

        assert_eq!(img[36], 0);
        assert_eq!(img[37], 1);
        assert_eq!(u16le(&img, 38), 24);
        assert_eq!(u16le(&img, 40), 4);
        assert_eq!(u16le(&img, 42), 4);
        assert_eq!(img[44], 1);
        assert_eq!(img[45], 71);
        assert_eq!(img[46], 0);
        assert_eq!(img[47], 8);

        assert_eq!(u64le(&img, 48), 72);
        assert_eq!(u32le(&img, 60), 8);
        assert_eq!(u16le(&img, 64), 0);
        assert_eq!(u16le(&img, 66), 0);
        assert_eq!(u32le(&img, 68), 0xBAAD_F00D);
    }

    #[test]
    fn rejects_a_duplicate_texture() {
        let dds = dx10_dds(4, 4, 1, 71, false, 8);
        let mut w = Dx10Writer::new();
        w.add_texture("Textures\\a.dds", dds.clone()).unwrap();
        assert!(matches!(
            w.add_texture("textures/A.DDS", dds),
            Err(Ba2WriteError::DuplicatePath(_))
        ));
    }

    #[test]
    fn rejects_an_invalid_dds() {
        let mut w = Dx10Writer::new();
        assert!(matches!(
            w.add_texture("a.dds", b"NOPE".to_vec()),
            Err(Ba2WriteError::Dds { .. })
        ));
    }
}
