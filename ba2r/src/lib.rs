//! Pure-Rust reader for Fallout 4 Bethesda archives (BA2, magic `BTDX`).
//!
//! Handles the general (`GNRL`) and texture (`DX10`) variants across FO4 versions v1 (Old-Gen) and
//! v7/v8 (Next-Gen). Reading is verified byte-exact against the `ba2` crate on real archives; writing
//! is not yet implemented (see the workspace `AGENTS.md`).

#![forbid(unsafe_code)]

use std::io::Read;
use thiserror::Error;

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

/// Why a BA2 could not be read
#[derive(Debug, Error)]
pub enum Ba2Error {
    /// The buffer ended before a required structure could be read
    #[error("buffer is too short to hold a BA2 {what}")]
    TooShort {
        /// The structure that overran the buffer
        what: &'static str,
    },
    /// The buffer did not begin with the `BTDX` magic
    #[error("not a BA2 archive (missing BTDX magic)")]
    BadMagic,
    /// The archive type tag is neither `GNRL` nor `DX10`
    #[error("unsupported archive type {0:?}")]
    UnsupportedType([u8; 4]),
    /// A structural invariant was violated
    #[error("malformed archive: {0}")]
    Malformed(&'static str),
    /// A compressed file or chunk failed to inflate
    #[error("zlib decompression failed")]
    Zlib(#[source] std::io::Error),
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
    let off = entry.offset as usize;
    if entry.packed_size == 0 {
        let end = off + entry.unpacked_size as usize;
        bytes
            .get(off..end)
            .map(<[u8]>::to_vec)
            .ok_or(Ba2Error::Malformed("file data out of bounds"))
    } else {
        let end = off + entry.packed_size as usize;
        let comp = bytes
            .get(off..end)
            .ok_or(Ba2Error::Malformed("file data out of bounds"))?;
        let mut out = Vec::with_capacity(entry.unpacked_size as usize);
        flate2::read::ZlibDecoder::new(comp)
            .read_to_end(&mut out)
            .map_err(Ba2Error::Zlib)?;
        Ok(out)
    }
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
        let (height, width, num_mips, format) = (u16le(rec, 16), u16le(rec, 18), rec[20], rec[21]);
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
            chunks,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
