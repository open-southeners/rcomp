# Milestone 6 — Hardening & docs

Implements milestone 6 of `PLAN.md`: interop fixtures, overwrite policy edge
cases, README with usage examples (incl. the unrar license note), rustdoc on
the public API, `LICENSE-MIT` + `LICENSE-APACHE` files. Also picks up the
CURRENT_ISSUES.md items earmarked for M6: Ctrl-C cancellation wiring and
directory-permission restore on extraction. Branch: `feat/m1-scaffold`.

Units A–D are **independent** (disjoint files, run in parallel). Unit E is
**sequential, last** — it documents outcomes of the others and touches the
same Cargo.tomls as B.

Verification for every unit (all must pass):

```
cargo build && cargo test --workspace --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Unit E additionally: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p rcomp-core`.

## Binding decisions

- Ctrl-C maps to the **existing cancellation path**: handler trips a shared
  `CancelToken`, the operation unwinds through `Error::Cancelled` (cleaning up
  partial output), CLI prints a "cancelled" line to stderr and exits **1**
  (operation error — the locked 0/1/2 exit-code contract is unchanged).
- Directory modes are restored **after** all entries are written, in
  reverse-depth order (deepest first), unix-only — same two-phase pattern the
  zip crate uses internally.
- Interop fixtures follow the existing `FIXTURES.md` tier convention:
  tier (a) generated locally by a reference tool (gzip, bzip2, xz, zstd,
  Info-ZIP, GNU tar are on PATH), tier (b) copied from a crate's bundled test
  data (lz4, brotli, 7z — no local tool). Every fixture gets a FIXTURES.md
  entry with origin/commands and exact contents.
- MIT copyright line: `Copyright (c) 2026 rcomp contributors`.
- No `repository` field in Cargo.toml yet (no git remote configured — M7/CI
  territory).

## Unit A — restore directory permissions on extraction (independent)

**Files:** `crates/rcomp-core/src/archive/tar.rs`, `src/archive/zip.rs`,
new `crates/rcomp-core/tests/permissions.rs`.

During extraction, when an entry is a directory with a unix mode, do **not**
apply the mode immediately; collect `(path, mode)` pairs and apply them after
all entries are written, deepest-first, in both backends (`#[cfg(unix)]`;
non-unix keeps current behavior). Regular-file mode restore stays as is.

**Tests** (`permissions.rs`, `#[cfg(unix)]`): build a source tree containing a
dir with mode `0o750` and a read-only dir `0o550` that contains a file; for
each of tar and zip: compress via `ops::compress`, extract via `ops::extract`,
assert both dir modes are restored exactly and extraction succeeds (the
read-only dir must not block its own child being written). Also one roundtrip
through `tar.gz` to prove the codec-wrapped path inherits the fix.

## Unit B — Ctrl-C graceful cancellation in the CLI (independent)

**Files:** root `Cargo.toml` (add `ctrlc = "3"` to workspace deps),
`crates/rcomp/Cargo.toml`, `crates/rcomp/src/main.rs` and/or `src/run.rs`,
new `crates/rcomp/tests/cancel.rs`.

Install a `ctrlc` handler at startup that trips a shared
`rcomp_core::CancelToken`; pass that token into every `compress`/`extract`
call (the options structs already carry one — check how core receives it and
wire accordingly). On `Error::Cancelled`: stderr line `cancelled`, exit 1.
The handler must not break the indicatif bar teardown (finish/abandon the bar
on the error path).

**Test** (`cancel.rs`, `#[cfg(unix)]`, assert_cmd/std::process): generate a
large incompressible temp file (e.g. 64–128 MiB from `/dev/urandom`), spawn
the built binary compressing it with `--edge` xz (slow enough to interrupt),
sleep briefly, send SIGINT via `kill -INT <pid>`, wait. Assert: nonzero exit,
stderr mentions cancellation, and **no partial output file remains** (core's
Cancelled cleanup ran). Tune size/codec so the test stays under ~10 s.

## Unit C — interop fixtures + detection/extraction tests (independent)

**Files:** `crates/rcomp-core/tests/fixtures/` (new fixture files +
`FIXTURES.md` update), new `crates/rcomp-core/tests/interop.rs`.

