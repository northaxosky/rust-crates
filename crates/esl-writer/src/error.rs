//! Error type for writing Bethesda light-master (ESL) plugins.

use thiserror::Error;

/// Why a plugin could not be serialized
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteError {
    /// A string field held a character not representable in Windows-1252
    #[error("{field} contains a character not representable in Windows-1252: {ch:?}")]
    Encoding {
        /// The field being written (`author`, `description`, or `master`)
        field: &'static str,
        /// The character that could not be encoded
        ch: char,
    },
    /// A string field contained an interior NUL, which a zstring cannot represent
    #[error("{field} contains an interior NUL byte")]
    InteriorNul {
        /// The field being written
        field: &'static str,
    },
    /// A string field's encoded length did not fit the 16-bit field size
    #[error("{field} is too long: {len} bytes exceeds the 65535-byte field limit")]
    StringTooLong {
        /// The field being written
        field: &'static str,
        /// The encoded byte length including the NUL terminator
        len: usize,
    },
    /// The TES4 record payload exceeded the 32-bit record size field
    #[error("record payload is too long: {len} bytes exceeds the 4 GiB record limit")]
    RecordTooLong {
        /// The record payload length that overflowed
        len: usize,
    },
}
