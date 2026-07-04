# btdx

[![crates.io](https://img.shields.io/crates/v/btdx.svg)](https://crates.io/crates/btdx)
[![docs.rs](https://docs.rs/btdx/badge.svg)](https://docs.rs/btdx)

Pure-Rust reader and writer for Fallout 4 Bethesda archives (BA2, magic `BTDX`).

`btdx` handles the two BA2 variants Fallout 4 ships:

- **General** (`GNRL`) archives hold arbitrary files (meshes, scripts, sounds),
  each stored raw or zlib-compressed.
- **Texture** (`DX10`) archives hold DDS textures split into per-mip chunks,
  which `btdx` reassembles into standard DDS files on extraction.

It reads archives from both the original (version 1) and Next-Gen (versions 7
and 8) releases, and always writes version-1 archives, which load on every
Fallout 4 build. There is no `unsafe` and no C dependency: just `thiserror`
and `flate2`.

## Features

- Read `GNRL` and `DX10` archives and enumerate their entries and paths.
- Extract files, transparently inflating zlib-compressed chunks.
- Rebuild standards-compliant DDS (with the DX10 extended header) from texture
  chunks.
- Write `GNRL` archives, stored or compressed, with the game-required BA2 name
  hashes so the archive loads in-game.
- Write `DX10` texture archives, splitting mips into chunks the way Bethesda's
  Archive2 does.
- `#![forbid(unsafe_code)]`.

## Read and extract

```rust
use btdx::{read, extract, extract_texture, Entries};

let bytes = std::fs::read("Fallout4 - Textures1.ba2").unwrap();
let (header, entries) = read(&bytes).unwrap();
println!("{} files", header.file_count);

match entries {
    Entries::General(files) => {
        for f in &files {
            let data = extract(&bytes, f).unwrap();
            // write `data` to disk under `f.path`
        }
    }
    Entries::Texture(textures) => {
        for t in &textures {
            let dds = extract_texture(&bytes, t).unwrap();
            // `dds` is a complete DDS file, ready to save
        }
    }
}
```

## Write a general archive

```rust
use btdx::GnrlWriter;

let mut w = GnrlWriter::new();
w.add_file("Meshes\\weapon.nif", nif_bytes).unwrap();      // zlib-compressed
w.add_file_stored("Scripts\\quest.pex", pex_bytes).unwrap(); // stored raw
let archive = w.to_vec().unwrap();
std::fs::write("MyMod - Main.ba2", archive).unwrap();
```

## Write a texture archive

```rust
use btdx::Dx10Writer;

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
- Compression is zlib for every supported version.
- File and chunk sizes use BA2's 32-bit fields (4 GiB each).

## Limitations

- Fallout 4 only. Starfield BA2 (LZ4, version 3) and the older BSA format are
  out of scope.
- Console (Xbox) texture swizzling is rejected on read rather than unswizzled.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
