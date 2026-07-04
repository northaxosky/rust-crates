//! Writers for Fallout 4 and Starfield BA2 archives (`BTDX`).

use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

mod dx10;
mod gnrl;

pub use dx10::Dx10Writer;
pub use gnrl::GnrlWriter;

const MAGIC: &[u8; 4] = b"BTDX";
const CHUNK_SENTINEL: u32 = 0xBAAD_F00D;
const MAX_PATH_LEN: usize = 260;
const STARFIELD_HEADER_TAIL: u64 = 1;
const COMPRESSION_METHOD_LZ4: u32 = 3;

/// The BA2 output format: archive version and its archive-wide compression codec
///
/// These are the formats Fallout 4 and Starfield actually ship. `Fo4` is a version-1 archive that
/// loads on every Fallout 4 build; `StarfieldV2` and `StarfieldV3Lz4` are the versions Starfield's
/// Archive2 emits (general archives and textures respectively). Other valid `BTDX` encodings (v7/v8,
/// or v3 with zlib) exist but are not produced here; the enum is `#[non_exhaustive]` so they can be
/// added later without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ba2Format {
    /// Fallout 4 version 1, zlib
    Fo4,
    /// Starfield version 2, zlib
    StarfieldV2,
    /// Starfield version 3, raw LZ4 block
    StarfieldV3Lz4,
}

impl Ba2Format {
    /// The BA2 `version` field this format writes
    fn version(self) -> u32 {
        match self {
            Ba2Format::Fo4 => 1,
            Ba2Format::StarfieldV2 => 2,
            Ba2Format::StarfieldV3Lz4 => 3,
        }
    }

    /// Byte size of the header this format writes; records begin immediately after
    fn header_size(self) -> u64 {
        match self {
            Ba2Format::Fo4 => 24,
            Ba2Format::StarfieldV2 => 32,
            Ba2Format::StarfieldV3Lz4 => 36,
        }
    }

    /// Whether this format compresses chunks with raw LZ4 block rather than zlib
    fn uses_lz4(self) -> bool {
        matches!(self, Ba2Format::StarfieldV3Lz4)
    }
}

/// Render path bytes as a lossy string for error messages
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Compress `data` into a standalone zlib stream at the default level
fn zlib_compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

/// Compress `data` with `format`'s archive-wide codec; only the zlib path can fail
fn compress(format: Ba2Format, data: &[u8]) -> std::io::Result<Vec<u8>> {
    if format.uses_lz4() {
        Ok(lz4_flex::block::compress(data))
    } else {
        zlib_compress(data)
    }
}

/// Write the BA2 header: magic, version, type tag, file count, name-table offset, and any
/// format-specific trailing fields (the Starfield `u64` and the v3 compression-method `u32`)
fn write_header(
    out: &mut Vec<u8>,
    format: Ba2Format,
    tag: &[u8; 4],
    file_count: u32,
    names_offset: u64,
) {
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&format.version().to_le_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(&file_count.to_le_bytes());
    out.extend_from_slice(&names_offset.to_le_bytes());
    match format {
        Ba2Format::Fo4 => {}
        Ba2Format::StarfieldV2 => {
            out.extend_from_slice(&STARFIELD_HEADER_TAIL.to_le_bytes());
        }
        Ba2Format::StarfieldV3Lz4 => {
            out.extend_from_slice(&STARFIELD_HEADER_TAIL.to_le_bytes());
            out.extend_from_slice(&COMPRESSION_METHOD_LZ4.to_le_bytes());
        }
    }
}
