//! Pure-Rust VCDIFF (RFC 3284) decoder for xdelta3-style binary deltas.
//!
//! Applies a VCDIFF delta to a source buffer to reconstruct the target, via the single entry point
//! [`decode`]. Supports the full RFC 3284 core: the default code table, the SAME/NEAR address cache,
//! ADD/RUN/COPY instructions, overlapping copies, multiple windows, and both `VCD_SOURCE` and
//! `VCD_TARGET` segments. It also handles the xdelta3 defaults that need no external codec, so output
//! from `xdelta3 -e -S none` decodes directly: the application header is skipped and the per-window
//! Adler32 checksum is verified.
//!
//! Secondary compression (LZMA, djw, FGK) and custom code tables are rejected with a clear error;
//! [`DecodeError`] is `#[non_exhaustive]` so that support can be added later without a breaking change.
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
mod cursor;
mod decoder;
mod error;

pub use decoder::decode;
pub use error::DecodeError;
