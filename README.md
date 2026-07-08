# rust-crates

Pure-Rust libraries extracted from [Overseer](https://github.com/northaxosky/overseer), a Fallout 4
mod manager. No external tools, no C++ — cross-platform by default.

| Crate | Role | Status |
|---|---|---|
| [`btdx`](crates/btdx) | Fallout 4 & Starfield BA2 (`BTDX`) archive read/write | GNRL + DX10, read + write |
| [`esl-writer`](crates/esl-writer) | Bethesda light-master (ESL) plugin writer | headers + records |
| [`vcdiff-rs`](crates/vcdiff) | VCDIFF (RFC 3284) binary-delta decoder | decode |
| [`bsa`](crates/bsa) | Bethesda Softworks Archive (BSA) read/write | stub |

## Build, test, lint

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Git hooks

Enable the pre-commit (fmt) and pre-push (full gate) hooks per clone:

```sh
git config core.hooksPath .githooks
```

## License

Dual-licensed under MIT OR Apache-2.0.
