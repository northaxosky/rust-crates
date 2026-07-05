//! Pure-Rust writer for minimal Bethesda light-master (ESL) carrier plugins.
//!
//! Emits tiny TES4-only plugins for Skyrim SE/AE, Fallout 4, and Starfield. The headline use is a
//! carrier plugin: an empty light master whose only job is to make the game auto-load a same-named
//! BA2 archive (the companion to the `btdx` crate). Save [`carrier_plugin`] output next to the
//! archives as `<base>.esl` (Fallout 4 / Skyrim SE) or `<base>.esm` (Starfield), where `<base>`
//! matches the archive file-name stem. The [`Plugin`] builder additionally writes an author, a
//! description, and master dependencies.
//!
//! Beyond headers, the [`Plugin`] builder writes content records grouped into top-level [`Group`]s:
//! each [`Record`] carries a signature, a FormID, and raw fields, and the writer computes every size
//! and count. FormIDs are caller-owned; for a light plugin's own new records, Fallout 4 and Skyrim SE
//! encode them as `(master_count << 24) | object_index` with the object index in `0x000..=0xFFF`.

#![forbid(unsafe_code)]

mod error;
mod game;
mod plugin;
mod record;
mod win1252;
mod write;

pub use error::WriteError;
pub use game::Game;
pub use plugin::{Plugin, carrier_plugin};
pub use record::{Group, Record};
