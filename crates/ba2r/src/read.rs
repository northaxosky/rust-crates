//! Reader for Fallout 4 BA2 archives: header, records, and file/texture extraction.

use std::io::Read;

use crate::dds;
use crate::error::Ba2Error;

const MAGIC: &[u8; 4] = b"BTDX";

/// What a BA2 archive holds, from its 4-byte type tag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ba2Kind {
    /// `GNRL`: general files (meshes, scripts, sounds, ...)
    General,
    /// `DX10`: DirectX textures, stored as chunked mips
    Texture,
    /// Any other tag, preserved for reporting
    Other([u8; 4]),
}

impl Ba2Kind {
    fn from_tag(tag: [u8; 4]) -> Self {
        match &tag {
            b"GNRL" => Self::General,
            b"DX10" => Self::Texture,
            _ => Self::Other(tag),
        }
    }
}

/// The fixed 24-byte BA2 header shared by every FO4 version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Format version: 1 = FO4 Old-Gen, 7/8 = FO4 Next-Gen
    pub version: u32,
    /// The archive's content kind
    pub kind: Ba2Kind,
    /// Number of files the archive contains
    pub file_count: u32,
    /// Byte offset of the file-name table, or 0 when absent
    pub name_table_offset: u64,
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
    /// Parse the 24-byte header from the start of a BA2
    pub fn parse(b: &[u8]) -> Result<Self, Ba2Error> {
        if b.len() < 24 {
            return Err(Ba2Error::TooShort { what: "header" });
        }
        if &b[0..4] != MAGIC {
            return Err(Ba2Error::BadMagic);
        }
        Ok(Self {
            version: u32le(b, 4),
            kind: Ba2Kind::from_tag(b[8..12].try_into().unwrap()),
            file_count: u32le(b, 12),
            name_table_offset: u64le(b, 16),
        })
    }
}

/// Read the header and every file record from a whole BA2 image held in memory
pub fn read(bytes: &[u8]) -> Result<(Header, Entries), Ba2Error> {
    let header = Header::parse(bytes)?;
    let names = read_names(
        bytes,
        header.name_table_offset as usize,
        header.file_count as usize,
    )?;
    let entries = match header.kind {
        Ba2Kind::General => Entries::General(read_gnrl(bytes, &names)?),
        Ba2Kind::Texture => Entries::Texture(read_dx10(bytes, &names)?),
        Ba2Kind::Other(tag) => return Err(Ba2Error::UnsupportedType(tag)),
    };
    Ok((header, entries))
}

/// Decompress (or copy) one general-archive file's bytes out of the archive image
pub fn extract(bytes: &[u8], entry: &GnrlEntry) -> Result<Vec<u8>, Ba2Error> {
    read_chunk(bytes, entry.offset, entry.packed_size, entry.unpacked_size)
}

/// Rebuild a standalone DDS file for one DX10 texture record out of the archive image
pub fn extract_texture(bytes: &[u8], entry: &Dx10Entry) -> Result<Vec<u8>, Ba2Error> {
    if entry.width == 0 || entry.height == 0 {
        return Err(Ba2Error::Malformed("texture has zero dimensions"));
    }
    if entry.chunks.is_empty() {
        return Err(Ba2Error::Malformed("texture has no chunks"));
    }
    if entry.tile_mode != 8 {
        return Err(Ba2Error::Unsupported("non-linear DX10 texture tile mode"));
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
        let data = read_chunk(bytes, chunk.offset, chunk.packed_size, chunk.unpacked_size)?;
        out.extend_from_slice(&data);
    }
    Ok(out)
}

/// Read one chunk's decompressed bytes from the archive image, checking its declared size
fn read_chunk(
    bytes: &[u8],
    offset: u64,
    packed_size: u32,
    unpacked_size: u32,
) -> Result<Vec<u8>, Ba2Error> {
    let off =
        usize::try_from(offset).map_err(|_| Ba2Error::Malformed("chunk offset out of range"))?;
    let stored = if packed_size == 0 {
        unpacked_size
    } else {
        packed_size
    } as usize;
    let end = off
        .checked_add(stored)
        .ok_or(Ba2Error::Malformed("chunk data out of bounds"))?;
    let data = bytes
        .get(off..end)
        .ok_or(Ba2Error::Malformed("chunk data out of bounds"))?;
    if packed_size == 0 {
        Ok(data.to_vec())
    } else {
        inflate(data, unpacked_size)
    }
}

