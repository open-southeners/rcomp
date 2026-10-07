# Current issues

Discovered gaps, bugs, and spec questions routed here during orchestrated
implementation. Each entry: **Where**, **What**, suggested **Fix**.

## Open

- **Where:** Release notes / README + `.github/workflows/release.yml` (`desktop` job)
  **What:** The desktop bundles built and attached by the new tag-triggered
  `desktop` job are built with the default `rar` feature on, so they link the
  freeware `unrar` library (extract-only, **not** OSI-approved). This is fine
  for binary distribution but is currently undocumented for end users, and the
  bundles are **unsigned** (macOS Gatekeeper / Windows SmartScreen warnings
  expected). Discovered while implementing milestone step 2.
  **Fix:** Milestone step 4 (docs) — add a line to the release notes / README
  noting the unsigned + bundled-`unrar` status. Optionally offer an OSI-clean
  (`--no-default-features`) bundle variant later (tracked in `PLAN_EXTRAS.md`).

- **Where:** `apps/rcomp-desktop` (M4 UI flows — interactive behaviour)
  **What:** M4 was implemented and verified only headlessly (`bun run build`,
  `bunx svelte-check` = 0 errors, `cargo build -p rcomp-desktop`). The GUI was
  never launched (no display in the build env), so the interactive flows are
  **unproven at runtime**. Specifically unverified: (a) drag-drop delivers real
  FS paths via `getCurrentWebview().onDragDropEvent` (`dragDropEnabled` defaults
  true and is not disabled in `tauri.conf.json`, but the `event.payload.paths`
  shape is version-sensitive); (b) the dialog plugin pickers/confirm fire
  (`dialog:default` is granted, which covers open/save/ask/confirm); (c) the
  `invoke` top-level arg casing (`jobId` → `job_id`, etc.) round-trips as
  expected; (d) `write_sidecar`'s `content_sha256: null` serialises through the
  JSON bridge; (e) the three policy dialogs (silent-tar, overwrite-retry, wrap)
  and the checksum-mismatch alert actually display.
  **Fix:** Run the plan's manual checklist on a real desktop: drop file / drop
  folder / drop archive / pick via Browse / cancel mid-way / overwrite flow /
  corrupted-sidecar flow / silent-tar confirm / wrap confirm. All `invoke`/Channel
  usage is centralised in `src/lib/ipc.ts`, so any casing fix is one place.

- **Where:** `apps/rcomp-desktop/src/lib/components/{CompressCard,ExtractCard}.svelte`
  **What:** Two `svelte-check` warnings (`state_referenced_locally`): `$state`
  initialised directly from an immutable prop (`defaultFormat(inspect.is_dir)`,
  `deriveParent(inputPath)`). Intentional one-time defaults; the compiler can't
  tell. Build is otherwise warning-clean (0 errors).
  **Fix:** Cosmetic — silence by computing the default in the parent and passing
  it as a prop, or wrap the initial read in Svelte's `untrack()`. No behaviour
  change; do at leisure.

- **Where:** `.github/workflows/ci.yml` (`test` job) + `PLAN_TAURI.md` Acceptance
  **What:** Adding `apps/rcomp-desktop/src-tauri` as a workspace member means
  `cargo test --workspace --all-features` now pulls in the heavy Tauri GUI crate,
  which needs webkit2gtk/gtk system libs — it would fail on the macOS/Windows
  `test` matrix legs. The CI `test` job was therefore changed to
  `cargo test --workspace --all-features --exclude rcomp-desktop --locked`, and a
  separate `desktop` job (Linux, with apt webkit deps) builds the app. This means
  the plan's literal acceptance command (`cargo test --workspace --all-features`,
  no exclude) no longer holds as-written on all three OSes.
  **Fix:** Treat `--exclude rcomp-desktop` (core/CLI suite) + the dedicated
  `desktop` build job as the acceptance bar, and update the wording in
  `PLAN_TAURI.md` Acceptance to match. If a true all-in-one `--workspace` test is
  wanted later, every OS leg must install the Tauri system deps and pre-build the
  frontend `dist/`. Note: the `clippy` job has no `--workspace`, so it follows
  `default-members` (core+CLI only) and stays fast — if `--workspace` is ever
  added there, it needs the same apt-dep step.

