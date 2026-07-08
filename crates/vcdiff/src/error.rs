//! Error type for decoding VCDIFF deltas.

use thiserror::Error;

/// Why a VCDIFF delta could not be decoded
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The delta did not begin with the VCDIFF magic bytes
    #[error("not a VCDIFF delta (missing magic)")]
    BadMagic,
    /// The header version byte was not the supported value 0
    #[error("unsupported VCDIFF version {0}")]
    UnsupportedVersion(u8),
    /// The delta ended before a required structure could be read
    #[error("unexpected end of delta")]
    UnexpectedEof,
    /// A variable-length integer did not fit in 64 bits
    #[error("integer overflow while decoding a varint")]
    IntegerOverflow,
    /// The delta declares a secondary compressor, which this crate does not support
    #[error("secondary compression is not supported")]
    UnsupportedSecondaryCompressor,
    /// The delta carries a custom code table, which this crate does not support
    #[error("custom code tables are not supported")]
    UnsupportedCodeTable,
    /// The header indicator had reserved bits set
    #[error("invalid header indicator {0:#04x}")]
    InvalidHeaderIndicator(u8),
    /// The window indicator had reserved or conflicting bits set
    #[error("invalid window indicator {0:#04x}")]
    InvalidWindowIndicator(u8),
    /// The delta indicator requested secondary compression of a section
    #[error("invalid delta indicator {0:#04x}")]
    InvalidDeltaIndicator(u8),
    /// An instruction opcode selected an address mode outside the table range
    #[error("invalid COPY address mode {0}")]
    InvalidAddressMode(u8),
    /// A COPY address pointed at or beyond the current output position
    #[error("COPY address out of bounds")]
    AddressOutOfBounds,
    /// A COPY spanned the boundary between the source segment and the target
    #[error("COPY crosses the source/target boundary")]
    CopyCrossesSourceTarget,
    /// A source or target segment lay outside its backing buffer
    #[error("source segment out of bounds")]
    SegmentOutOfBounds,
    /// A window produced a different number of bytes than it declared
    #[error("target window size mismatch")]
    TargetSizeMismatch,
    /// A window's declared delta-encoding length did not match its contents
    #[error("delta encoding length mismatch")]
    DeltaLengthMismatch,
    /// A section held more bytes than its instructions consumed
    #[error("trailing bytes in a window section")]
    TrailingSectionData,
    /// A window's Adler32 checksum did not match the decoded output
    #[error("checksum mismatch")]
    ChecksumMismatch,
    /// A declared size exceeded the decoder's safety limit
    #[error("size limit exceeded")]
    SizeLimitExceeded,
}
