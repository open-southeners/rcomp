# Milestone 4 — 7z (read/write) + rar (extract-only, feature-gated)

Implements milestone 4 of `PLAN.md`: 7z via `sevenz-rust2`, rar via `unrar`
behind a `rar` cargo feature (off by default in core; the CLI enables it in
milestone 5). Branch: `feat/m1-scaffold`.

Two sequential units (both touch `ops.rs` + `archive/mod.rs`). Verification
for both (all must pass):

```
cargo build
cargo test
cargo test -p rcomp-core --features rar
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
```

## Design decisions binding both units

- **Both backends are `&Path`-based** like zip (7z needs `Seek`; unrar drives
  its own file I/O). Extraction progress follows the zip asymmetry:
  `bytes_total = None`, `bytes_done` advances by bytes written / entry sizes
  (the `extract()` doc comment already documents this for zip — extend it).
- **Sanitizer everywhere:** every entry name from a 7z or rar archive goes
  through `sanitize_entry_path` before any write; failures abort with
  `PathTraversal` (consistent with tar/zip).
- **Existing M3 test changes:** `tests/ops_roundtrip.rs` currently asserts 7z
  compress → `UnsupportedOperation`. Unit 1 REPLACES that with real 7z
  coverage. Keep an equivalent assertion only for rar-create (always
  unsupported) and for rar-extract when the feature is off.
- **No new public API.** Everything lands behind the existing
  `compress`/`extract`/`list` dispatch.

## Unit 1 — 7z backend (sequential, first)

**Files:** root `Cargo.toml` + `crates/rcomp-core/Cargo.toml` (add
`sevenz-rust2`, current latest, default features — check what's needed for
LZMA2 write), `src/archive/sevenz.rs` (new), `src/archive/mod.rs` (submodule
decl), `src/ops.rs` (SevenZ dispatch arms), `tests/ops_roundtrip.rs`.

- Contract (mirrors zip.rs):
  ```rust
  pub(crate) fn create(src: &Path, out: &Path, level: Level, ctx: &mut OpCtx<'_>) -> Result<u64>;
  pub(crate) fn extract(archive: &Path, dest: &Path, overwrite: bool, ctx: &mut OpCtx<'_>) -> Result<u64>;
  pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>>;
  ```
- `create`: file-or-dir, same relative-path rule as tar/zip (dir contents,
  dir itself not an entry), sorted walk, LZMA2 preset Fast=1 Best=5 Edge=9
  (per PLAN level table) via the crate's LZMA2 options API. Directory entries
  included. Per-entry `set_entry` + `check_cancel`.
- `extract`: iterate entries with the crate's reader API; sanitize names;
  dirs → create_dir_all; files → overwrite policy + write with progress +
  cancel between chunks if the API streams, else between entries; unix
  permissions/symlinks only if the crate exposes them — if not, skip and
  REPORT (do not contort).
- `list`: names/sizes/is_dir without writing.
- `ops.rs`: SevenZ arms in compress/extract/list dispatch (replace
  UnsupportedOperation). A dir → `out.7z` is a normal archive build (NO
  silent-tar — 7z is a container).
- Tests in sevenz.rs (mirror zip.rs suite): nested dir roundtrip + unicode
  name + empty dir; single file; all three Levels; **malicious entry name**
  (write an entry named `../evil` via the writer API if it allows arbitrary
  names — if the writer refuses, construct the header bytes manually or, if
  impractical, document why and rely on the sanitizer unit tests) →
  PathTraversal, clean dest; overwrite both ways; list; cancel.
  Integration tests in ops_roundtrip.rs: dir → `out.7z` → extract roundtrip,
  list on 7z, progress invariant test extended to 7z (bytes_total None).

## Unit 2 — rar backend, feature-gated (sequential, after 1)

**Files:** root `Cargo.toml` + `crates/rcomp-core/Cargo.toml` (`unrar` as
optional dep; `[features] rar = ["dep:unrar"]`), `src/archive/rar.rs` (new,
`#[cfg(feature = "rar")]` module), `src/archive/mod.rs`, `src/ops.rs`,
`tests/ops_roundtrip.rs`, possibly `tests/fixtures/`.

- Contract:
  ```rust
  pub(crate) fn extract(archive: &Path, dest: &Path, overwrite: bool, ctx: &mut OpCtx<'_>) -> Result<u64>;
  pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>>;
  ```
- `extract`: open for processing; per header: sanitize the entry name
  ourselves, compute the safe destination path, honor overwrite, extract via
  the per-entry API (`extract_to`-style with our sanitized path — do NOT let
  unrar derive the path from the entry name unchecked), dirs handled, cancel
  checked between entries, `set_entry` per entry, `add_bytes` by unpacked
  size after each entry.
- `list`: open for listing → Entry vec.
- `ops.rs`: Rar arms — extract/list delegate when `cfg(feature = "rar")`,
  else `UnsupportedOperation` with a message mentioning the `rar` feature;
  rar compress → always `UnsupportedOperation` (RAR creation is proprietary).
- **Fixture strategy** (in order of preference):
  1. If a `rar` binary exists on PATH, generate `tests/fixtures/sample.rar`
     (a small dir with 2 files + subdir) and commit it.
  2. Else look for a small `.rar` test archive bundled inside the unrar crate
     sources in `~/.cargo/registry/src/*/unrar-*/` — copy it to
     `tests/fixtures/` with a `FIXTURES.md` note recording its origin.
  3. Else mark rar tests `#[ignore = "no rar fixture available"]` and report
     it (CURRENT_ISSUES + milestone 6 fixtures will close the gap).
- Tests (`#[cfg(feature = "rar")]`): extract fixture → expected tree; list
  fixture; rar compress → UnsupportedOperation; traversal protection is
  enforced by our sanitizer (a malicious rar fixture can't be crafted without
  a rar encoder — note this; the sanitize call-path is shared and unit-tested).
  Plus a `#[cfg(not(feature = "rar"))]` test: rar extract →
  UnsupportedOperation mentioning the feature.

## Out of scope for both units

Passwords/encrypted archives, multi-volume rar/7z, CLI, interop fixtures
beyond the rar sample, permission fidelity beyond what the crates expose.
Report — do not fix — anything beyond your unit.
