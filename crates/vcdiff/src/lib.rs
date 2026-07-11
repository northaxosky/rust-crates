//! Pure-Rust VCDIFF (RFC 3284) decoder for xdelta3-style binary deltas.
//!
//! Applies a VCDIFF delta with either [`decode`] for slices or [`decode_to`] for seekable streams.
//! Supports the full RFC 3284 core: the default code table, the SAME/NEAR address cache,
//! ADD/RUN/COPY instructions, overlapping copies, multiple windows, and both `VCD_SOURCE` and
//! `VCD_TARGET` segments. Output from `xdelta3 -e -S none` decodes directly: application headers are
//! skipped and per-window Adler32 checksums are verified.
//!
//! Xdelta secondary compressor ID 1 is supported as stateless Static Huffman/DJW sections. ID 2 uses
//! XZ-framed LZMA2 with independent persistent DATA, INST, and ADDR states. Custom code tables and
//! other secondary compressors are rejected. Interoperability fixtures cover xdelta 3.1.0 and 3.2.0
//! none/DJW/LZMA output plus literal 3.2.0 defaults. Ignored stress and manifest-driven corpus tests
//! plus Windows scripts support local acceptance without bundling private data.
//! [`DecodeOptions::default`] leaves target size unlimited, so callers decoding untrusted deltas should
//! set [`DecodeOptions::max_target_size`] to prevent unbounded disk consumption.
//! [`decode_to`] measures and resets both inputs, requires an empty target, and leaves successful output
//! flushed and positioned at its decoded length. Failures may leave partial target bytes and unspecified
//! stream positions; the decoder never truncates, deletes, renames, or syncs the target.
//!
//! # Example
//!
//! ```
//! // A minimal VCDIFF delta that appends the literal bytes "abc"
//! let delta = [
//!     0xD6, 0xC3, 0xC4, 0x00, 0x00, 0x00, 0x09, 0x03,
//!     0x00, 0x03, 0x01, 0x00, 0x61, 0x62, 0x63, 0x04,
//! ];
//! let target = vcdiff_rs::decode(b"", &delta).unwrap();
//! assert_eq!(target, b"abc");
//! ```

#![forbid(unsafe_code)]

mod cache;
mod code_table;
mod decoder;
mod djw;
mod error;
mod input;
mod options;
mod secondary;
mod target;

pub use decoder::{decode, decode_to};
pub use error::{ByteRange, DecodeContext, DecodeError, IoOperation, SecondaryError, SectionKind};
pub use options::DecodeOptions;
