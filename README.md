# rust-crates

Pure-Rust libraries extracted from [Overseer](https://github.com/northaxosky/overseer), a Fallout 4
mod manager. No external tools, no C++ — cross-platform by default.

| Crate | Role | Status |
|---|---|---|
| [`ba2r`](ba2r) | Fallout 4 BA2 (`BTDX`) archive read/write | reader working, writer WIP |
| [`esl-writer`](esl-writer) | minimal Bethesda light-master (ESL) plugin writer | stub |
| [`vcdiff`](vcdiff) | VCDIFF (RFC 3284) binary-delta decoder | stub |

## Build, test, lint

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

Dual-licensed under MIT OR Apache-2.0.
