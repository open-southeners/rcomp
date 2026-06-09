# Milestone 3 — archives (tar, zip), sanitizer, top-level operations

Implements milestone 3 of `PLAN.md`: tar + tar.codec composition (incl. the
silent-tar-folder rule), zip, the shared entry-path sanitizer with security
tests — plus the first version of the public `compress`/`extract`/`list`
operations covering everything supported so far (7z/rar extend the same
dispatch in milestone 4). Branch: `feat/m1-scaffold`.

Four sequential units; each leaves the workspace green:
`cargo build && cargo test && cargo clippy --all-targets -- -D warnings`.

## Design decisions binding all units

- **Wrapping is NOT core.** The CLI's extract auto-wrap folder (PLAN.md
  "Extraction wrapping") is computed by the CLI in milestone 5 using `list()`.
  Core `extract()` extracts into `dest` exactly as given.
- **zip is file-based, not stream-based.** The `zip` crate requires `Seek` on
  both ends, so the zip backend takes `&Path`s. It never composes with codecs.
- **tar is stream-based** (`Read`/`Write` trait objects) so it composes with
  the codec layer for `tar.*`.
- **Symlink policy:** archive entries that are symlinks are created only if
  the lexically-resolved target stays inside `dest` (every hop inside `dest`
  cannot escape). On non-unix targets, symlink entries are skipped. Symlink
  tests are `#[cfg(unix)]`.
- **Overwrite policy:** core refuses to overwrite existing outputs unless
  `overwrite: true` (new `Error::AlreadyExists`). On error/cancel mid-write,
  best-effort removal of the incomplete output.

## Shared contracts

```rust
// error.rs — ADD variant (unit 1):
#[error("output already exists: {path}")]
AlreadyExists { path: PathBuf },

// archive/mod.rs (unit 1)
pub(crate) struct OpCtx<'a> {
    pub cancel: CancelToken,
    pub on_progress: &'a mut dyn FnMut(&Progress),
    pub progress: Progress,
}
impl OpCtx<'_> {
    pub(crate) fn check_cancel(&self) -> Result<()>;       // Err(Cancelled)
    pub(crate) fn add_bytes(&mut self, n: u64);            // bump + emit
    pub(crate) fn set_entry(&mut self, name: impl Into<String>); // set + emit
}

// archive/sanitize.rs (unit 1)
pub(crate) fn sanitize_entry_path(dest: &Path, entry: &Path) -> Result<PathBuf>;
pub(crate) fn sanitize_link_target(dest: &Path, link_path: &Path, target: &Path) -> Result<()>;

// progress.rs (unit 1) — cancellable counting copy
pub(crate) fn copy_with_progress(
    r: &mut dyn Read, w: &mut dyn Write, ctx: &mut OpCtx<'_>) -> Result<u64>;

// archive/tar.rs (unit 2)
pub(crate) fn create(src: &Path, w: Box<dyn Write + '_>, ctx: &mut OpCtx<'_>) -> Result<u64>; // entry count
pub(crate) fn extract(r: Box<dyn Read + '_>, dest: &Path, overwrite: bool, ctx: &mut OpCtx<'_>) -> Result<u64>;
pub(crate) fn list(r: Box<dyn Read + '_>) -> Result<Vec<Entry>>;

// archive/zip.rs (unit 3)
pub(crate) fn create(src: &Path, out: &Path, level: Level, ctx: &mut OpCtx<'_>) -> Result<u64>;
pub(crate) fn extract(archive: &Path, dest: &Path, overwrite: bool, ctx: &mut OpCtx<'_>) -> Result<u64>;
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>>;

// ops.rs (unit 4) — PUBLIC API per PLAN.md sketch
#[derive(Debug, Clone, Default)]
pub struct CompressOptions { pub format: Option<Format>, pub level: Level, pub overwrite: bool, pub cancel: CancelToken }
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions  { pub format: Option<Format>, pub overwrite: bool, pub cancel: CancelToken }

pub fn compress(input: &Path, output: &Path, opts: &CompressOptions, on_progress: impl FnMut(&Progress)) -> Result<Report>;
pub fn extract(input: &Path, dest: &Path, opts: &ExtractOptions, on_progress: impl FnMut(&Progress)) -> Result<Report>;
pub fn list(archive: &Path) -> Result<Vec<Entry>>;
```

## Unit 1 — sanitizer, OpCtx, copy util, deps (sequential, first)

