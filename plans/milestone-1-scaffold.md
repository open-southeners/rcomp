# Milestone 1 — workspace scaffold, core types, format detection

Implements milestone 1 of `PLAN.md`. Three sequential units; each must leave
`cargo build && cargo test` green across the workspace before the next starts.
Toolchain: Rust 1.94 / edition 2024. Branch: `feat/m1-scaffold`.

## Unit 1 — workspace scaffold (sequential, first)

**Files:** `Cargo.toml` (workspace root), `crates/rcomp-core/Cargo.toml`,
`crates/rcomp-core/src/lib.rs`, `crates/rcomp/Cargo.toml`,
`crates/rcomp/src/main.rs`, `.gitignore`.

- Workspace root `Cargo.toml`: `[workspace]` with `members = ["crates/*"]`,
  `resolver = "3"`, and `[workspace.package]` sharing `edition = "2024"`,
  `license = "MIT OR Apache-2.0"`, `repository`/`description` placeholders,
  `version = "0.1.0"`. `[workspace.dependencies]`: `thiserror = "2"`.
- `rcomp-core`: lib crate, depends on workspace `thiserror`. `lib.rs` declares
  modules `error`, `level`, `progress`, `format`, `detect` and re-exports their
  public items; create each module as a minimal compiling stub (e.g. empty or
  `//! TODO` doc comment) — units 2 and 3 fill them in.
- `rcomp`: bin crate named `rcomp`, depends on `rcomp-core` (path). `main.rs`
  is a placeholder that prints the version from `env!("CARGO_PKG_VERSION")`
  and exits 0 — the real CLI is milestone 5.
- `.gitignore`: `/target` (and nothing project-foreign).

**Verify:** `cargo build && cargo test && cargo run -p rcomp` from repo root.

## Unit 2 — core types (sequential, after unit 1)

**Files:** `crates/rcomp-core/src/error.rs`, `src/level.rs`, `src/progress.rs`,
plus the re-exports in `lib.rs`.

Public contract (unit 3 compiles against this — keep names exact):

```rust
// error.rs
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unrecognized or unsupported format: {path}")]
    UnknownFormat { path: PathBuf },
    #[error("{operation} is not supported for {format}")]
    UnsupportedOperation { format: String, operation: String },
    #[error("operation cancelled")]
    Cancelled,
    #[error("archive entry escapes destination: {entry}")]
    PathTraversal { entry: PathBuf },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

// level.rs — unified levels per PLAN.md (mapping tables land with the codecs)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level { Fast, #[default] Best, Edge }

// progress.rs
#[derive(Debug, Clone)]
pub struct Progress { pub bytes_done: u64, pub bytes_total: Option<u64>, pub current_entry: Option<String> }

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);   // cancel(&self), is_cancelled(&self) -> bool
                                            // is_cancelled uses Ordering::Relaxed

#[derive(Debug, Clone)]
pub struct Report { pub input_bytes: u64, pub output_bytes: u64, pub entries: u64, pub duration: Duration }
impl Report { pub fn ratio(&self) -> f64 /* output/input, 0.0 when input is 0 */ }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry { pub path: PathBuf, pub size: u64, pub is_dir: bool }
```

Unit tests: `CancelToken` clone shares state; `Report::ratio` incl. zero-input;
`Level::default()` is `Best`.

**Verify:** `cargo test -p rcomp-core`.

## Unit 3 — format model + detection (sequential, after unit 2)

**Files:** `crates/rcomp-core/src/format.rs`, `src/detect.rs`, re-exports in
`lib.rs`.

### format.rs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec { Gzip, Bzip2, Xz, Zstd, Lz4, Brotli }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container { Tar, Zip, SevenZ, Rar }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format { pub container: Option<Container>, pub codec: Option<Codec> }
```

- Invariant: at least one of the two is `Some`. Provide constructors
  (`Format::codec(Codec)`, `Format::container(Container)`,
  `Format::layered(Container, Codec)` for `tar.gz`-style) rather than a public
  check. `Display` impls for `Codec`/`Container`/`Format` (lowercase canonical
  names: `gzip`, `tar.gz`, `7z`, …).

### detect.rs

Three public functions, per PLAN.md's detection rules:

```rust
pub fn detect_from_extension(file_name: &str) -> Option<Format>;
pub fn detect_from_bytes(header: &[u8]) -> Option<Format>;
pub fn detect(path: &Path) -> Result<Format>;   // magic first, extension as fallback/tiebreaker
```

- **Extension table** (longest-suffix match wins, case-insensitive):
  `.tar.gz`/`.tgz`, `.tar.bz2`/`.tbz2`, `.tar.xz`/`.txz`, `.tar.zst`/`.tzst`,
  `.tar.lz4`, `.tar.br`, `.tar`, `.gz`, `.bz2`, `.xz`, `.zst`, `.lz4`, `.br`,
  `.zip`, `.7z`, `.rar`.
- **Magic table** (hand-rolled, no `infer` crate): gzip `1F 8B`; bzip2 `42 5A 68`
  ("BZh"); xz `FD 37 7A 58 5A 00`; zstd `28 B5 2F FD`; lz4 frame `04 22 4D 18`;
  zip `50 4B 03 04` (+ `50 4B 05 06` empty archive); 7z `37 7A BC AF 27 1C`;
  rar4 `52 61 72 21 1A 07 00`; rar5 `52 61 72 21 1A 07 01 00`; tar: `ustar` at
  offset 257. **Brotli has no magic bytes** — extension-only, document this on
  `detect_from_bytes`.
- `detect(path)`: read up to 512 bytes (enough for the tar check). Magic hit +
  extension that *refines* it (magic says gzip, name says `.tar.gz` → `TarGz`)
  → refined format. Magic hit alone → magic. No magic hit → extension. Neither
  → `Error::UnknownFormat`. Note: tar-inside-codec sniffing of *decompressed*
  content is milestone 3, not here.

### Tests

- Extension: every table row, longest-suffix precedence (`.tar.gz` ≠ `.gz`),
  case-insensitivity, unknown → `None`.
- Magic: one synthetic header per format incl. both rar versions and both zip
  signatures, tar at offset 257, short buffers (< 4 bytes, empty) → `None`.
- `detect`: tmpfiles (use `tempfile` as dev-dependency) covering magic+ext
  refinement, magic-only (extensionless gzip), extension-only (brotli),
  unknown → `Error::UnknownFormat`, missing file → `Error::Io`.

**Verify:** `cargo test -p rcomp-core && cargo build`.

## Out of scope for all units

Codec/archive implementations, CLI argument parsing, `compress`/`extract`/
`list` functions, CI. Anything discovered that's wrong or missing in the plan:
report back, do not fix — it gets routed to `CURRENT_ISSUES.md`.
