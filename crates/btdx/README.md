# btdx

[![crates.io](https://img.shields.io/crates/v/btdx.svg)](https://crates.io/crates/btdx)
[![docs.rs](https://docs.rs/btdx/badge.svg)](https://docs.rs/btdx)

Pure-Rust reader and writer for Fallout 4 and Starfield Bethesda archives (BA2, magic `BTDX`).

`btdx` handles the two BA2 variants Bethesda ships:

- **General** (`GNRL`) archives hold arbitrary files (meshes, scripts, sounds),
  each stored raw or compressed.
- **Texture** (`DX10`) archives hold DDS textures split into per-mip chunks,
  which `btdx` reassembles into standard DDS files on extraction.

It reads Fallout 4 archives (versions 1, 7, and 8, zlib) and Starfield archives
(versions 2 and 3, zlib or LZ4), and writes version-1 Fallout 4 archives, which
load on every Fallout 4 build. There is no `unsafe` and no C dependency: just
`thiserror`, `flate2`, and the pure-Rust `lz4_flex`.

## Features

- Read `GNRL` and `DX10` archives and enumerate their entries and paths.
- Fallout 4 (v1/7/8) and Starfield (v2/v3) archives, zlib or raw LZ4 block.
- Extract files, transparently decompressing chunks.
- Rebuild standards-compliant DDS (with the DX10 extended header) from texture
  chunks.
- Write `GNRL` archives, stored or compressed, with the game-required BA2 name
  hashes so the archive loads in-game.
- Write `DX10` texture archives, splitting mips into chunks the way Bethesda's
  Archive2 does.
- `#![forbid(unsafe_code)]`.

## Read and extract

```rust,no_run
use btdx::{Archive, Entries};

let bytes = std::fs::read("Starfield - Textures01.ba2").unwrap();
let archive = Archive::read(&bytes).unwrap();
println!("{} files", archive.header().file_count);

match archive.entries() {
    Entries::General(files) => {
        for f in files {
            let data = archive.extract(f).unwrap();
            println!("{} ({} bytes)", f.path.as_deref().unwrap_or("?"), data.len());
        }
    }
    Entries::Texture(textures) => {
        for t in textures {
            let dds = archive.extract_texture(t).unwrap();
            println!("{} ({} bytes)", t.path.as_deref().unwrap_or("?"), dds.len());
        }
    }
}
```

## Write a general archive

```rust,no_run
use btdx::GnrlWriter;

let nif_bytes = std::fs::read("weapon.nif").unwrap();
let pex_bytes = std::fs::read("quest.pex").unwrap();

let mut w = GnrlWriter::new();
w.add_file("Meshes\\weapon.nif", nif_bytes).unwrap();      // zlib-compressed
w.add_file_stored("Scripts\\quest.pex", pex_bytes).unwrap(); // stored raw
let archive = w.to_vec().unwrap();
std::fs::write("MyMod - Main.ba2", archive).unwrap();
```

## Write a texture archive

```rust,no_run
use btdx::Dx10Writer;

let dds_bytes = std::fs::read("armor_d.dds").unwrap();

let mut w = Dx10Writer::new();
w.add_texture("Textures\\armor_d.dds", dds_bytes).unwrap(); // parsed and split
let archive = w.to_vec().unwrap();
std::fs::write("MyMod - Textures.ba2", archive).unwrap();
```

## Format notes

- BA2 filenames are keyed by a table-based CRC-32 (not a standard CRC-32);
  `btdx` reproduces it exactly, so written archives resolve in-game.
- Texture chunking follows Archive2's "single mip chunk area" heuristic, up to
  four chunks per texture; cubemaps stay a single chunk.
- Compression is zlib (Fallout 4, Starfield v2) or raw LZ4 block (Starfield v3),
  chosen per archive.
- File and chunk sizes use BA2's 32-bit fields (4 GiB each).

## Limitations

- Reads Fallout 4 and Starfield; writing currently emits Fallout 4 version-1
  archives only. The older BSA format is out of scope.
- Console (Xbox) texture swizzling is rejected on read rather than unswizzled.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
