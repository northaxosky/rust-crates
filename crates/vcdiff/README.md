# vcdiff-rs

Pure-Rust decoder for [VCDIFF](https://www.rfc-editor.org/rfc/rfc3284) (RFC 3284) binary deltas, the
format produced by `xdelta3`. No C bindings, no external tools.

## Example

```rust
use vcdiff_rs::{decode, DecodeError};

fn apply(source: &[u8], delta: &[u8]) -> Result<Vec<u8>, DecodeError> {
    // `delta` is a VCDIFF patch, e.g. from `xdelta3 -e -S none -s source target patch`
    decode(source, delta)
}
```

## Scope

Decodes the full RFC 3284 core: the default code table, the SAME/NEAR address cache, ADD/RUN/COPY
instructions, overlapping copies, multiple windows, and both `VCD_SOURCE` and `VCD_TARGET` segments.
It also handles the xdelta3 defaults that need no external codec, so output from `xdelta3 -e -S none`
decodes directly: the application header is skipped and each window's Adler32 checksum is verified.

Secondary compression (LZMA, djw, FGK) and custom code tables are rejected with a clear error, and
`DecodeError` is `#[non_exhaustive]` so support can be added later. This crate decodes only; it does
not produce deltas.

## License

Licensed under either of MIT or Apache-2.0 at your option.
