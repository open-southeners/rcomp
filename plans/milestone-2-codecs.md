# Milestone 2 — stream codecs

Implements milestone 2 of `PLAN.md`: all six stream codecs (gzip, bzip2, xz,
zstd, lz4, brotli) behind one trait, the unified level mapping, and roundtrip
tests. Branch: `feat/m1-scaffold` (continues the same branch; commits split at
the end).

Four sequential units. Units 2–4 each implement two codecs; they are logically
independent but run sequentially because they share one compile unit
(rcomp-core) and each must leave the full workspace green:
`cargo build && cargo test && cargo clippy --all-targets -- -D warnings`.

## Shared contract (defined by unit 1, used by all)

```rust
// crates/rcomp-core/src/codec/mod.rs
use std::io::{Read, Write};

/// A compressed-stream writer. `finish()` is REQUIRED to flush trailers;
/// dropping without it may truncate the stream (documented on the trait).
pub trait Encoder: Write {
    fn finish(self: Box<Self>) -> crate::Result<()>;
}

pub fn new_encoder<'a>(codec: Codec, writer: Box<dyn Write + 'a>, level: Level)
    -> crate::Result<Box<dyn Encoder + 'a>>;
pub fn new_decoder<'a>(codec: Codec, reader: Box<dyn Read + 'a>)
    -> crate::Result<Box<dyn Read + 'a>>;

/// std::thread::available_parallelism(), 1 on error.
pub(crate) fn workers() -> u32;
```

Each codec lives in its own submodule (`codec/gzip.rs`, `codec/bzip2.rs`,
`codec/xz.rs`, `codec/zstd.rs`, `codec/lz4.rs`, `codec/brotli.rs`) exposing:

```rust
pub(crate) fn encoder<'a>(w: Box<dyn Write + 'a>, level: Level) -> crate::Result<Box<dyn Encoder + 'a>>;
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>>;
```

`new_encoder`/`new_decoder` dispatch via `match codec`.

### Level mapping (per PLAN.md — levels describe output ratio, never cost)

| Codec | Fast | Best | Edge | Edge extras / notes |
|---|---|---|---|---|
| gzip | 1 | 6 | 9 | `flate2`; decode with `MultiGzDecoder` (multi-member files) |
| bzip2 | 1 | 6 | 9 | |
| xz | 1 | 6 | 9 | Edge adds `PRESET_EXTREME`; threaded encode (`MtStreamBuilder`) when `workers() > 1` |
| zstd | 1 | 3 | 22 | Edge enables long-distance matching; `multithread(workers())` at every level (`zstdmt` feature) |
| lz4 | 1 | 6 | 12 | `lz4` C-bindings crate (HC levels) |
| brotli | 2 | 6 | 11 | lgwin 22 at Fast/Best, 24 at Edge |

If a crate's API makes an extra (threading, LDM) impractical, fall back to the
plain level and **report it** — it gets routed to `CURRENT_ISSUES.md`.

### Shared test helper (unit 1, `#[cfg(test)] pub(crate) mod test_util` in mod.rs)

- `corpus() -> Vec<(&'static str, Vec<u8>)>`: empty; `b"hello world"`; ~1 MiB
  repetitive pattern; ~256 KiB deterministic pseudo-random bytes (simple LCG —
  no `rand` dependency).
- `roundtrip(codec: Codec, level: Level)`: for each corpus entry, compress via
  `new_encoder` into a `Vec<u8>` (calling `finish()`), decompress via
  `new_decoder`, assert byte-identical; additionally assert
  `detect_from_bytes(&compressed)` returns `Some(Format::codec(codec))` —
  **except Brotli**, which must return `None` (no magic).
- `corrupt(codec: Codec)`: compress `b"hello world"` at Best, flip a byte in
  the middle, assert decompression returns `Err` (never panics).

## Unit 1 — codec module scaffold + dependencies (sequential, first)

**Files:** root `Cargo.toml`, `crates/rcomp-core/Cargo.toml`,
`crates/rcomp-core/src/codec/mod.rs`, six submodule stubs,
`crates/rcomp-core/src/lib.rs`.

- Add to `[workspace.dependencies]` (current versions from crates.io):
  `flate2`, `bzip2`, `liblzma`, `zstd` (with the `zstdmt` feature), `lz4`,
  `brotli`. Reference them from rcomp-core with `workspace = true`.
- `codec/mod.rs`: `Encoder` trait, dispatcher functions, `workers()`,
  `test_util` (fully implemented — it only uses the dispatcher), module docs.
- Six submodules with **compiling stubs**: correct signatures, `todo!()`
  bodies, `//! TODO` docs.
- `lib.rs`: `pub mod codec;` + `pub use codec::{Encoder, new_encoder, new_decoder};`.

**Verify:** `cargo build && cargo test && cargo clippy --all-targets -- -D warnings`
(stubs are never called by existing tests; `todo!()` bodies may need
`#[allow(unreachable_code)]`/underscore params to stay clippy-clean).

## Unit 2 — gzip + bzip2 (sequential, after unit 1)

**Files:** `codec/gzip.rs`, `codec/bzip2.rs` only.

- gzip: `flate2::write::GzEncoder` / `flate2::read::MultiGzDecoder`.
- bzip2: `bzip2::write::BzEncoder` / `bzip2::read::BzDecoder`.
- Levels per table. Tests in each file: `roundtrip` at all three levels,
  `corrupt`. **Verify:** full workspace command above.

## Unit 3 — xz + zstd (sequential, after unit 2)

**Files:** `codec/xz.rs`, `codec/zstd.rs` only.

- xz: `liblzma` crate. Encode: preset from table, `PRESET_EXTREME` at Edge,
  multithreaded via `MtStreamBuilder` when `workers() > 1` (single-threaded
  fallback otherwise). Decode: `XzDecoder` (auto-detects stream format).
- zstd: `zstd::stream::write::Encoder` with `.multithread(workers())`;
  Edge = level 22 + `enable_long_distance_matching(true)`. Decode:
  `zstd::stream::read::Decoder`. Window-log on decode large enough for
  LDM-encoded data (set `window_log_max` if the default rejects it — test
  roundtrip at Edge with the 1 MiB corpus entry specifically).
- Tests as unit 2. **Verify:** full workspace command.

## Unit 4 — lz4 + brotli + public-API integration test (sequential, after unit 3)

**Files:** `codec/lz4.rs`, `codec/brotli.rs`,
`crates/rcomp-core/tests/codec_roundtrip.rs` (new).

- lz4: `lz4::EncoderBuilder` with `.level()` per table (3+ engages HC);
  `lz4::Decoder`. Note `Encoder::finish()` returns `(W, Result)` — surface the
  error through `Encoder::finish`.
- brotli: `brotli::CompressorWriter` (quality/lgwin per table),
  `brotli::Decompressor` for read.
- Integration test (public API only, `use rcomp_core::{...}`): all 6 codecs ×
  3 levels roundtrip via re-exported `new_encoder`/`new_decoder`; brotli
  detection-returns-None documented in a test; one test that `finish()` is
  what completes the stream (gzip: without finish, decode of the partial
  buffer fails or truncates).
- **Verify:** full workspace command.

## Out of scope for all units

tar/zip/7z/rar containers, compress()/extract() top-level functions, CLI,
tar-inside-codec sniffing, fixtures from reference tools (milestone 6).
Report — do not fix — anything discovered beyond your unit.
