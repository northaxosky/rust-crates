//! Writers for Fallout 4 BA2 archives (`BTDX`).

use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

mod dx10;
mod gnrl;

pub use dx10::Dx10Writer;
pub use gnrl::GnrlWriter;

const MAGIC: &[u8; 4] = b"BTDX";
const HEADER_SIZE: u64 = 24;
const CHUNK_SENTINEL: u32 = 0xBAAD_F00D;
const MAX_PATH_LEN: usize = 260;

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
