# Milestone 7 — checksums & gitignore-aware compression

Implements build-order step 7 of `PLAN.md` (sections "Output checksum
(opt-in) + sidecar auto-verify" and ".gitignore-aware folder compression" are
the binding spec — read both before starting). Branch: `feat/m1-scaffold`.

Four **sequential** units — 1 and 2 both rework `ops.rs`/`progress.rs`, 3
consumes both, 4 documents the result.

Verification for every unit (all must pass):

```
cargo build && cargo test --workspace --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Note: `rcomp-core` has `#![warn(missing_docs)]` and rustdoc is built with
`-D warnings` in CI intent — every new public item must be documented as it
is introduced, not retro-fitted in unit 4.

## Binding decisions (beyond PLAN.md)

- **Progress totals respect filtering**: for directory inputs,
  `bytes_total` is the sum of *included* file sizes. A filtered-out 10 GiB
  `target/` must not inflate the denominator.
- **Excluded count** = number of filesystem entries (files *and* dirs,
  recursively — a pruned dir counts itself plus everything inside) under the
  input that were not archived. Computed as unfiltered count minus included
  count; the extra metadata-only walk is acceptable.
- **Plain `.tar` output**: artifact and content digests are the same bytes —
  emit both (consumers shouldn't special-case).
- **`Default` for `CompressOptions`** must keep `follow_gitignore: true`
  (manual `impl Default` — a derived one would default the bool to false).
- **Entry order**: the shared walker yields a deterministic sorted order
  (`sort_by_file_name`). Existing tests that pin archive contents must stay
  green; they compare trees/sets, not creation order, but verify.
- **Symlink handling on create is unchanged**: the walker reports symlinks
  the way `read_dir` recursion did (no following; `follow_links(false)`),
  and backends keep their current per-entry-type behavior.
- **CLI flag scoping**: `--checksum`, `--all`, `--exclude` on an *extract*
  operation is a usage error (exit 2) — they are compress-only and the mode
  is known after inference.
- **Sidecar pre-flight**: the CLI checks `<output>.sha256` for existence
  *before* starting compression (refuse without `--force`), so we never do
  the work and then fail.
- **`-q` quiets the note and digest lines** like all other non-error output;
  the sidecar file is still written.
- **Excluded-count note goes to stderr** (it is advisory, like the
  silent-tar warning; stdout keeps only the summary).

## Unit 1 — core: shared filtered walker (gitignore + exclude globs)

**Files:** new `crates/rcomp-core/src/walk.rs`; `src/ops.rs`;
`src/progress.rs` (`Report`); `src/lib.rs` (module + any re-export);
`src/archive/tar.rs`, `src/archive/zip.rs`, `src/archive/sevenz.rs`
(create paths only); root `Cargo.toml` + `crates/rcomp-core/Cargo.toml`
(add `ignore = "0.4"` workspace dep); new
`crates/rcomp-core/tests/gitignore.rs`.

1. **`walk.rs`** — `pub(crate) fn collect(root: &Path, opts: &WalkOptions)
   -> Result<WalkResult>` where `WalkOptions { follow_gitignore: bool,
   exclude: &[String] }` and `WalkResult { entries: Vec<WalkEntry>,
   bytes_total: u64, excluded: u64 }`, `WalkEntry { abs: PathBuf,
   rel: PathBuf, is_dir: bool, size: u64 }` (rel = path relative to root;
   root itself is not an entry). Built on `ignore::WalkBuilder`:
   - `.hidden(false)` (dotfiles are included), `.parents(false)`,
     `.git_global(false)`, `.git_exclude(false)`, `.ignore(false)`
     (no `.ignore` files — git semantics only),
     `.git_ignore(follow_gitignore)`, `.require_git(false)` (a `.gitignore`
     works without a `.git` dir — PLAN.md decision),
     `.follow_links(false)`, `.sort_by_file_name(...)`.
   - The `.git` directory is pruned via `filter_entry` whenever
     `follow_gitignore` is true (it is NOT excluded under `--all`).
   - `exclude` globs go through `ignore::overrides::OverrideBuilder` with
     each pattern prefixed `!` (overrides whitelist by default; `!pat`
     means exclude) — verify semantics against the crate docs and test it.
     Invalid glob → `Error::InvalidGlob { pattern, source-ish message }`
     (new variant; CLI maps it to exit 2). Excludes apply even when
     `follow_gitignore` is false.
   - `excluded` count: when any filtering is active, also do a plain
     unfiltered recursive count and subtract; when no filtering applies
     (no .gitignore anywhere, no excludes, or follow=false+no excludes),
     skip the second walk and report 0.
2. **Options/Report plumbing** (`ops.rs`, `progress.rs`):
   `CompressOptions` gains `follow_gitignore: bool` (default **true**) and
   `exclude: Vec<String>`; manual `impl Default`. `Report` gains
   `entries_excluded: u64` (0 for non-directory inputs and extraction).
3. **Backend refactor**: `tar::create`, `zip::create`, `sevenz::create`
   currently each recurse with `fs::read_dir`. Change them to consume the
   precomputed `&[WalkEntry]` (plus the input root for opening files)
   instead of walking themselves; `ops::do_compress` calls `walk::collect`
   once for directory inputs and passes the slice to whichever backend runs
   (the silent-tar path therefore inherits filtering for free). File inputs
   bypass the walker entirely (no filtering, `excluded = 0`).
   `scan_input_size` stays only for the file-input case;
   `ctx.progress.bytes_total` for dir inputs comes from
   `WalkResult.bytes_total`. Preserve each backend's existing per-entry
   behavior exactly (modes, mtimes, symlinks, empty dirs, entry naming) —
   the whole existing test suite is the regression net.
4. **Tests** (`tests/gitignore.rs`): tree with root `.gitignore`
   (`target/`, `*.log`), a nested `sub/.gitignore` (`local-only.txt`), a
   `.git/` dir with a dummy file, a hidden `.editorconfig`, and matching
   ignored + non-ignored files. For each of tar, zip, 7z, and the
   silent-tar `.bz2` path: default compress excludes ignored files + `.git`
   but keeps `.editorconfig` and `.gitignore` files themselves;
   `follow_gitignore = false` includes everything (`.git` too);
   `exclude: ["*.txt"]` filters with both follow settings;
   `Report.entries_excluded` correct in each case and 0 when the tree has
   no `.gitignore` and no excludes; invalid glob errors; `bytes_total` seen
   by the progress callback equals the included sizes only (assert the
   final `bytes_done == bytes_total` for a filtered tar compress).

## Unit 2 — core: digests + verify (after 1)

**Files:** `crates/rcomp-core/src/ops.rs`, `src/error.rs`,
`src/progress.rs` (`Report`), `src/lib.rs`; root `Cargo.toml` +
`crates/rcomp-core/Cargo.toml` (add `sha2 = "0.10"`); new
`crates/rcomp-core/tests/checksum.rs`. (A small private `hash` helper
module inside ops.rs or its own file — implementer's choice.)

1. **Types**: `CompressOptions.checksum: bool` (default false).
   `Report.sha256: Option<String>` + `content_sha256: Option<String>`
   (lowercase hex). `ExtractOptions.verify_sha256: Option<String>` +
   `verify_content_sha256: Option<String>`. New
   `Error::ChecksumMismatch { kind: &'static str /* "artifact"|"content" */,
   expected: String, actual: String }` (or a small enum — match error.rs
   style; document variants).
2. **Compress digests** (only when `opts.checksum`):
   - *Artifact*: tee everything written to the output file through a
     `HashingWriter` (wraps the `File`, updates `Sha256` on write). All
     paths: codec-only, tar(+codec), zip, 7z — the artifact digest always
     exists.
   - *Content*: codec-only file input → hash the input bytes as they are
     read (HashingReader). tar+codec → insert a HashingWriter between
     `tar::create` and the encoder (tar bytes = content). Plain tar →
     content = artifact (emit both, same value). zip/7z → `None`.
3. **Verify on extract**:
   - `verify_sha256` set → stream-hash the input file *before* `do_extract`
     (separate read pass) and fail with `ChecksumMismatch` before anything
     is written to `dest`.
   - `verify_content_sha256` set → hash the decompressed stream during
     extraction and compare at the end: wrap the decoder (or raw reader for
     plain tar) in a shared-state hashing reader **before** the tar sniff
     so sniffed bytes are hashed too (the sniff prefix is re-chained, but
     hashing happened at the original read — do not hash the replayed
     prefix twice; an `Arc<Mutex<Sha256>>`-backed reader the caller can
     finalize after the stream is consumed works). Covers: tar(+codec),
     tar-inside-codec via sniff, single-file codec. For zip/7z/rar paths,
     `verify_content_sha256: Some(_)` → `Error::UnsupportedOperation`
     (content digests are never produced for them; refuse rather than
     silently skip).
4. **Tests** (`tests/checksum.rs`): artifact digest matches `sha256sum`
   run on the output (`std::process::Command`, `#[cfg(unix)]` where
   needed) for a `.zst` file, a `.tar.gz` dir, a `.zip` dir; content digest
   present + correct for codec/tar paths (for `.tar.gz`: equals sha256 of
   the decompressed stream — check via `gzip -dc | sha256sum` or
   re-hashing with sha2), `None` for zip/7z; plain `.tar` → both equal;
   `checksum: false` → both `None`; roundtrip: compress with checksum,
   extract with both verify fields from the report → Ok; flip one byte in
   the artifact → artifact `ChecksumMismatch` and dest dir still empty;
   pass a wrong content digest → content `ChecksumMismatch`; zip +
   `verify_content_sha256` → `UnsupportedOperation`.

## Unit 3 — CLI: flags, sidecar, auto-verify, notes (after 2)

**Files:** `crates/rcomp/src/cli.rs`, `src/run.rs`, `src/ui.rs` (if the
note/summary helpers live there); new `crates/rcomp/tests/checksum_cli.rs`
and `crates/rcomp/tests/gitignore_cli.rs`.

1. **Flags** (cli.rs): `--checksum`, `--all`,
   `--exclude <GLOB>` (repeatable, `ArgAction::Append`). Help text per
   PLAN.md's options block. After inference resolves to Extract, any of the
   three → usage error, exit 2, message saying they apply to compression.
2. **Compress path** (run.rs): map flags into `CompressOptions`
   (`checksum`, `follow_gitignore: !all`, `exclude`). Pre-flight: when
   `--checksum` and `<output>.sha256` exists and no `--force` → exit 1 with
   the `--force` hint before any work. After success write the sidecar:

   ```
   # content-sha256: <hex>\n      ← only when report.content_sha256 is Some
   <hex>  <output-file-name>\n
   ```

   (file name only, no directory components). Print to stdout after the
   summary line: `sha256: <hex>` (artifact), suppressed by `-q`.
   `Error::InvalidGlob` from core → exit 2.
3. **Excluded note** (stderr, not `-q`): when `report.entries_excluded > 0`
   print one line — `excluded N paths via .gitignore (use --all to
   include)` when only gitignore filtering was active, `... via --exclude`
   when only excludes, `... via .gitignore and --exclude` when both. (The
   CLI knows which sources were configured; core only reports the count.)
4. **Extract path** (run.rs): if `<input>.sha256` exists: parse it —
   artifact line = the line whose filename field matches the input's
   `file_name()` (tolerate other lines), optional `# content-sha256: <hex>`
   comment. Unparseable / no matching line → error, exit 1 (an existing
   sidecar is a promise). Feed digests into `ExtractOptions`;
   `ChecksumMismatch` → clear stderr message, exit 1. Print a
   `verified sha256` note (stderr, not under `-q`) on success so users see
   verification happened. No sidecar → exactly today's behavior.
5. **Tests**: checksum_cli — compress `--checksum` writes a sidecar that
   `sha256sum -c` accepts (run the real tool); summary shows the digest;
   `-q` shows nothing on stdout but still writes the sidecar; sidecar
   collision → exit 1 + hint, `--force` overwrites; extract with sidecar
   succeeds + mentions verification; corrupt the archive → exit 1, mismatch
   message; corrupt/garbage sidecar → exit 1; `--checksum` on extract →
   exit 2. gitignore_cli — tree with `.gitignore`: default compress
   excludes + note on stderr with correct count; `--all` includes
   everything, no note; `--exclude '*.log'` works alone and with `--all`;
   no exclusions → no note; bad glob → exit 2; `--all` on extract → exit 2.

## Unit 4 — README + rustdoc polish (after 3)

**Files:** `README.md`; only doc-comment-level touch-ups in
`crates/rcomp-core/src/*` if anything was left unclear.

- README: extend Usage/Options for the three flags; new "Checksums"
  section: `--checksum` example, sidecar format (artifact line +
  `# content-sha256` comment), `sha256sum -c` verification example,
  auto-verify on extraction (and that a missing sidecar changes nothing);
  extend the gitignore behavior into a short ".gitignore awareness"
  section (what is excluded, `--all`, `--exclude`, the note line,
  reproducibility decision — no global gitignore). Mention content digest
  as brotli's only integrity check in Limitations.
- Verify `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p rcomp-core`.

## Out of scope for all units

`rcomp verify` subcommand, checksum algorithms other than SHA-256,
machine-local git ignore sources, xz-mt cancellation latency (logged),
CI (M8). Report — do not fix — anything beyond your unit.
