//! Writer for Fallout 4 version-1 DX10 (`BTDX`) texture archives.
//!
//! Packs `.dds` files into Old-Gen (`version = 1`) texture archives, splitting each texture's mips
//! into chunks with the Archive2 512x512 rule (cubemaps stay one chunk) and zlib-compressing them.

use super::{CHUNK_SENTINEL, HEADER_SIZE, MAGIC, MAX_PATH_LEN, lossy, zlib_compress};
use crate::dds::{self, ParsedTexture};
use crate::error::Ba2WriteError;
use crate::hashing::{FileHash, hash_file};
use std::collections::HashMap;
use std::io::Write;

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
    use crate::{Entries, extract_texture, read};

    fn u16le(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn u32le(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn u64le(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
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
