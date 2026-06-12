# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Cargo workspace (edition 2024, resolver 3) with two crates:

- **`rcomp-core`** (`crates/rcomp-core`) — the product: a reusable compression/archive
  library. This is where almost all logic lives.
- **`rcomp`** (`crates/rcomp`) — the first consumer: a CLI that wires `rcomp-core` to
  argument parsing, prompts, progress bars, and exit codes. A Tauri desktop app is the
  planned second consumer, so keep policy/UX in the CLI and keep the core UI-agnostic.

The core takes an `on_progress: impl FnMut(&Progress)` callback and a `CancelToken`
rather than touching the terminal itself — preserve that boundary when adding features.

## Commands

```bash
cargo build                                    # build the workspace
cargo test --workspace --all-features          # full test suite (matches CI)
cargo test -p rcomp-core <name>                # run one test by name substring
cargo test -p rcomp-core --test interop        # run one integration test file
cargo run -p rcomp -- folder/ out.tar.zst      # run the CLI

# Lint/format gates — CI fails on any of these:
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo doc --no-deps -p rcomp-core --locked     # RUSTDOCFLAGS="-D warnings"; missing_docs is enforced

# The `rar` feature is on by default. Verify the OSI-clean build still works:
cargo build --no-default-features -p rcomp
cargo test  --no-default-features -p rcomp-core
```

CI also runs the test job on macOS and Windows. `rcomp-core/src/lib.rs` sets
`#![forbid(unsafe_code)]` and `#![warn(missing_docs)]` — every public item needs a doc
comment or the docs job fails.

## Architecture

### Two-layer format model (`core/src/format.rs`)

Every format is a `Format { container: Option<Container>, codec: Option<Codec> }` with the
invariant that at least one is `Some` (enforced by the `Format::codec`/`container`/`layered`
constructors — never build the struct literally). `Codec` = single-stream compressor
(gzip, bzip2, xz, zstd, lz4, brotli). `Container` = archive (tar, zip, 7z, rar). They
combine: `.tar.gz` is `layered(Tar, Gzip)`; `.zip` is `container(Zip)`; `.bz2` is
`codec(Bzip2)`. `Display`/`FromStr` round-trip the canonical names.

### Detection (`core/src/detect.rs`)

`detect()` reads ≤512 bytes and combines magic-byte and extension lookup, **magic bytes
win**. `detect_from_extension` and `split_format_suffix` are pure name-based helpers with a
longest-suffix-first table (`.tar.gz` before `.gz`). Brotli has no magic bytes — an
extensionless `.br` file can only be handled via an explicit `--algo brotli` / `opts.format`.

### Operation dispatch (`core/src/ops.rs`)

`compress`, `extract`, `list` are the public API. Both `compress` and `extract` are thin
guard/setup wrappers around `do_compress`/`do_extract`, which `match (container, codec)` and
dispatch to a codec stream or an archive backend. Key behaviours that live here:

- **Silent-tar rule** — codec-only format + directory input is rewritten to
  `layered(Tar, codec)` so a folder transparently becomes tar-then-codec. The CLI is
  responsible for warning the user before this happens.
- **Tar sniff on extract** — for a codec-only stream, the first 512 decompressed bytes are
  checked for `ustar` magic at offset 257; if present it is untarred, else written as a
  single file. The sniffed prefix is re-chained with `Cursor::chain` so it is not lost.
- **Encoder-finish-after-tar** — `tar::create` borrows the encoder via the `WriteRef`
  wrapper (a `&mut`-forwarding `Write`) so the encoder stays owned by the outer scope and
  `encoder.finish()` can be called after tar returns. Always call `Encoder::finish` — see
  `codec/mod.rs`; dropping an encoder truncates the stream.
- **Progress accounting** — for tar/codec streams `bytes_total` is the *compressed* input
  size and an `AtomicCountingReader` counts compressed bytes so `bytes_done <= bytes_total`
  always holds. zip/7z/rar are random-access, so they set `bytes_total = None`.