- **Where:** `.github/workflows/ci.yml` (`bundle` job) — AppImage/dmg/msi
  **What:** The M5 `bundle` job builds installable packages on all three OSes via
  `bunx tauri build` and uploads them as artifacts (gated to `push` events).
  Locally only the **Linux `.deb`** path was proven end-to-end (a valid 5.9 MB
  `rcomp_0.1.0_amd64.deb` with the `rcomp-desktop` binary + `.desktop` + icons).
  AppImage could not be built here (`appimagetool`/`linuxdeploy` absent), and
  dmg/msi need macOS/Windows — so those three bundle types are **verified by CI
  only**. AppImage on `ubuntu-latest` (24.04) sometimes needs FUSE handling
  (Tauri uses extract-and-run); `libfuse2` was deliberately NOT pinned (the
  package is `libfuse2t64` on 24.04 — a wrong name would break apt).
  **Fix:** Watch the first real `bundle` run; if AppImage fails, add the correct
  FUSE package for the runner image or restrict Linux to `--bundles deb`. (Folds
  into the existing "CI unverified by a real run" entry below.)

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

- **Where:** root `Cargo.toml` `[workspace.package]` (+ README)
  **What:** No `repository`/`homepage`/`documentation` metadata — `cargo
  publish --dry-run` warns "manifest has no documentation, homepage or
  repository" on every run, and the README has no CI badge. All blocked on
  the GitHub remote URL, which doesn't exist yet.
  **Fix:** Once the remote is created: add `repository =
  "https://github.com/<org>/rcomp"` to `[workspace.package]` (crates inherit
  it), consider `documentation = "https://docs.rs/rcomp-core"` for the core
  crate, and add the CI badge to README.

- **Where:** `.github/workflows/ci.yml` + `release.yml` (unverified by a real run)
  **What:** Written ahead of the remote; validated locally (YAML parse, every
  CI cargo command run green on Linux) but never executed by GitHub Actions.
  Specifically unproven: the Windows and macOS test-matrix legs (C-dependency
  builds: liblzma, lz4, bzip2), rust-cache behavior, the release tag guard,
  crates.io auth, and the publish-core-then-cli sequencing/idempotency.
  **Fix:** After the first push, watch the first CI run on all three OSes and
  the first tagged release end-to-end; fix fallout then. Until then treat the
  workflows as best-effort.

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

- **Where:** `crates/rcomp-wgpu/src/shaders/lz4.wgsl` (one invocation per
  64 KiB block)
  **What:** The portable kernel under-occupies large discrete GPUs. A 64 MiB
  batch is 1,024 invocations (16 workgroups) on an 82-SM RTX 3090.
  `nvidia-smi dmon` shows 80–100% SM time but only about 150 W of 390 W and
  1–2% memory-controller use. On the 3090 the warm GPU path is about 3x slower
  than CPU on incompressible data (0.664 s vs 0.213 s for 128 MiB) and 2x slower
  on a realistic 1 GiB tar (3.49 s vs 1.76 s). Output is about 7.6% larger than
  CPU LZ4 on the tar. This is evidence, not a correctness defect.
  **Fix:** Design work, out of scope for qualification: more parallelism per
  block (for example cooperative match search within a workgroup) or more,
  smaller independent blocks. Keep one shared WGSL kernel.

- **Where:** `crates/rcomp-wgpu` (host staging), CLI memory footprint
  **What:** GPU compression peaks at about 700 MiB working set on the RTX 3090
  and about 1 GiB on the Radeon iGPU (shared memory), versus 139 MiB for CPU
  LZ4, for the same 1 GiB input. The cause is two in-flight 64 MiB batches plus
  the padded host copy, the upload staging for `write_buffer`, and the mapped
  readback buffers. This matters for the desktop app's memory budget.
  **Fix:** Reuse the padded input allocation and write directly from the
  pending batch when it is already 4-byte aligned. Consider a mapped upload
  ring instead of `queue.write_buffer`. Record the budget in the README.

