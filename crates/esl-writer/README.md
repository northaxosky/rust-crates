# esl-writer

Pure-Rust writer for minimal Bethesda light-master (ESL) plugins for Skyrim SE/AE, Fallout 4, and
Starfield.

The headline use is a **carrier plugin**: a tiny empty light master whose only job is to make the
game auto-load a same-named BA2 archive (the companion to [`btdx`](../btdx)).

## Example

```rust
use esl_writer::{carrier_plugin, Game, Plugin};

// Empty carrier: save as `MyMod.esl` next to `MyMod - Main.ba2`
let bytes = carrier_plugin(Game::Fallout4);
std::fs::write("MyMod.esl", bytes).unwrap();

// Or build a light master with metadata
let bytes = Plugin::new(Game::Starfield)
    .author("muteptr")
    .description("carrier for MyMod archives")
    .to_bytes()
    .unwrap();
```

The light-master flag bit differs by game (`0x200` for Skyrim SE / Fallout 4, `0x100` for Starfield),
so the target [`Game`] is required. Save Starfield carriers as `<base>.esm`, and Fallout 4 / Skyrim
SE carriers as `<base>.esl`, where `<base>` matches the archive file-name stem.

## Records and groups

```rust
use esl_writer::{Game, Group, Plugin, Record};

let bytes = Plugin::new(Game::SkyrimSe)
    .group(
        Group::top(b"GLOB").record(
            Record::new(b"GLOB", 0x0100_0801)
                .field(b"EDID", b"MyGlobal\0")
                .field(b"FLTV", 1.0f32.to_le_bytes()),
        ),
    )
    .to_bytes()
    .unwrap();
```

The writer computes record and group sizes, the header record count, and emits the `XXXX` overflow
prefix for fields larger than 64 KB. FormIDs are caller-owned.

## Scope

Writes the TES4 header plus top-level groups of records. Nested groups (CELL/WRLD/DIAL), record
compression, and reading are not yet supported; the crate writes syntactically valid containers, so
which fields a record needs is the caller's responsibility.

## License

Licensed under either of MIT or Apache-2.0 at your option.
