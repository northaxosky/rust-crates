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

## Scope

v0.1 writes the TES4 header record only: an empty light master with optional author, description, and
masters. Content records and GRUP groups are not yet supported.

## License

Licensed under either of MIT or Apache-2.0 at your option.
