//! Reader for Fallout 4 BA2 archives: header, records, and file/texture extraction.

use std::io::Read;

use crate::dds;
use crate::error::ReadError;

const MAGIC: &[u8; 4] = b"BTDX";

/// What a BA2 archive holds, from its 4-byte type tag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// `GNRL`: general files (meshes, scripts, sounds, ...)
    General,
    /// `DX10`: DirectX textures, stored as chunked mips
    Texture,
    /// Any other tag, preserved for reporting
    Other([u8; 4]),
}

impl ArchiveKind {
    fn from_tag(tag: [u8; 4]) -> Self {
        match &tag {
            b"GNRL" => Self::General,
            b"DX10" => Self::Texture,
            _ => Self::Other(tag),
        }
    }
}

/// The archive-wide compression codec, derived from the version and (v3) method field
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// zlib: Fallout 4 and Starfield v2
    Zlib,
    /// Raw LZ4: Starfield v3 with compression method 3
    Lz4,
}

/// Upper bound on one chunk's decompressed size, to reject corrup or hostile sizes (1 GiB)
const MAX_CHUNK_UNPACKED: u64 = 1 << 30;

/// The fixed 24-byte BA2 header shared by every FO4 version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Header {
    /// Format version: 1 = FO4 Old-Gen, 7/8 = FO4 Next-Gen
    pub version: u32,
    /// The archive's content kind
    pub kind: ArchiveKind,
    /// Number of files the archive contains
    pub file_count: u32,
    /// Byte offset of the file-name table, or 0 when absent
    pub name_table_offset: u64,
    /// The archive-wide compression codec
    pub compression: Compression,
}

/// Byte size of the header for a given version; records begin immediately after
fn header_size(version: u32) -> usize {
    match version {
        2 => 32,
        3 => 36,
        _ => 24,
    }
}

/// One general-archive file record
#[derive(Debug, Clone)]
pub struct GnrlEntry {
    /// Archive path (backslash-separated), if the name table was present
    pub path: Option<String>,
    /// Byte offset of the file data within the archive
    pub offset: u64,
    /// Stored size; 0 means the data is stored uncompressed
    pub packed_size: u32,
    /// Original (decompressed) size
    pub unpacked_size: u32,
}

/// One texture-archive record: a DX10 header plus its mip chunks
#[derive(Debug, Clone)]
pub struct Dx10Entry {
    /// Archive path, if the name table was present
    pub path: Option<String>,
    /// Texture height in pixels
    pub height: u16,
    /// Texture width in pixels
    pub width: u16,
    /// Mip-level count
    pub num_mips: u8,
    /// DXGI format code (opaque here; not interpreted)
    pub format: u8,
    /// Record flag bits; bit 0 set means the texture is a cubemap
    pub flags: u8,
    /// Texture tile mode: `8` is the std linear layout on Fallout4 PC
    pub tile_mode: u8,
    /// The texture's mip chunks, in order
    pub chunks: Vec<Dx10Chunk>,
}

/// One texture chunk: a contiguous range of mip levels
#[derive(Debug, Clone, Copy)]
pub struct Dx10Chunk {
    /// Byte offset of the chunk data within the archive
    pub offset: u64,
    /// Stored size; 0 means uncompressed
    pub packed_size: u32,
    /// Original (decompressed) size
    pub unpacked_size: u32,
    /// First mip level in this chunk
    pub start_mip: u16,
    /// Last mip level in this chunk
    pub end_mip: u16,
}

/// The parsed file records of a BA2 archive
#[derive(Debug, Clone)]
pub enum Entries {
    /// A `GNRL` archive's general files
    General(Vec<GnrlEntry>),
    /// A `DX10` archive's textures
    Texture(Vec<Dx10Entry>),
}

impl Entries {
    /// The general-file records, or `None` if this is a texture archive
    pub fn general(&self) -> Option<&[GnrlEntry]> {
        match self {
            Entries::General(v) => Some(v),
            Entries::Texture(_) => None,
        }
    }

