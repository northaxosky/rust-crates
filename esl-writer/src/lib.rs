//! Pure-Rust writer for minimal Bethesda light-master (ESL) carrier plugins.
//!
//! Emits tiny TES4-only plugins (flags `0x201` = Master | Light, empty HEDR, no masters) whose sole
//! job is to make Fallout 4 auto-load a same-named BA2. Not yet implemented; see the workspace
//! `AGENTS.md` and the Overseer CC-merger design for the byte layout.

#![forbid(unsafe_code)]

// TODO: pub fn carrier_plugin(base_name: &str) -> Vec<u8> — TES4 header (flags 0x201) + HEDR (1.0f, 0 records).