/// Inflate a zlib stream and require it to expand to exactly `unpacked_size` bytes
fn inflate(comp: &[u8], unpacked_size: u32) -> Result<Vec<u8>, Ba2Error> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(comp)
        .take(u64::from(unpacked_size) + 1)
        .read_to_end(&mut out)
        .map_err(Ba2Error::Zlib)?;
    if out.len() != unpacked_size as usize {
        return Err(Ba2Error::Malformed("decompressed chunk size mismatch"));
    }
    Ok(out)
}

fn read_names(b: &[u8], off: usize, count: usize) -> Result<Vec<Option<String>>, Ba2Error> {
    if off == 0 {
        return Ok(vec![None; count]);
    }
    let mut names = Vec::with_capacity(count);
    let mut p = off;
    for _ in 0..count {
        if p + 2 > b.len() {
            return Err(Ba2Error::TooShort { what: "name table" });
        }
        let len = u16le(b, p) as usize;
        p += 2;
        let end = p + len;
        if end > b.len() {
            return Err(Ba2Error::TooShort { what: "name entry" });
        }
        names.push(Some(String::from_utf8_lossy(&b[p..end]).into_owned()));
        p = end;
    }
    Ok(names)
}

fn read_gnrl(b: &[u8], names: &[Option<String>]) -> Result<Vec<GnrlEntry>, Ba2Error> {
    let mut out = Vec::with_capacity(names.len());
    let mut p = 24;
    for name in names {
        if p + 36 > b.len() {
            return Err(Ba2Error::TooShort {
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

fn read_dx10(b: &[u8], names: &[Option<String>]) -> Result<Vec<Dx10Entry>, Ba2Error> {
    let mut out = Vec::with_capacity(names.len());
    let mut p = 24;
    for name in names {
        if p + 24 > b.len() {
            return Err(Ba2Error::TooShort {
                what: "DX10 record",
            });
        }
        let rec = &b[p..p + 24];
        let num_chunks = rec[13] as usize;
        if u16le(rec, 14) != 24 {
            return Err(Ba2Error::Malformed("unexpected DX10 chunk header size"));
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
                return Err(Ba2Error::TooShort { what: "DX10 chunk" });
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
        assert_eq!(h.kind, Ba2Kind::General);
        assert_eq!(h.file_count, 1);
    }

    #[test]
    fn rejects_a_bad_magic() {
        assert!(matches!(
            Header::parse(b"NOPE0000________________"),
            Err(Ba2Error::BadMagic)
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
            Err(Ba2Error::Unsupported(_))
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
            Err(Ba2Error::TooShort { .. })
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
        assert!(matches!(read(&b), Err(Ba2Error::UnsupportedType(t)) if &t == b"MEOW"));
    }

    #[test]
    fn extract_rejects_out_of_bounds_data() {
        let entry = GnrlEntry {
            path: None,
            offset: 10_000,
            packed_size: 0,
            unpacked_size: 8,
        };
        assert!(matches!(
            extract(&[0u8; 64], &entry),
            Err(Ba2Error::Malformed(_))
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
            Err(Ba2Error::Malformed(_))
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

    // Build a BTDX archive with a plausible header but otherwise random body.
    fn arbitrary_archive() -> impl Strategy<Value = Vec<u8>> {
        (
            prop::sample::select(vec![*b"GNRL", *b"DX10", *b"MEOW"]),
            0u32..4,
            any::<u64>(),
            proptest::collection::vec(any::<u8>(), 0..400),
        )
            .prop_map(|(tag, count, name_off, tail)| {
                let mut b = Vec::new();
                b.extend_from_slice(MAGIC);
                b.extend_from_slice(&1u32.to_le_bytes());
                b.extend_from_slice(&tag);
                b.extend_from_slice(&count.to_le_bytes());
                b.extend_from_slice(&name_off.to_le_bytes());
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