    /// The texture records, or `None` if this is a general archive
    pub fn texture(&self) -> Option<&[Dx10Entry]> {
        match self {
            Entries::Texture(v) => Some(v),
            Entries::General(_) => None,
        }
    }
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

impl Header {
    /// Parse the BA2 header, accepting Fallout 4 (v1/7/8) and Starfield (v2/v3) layouts
    pub fn parse(b: &[u8]) -> Result<Self, ReadError> {
        if b.len() < 24 {
            return Err(ReadError::TooShort { what: "header" });
        }
        if &b[0..4] != MAGIC {
            return Err(ReadError::BadMagic);
        }
        let version = u32le(b, 4);
        let compression = match version {
            1 | 7 | 8 => Compression::Zlib,
            2 => {
                if b.len() < 32 {
                    return Err(ReadError::TooShort { what: "v2 header" });
                }
                Compression::Zlib
            }
            3 => {
                if b.len() < 36 {
                    return Err(ReadError::TooShort { what: "v3 header" });
                }
                match u32le(b, 32) {
                    0 => Compression::Zlib,
                    3 => Compression::Lz4,
                    m => return Err(ReadError::UnsupportedCompression(m)),
                }
            }
            v => return Err(ReadError::UnsupportedVersion(v)),
        };
        Ok(Self {
            version,
            kind: ArchiveKind::from_tag(b[8..12].try_into().unwrap()),
            file_count: u32le(b, 12),
            name_table_offset: u64le(b, 16),
            compression,
        })
    }
}

/// A parsed BA2 archive: borrowed bytes, header, and file records, ready for extraction
#[derive(Debug)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    header: Header,
    entries: Entries,
}

impl<'a> Archive<'a> {
    /// Read the header and every file record from a whole BA2 image held in memory
    pub fn read(bytes: &'a [u8]) -> Result<Self, ReadError> {
        let header = Header::parse(bytes)?;
        let start = header_size(header.version);
        // Reject an impossible file count early, so a hostile header cannot force a huge allocation.
        let min_record: u64 = if header.kind == ArchiveKind::Texture {
            24
        } else {
            36
        };
        if u64::from(header.file_count)
            .checked_mul(min_record)
            .is_none_or(|need| need > bytes.len() as u64)
        {
            return Err(ReadError::Malformed("file count exceeds archive size"));
        }
        let names = read_names(
            bytes,
            usize::try_from(header.name_table_offset)
                .map_err(|_| ReadError::Malformed("name table offset out of range"))?,
            header.file_count as usize,
        )?;
        let entries = match header.kind {
            ArchiveKind::General => Entries::General(read_gnrl(bytes, start, &names)?),
            ArchiveKind::Texture => Entries::Texture(read_dx10(bytes, start, &names)?),
            ArchiveKind::Other(tag) => return Err(ReadError::UnsupportedType(tag)),
        };
        Ok(Self {
            bytes,
            header,
            entries,
        })
    }

    /// The parsed header
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The parsed file records
    pub fn entries(&self) -> &Entries {
        &self.entries
    }

    /// Decompress (or copy) one general-archive file's bytes out of the archive image
    pub fn extract(&self, entry: &GnrlEntry) -> Result<Vec<u8>, ReadError> {
        read_chunk(
            self.bytes,
            self.header.compression,
            entry.offset,
            entry.packed_size,
            entry.unpacked_size,
        )
    }

    /// Rebuild a standalone DDS file for one DX10 texture record out of the archive image
    pub fn extract_texture(&self, entry: &Dx10Entry) -> Result<Vec<u8>, ReadError> {
        if entry.width == 0 || entry.height == 0 {
            return Err(ReadError::Malformed("texture has zero dimensions"));
        }
        if entry.chunks.is_empty() {
            return Err(ReadError::Malformed("texture has no chunks"));
        }
        if entry.tile_mode != 8 {
            return Err(ReadError::Unsupported("non-linear DX10 texture tile mode"));
        }
        let cubemap = entry.flags & 1 != 0;
        let mut out = dds::header(
            u32::from(entry.width),
            u32::from(entry.height),
            u32::from(entry.num_mips),
            u32::from(entry.format),
            cubemap,
        );
        for chunk in &entry.chunks {
            let data = read_chunk(
                self.bytes,
                self.header.compression,
                chunk.offset,
                chunk.packed_size,
                chunk.unpacked_size,
            )?;
            out.extend_from_slice(&data);
        }
        Ok(out)
    }
}

