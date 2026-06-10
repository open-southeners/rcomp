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
  **Fix:** Documented as a format/crate limitation in the README (M6).
  Remaining open part: the `0x8000 | mode << 16` windows_attributes mapping
  idea. M6 investigation outcome: blocked — the sevenz-rust2 bundled test
  archives have `has_windows_attributes == false` throughout, and no
  p7zip/7z tool is on PATH to create a Linux fixture with known unix
  permissions. Revisit when a p7zip-created fixture is available or
  sevenz-rust2 grows unix-attribute support.

- **Where:** `crates/rcomp-core/src/codec/xz.rs:79-90` (multithreaded encoder)
  **What:** With `workers() > 1` the xz `MtStreamBuilder` encoder is
  block-parallel and asynchronous: `write()` calls return quickly (data is
  queued to worker threads) and the bulk of compression happens inside
  `finish()`. Core's cancellation check runs between 64 KiB `write` chunks,
  so once the input is fully queued a `CancelToken` trip (e.g. Ctrl-C) can
  no longer interrupt the operation — it completes normally. bzip2 and the
  other synchronous codecs cancel fine (the cancel integration test uses
  bzip2 for this reason).
  **Fix:** Either wrap `finish()` with a cancel-checked draining adapter, or
  fall back to the single-threaded xz encoder when cancellation
  responsiveness matters. Needs a small core design decision; not blocking
  V1.

- **Where:** `crates/rcomp-core/src/ops.rs:183-209` (compress, overwrite guard)
  **What:** Compressing to an output path that is an existing **directory**
  with `overwrite = true` bypasses the `AlreadyExists` guard and fails later
  in `fs::File::create` with a raw OS `Error::Io` ("Is a directory"). Safe
  (nothing is clobbered) but the error is unfriendly. Pinned by
  `compress_output_is_existing_directory_returns_error` in
  `crates/rcomp-core/tests/overwrite.rs`.
  **Fix:** Add an explicit `output.is_dir()` check that returns
  `Error::AlreadyExists` regardless of the `overwrite` flag, and update the
  pinning test.

- **Where:** `crates/rcomp/src/run.rs` (`cmd_extract`, sidecar verification order)
  **What:** For a corrupted codec-only archive (`.gz`, `.zst`, …) with a
  sidecar present, the CLI calls core `list()` (for the wrap decision) before
  `extract()`, and `list()` fails decoding the corrupt stream with a raw
  `Error::Io` — so the user sees a decode error instead of the more useful
  `ChecksumMismatch` the sidecar could have produced. Container formats whose
  `list()` doesn't decode payload data (zip) report the mismatch correctly.
  **Fix:** In `cmd_extract`, when a sidecar was parsed, hash the input and
  check the artifact digest *before* the `list()` wrap-decision call (the CLI
  already has the expected digest; a small streaming hash helper or a core
  `verify_file_sha256` export would do it). Cosmetic-priority: the operation
  still fails safely today, just with a worse message.

- **Where:** `crates/rcomp-core/tests/fixtures/` (lz4 interop gap)
  **What:** No lz4 interop fixture: `lz4` is not on PATH and no crate in the
  local cargo registry bundles a reference frame-format file (`lz4-sys` only
  ships a raw block-format sample). Documented in FIXTURES.md. The unused
  `sample_two_files.7z` (tier b, two entries) was copied alongside
  `sample.7z` and is available for a future multi-entry 7z interop test.
  **Fix:** When an `lz4` CLI is available on the build host, generate
  `sample.txt.lz4` with the reference tool and add detection + extraction
  cases to `tests/interop.rs`; optionally wire `sample_two_files.7z` into a
  multi-entry list test.

- **Where:** `crates/rcomp-core/src/archive/sevenz.rs` (`create`)
  **What:** 7z create progress is per-entry, not per-chunk —
  `push_archive_entry` consumes the whole file internally, so a single huge
  file shows no intermediate progress (and cancel is only honored between
  entries on the write path).
  **Fix:** Acceptable for V1. If needed later, wrap the source reader in a
  chunk-counting adapter that also checks cancellation (the crate accepts any
  `Read`), which restores both granular progress and intra-file cancel.

- **Where:** `crates/rcomp-core/src/ops.rs` (`extract`, zip branch)
  **What:** Zip extraction reports `bytes_total = None` (documented): the zip
  backend is `&Path`-based, so there's no compressed-bytes counting reader as
  with the streamed formats. Progress bars for zip will be spinner-style
  (count of bytes written, no percentage).
  **Fix:** Acceptable for V1 CLI. If percent-accurate zip progress is wanted
  later, sum entry compressed sizes from the central directory and count
  per-entry consumption instead.

- **Where:** `crates/rcomp-core/src/codec/lz4.rs` (`Encoder::finish` impl)
  **What:** The `Encoder` trait's `finish(self: Box<Self>) -> Result<()>`
  cannot return the inner writer, so lz4's `(W, Result)` writer is dropped.
  Fine for files/Vec sinks, but a future consumer wanting the writer back
  (e.g. streaming into a network socket it reuses) can't get it.
  **Fix:** If a real consumer needs it, change the trait to
  `finish(self: Box<Self>) -> Result<Box<dyn Write>>` in a minor refactor;
  don't pre-build it now.


## Resolved

- **Where:** `crates/rcomp/src/main.rs` (no Ctrl-C handler)
  **Outcome:** Resolved in milestone 6 — a `ctrlc` handler trips a shared
  `CancelToken` threaded through `run()` into every compress/extract call;
  `Error::Cancelled` abandons the progress bar, prints `cancelled`, exits 1.
  Pinned by `crates/rcomp/tests/cancel.rs` (SIGINT mid-compress: nonzero
  exit, no partial output left). Caveat: the xz multithreaded encoder limits
  cancel responsiveness — see the open xz entry above.

- **Where:** `crates/rcomp-core/src/archive/tar.rs` + `archive/zip.rs` (extract)
  **Outcome:** Resolved in milestone 6 — both backends now collect
  `(dir, mode)` pairs during extraction and apply them deepest-first after
  all entries are written, so read-only directories no longer block their
  own children. Pinned by `crates/rcomp-core/tests/permissions.rs`.

- **Where:** `crates/rcomp-core/src/codec/brotli.rs` (no magic bytes / checksum)
  **Outcome:** Resolved in milestone 6 — the caveat (extensionless `.br`
  needs `--algo brotli`; corrupted streams can decode into wrong bytes) is
  now documented in the README Limitations section and on the
  `Codec::Brotli` rustdoc. The `rcomp verify` integrity idea stays in
  `PLAN_EXTRAS.md`.

- **Where:** `crates/rcomp-core/src/format.rs` (`Codec::short_ext`)
  **Outcome:** Resolved by `split_format_suffix` in milestone 5 — external
  consumers use `split_format_suffix` to strip the recognised suffix and obtain
  both the stem and the `Format`, so `Codec::short_ext` remains `pub(crate)`.
