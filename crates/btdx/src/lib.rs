//! Pure-Rust reader and writer for Fallout 4 Bethesda archives (BA2, magic `BTDX`).
//!
//! Handles the general (`GNRL`) and texture (`DX10`) variants across FO4 versions v1 (Old-Gen) and
//! v7/v8 (Next-Gen). Reading is verified byte-exact against the `ba2` crate on real archives; the
//! writer emits version-1 `GNRL` archives (see [`GnrlWriter`]).

#![forbid(unsafe_code)]

mod dds;
mod error;
mod hashing;
mod read;
mod write;

pub use error::{DdsError, Error, WriteError};
pub use read::{
    ArchiveKind, Dx10Chunk, Dx10Entry, Entries, GnrlEntry, Header, extract, extract_texture, read,
};
pub use write::{Dx10Writer, GnrlWriter};
