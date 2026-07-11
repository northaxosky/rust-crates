//! Errors and concise decoding context for VCDIFF streams

use std::fmt;
use std::io;

use thiserror::Error;
use xz4rust::XzError;

use crate::djw::DjwFault;

/// A VCDIFF section carried by a target window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectionKind {
    /// Literal bytes consumed by ADD and RUN instructions
    Data,
    /// Instruction opcodes and variable instruction sizes
    Instructions,
    /// Encoded COPY addresses
    Addresses,
}

impl SectionKind {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Data => 0,
            Self::Instructions => 1,
            Self::Addresses => 2,
        }
    }
}

impl fmt::Display for SectionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data => formatter.write_str("DATA"),
            Self::Instructions => formatter.write_str("INST"),
            Self::Addresses => formatter.write_str("ADDR"),
        }
    }
}

/// An operation performed on a caller-provided stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IoOperation {
    /// Determine a stream length
    Length,
    /// Read bytes
    Read,
    /// Write bytes
    Write,
    /// Change the stream position
    Seek,
    /// Flush buffered writes
    Flush,
}

impl fmt::Display for IoOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length => formatter.write_str("length query"),
            Self::Read => formatter.write_str("read"),
            Self::Write => formatter.write_str("write"),
            Self::Seek => formatter.write_str("seek"),
            Self::Flush => formatter.write_str("flush"),
        }
    }
}

/// A byte range identified by its start and length
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// Absolute byte offset
    pub start: u64,
    /// Number of bytes
    pub len: u64,
}

impl ByteRange {
    pub(crate) const fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }
}

impl fmt::Display for ByteRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}+{}", self.start, self.len)
    }
}

/// The delta location associated with a decoding failure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeContext {
    /// Absolute offset in the delta stream
    pub delta_offset: u64,
    /// Zero-based target-window index when known
    pub window: Option<u64>,
    /// Active section when known
    pub section: Option<SectionKind>,
}

impl DecodeContext {
    pub(crate) const fn new(
        delta_offset: u64,
        window: Option<u64>,
        section: Option<SectionKind>,
    ) -> Self {
        Self {
            delta_offset,
            window,
            section,
        }
    }
}

impl fmt::Display for DecodeContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "delta offset {}", self.delta_offset)?;
        if let Some(window) = self.window {
            write!(formatter, ", window {window}")?;
        }
        if let Some(section) = self.section {
            write!(formatter, ", {section}")?;
        }
        Ok(())
    }
}

/// Opaque secondary-codec failure preserved in the standard error chain
#[derive(Debug)]
pub struct SecondaryError {
    inner: SecondaryErrorKind,
}

#[derive(Debug)]
enum SecondaryErrorKind {
    Djw(DjwFault),
    Lzma(XzError),
}

impl SecondaryError {
    pub(crate) const fn djw(error: DjwFault) -> Self {
        Self {
            inner: SecondaryErrorKind::Djw(error),
        }
    }

    pub(crate) const fn lzma(error: XzError) -> Self {
        Self {
            inner: SecondaryErrorKind::Lzma(error),
        }
    }
}