**Files:** root `Cargo.toml` + `crates/rcomp-core/Cargo.toml` (add `tar`,
`zip` workspace deps at current versions), `src/error.rs` (AlreadyExists),
`src/archive/mod.rs` (OpCtx + submodule decls), `src/archive/sanitize.rs`
(full), `src/archive/tar.rs` + `src/archive/zip.rs` (todo!() stubs with the
contract signatures), `src/progress.rs` (copy_with_progress), `src/lib.rs`
(`pub(crate) mod archive` — nothing public yet... `mod archive;` suffices).

- `sanitize_entry_path`: iterate components; reject `RootDir`/`Prefix`
  (absolute), any `ParentDir` (Error::PathTraversal), skip `CurDir`; result =
  dest.join(collected). Empty result (no components) → PathTraversal.
- `sanitize_link_target`: absolute target → PathTraversal. Else lexically
  normalize `link_path.parent().join(target)` (track depth; `..` below zero →
  PathTraversal) and require the result starts_with dest.
- `copy_with_progress`: 64 KiB buffer loop; `ctx.check_cancel()` each
  iteration; `ctx.add_bytes(n)`; returns bytes copied.
- **Security tests (the point of this unit):** entry `../evil`, `/abs/evil`,
  `foo/../../evil`, `C:\evil`-style prefix (cfg(windows) or string-built),
  empty path, `./ok/file` accepted, nested `a/b/c` accepted; link targets:
  absolute → reject, `../../x` → reject, `sub/ok` → accept, `../sibling`
  (inside dest) → accept. Cancel test: pre-cancelled token makes
  copy_with_progress return Cancelled without copying.

**Verify:** full workspace command.

## Unit 2 — tar backend (sequential, after 1)

**Files:** `src/archive/tar.rs` only.

- `create`: input file or dir. Dir: recursive walk (std, sorted entries for
  determinism), `Builder::append_path_with_name` / `append_dir` with paths
  relative to `src` (the dir itself is NOT an entry; its children are — i.e.
  archiving `photos/` yields entries `a.jpg`, `sub/b.jpg`). Single file:
  one entry with its file name. Symlinks in input preserved as symlink
  entries. Per entry: `ctx.set_entry`, `check_cancel`; for file bodies append
  via reader so `ctx.add_bytes` advances (manual header+append or
  append_data). Finish with `builder.into_inner()` (caller finishes encoder).
- `extract`: iterate `Archive::entries`; per entry sanitize path; dirs →
  create_dir_all; files → honor `overwrite` (AlreadyExists), write via
  copy_with_progress, restore unix permissions + mtime where the tar crate
  exposes them; symlinks → sanitize_link_target then create (unix), skip on
  non-unix; hardlinks → treat link target like a symlink target (sanitize,
  then std::fs::hard_link). Unknown entry types skipped.