/// Read one chunk's decompressed bytes from the archive image, checking its declared size
fn read_chunk(
    bytes: &[u8],
    codec: Compression,
    offset: u64,
    packed_size: u32,
    unpacked_size: u32,
) -> Result<Vec<u8>, ReadError> {
    let off =
        usize::try_from(offset).map_err(|_| ReadError::Malformed("chunk offset out of range"))?;
    let stored = if packed_size == 0 {
        unpacked_size
    } else {
        packed_size
    } as usize;
    let end = off
        .checked_add(stored)
        .ok_or(ReadError::Malformed("chunk data out of bounds"))?;
    let data = bytes
        .get(off..end)
        .ok_or(ReadError::Malformed("chunk data out of bounds"))?;
    if packed_size == 0 {
        return Ok(data.to_vec());
    }
    if u64::from(unpacked_size) > MAX_CHUNK_UNPACKED {
        return Err(ReadError::TooLarge {
            size: u64::from(unpacked_size),
            limit: MAX_CHUNK_UNPACKED,
        });
    }
    match codec {
        Compression::Zlib => inflate(data, unpacked_size),
        Compression::Lz4 => lz4_block(data, unpacked_size),
    }
}

/// Inflate a zlib stream and require it to expand to exactly `unpacked_size` bytes
fn inflate(comp: &[u8], unpacked_size: u32) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(comp)
        .take(u64::from(unpacked_size) + 1)
        .read_to_end(&mut out)
        .map_err(ReadError::Zlib)?;
    if out.len() != unpacked_size as usize {
        return Err(ReadError::Malformed("decompressed chunk size mismatch"));
    }
    Ok(out)
}

/// Decode a raw LZ4 block into exactly `unpacked_size` bytes
fn lz4_block(comp: &[u8], unpacked_size: u32) -> Result<Vec<u8>, ReadError> {
    let mut out = vec![0u8; unpacked_size as usize];
    let written = lz4_flex::block::decompress_into(comp, &mut out).map_err(|_| ReadError::Lz4)?;
    if written != unpacked_size as usize {
        return Err(ReadError::Malformed("decompressed chunk size mismatch"));
    }
    Ok(out)
}

fn read_names(b: &[u8], off: usize, count: usize) -> Result<Vec<Option<String>>, ReadError> {
    if off == 0 {
        return Ok(vec![None; count]);
    }
    let mut names = Vec::with_capacity(count);
    let mut p = off;
    for _ in 0..count {
        let len_end = p
            .checked_add(2)
            .ok_or(ReadError::TooShort { what: "name table" })?;
        let len_bytes = b
            .get(p..len_end)
            .ok_or(ReadError::TooShort { what: "name table" })?;
        let len = u16::from_le_bytes([len_bytes[0], len_bytes[1]]) as usize;
        let end = len_end
            .checked_add(len)
            .ok_or(ReadError::TooShort { what: "name entry" })?;
        let name = b
            .get(len_end..end)
            .ok_or(ReadError::TooShort { what: "name entry" })?;
        names.push(Some(String::from_utf8_lossy(name).into_owned()));
        p = end;
    }
    Ok(names)
}

fn read_gnrl(
    b: &[u8],
    start: usize,
    names: &[Option<String>],
) -> Result<Vec<GnrlEntry>, ReadError> {
    let mut out = Vec::with_capacity(names.len());
    let mut p = start;
    for name in names {
        if p + 36 > b.len() {
            return Err(ReadError::TooShort {
                what: "GNRL record",
            });
        }
        let rec = &b[p..p + 36];
        out.push(GnrlEntry {
            path: name.clone(),
            offset: u64le(rec, 16),
            packed_size: u32le(rec, 24),
            unpacked_size: u32le(rec, 28),
        });
        p += 36;
    }
    Ok(out)
}

