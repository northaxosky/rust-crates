//! Pure-Rust VCDIFF (RFC 3284) decoder for xdelta3-style binary deltas.
//!
//! Intended as a drop-in replacement for shelling out to `xdelta3.exe`: parse the VCDIFF window
//! structure and apply COPY/ADD/RUN instructions, with an `xz`/LZMA secondary decompressor for
//! xdelta3 output. Not yet implemented; see the workspace `AGENTS.md`.

#![forbid(unsafe_code)]

// TODO: pub fn decode(source: &[u8], delta: &[u8]) -> Result<Vec<u8>, VcdiffError> — window parser + COPY/ADD/RUN.