- `list`: entries → `Entry { path, size, is_dir }` without writing anything.
- Tests: dir roundtrip (nested dirs, empty dir, empty file, permissions
  #[cfg(unix)]: 0o755 vs 0o644 preserved); single-file roundtrip; symlink
  roundtrip #[cfg(unix)]; **malicious archives built by hand with
  tar::Builder/raw headers**: `../evil` path → PathTraversal, absolute path →
  PathTraversal, symlink → `/etc` → PathTraversal, symlink → `../../x` →
  PathTraversal (all must leave dest clean); overwrite=false → AlreadyExists;
  list matches created entries; cancel mid-extract returns Cancelled.

**Verify:** full workspace command.

## Unit 3 — zip backend (sequential, after 2)

**Files:** `src/archive/zip.rs` only.

- `create`: file-or-dir walk like tar (same relative-path rule), deflate with
  level Fast=1 Best=6 Edge=9 (`FileOptions::compression_level`), directory
  entries for dirs, unix permissions into external attrs
  (`FileOptions::unix_permissions`), per-entry cancel/progress via
  copy_with_progress into the writer.
- `extract`: `ZipArchive::new(File)`. Per entry: sanitize (use OUR sanitizer
  on the raw name — do not rely solely on `enclosed_name`), dirs/files like
  tar with overwrite policy, restore unix permissions from external attrs;
  symlink entries (unix mode S_IFLNK in external attrs): read target from
  body, sanitize_link_target, create (unix only).
- `list`: entries without extraction.
- Tests: dir roundtrip incl. unicode filename (`héllo wörld.txt`) and empty
  dir; permissions #[cfg(unix)]; **zip-slip**: craft archives via ZipWriter
  `start_file("../evil.txt", ...)` and absolute name → PathTraversal, dest
  stays clean; overwrite policy; list; cancel; all three Levels produce
  decodable archives (and Edge ≤ Fast size on the repetitive corpus — skip
  the size assert if flaky, just roundtrip all levels).

**Verify:** full workspace command.

## Unit 4 — public ops: compress / extract / list (sequential, after 3)

**Files:** `src/ops.rs` (new), `src/lib.rs` (re-exports),
`crates/rcomp-core/tests/ops_roundtrip.rs` (new).

### compress(input, output, opts, on_progress)

1. Input must exist (Io otherwise). Output exists && !overwrite →
   AlreadyExists.
2. `format = opts.format` else `detect_from_extension(output file name)` else
   UnknownFormat{output}.
3. **Silent-tar rule:** format is codec-only AND input is a dir → treat as
   `Format::layered(Tar, codec)`. (Core applies it unconditionally; the
   warn+confirm UX is CLI, milestone 5.)
4. Dispatch:
   - codec-only + file → File reader → `new_encoder` → copy_with_progress.
   - Tar container (codec or not) → File writer, optionally wrapped in
     `new_encoder`, → `tar::create` → finish encoder.
   - Zip → `zip::create` (input file or dir both fine).
   - SevenZ/Rar → UnsupportedOperation (milestone 4).
5. Progress: pre-scan input (file len, or walk dir summing file sizes) →
   `bytes_total = Some(total)`; bytes_done advances on input bytes read.
6. On Err/cancel after output creation: best-effort `remove_file(output)`.
7. Report { input_bytes, output_bytes (output file len), entries, duration }.

### extract(input, dest, opts, on_progress)

1. `format = opts.format` else `detect(input)?`. `create_dir_all(dest)`.
2. Progress: `bytes_total = Some(input file len)`, bytes_done = **compressed**
   bytes consumed (wrap the File in a counting reader feeding ctx) — gives a
   true % for extraction.
3. Dispatch:
   - Zip → `zip::extract`.
   - Tar ± codec → (decoder-wrapped) reader → `tar::extract`.
   - codec-only → decode stream; **sniff the first 512 decompressed bytes**:
     `ustar` at offset 257 → it's tar-inside-codec (the silent-tar reverse) →
     chain sniffed bytes + rest into `tar::extract`. Otherwise single file
     into dest: name = (gzip only) embedded original-filename header if
     present — **take only the final file_name component** of it (header is
     attacker-controlled) — else input stem minus the codec extension, else
     `<input file name>.out`; overwrite policy; copy_with_progress.
   - Rar/SevenZ → UnsupportedOperation (milestone 4).
   - gzip header peek: `flate2::read::GzDecoder::header()` after construction
     (then decode through MultiGzDecoder on a fresh/rewound reader — or reuse
     the same GzDecoder if multi-member isn't compromised; implementer's call,
     note the choice).
4. Report as compress (entries = files+dirs+links written; 1 for single file).

### list(archive)

Zip → zip::list; Tar ± codec → tar::list (through decoder when layered);
codec-only / SevenZ / Rar → UnsupportedOperation. (7z/rar in milestone 4.)

### lib.rs

`pub mod ops;` is private detail — declare `mod ops;` and
`pub use ops::{compress, extract, list, CompressOptions, ExtractOptions};`.

### Integration tests (tests/ops_roundtrip.rs, public API only)

- Dir → `out.tar.gz` → extract → tree byte-identical (incl. nested + empty
  dir + #[cfg(unix)] permissions).
- **Silent-tar:** dir → `out.bz2` (codec-only name!) → compress applies tar
  wrap → extract `out.bz2` → sniff finds tar → original tree restored. THE
  flagship test of this milestone.
- File → `out.zst` → extract → identical file with stripped name.
- Gzip embedded-name: compress a file via raw flate2 with a filename header
  (test fixture built inline) → extract honors the embedded name.
- Dir → zip → extract roundtrip.
- `list` on tar.gz and zip match the created trees; list on `.zst` →
  UnsupportedOperation.
- Overwrite refusals both directions; pre-cancelled token → Cancelled and no
  output left behind; UnknownFormat on `compress(x, "out.weird")`.

**Verify:** full workspace command.

## Out of scope for all units

7z/rar (M4), CLI/inference/wrap-folder UX (M5), interop fixtures from
reference tools (M6), multi-input, passwords. Report — do not fix — anything
discovered beyond your unit.