impl fmt::Display for SecondaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            SecondaryErrorKind::Djw(error) => fmt::Display::fmt(error, formatter),
            SecondaryErrorKind::Lzma(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for SecondaryError {}

/// Why a VCDIFF delta could not be decoded
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// A source-stream operation failed
    #[error("source {operation} failed for {range:?} in window {window:?}")]
    SourceIo {
        /// Failed operation
        operation: IoOperation,
        /// Relevant source range when known
        range: Option<ByteRange>,
        /// Zero-based target-window index when known
        window: Option<u64>,
        /// Underlying I/O error
        #[source]
        source: io::Error,
    },
    /// A delta-stream operation failed
    #[error("delta {operation} failed at {context}")]
    DeltaIo {
        /// Failed operation
        operation: IoOperation,
        /// Delta decoding location
        context: DecodeContext,
        /// Underlying I/O error
        #[source]
        source: io::Error,
    },
    /// A target-stream operation failed
    #[error("target {operation} failed for {range:?} in window {window:?}")]
    TargetIo {
        /// Failed operation
        operation: IoOperation,
        /// Relevant target range when known
        range: Option<ByteRange>,
        /// Zero-based target-window index when known
        window: Option<u64>,
        /// Underlying I/O error
        #[source]
        source: io::Error,
    },
    /// The caller supplied a target that already held bytes
    #[error("target is not empty ({len} bytes)")]
    TargetNotEmpty {
        /// Existing target length
        len: u64,
    },
    /// The delta did not begin with the VCDIFF magic bytes
    #[error("not a VCDIFF delta at {context}")]
    BadMagic {
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The header version byte was not the supported value zero
    #[error("unsupported VCDIFF version {version} at {context}")]
    UnsupportedVersion {
        /// Unsupported version
        version: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The header indicator had reserved bits set
    #[error("invalid header indicator {indicator:#04x} at {context}")]
    InvalidHeaderIndicator {
        /// Invalid indicator
        indicator: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The window indicator had reserved or conflicting bits set
    #[error("invalid window indicator {indicator:#04x} at {context}")]
    InvalidWindowIndicator {
        /// Invalid indicator
        indicator: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The delta indicator had reserved bits set
    #[error("invalid delta indicator {indicator:#04x} at {context}")]
    InvalidDeltaIndicator {
        /// Invalid indicator
        indicator: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The delta carries a custom code table
    #[error("custom code tables are not supported at {context}")]
    UnsupportedCodeTable {
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The delta declares an unknown or unsupported secondary compressor
    #[error("secondary compressor ID {id} is not supported at {context}")]
    UnsupportedSecondaryCompressor {
        /// Declared compressor ID
        id: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A section is compressed without a declared compressor
    #[error("compressed {section} section has no declared compressor at {context}")]
    CompressedSectionWithoutCompressor {
        /// Compressed section
        section: SectionKind,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The secondary codec rejected a compressed section
    #[error("secondary compressor ID {compressor_id} failed at {context}")]
    SecondaryDecompression {
        /// Active compressor ID
        compressor_id: u8,
        /// Compressed section location
        context: DecodeContext,
        /// Opaque codec error
        #[source]
        source: SecondaryError,
    },
    /// A compressed section declared an invalid decoded size
    #[error("invalid secondary decoded size {value} at {context}")]
    InvalidSecondarySize {
        /// Invalid decoded byte count
        value: u64,
        /// Compressed section location
        context: DecodeContext,
    },
    /// A secondary fragment produced a different decoded size
    #[error("secondary section expected {expected} bytes but produced {actual} at {context}")]
    SecondarySizeMismatch {
        /// Declared decoded byte count
        expected: u64,
        /// Produced decoded byte count
        actual: u64,
        /// Compressed section location
        context: DecodeContext,
    },
    /// A secondary fragment did not consume its exact encoded range
    #[error(
        "secondary section expected {expected} encoded bytes but consumed {actual} at {context}"
    )]
    SecondaryInputMismatch {
        /// Encoded fragment byte count
        expected: u64,
        /// Consumed encoded byte count
        actual: u64,
        /// Compressed section location
        context: DecodeContext,
    },
    /// A secondary fragment ended in an invalid decoder state
    #[error("malformed secondary fragment for compressor ID {compressor_id} at {context}")]
    MalformedSecondarySection {
        /// Active compressor ID
        compressor_id: u8,
        /// Compressed section location
        context: DecodeContext,
    },
    /// A secondary stream requires a dictionary above the configured limit
    #[error("secondary dictionary size {required} exceeds limit {limit} at {context}")]
    SecondaryDictionaryLimit {
        /// Dictionary size required by the stream or decoder
        required: u64,
        /// Configured dictionary limit
        limit: u64,
        /// Compressed section location
        context: DecodeContext,
    },
    /// The delta ended before a required structure or section
    #[error("truncated delta at {context}")]
    TruncatedDelta {
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A variable-length integer did not fit in 64 bits
    #[error("varint overflow at {context}")]
    VarintOverflow {
        /// Delta decoding location
        context: DecodeContext,
    },
    /// Checked size or offset arithmetic overflowed
    #[error("arithmetic overflow at {context}")]
    ArithmeticOverflow {
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A resident section size did not fit the platform address space
    #[error("value {value} exceeds the platform size limit at {context}")]
    PlatformSizeLimit {
        /// Value that did not fit `usize`
        value: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// The cumulative target would exceed the configured limit
    #[error("target size {attempted} exceeds limit {limit} at {context}")]
    TargetSizeLimit {
        /// Attempted cumulative target size
        attempted: u64,
        /// Configured cumulative target limit
        limit: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// Active decoded sections would exceed the configured memory limit
    #[error("active window memory {attempted} exceeds limit {limit} at {context}")]
    WindowMemoryLimit {
        /// Attempted active section bytes
        attempted: u64,
        /// Configured active section limit
        limit: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A bounded decoder allocation failed
    #[error("could not allocate {requested} bytes at {context}")]
    AllocationFailed {
        /// Requested allocation size
        requested: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A source or prior-target segment exceeded its backing stream
    #[error("segment {range} exceeds backing length {available} at {context}")]
    SegmentOutOfBounds {
        /// Declared segment range
        range: ByteRange,
        /// Available backing length
        available: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// An instruction selected an address mode outside the default table
    #[error("invalid COPY address mode {mode} at {context}")]
    InvalidAddressMode {
        /// Invalid address mode
        mode: u8,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A COPY address did not refer to available source or target bytes
    #[error("COPY address {address} is not before position {here} at {context}")]
    AddressOutOfBounds {
        /// Decoded COPY address
        address: u64,
        /// Current combined-window position
        here: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A COPY spanned the source-segment and current-target boundary
    #[error("COPY {address}+{size} crosses source/target boundary {boundary} at {context}")]
    CopyCrossesSourceTarget {
        /// Decoded COPY address
        address: u64,
        /// COPY instruction size
        size: u64,
        /// Combined-window boundary
        boundary: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// An instruction requested more section bytes than remained
    #[error("{section} section needs {requested} bytes with {remaining} remaining at {context}")]
    SectionOutOfBounds {
        /// Exhausted section
        section: SectionKind,
        /// Requested byte count
        requested: u64,
        /// Remaining byte count
        remaining: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A window produced a different number of bytes than declared
    #[error("target window expected {expected} bytes but reached {actual} at {context}")]
    TargetSizeMismatch {
        /// Declared target-window size
        expected: u64,
        /// Produced or attempted target-window size
        actual: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A window's declared delta-encoding length did not match its contents
    #[error("delta encoding ends at {expected_end} but sections end at {actual_end} at {context}")]
    DeltaLengthMismatch {
        /// Endpoint declared by the delta-encoding length
        expected_end: u64,
        /// Endpoint implied by parsed section lengths
        actual_end: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A section held bytes that no instruction consumed
    #[error("{section} section has {remaining} trailing bytes at {context}")]
    TrailingSectionData {
        /// Section with unconsumed bytes
        section: SectionKind,
        /// Unconsumed byte count
        remaining: u64,
        /// Delta decoding location
        context: DecodeContext,
    },
    /// A window's Adler32 checksum did not match its decoded output
    #[error(
        "window {window} checksum expected {expected:#010x} but got {actual:#010x} at delta offset {delta_offset}"
    )]
    ChecksumMismatch {
        /// Zero-based target-window index
        window: u64,
        /// Declared Adler32 value
        expected: u32,
        /// Incrementally computed Adler32 value
        actual: u32,
        /// Offset of the declared checksum
        delta_offset: u64,
    },
}
