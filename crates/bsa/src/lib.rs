//! Pure-Rust reader and writer for Bethesda Softworks Archives (BSA).
//!
//! Targets the TES4-era BSA format used by Oblivion (v103), Fallout 3 / New Vegas / Skyrim (v104),
//! and Skyrim Special Edition / Anniversary Edition (v105). Distinct from the BA2/BTDX archives the
//! `btdx` crate handles: BSA uses a folder/file hash-table directory, the classic TES hash, and zlib
//! (v103/104) or LZ4 (v105) compression. Not yet implemented; see the workspace `AGENTS.md`.

#![forbid(unsafe_code)]

// TODO: pub fn read(bytes: &[u8]) -> Result<Archive, BsaError> — header + folder/file hash tables, TES hash, zlib/LZ4.