Tier (a) — generate with the on-PATH reference tools, small (<4 KiB inputs),
committed: `sample.txt.gz` (gzip, embedded original name), `sample.txt.bz2`,
`sample.txt.xz`, `sample.txt.zst`, `sample.tar` (GNU tar, ≥2 entries incl. a
subdirectory), `sample.tar.gz`, `sample.zip` (Info-ZIP, ≥2 entries incl. a
subdirectory). Record the exact creation commands in FIXTURES.md.

Tier (b) — look for usable bundled test data in `~/.cargo/registry/src/` for
`lz4`, `brotli`, and `sevenz-rust2` (the existing `sample.rar` came from the
unrar crate this way). Copy + document what you find; if a format has no
usable upstream fixture, note the gap in FIXTURES.md and **report it** — do
not synthesize a fixture with our own encoder (that defeats interop).

**Tests** (`interop.rs`): for every fixture — `detect()` returns the expected
`Format`; a copy with the extension stripped still detects via magic bytes
(skip brotli: no magic); codec fixtures decompress to the known content;
archive fixtures `list()` the expected entries and extract to the expected
tree; the extensionless `.gz` copy extracts to the gzip-header original name.

Known blocker to report, not solve: the 7z unix-attribute mapping
investigation (CURRENT_ISSUES.md) needs a p7zip-created fixture with known
permissions; no 7z tool is on PATH.

## Unit D — overwrite-policy edge-case tests (independent)

**Files:** new `crates/rcomp-core/tests/overwrite.rs`, new
`crates/rcomp/tests/overwrite_cli.rs`. Test-only unit: **report** any bug
found, do not change src.

Core (`overwrite.rs`): compress to an existing output without overwrite →
`AlreadyExists` and the existing file's content is untouched; with
`overwrite` → replaced; compress where the output path is an existing
directory → error, not silent clobber; extract where one target file already
exists in dest → error and the pre-existing file keeps its content; unrelated
pre-existing files in dest stay untouched; extract with overwrite → succeeds
and replaces.

CLI (`overwrite_cli.rs`, assert_cmd): refused overwrite → exit 1 + stderr
contains a `--force` hint; `--force` → exit 0; extracting the same archive
twice into the same dest (wrap folder collision) → second run's behavior is
pinned by a test (error without `--force`, success with it).

## Unit E — README, licenses, rustdoc, crate metadata (sequential, after A–D)

**Files:** new `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`;
`crates/rcomp-core/src/*.rs` (rustdoc only — no behavior changes); root +
crate `Cargo.toml` metadata.

- **README.md:** what rcomp is (library + CLI), install (`cargo install
  rcomp`), the usage examples from PLAN.md's Goal block, options summary,
  the unified level table, supported-formats matrix (rar = extract-only),
  **unrar license note** (the default `rar` feature binds the freeware unrar
  library — not OSI; `--no-default-features`/`--no-default-features
  --features ...` builds without it), a **Limitations** section: 7z doesn't
  preserve unix permissions/symlinks (crate limitation), brotli has no magic
  bytes and no checksum (corrupt streams can decode silently; extensionless
  brotli needs `--algo`), zip extraction progress has no percentage. Dual
  MIT/Apache license section.
- **LICENSE-MIT** (`Copyright (c) 2026 rcomp contributors`) and
  **LICENSE-APACHE** (standard Apache-2.0 text).
- **Rustdoc:** add `#![warn(missing_docs)]` to `rcomp-core`'s lib.rs and
  document every public item it flags (error variants, Format/Codec/Container,
  Level, options fields, Progress/Report/Entry/CancelToken, detect fns, ops
  fns). Put the brotli caveat on the relevant codec/extract docs. Gate:
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p rcomp-core` clean.
- **Cargo.toml metadata:** `readme = "README.md"`, `keywords`, `categories`
  in `[workspace.package]` + inherit in both crates (publishing readiness;
  no `repository` yet).

## Out of scope for all units

CI workflows and crates.io publishing (M7), the 7z unix-attribute mapping
implementation (blocked on a p7zip fixture — log it), `rcomp verify`,
extended attributes/ACLs. Report — do not fix — anything beyond your unit.
