#![doc = include_str!("../README.md")]
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