fn read_dx10(
    b: &[u8],
    start: usize,
    names: &[Option<String>],
) -> Result<Vec<Dx10Entry>, ReadError> {
    let mut out = Vec::with_capacity(names.len());
    let mut p = start;
    for name in names {
        if p + 24 > b.len() {
            return Err(ReadError::TooShort {
                what: "DX10 record",
            });
        }
        let rec = &b[p..p + 24];
        let num_chunks = rec[13] as usize;
        if u16le(rec, 14) != 24 {
            return Err(ReadError::Malformed("unexpected DX10 chunk header size"));
        }
        let (height, width, num_mips, format, flags, tile_mode) = (
            u16le(rec, 16),
            u16le(rec, 18),
            rec[20],
            rec[21],
            rec[22],
            rec[23],
        );
        p += 24;
        let mut chunks = Vec::with_capacity(num_chunks);
        for _ in 0..num_chunks {
            if p + 24 > b.len() {
                return Err(ReadError::TooShort { what: "DX10 chunk" });
            }
            let c = &b[p..p + 24];
            chunks.push(Dx10Chunk {
                offset: u64le(c, 0),
                packed_size: u32le(c, 8),
                unpacked_size: u32le(c, 12),
                start_mip: u16le(c, 16),
                end_mip: u16le(c, 18),
            });
            p += 24;
        }
        out.push(Dx10Entry {
            path: name.clone(),
            height,
            width,
            num_mips,
            format,
            flags,
            tile_mode,
            chunks,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Write;

    // Thin adapters over the Archive container, so the existing format tests read unchanged.
    fn read(bytes: &[u8]) -> Result<(Header, Entries), ReadError> {
        let a = Archive::read(bytes)?;
        Ok((*a.header(), a.entries().clone()))
    }
    fn extract(bytes: &[u8], entry: &GnrlEntry) -> Result<Vec<u8>, ReadError> {
        Archive::read(bytes)?.extract(entry)
    }
    fn extract_texture(bytes: &[u8], entry: &Dx10Entry) -> Result<Vec<u8>, ReadError> {
        Archive::read(bytes)?.extract_texture(entry)
    }

    // Build a one-file GNRL archive: header, one 36-byte record, the stored data, then the name table.
    fn gnrl_archive(name: &str, stored: &[u8], packed: u32, unpacked: u32) -> Vec<u8> {
        let data_off = 24 + 36;
        let name_off = data_off + stored.len();
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(name_off as u64).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"nif\0");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&(data_off as u64).to_le_bytes());
        b.extend_from_slice(&packed.to_le_bytes());
        b.extend_from_slice(&unpacked.to_le_bytes());
        b.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes());
        b.extend_from_slice(stored);
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b
    }

    fn zlib(payload: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(payload).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn parses_the_header() {
        let img = gnrl_archive("Meshes\\a.nif", b"data", 0, 4);
        let h = Header::parse(&img).unwrap();
        assert_eq!(h.version, 1);
        assert_eq!(h.kind, ArchiveKind::General);
        assert_eq!(h.file_count, 1);
    }

    #[test]
    fn rejects_a_bad_magic() {
        assert!(matches!(
            Header::parse(b"NOPE0000________________"),
            Err(ReadError::BadMagic)
        ));
    }

    #[test]
    fn reads_and_extracts_an_uncompressed_file() {
        let img = gnrl_archive("Meshes\\a.nif", b"hello world", 0, 11);
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_deref(), Some("Meshes\\a.nif"));
        assert_eq!(extract(&img, &files[0]).unwrap(), b"hello world");
    }

    #[test]
    fn reads_and_extracts_a_compressed_file() {
        let payload = b"the quick brown fox jumps over the lazy dog".repeat(16);
        let comp = zlib(&payload);
        let img = gnrl_archive(
            "Scripts\\b.pex",
            &comp,
            comp.len() as u32,
            payload.len() as u32,
        );
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert_eq!(extract(&img, &files[0]).unwrap(), payload);
    }

    // Build a one-texture DX10 archive with the given record fields and chunk payloads.
    fn dx10_archive(
        height: u16,
        width: u16,
        num_mips: u8,
        format: u8,
        flags: u8,
        tile_mode: u8,
        chunks: &[(Vec<u8>, bool)],
    ) -> Vec<u8> {
        let mut stored = Vec::new();
        for (payload, compress) in chunks {
            if *compress {
                let comp = zlib(payload);
                stored.push((comp.len() as u32, payload.len() as u32, comp));
            } else {
                stored.push((0u32, payload.len() as u32, payload.clone()));
            }
        }
        let data_start = 24 + 24 + chunks.len() * 24;
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"DX10");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&[0u8; 12]);
        b.push(0);
        b.push(chunks.len() as u8);
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&height.to_le_bytes());
        b.extend_from_slice(&width.to_le_bytes());
        b.push(num_mips);
        b.push(format);
        b.push(flags);
        b.push(tile_mode);
        let mut off = data_start;
        for (i, (packed, unpacked, data)) in stored.iter().enumerate() {
            b.extend_from_slice(&(off as u64).to_le_bytes());
            b.extend_from_slice(&packed.to_le_bytes());
            b.extend_from_slice(&unpacked.to_le_bytes());
            b.extend_from_slice(&(i as u16).to_le_bytes());
            b.extend_from_slice(&(i as u16).to_le_bytes());
            b.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes());
            off += data.len();
        }
        for (_, _, data) in &stored {
            b.extend_from_slice(data);
        }
        b
    }

    #[test]
    fn reads_and_extracts_a_texture() {
        let mip0 = vec![0xABu8; 64];
        let mip1 = vec![0xCDu8; 16];
        let img = dx10_archive(
            64,
            64,
            2,
            98,
            0,
            8,
            &[(mip0.clone(), false), (mip1.clone(), true)],
        );
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert_eq!(textures.len(), 1);
        let dds = extract_texture(&img, &textures[0]).unwrap();
        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(u32le(&dds, 0x0C), 64);
        assert_eq!(u32le(&dds, 0x10), 64);
        assert_eq!(u32le(&dds, 0x80), 98);
        let mut payload = mip0;
        payload.extend_from_slice(&mip1);
        assert_eq!(&dds[148..], &payload[..]);
    }

    #[test]
    fn rejects_a_tiled_texture() {
        let img = dx10_archive(64, 64, 1, 98, 0, 0, &[(vec![0u8; 64], false)]);
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert!(matches!(
            extract_texture(&img, &textures[0]),
            Err(ReadError::Unsupported(_))
        ));
    }

    #[test]
    fn extracts_a_cubemap_header() {
        let img = dx10_archive(32, 32, 1, 98, 1, 8, &[(vec![0u8; 64], false)]);
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert_eq!(textures[0].flags, 1);
        let dds = extract_texture(&img, &textures[0]).unwrap();
        assert_eq!(u32le(&dds, 0x70), 0xFE00);
        assert_eq!(u32le(&dds, 0x88), 0x4);
    }

    #[test]
    fn rejects_a_short_header() {
        assert!(matches!(
            Header::parse(b"BTDX00"),
            Err(ReadError::TooShort { .. })
        ));
    }

    #[test]
    fn rejects_an_unsupported_type() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"MEOW");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(read(&b), Err(ReadError::UnsupportedType(t)) if &t == b"MEOW"));
    }

    #[test]
    fn extract_rejects_out_of_bounds_data() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&[0u8; 12]);
        b.push(0);
        b.push(1);
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&10_000u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes());
        let archive = Archive::read(&b).unwrap();
        let files = archive.entries().general().unwrap();
        assert!(matches!(
            archive.extract(&files[0]),
            Err(ReadError::Malformed(_))
        ));
    }

    #[test]
    fn extract_detects_a_decompressed_size_mismatch() {
        let comp = zlib(b"hello world");
        let img = gnrl_archive("Meshes\\a.nif", &comp, comp.len() as u32, 99);
        let (_h, entries) = read(&img).unwrap();
        let Entries::General(files) = entries else {
            panic!("expected general");
        };
        assert!(matches!(
            extract(&img, &files[0]),
            Err(ReadError::Malformed(_))
        ));
    }

    #[test]
    fn reads_a_multi_chunk_texture() {
        let mip0 = vec![1u8; 100];
        let mip1 = vec![2u8; 20];
        let img = dx10_archive(
            64,
            64,
            2,
            98,
            0,
            8,
            &[(mip0.clone(), false), (mip1.clone(), true)],
        );
        let (_h, entries) = read(&img).unwrap();
        let Entries::Texture(textures) = entries else {
            panic!("expected texture");
        };
        assert_eq!(textures[0].chunks.len(), 2);
        assert_eq!(
            (
                textures[0].chunks[0].start_mip,
                textures[0].chunks[0].end_mip
            ),
            (0, 0)
        );
        assert_eq!(
            (
                textures[0].chunks[1].start_mip,
                textures[0].chunks[1].end_mip
            ),
            (1, 1)
        );
        let dds = extract_texture(&img, &textures[0]).unwrap();
        let mut payload = mip0;
        payload.extend_from_slice(&mip1);
        assert_eq!(&dds[148..], &payload[..]);
    }

    // Build a one-file Starfield v2 GNRL archive: 32-byte header, one record, data, name table.
    fn sf_gnrl_v2(name: &str, stored: &[u8], packed: u32, unpacked: u32) -> Vec<u8> {
        let data_off = 32 + 36;
        let name_off = data_off + stored.len();
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(name_off as u64).to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"nif\0");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.push(0);
        b.push(1);
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&(data_off as u64).to_le_bytes());
        b.extend_from_slice(&packed.to_le_bytes());
        b.extend_from_slice(&unpacked.to_le_bytes());
        b.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes());
        b.extend_from_slice(stored);
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b
    }

    // Build a Starfield DX10 archive header (32 bytes for v2, 36 for v3 with a method).
    fn sf_dx10_header(version: u32, method: Option<u32>) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&version.to_le_bytes());
        b.extend_from_slice(b"DX10");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        if let Some(m) = method {
            b.extend_from_slice(&m.to_le_bytes());
        }
        b
    }

    // Append a one-texture DX10 record and its chunks to an archive header already in `b`.
    fn sf_dx10_body(
        mut b: Vec<u8>,
        width: u16,
        height: u16,
        num_mips: u8,
        format: u8,
        chunks: &[(Vec<u8>, u32, u32, u16, u16)],
    ) -> Vec<u8> {
        let data_start = b.len() + 24 + chunks.len() * 24;
        b.extend_from_slice(&[0u8; 12]);
        b.push(0);
        b.push(chunks.len() as u8);
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&height.to_le_bytes());
        b.extend_from_slice(&width.to_le_bytes());
        b.push(num_mips);
        b.push(format);
        b.push(0);
        b.push(8);
        let mut off = data_start;
        for (payload, packed, unpacked, sm, em) in chunks {
            b.extend_from_slice(&(off as u64).to_le_bytes());
            b.extend_from_slice(&packed.to_le_bytes());
            b.extend_from_slice(&unpacked.to_le_bytes());
            b.extend_from_slice(&sm.to_le_bytes());
            b.extend_from_slice(&em.to_le_bytes());
            b.extend_from_slice(&0xBAAD_F00Du32.to_le_bytes());
            off += payload.len();
        }
        for (payload, _, _, _, _) in chunks {
            b.extend_from_slice(payload);
        }
        b
    }

    // Build a one-texture Starfield v3 DX10 archive; chunks are (payload, packed, unpacked, startMip, endMip).
    fn sf_dx10_v3(
        method: u32,
        width: u16,
        height: u16,
        num_mips: u8,
        format: u8,
        chunks: &[(Vec<u8>, u32, u32, u16, u16)],
    ) -> Vec<u8> {
        sf_dx10_body(
            sf_dx10_header(3, Some(method)),
            width,
            height,
            num_mips,
            format,
            chunks,
        )
    }

    // Build a one-texture Starfield v2 DX10 archive (32-byte header, always zlib).
    fn sf_dx10_v2(
        width: u16,
        height: u16,
        num_mips: u8,
        format: u8,
        chunks: &[(Vec<u8>, u32, u32, u16, u16)],
    ) -> Vec<u8> {
        sf_dx10_body(
            sf_dx10_header(2, None),
            width,
            height,
            num_mips,
            format,
            chunks,
        )
    }

    fn lz4_compress(data: &[u8]) -> Vec<u8> {
        lz4_flex::block::compress(data)
    }

    #[test]
    fn parses_a_v2_header() {
        let img = sf_gnrl_v2("strings\\a.strings", b"data", 0, 4);
        let h = Header::parse(&img).unwrap();
        assert_eq!(h.version, 2);
        assert_eq!(h.compression, Compression::Zlib);
    }

    #[test]
    fn parses_a_v3_lz4_header() {
        let img = sf_dx10_v3(3, 4, 4, 1, 98, &[(vec![0u8; 8], 8, 16, 0, 0)]);
        let h = Header::parse(&img).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.compression, Compression::Lz4);
    }

    #[test]
    fn v3_method_zero_is_zlib() {
        let img = sf_dx10_v3(0, 4, 4, 1, 98, &[(vec![0u8; 8], 0, 8, 0, 0)]);
        assert_eq!(Header::parse(&img).unwrap().compression, Compression::Zlib);
    }

    #[test]
    fn rejects_an_unknown_version() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(
            Header::parse(&b),
            Err(ReadError::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn rejects_an_unknown_compression_method() {
        let img = sf_dx10_v3(5, 4, 4, 1, 98, &[(vec![0u8; 8], 8, 16, 0, 0)]);
        assert!(matches!(
            Header::parse(&img),
            Err(ReadError::UnsupportedCompression(5))
        ));
    }

    #[test]
    fn rejects_a_truncated_v3_header() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"DX10");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        assert!(matches!(Header::parse(&b), Err(ReadError::TooShort { .. })));
    }

    #[test]
    fn reads_v2_records_after_the_extended_header() {
        let img = sf_gnrl_v2("strings\\quest.strings", b"hello", 0, 5);
        let archive = Archive::read(&img).unwrap();
        let files = archive.entries().general().unwrap();
        assert_eq!(files[0].path.as_deref(), Some("strings\\quest.strings"));
        assert_eq!(archive.extract(&files[0]).unwrap(), b"hello");
    }

    #[test]
    fn reads_a_v3_lz4_texture() {
        let mip0: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let mip1: Vec<u8> = (0..16u32).map(|i| (i * 3) as u8).collect();
        let c0 = lz4_compress(&mip0);
        let c1 = lz4_compress(&mip1);
        let img = sf_dx10_v3(
            3,
            8,
            8,
            2,
            98,
            &[
                (c0.clone(), c0.len() as u32, mip0.len() as u32, 0, 0),
                (c1.clone(), c1.len() as u32, mip1.len() as u32, 1, 1),
            ],
        );
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        let dds = archive.extract_texture(&textures[0]).unwrap();
        let mut payload = mip0.clone();
        payload.extend_from_slice(&mip1);
        assert_eq!(&dds[148..], &payload[..]);
    }

    #[test]
    fn lz4_archive_stored_chunk_bypasses_lz4() {
        let raw = vec![7u8; 40];
        let img = sf_dx10_v3(3, 4, 4, 1, 98, &[(raw.clone(), 0, raw.len() as u32, 0, 0)]);
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        let dds = archive.extract_texture(&textures[0]).unwrap();
        assert_eq!(&dds[148..], &raw[..]);
    }

    #[test]
    fn lz4_archive_rejects_zlib_data() {
        let zlibbed = zlib(&[9u8; 32]);
        let img = sf_dx10_v3(
            3,
            4,
            4,
            1,
            98,
            &[(zlibbed.clone(), zlibbed.len() as u32, 32, 0, 0)],
        );
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        assert!(archive.extract_texture(&textures[0]).is_err());
    }

    #[test]
    fn rejects_an_oversized_chunk() {
        let img = sf_dx10_v3(3, 4, 4, 1, 98, &[(vec![0u8; 8], 8, (1u32 << 30) + 1, 0, 0)]);
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        assert!(matches!(
            archive.extract_texture(&textures[0]),
            Err(ReadError::TooLarge { .. })
        ));
    }

    #[test]
    fn rejects_an_impossible_file_count() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&1_000_000u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(Archive::read(&b), Err(ReadError::Malformed(_))));
    }

    #[test]
    fn a_hostile_name_offset_does_not_panic() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(b"GNRL");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&u64::MAX.to_le_bytes());
        b.extend_from_slice(&[0u8; 36]);
        assert!(Archive::read(&b).is_err());
    }

    #[test]
    fn reads_a_v3_zlib_texture() {
        let payload = vec![5u8; 200];
        let comp = zlib(&payload);
        let img = sf_dx10_v3(
            0,
            8,
            8,
            1,
            98,
            &[(comp.clone(), comp.len() as u32, payload.len() as u32, 0, 0)],
        );
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        let dds = archive.extract_texture(&textures[0]).unwrap();
        assert_eq!(&dds[148..], &payload[..]);
    }

    #[test]
    fn reads_a_v2_dx10_zlib_texture() {
        let payload = vec![3u8; 128];
        let comp = zlib(&payload);
        let img = sf_dx10_v2(
            8,
            8,
            1,
            98,
            &[(comp.clone(), comp.len() as u32, payload.len() as u32, 0, 0)],
        );
        let archive = Archive::read(&img).unwrap();
        assert_eq!(archive.header().compression, Compression::Zlib);
        let textures = archive.entries().texture().unwrap();
        let dds = archive.extract_texture(&textures[0]).unwrap();
        assert_eq!(&dds[148..], &payload[..]);
    }

    #[test]
    fn lz4_chunk_with_zero_unpacked_but_data_is_rejected() {
        let comp = lz4_compress(&[1u8, 2, 3]);
        let img = sf_dx10_v3(
            3,
            4,
            4,
            1,
            98,
            &[(comp.clone(), comp.len() as u32, 0, 0, 0)],
        );
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        assert!(archive.extract_texture(&textures[0]).is_err());
    }

    #[test]
    fn a_texture_with_no_chunks_is_rejected() {
        let img = sf_dx10_v3(3, 8, 8, 1, 98, &[]);
        let archive = Archive::read(&img).unwrap();
        let textures = archive.entries().texture().unwrap();
        assert!(matches!(
            archive.extract_texture(&textures[0]),
            Err(ReadError::Malformed(_))
        ));
    }

    proptest! {
        // LZ4-compressed chunks round-trip through a v3 texture archive.
        #[test]
        fn v3_lz4_round_trips(payload in proptest::collection::vec(any::<u8>(), 1..2048)) {
            let comp = lz4_compress(&payload);
            let img = sf_dx10_v3(3, 16, 16, 1, 98,
                &[(comp.clone(), comp.len() as u32, payload.len() as u32, 0, 0)]);
            let archive = Archive::read(&img).unwrap();
            let textures = archive.entries().texture().unwrap();
            let dds = archive.extract_texture(&textures[0]).unwrap();
            prop_assert_eq!(&dds[148..], &payload[..]);
        }
    }

    // Build a BTDX archive with a plausible FO4 or Starfield header but otherwise random body.
    fn arbitrary_archive() -> impl Strategy<Value = Vec<u8>> {
        (
            prop::sample::select(vec![*b"GNRL", *b"DX10", *b"MEOW"]),
            prop::sample::select(vec![1u32, 2, 3, 4, 7, 8]),
            0u32..4,
            prop_oneof![Just(0u64), Just(u64::MAX), Just(u64::MAX - 1), any::<u64>()],
            proptest::collection::vec(any::<u8>(), 0..400),
        )
            .prop_map(|(tag, version, count, name_off, tail)| {
                let mut b = Vec::new();
                b.extend_from_slice(MAGIC);
                b.extend_from_slice(&version.to_le_bytes());
                b.extend_from_slice(&tag);
                b.extend_from_slice(&count.to_le_bytes());
                b.extend_from_slice(&name_off.to_le_bytes());
                if version >= 2 {
                    b.extend_from_slice(&1u64.to_le_bytes());
                }
                if version == 3 {
                    b.extend_from_slice(&3u32.to_le_bytes());
                }
                b.extend_from_slice(&tail);
                b
            })
    }

    proptest! {
        // Reading and extracting arbitrary or near-valid bytes must never panic.
        #[test]
        fn read_never_panics(
            bytes in prop_oneof![
                proptest::collection::vec(any::<u8>(), 0..512),
                arbitrary_archive(),
            ],
        ) {
            if let Ok((_h, entries)) = read(&bytes) {
                match entries {
                    Entries::General(files) => {
                        for f in &files {
                            let _ = extract(&bytes, f);
                        }
                    }
                    Entries::Texture(textures) => {
                        for t in &textures {
                            let _ = extract_texture(&bytes, t);
                        }
                    }
                }
            }
        }
    }
}
