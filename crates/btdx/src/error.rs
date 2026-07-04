//! Error types for reading and writing Fallout 4 BA2 archives.

use thiserror::Error;

/// Why a BA2 could not be read
#[derive(Debug, Error)]
pub enum Error {
    /// The buffer ended before a required structure could be read
    #[error("buffer is too short to hold a BA2 {what}")]
    TooShort { what: &'static str },
    /// The buffer did not begin with the `BTDX` magic
    #[error("not a BA2 archive (missing BTDX magic)")]
    BadMagic,
    /// The archive type tag is neither `GNRL` nor `DX10`
    #[error("unsupported archive type {0:?}")]
    UnsupportedType([u8; 4]),
    /// A structural invariant was violated
    #[error("malformed archive: {0}")]
    Malformed(&'static str),
    /// A feature or encoding the reader does not support (console)
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    /// A compressed file or chunk failed to inflate
    #[error("zlib decompression failed")]
    Zlib(#[source] std::io::Error),
}

/// Why a GNRL archive could not be written
#[derive(Debug, Error)]
pub enum WriteError {
    /// A path was empty, only separators, or too long after normalization
    #[error("invalid archive path: {reason}")]
    InvalidPath { reason: &'static str },
    /// Two files normalized to the same path
    #[error("duplicate file path: {0}")]
    DuplicatePath(String),
    /// Two different paths produced the same BA2 key, so the game could not tell them apart
    #[error("hash collision between {first} and {second}")]
    HashCollision { first: String, second: String },
    /// A single file's stored or original length does not fit the 32-bit BA2 size field
    #[error("file exceeds the BA2 32-bit size field (4 GiB per file): {path} ({size} bytes)")]
    FileTooLarge { path: String, size: usize },
    /// The archive holds more files than the 32-bit count field allows
    #[error("too many files for a single BA2: {0}")]
    TooManyFiles(usize),
    /// The archive's byte offsets overflowed a 64-bit integer
    #[error("archive too large: byte offsets overflowed")]
    OffsetOverflow,
    /// zlib compression of a file failed
    #[error("zlib compression failed for {path}")]
    ZlibCompress {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// An input DDS texture could not be parsed
    #[error("invalid DDS for {path}: {source}")]
    Dds { path: String, source: DdsError },
    /// Writing the archive to the output sink failed
    #[error("failed to write archive")]
    Io(#[source] std::io::Error),
}

/// Why a DDS texture could not be parsed for packing into a DX10 archive
#[derive(Debug, Error)]
pub enum DdsError {
    /// The data did not begin with the `DDS ` magic
    #[error("not a DDS file")]
    NotDds,
    /// The header or pixel data was shorter than the format requires
    #[error("DDS data is truncated")]
    Truncated,
    /// The DXGI format is not one this crate can size
    #[error("unsupported DXGI format: {0}")]
    UnsupportedFormat(u8),
    /// The texture shape cannot be represented in a BA2 DX10 record
    #[error("unsupported DDS shape: {0}")]
    UnsupportedShape(&'static str),
    /// The pixel data length did not match the format, dimensions, and mip count
    #[error("DDS pixel data does not match its header")]
    SizeMismatch,
}