- **Where:** `crates/rcomp-core/src/acceleration.rs:597` (and the explicit
  device message at :539)
  **What:** `--accelerator required` failures end with "...; using the CPU is
  the only supported path on this system". The operation actually fails and
  does **not** fall back, so the wording suggests a fallback that didn't
  happen. With an explicit `--accelerator-device`, `--best` fails with the
  generic "unavailable or incompatible" message and does not say that only
  `Fast` is supported.
  **Fix:** Use different wording for required-mode errors (for example "GPU
  encoding requires --fast on this device") and keep "using CPU" for `auto`
  notices only.

- **Where:** `.github/workflows/ci.yml` (`clippy` job runs on Linux only), CLI
  cancel test (`crates/rcomp/tests/cancel.rs` is `#![cfg(unix)]` and uses
  codec-only `.bz2`)
  **What:** Windows-only lint failures went unnoticed in `rcomp-core` (unused
  unix-only imports), in the CLI checksum tests (fixed during Windows
  qualification) and in `rcomp-metal`, where
  `cargo clippy --workspace --all-targets --all-features --exclude rcomp-desktop`
  still fails on Windows (`provider.rs`: unused `CompressedBlock` and
  `MetalError` imports on non-Apple targets; not fixed, out of scope). The
  tar cancellation hang (now fixed) was also never exercised: the only CLI
  cancel test is unix-only and tar-free.
  **Fix:** Add `windows-latest` to the clippy job (or a Windows clippy step to
  the `test` matrix leg), and gate the `rcomp-metal` imports behind the same
  `cfg` as their users. Consider a Windows CLI cancel test that sends a real
  `CTRL_C_EVENT` from a helper process on a private console.


## Resolved

- **Where:** `crates/rcomp-wgpu/src/provider.rs` (one dispatch per batch)
  **Outcome:** Resolved. Each 64 MiB batch was a single compute dispatch. On
  the Ryzen 7 7800X3D's Radeon iGPU, an incompressible batch took 1.36 s of
  GPU time, about 70% of the 2 s Windows TDR timeout. Batches are now
  compressed in submissions sized by a per-device 500 ms time budget
  (`SubmissionBudget`). They start at 8 MiB, double while 2x the last time
  fits, and halve on an overrun. The worst submission is now 282 ms
  (random input) or 441 ms (realistic tar) on the iGPU, and 222 ms on an
  RTX 3090, which still runs whole batches. A fixed split was rejected
  because it made the RTX 3090 3x slower on realistic data. Unit tests drive
  the controller with the measured narrow and wide device behaviour.
  Remaining cost: about 0.5 s of ramp-up per process on wide GPUs, and the
  iGPU tar is 15% slower (10.9 s → 12.5 s). If the ramp-up matters, persist
  the learned size per device or start larger once a device is known.

- **Where:** `crates/rcomp-core/src/ops.rs` — `extract()`'s `output_bytes`.
  **Outcome:** Resolved — `output_bytes` was computed by recursively summing
  every file currently under `dest` (`sum_dir_size`), which folded in
  unrelated pre-existing files whenever the archive's single top-level root
  meant `dest` was not a subfolder created fresh for this extraction (e.g.
  extracting into `~/Downloads` reported the size of everything else already
  there — reproduced live as `output_bytes` of 5.33 GB and a "32518610.9%"
  ratio in the desktop app's summary view for a real ~49 KB, 7-entry zip). No
  data was ever corrupted or overwritten incorrectly; this was a
  reporting-only bug. Fixed by having every archive backend (`tar`, `zip`,
  `sevenz`, `rar`) return the actual bytes it wrote to disk alongside the
  entry count, and using that in `do_extract`/`extract()` instead of scanning
  `dest` after the fact. Pinned by
  `extract_output_bytes_excludes_preexisting_unrelated_files` in
  `crates/rcomp-core/tests/overwrite.rs`.

- **Where:** `apps/rcomp-desktop/src-tauri/icons/` (placeholder icons)
  **Outcome:** Resolved in M5 — a real 1024×1024 source PNG
  (`icons/icon-source.png`) was run through `bunx tauri icon`, producing a valid
  multi-size `icon.icns` (52 KB, `ic12`) and 6-icon `icon.ico` plus the PNG set.
  The Linux `.deb` bundle embeds them correctly. The unused `android/`/`ios/`
  sets `tauri icon` also emits are gitignored (desktop-only project).

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
