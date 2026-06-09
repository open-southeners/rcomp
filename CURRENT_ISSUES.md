# Current issues

Discovered gaps, bugs, and spec questions routed here during orchestrated
implementation. Each entry: **Where**, **What**, suggested **Fix**.

## Open

- **Where:** `crates/rcomp-core/src/archive/sevenz.rs` (+ `rar.rs`)
  **What:** `sevenz-rust2` 0.21 exposes only `windows_attributes` — no unix
  mode bits, no symlink entries. So 7z archives don't preserve permissions
  (e.g. executable scripts come back 0644) and symlinks in the source are
  stored dereferenced as regular files. The `unrar` API similarly exposes no
  cross-platform unix metadata.
  **Fix:** Document as a format/crate limitation in README (milestone 6).
  Revisit if sevenz-rust2 grows unix-attribute support; consider mapping the
  7z "unix extension" attribute bits (`0x8000 | mode << 16`) which some tools
  write into windows_attributes — needs interop testing against p7zip output
  in milestone 6.

- **Where:** `crates/rcomp-core/src/archive/sevenz.rs` (`create`)
  **What:** 7z create progress is per-entry, not per-chunk —
  `push_archive_entry` consumes the whole file internally, so a single huge
  file shows no intermediate progress (and cancel is only honored between
  entries on the write path).
  **Fix:** Acceptable for V1. If needed later, wrap the source reader in a
  chunk-counting adapter that also checks cancellation (the crate accepts any
  `Read`), which restores both granular progress and intra-file cancel.

- **Where:** `crates/rcomp-core/src/archive/tar.rs` + `archive/zip.rs` (extract)
  **What:** Directory permissions are not restored on extraction (only regular
  files get their unix mode back). Restoring dir modes correctly requires
  applying them *after* all children are written (a read-only dir would block
  its own children otherwise) — the zip crate uses that two-phase pattern in
  its own extract.
  **Fix:** Milestone 6 hardening: collect (dir, mode) pairs during extraction,
  apply in reverse-depth order at the end, for both backends.

- **Where:** `crates/rcomp-core/src/ops.rs` (`extract`, zip branch)
  **What:** Zip extraction reports `bytes_total = None` (documented): the zip
  backend is `&Path`-based, so there's no compressed-bytes counting reader as
  with the streamed formats. Progress bars for zip will be spinner-style
  (count of bytes written, no percentage).
  **Fix:** Acceptable for V1 CLI. If percent-accurate zip progress is wanted
  later, sum entry compressed sizes from the central directory and count
  per-entry consumption instead.

- **Where:** `crates/rcomp-core/src/codec/brotli.rs` (format property, not a bug)
  **What:** Brotli has no magic bytes *and* no content checksum, so a corrupted
  `.br` stream can decode "successfully" into wrong bytes instead of erroring
  (covered by `corrupt_brotli_flipped_byte`, which has a permissive assertion;
  `test_util::corrupt` is unusable for brotli).
  **Fix:** Document this caveat in user-facing docs (README/rustdoc) during
  milestone 6 hardening; consider an integrity-check note in the future
  `rcomp verify` idea (PLAN_EXTRAS.md).

- **Where:** `crates/rcomp-core/src/codec/lz4.rs` (`Encoder::finish` impl)
  **What:** The `Encoder` trait's `finish(self: Box<Self>) -> Result<()>`
  cannot return the inner writer, so lz4's `(W, Result)` writer is dropped.
  Fine for files/Vec sinks, but a future consumer wanting the writer back
  (e.g. streaming into a network socket it reuses) can't get it.
  **Fix:** If a real consumer needs it, change the trait to
  `finish(self: Box<Self>) -> Result<Box<dyn Write>>` in a minor refactor;
  don't pre-build it now.

- **Where:** `crates/rcomp-core/src/format.rs` (`Codec::short_ext`)
  **What:** The codec→short-extension mapping (`gz`, `bz2`, …) is `pub(crate)`.
  The CLI (milestone 5) will likely need it for output-name derivation when
  stripping/choosing extensions (e.g. bare-codec extraction naming, `--algo`
  handling), and the Tauri app may too.
  **Fix:** When the first external consumer appears, promote it to `pub` (or
  expose an equivalent method on `Format`) instead of duplicating the table.