### Checksums (`core/src/hash.rs`, the `checksum`/`verify_*` options)

SHA-256 digests are computed *streaming*, by teeing `HashingReader`/`HashingWriter` into the
existing pipeline — no extra read pass. Two digests:

- **artifact** — the compressed bytes on disk (hashed inline for tar/codec; for zip/7z the
  file is re-hashed afterward via `hash_file` since those backends own their I/O).
- **content** — the pre-compression stream. Only exists for codec-only / tar / tar+codec.
  zip/7z/rar have no single content stream, so `verify_content_sha256` on them is a hard
  `UnsupportedOperation`. The CLI writes/reads these as `<output>.sha256` sidecars.

### Archive backends (`core/src/archive/`)

`tar.rs`, `zip.rs`, `sevenz.rs`, and feature-gated `rar.rs` each expose `create`/`extract`/
`list`. `OpCtx` (in `archive/mod.rs`) threads the `CancelToken`, the `on_progress` callback,
and the live `Progress` through every backend so cancellation and progress are handled once.
`sanitize.rs` validates entry paths and symlink targets against path-traversal (zip-slip)
before anything is written — route all new extraction paths through it.

### CLI layer (`crates/rcomp/src/`)

- `cli.rs` — clap parser. Compress vs. extract is **not** a subcommand; it is inferred.
- `infer.rs` — `infer_mode(InferFacts)` is **pure** (no filesystem access; the caller in
  `run.rs` pre-computes all facts) and applies the 4 ordered compress-vs-extract rules.
  Keep it pure so it stays unit-testable.
- `run.rs` — owns all policy: the silent-tar confirmation prompt, the extraction wrap-folder
  decision (calls `list()` to count distinct roots), sidecar read/write/parse, summary
  lines, and `CoreError` → exit-code mapping.
- `main.rs` — installs the Ctrl-C handler (trips a shared `CancelToken`) and maps errors to
  exit codes: **0** success, **1** operation error, **2** usage/ambiguity (downcasts
  `AmbiguityError`/`UsageError`).

## Testing conventions

Integration tests live in `crates/*/tests/`. `rcomp-core` tests exercise the library API
directly; `rcomp` tests drive the built binary via `assert_cmd`. `tests/interop.rs` decodes
committed fixtures from `tests/fixtures/` (created by *reference* tools, not rcomp itself) to
prove real-world compatibility — see `tests/fixtures/FIXTURES.md`. When adding a format or
flag, add both a core round-trip test and a CLI test, and an interop fixture if an external
tool produces the format.

## RAR feature

`rar` is a default feature linking the freeware `unrar` library (extract-only; **not
OSI-approved**). It is gated with `#[cfg(feature = "rar")]` throughout core. `build.rs` links
`advapi32` on Windows when the feature is on. Always confirm `--no-default-features` still
builds and tests green — CI has a dedicated job for it.

## Development workflow

This repo follows a plan-driven flow. Planning docs live under `.claude/plans/`, which is
**gitignored** (local-only, not committed):

- `.claude/plans/PLAN.md` — the current milestone's scoped plan. Keep it strictly scoped to
  what was asked.
- `.claude/plans/PLAN_EXTRAS.md` — open questions, nice-to-haves, out-of-scope ideas. Items
  graduate into `PLAN.md` only when the user answers a question or accepts a suggestion.
- `.claude/plans/milestone-*.md` — per-milestone plans.
- `CURRENT_ISSUES.md` — gaps/bugs/spec-questions discovered during implementation (tracked).
- `RELEASING.md` — the crates.io release runbook (`rcomp-core` publishes before `rcomp`;
  triggered by a tagged GitHub release).

Commits use Conventional Commits (`feat(core):`, `fix(build):`, `docs:`, `ci:`, …). Per the
user's global rules: never add a `Co-Authored-By` or "Generated with" trailer, and never
`git push` as part of a commit request.
