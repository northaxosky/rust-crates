# vcdiff-rs

Pure-Rust decoder for [VCDIFF](https://www.rfc-editor.org/rfc/rfc3284) (RFC 3284) binary deltas, the
format produced by `xdelta3`. No C bindings, no external tools.

## Example

```rust
use vcdiff_rs::{DecodeError, decode};

fn apply(source: &[u8], delta: &[u8]) -> Result<Vec<u8>, DecodeError> {
    // `delta` is a VCDIFF patch, e.g. from `xdelta3 -e -S none -s source target patch`
    decode(source, delta)
}
```

For file-backed output, use `decode_to` with seekable source and delta readers plus an empty
read/write/seek target. `DecodeOptions::default()` bounds active window memory but leaves
`max_target_size` unlimited; set that field when an untrusted delta must not consume unbounded disk
space. The slice wrapper keeps its private 1 GiB target guard.

`decode_to` ignores incoming stream positions and measures then resets both inputs. The target must be
empty; successful decoding flushes it and leaves it positioned at the decoded length. A failure may
leave partial output and unspecified stream positions. The decoder does not truncate, delete, rename,
or sync the target. Target implementations must make flushed writes visible to later reads and seeks.

## Scope

Decodes the full RFC 3284 core: the default code table, the SAME/NEAR address cache, ADD/RUN/COPY
instructions, overlapping copies, multiple windows, and both `VCD_SOURCE` and `VCD_TARGET` segments.
Application headers are skipped and each window's Adler32 checksum is verified.

Xdelta private secondary compressor ID 1 is supported as stateless Static Huffman/DJW sections. ID 2
is supported as XZ-framed LZMA2 with independent DATA, INST, and ADDR decoder states that persist
across windows. Decoded active sections, DJW selector scratch, and each ID-2 dictionary are bounded by
`DecodeOptions`.

Hermetic interoperability fixtures verify xdelta 3.1.0 and 3.2.0 explicit none/DJW/LZMA output,
multi-window and multi-table DJW, and literal xdelta 3.2.0 defaults including its standard
application-header armor metadata.

FGK and custom code tables remain unsupported. `DecodeError` reports stream origin and decoding
context and is `#[non_exhaustive]` for future format support. This crate decodes only; it does not
produce deltas.

## File decoding

The release example creates a new output file and refuses to overwrite an existing path:

```text
cargo run -p vcdiff-rs --release --example decode_file -- source.bin patch.vcdiff output.bin
```

Durability, synchronization, and atomic replacement remain caller policy.

## Large-target acceptance

The ignored release test streams a 2,148,007,936-byte synthetic target without retaining source or
target payload:

```text
cargo test -p vcdiff-rs --release --test stress_over_2gib -- --ignored --exact --nocapture
```

The Windows differential-memory runner builds that test once, then measures direct 256 MiB and 2+ GiB
child processes rather than Cargo:

```text
pwsh -File crates/vcdiff/scripts/measure_stress.ps1 `
  -ReportPath crates/vcdiff/results/local/2026-07-10-machine/stress-memory.json
```

## Private corpus

Copy `tests/fixtures/overseer-corpus.example.toml` outside the repository, replace its synthetic paths
and independently verified identities, then run:

```text
$env:VCDIFF_OVERSEER_CORPUS = 'C:\path\to\corpus.toml'
cargo test -p vcdiff-rs --test overseer_corpus -- --ignored --exact --nocapture
```

Temporary decoded files are created beside the expected target when possible and removed by RAII.
Case names are reported, while private paths remain external.

Each ID-1 corpus case requires the clean, unmodified Steam pre-patch source whose SHA-1 matches
`expected_source_sha1`. The source is stream-hashed before any output is created. A
Game Pass/1.10.984 note in patch-tool configuration describes replacement-asset provenance; it does
not identify the VCDIFF base. Keep real source paths, hashes, deltas, and target identities only in
the external local manifest.

## Windows benchmark

Run on AC power with other workloads closed. Record Defender real-time scanning or exclusions and the
filesystem, cache, and storage context in the required parameters:

```text
pwsh -File crates/vcdiff/scripts/benchmark.ps1 `
  -Source C:\path\to\source.bin -Delta C:\path\to\patch.vcdiff `
  -Expected C:\path\to\verified-target.bin -Output C:\path\to\owned-output.bin `
  -Report crates/vcdiff/results/local/2026-07-10-machine/benchmark.json `
  -Producer xdelta3 -ProducerVersion 3.2.0 -Compression lzma `
  -PowerContext 'AC power' -DefenderContext 'record actual state' `
  -FilesystemContext 'record filesystem and cache state'
```

The script performs one warm-up and five measured direct child processes, verifies output identity and
byte equality, removes owned outputs, and omits all supplied file paths from its JSON report. Producer
and environment values must be path-free labels; the script rejects path separators in those values.
The report records the HEAD commit, dirty state, and a SHA-256 of the complete non-ignored working tree.
Store local results under `crates/vcdiff/results/local/<yyyy-mm-dd>-<machine>/`; the results tree is
ignored by Git.

## Overseer onboarding status

- [x] Streaming decoder, ID-1/ID-2 support, and external xdelta fixture matrix
- [x] Synthetic 2+ GiB acceptance and differential-memory tooling
- [x] Opt-in corpus and representative benchmark tooling
- [x] Real local Overseer corpus verification
- [x] Representative large-file benchmark report

## License

Licensed under either of MIT or Apache-2.0 at your option.
