#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod dds;
mod error;
mod hashing;
mod read;
mod write;

pub use error::{DdsError, ReadError, WriteError};
pub use read::{
    Archive, ArchiveKind, Compression, Dx10Chunk, Dx10Entry, Entries, GnrlEntry, Header,
};
pub use write::{Ba2Format, Dx10Writer, GnrlWriter};
