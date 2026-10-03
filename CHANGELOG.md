# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added shared hardware-acceleration selection for the core, CLI, and desktop
  app. CLI users can set `RCOMP_ACCELERATOR` or pass `--accelerator`; desktop
  users can choose Automatic, CPU only, or Require GPU in Settings. Automatic
  mode reports CPU fallback, while required mode fails clearly when no
  compatible provider is available.
- Added an opt-in `rcomp-metal` provider and safe core provider registry. The
  first experimental capability produces interoperable LZ4 Fast and tar.lz4
  frames on Apple Silicon when the CLI or Tauri backend is built with
  `--features metal` and GPU use is explicitly required. Reusable buffers and
  device-aware bounded batches, a two-batch overlap pipeline, cached native
  state, and payload-only host copies substantially reduce dispatch overhead.
  It remains excluded from automatic selection until crossover measurements
  exist across representative Apple Silicon devices.
- Added the first opt-in `rcomp-wgpu` provider slice, using a single baseline
  WGSL LZ4 kernel through Direct3D 12 on Windows, Vulkan on Linux, and Metal on
  Apple Silicon. It discovers and ranks compatible adapters, derives bounded
  batches from reported limits, reuses GPU buffers and pipeline state, and
  integrates with the existing core LZ4 framing, cancellation, and atomic
  publication path. The capability remains experimental and requires explicit
  `--accelerator required` selection while Intel, NVIDIA, AMD, and Apple
  hardware qualification is completed.
- Added `rcomp hardware` provider/device inventory and exact selection through
  `--accelerator-device PROVIDER:DEVICE-ID` or `RCOMP_ACCELERATOR_DEVICE`.
  Provider diagnostics include backend, driver, buffer limits, batch policy,
  and exact capabilities where available.
- Added stable structured acceleration fallback reasons alongside the existing
  human-readable report notice, covering missing providers/devices,
  unsupported operations, experimental capabilities, explicit selectors, and
  provider discovery or initialization failures.
- Expanded the deterministic accelerator harness with malformed block-count,
  invalid input-length, worker-panic, queued cancellation, active
  cancellation, and atomic cleanup coverage.
- Qualified the portable WGSL path on an Apple M3 Max, including block
  boundaries, incompressible input, a 256 MiB batch boundary, complete LZ4
  frame round trips, and payload-only readback. The provider remains
  experimental because the portable Metal path is still slower than CPU and
  native Metal on the fixed 573 MiB corpus.

### Changed

- Accelerated compression now writes to a synchronized temporary sibling and
  atomically publishes the completed artifact. Cancellation or provider
  failure removes staging data while preserving any existing destination.
- Automatic file compression now retries once on CPU when a selected supported
  accelerator fails during execution. The retry occurs before atomic
  publication, is reported structurally and in display text, and is never
  applied to `required`, cancellation, or unrelated codec/I/O failures.
- Native Metal session creation and each worker dispatch now run inside an
  explicit Objective-C autorelease pool; the complete native Metal hardware
  suite passes on the M3 Max with that lifecycle policy.

### Fixed

- Multi-stream `.bz2`, `.xz`, and `.lz4` files (for example output from
  `pbzip2`/`lbzip2`, or files joined with `cat`) now decode in full instead of
  silently stopping after the first stream.
- Extracting with overwrite enabled now replaces existing symlinks and hard
  links in the destination instead of failing with "already exists".

### Security

- Extraction no longer writes through symlinks: an archive can't chain its own
  symlinks, or reuse ones already in the destination, to place files outside
  the extraction folder. Such archives are rejected with a path-traversal
  error, and an existing symlink at an output path is treated as existing (or
  replaced with overwrite enabled) rather than followed.
- Extracted files and directories no longer keep setuid, setgid, or sticky
  bits from the archive; only the regular read/write/execute permissions are
  restored.

## [0.3.3] - 2026-09-11

### Changed

- GitHub release assets for the desktop app are now named
  `rcomp-desktop_<version>_<platform>-<arch>.<ext>` (e.g.
  `rcomp-desktop_0.3.2_macos-universal.dmg`), and the release notes now spell
  out which files are the desktop app versus the `rcomp` CLI (installed via
  `cargo install rcomp`, not distributed as a release binary).

### Fixed

- The desktop app no longer crashes with a raw GTK panic when launched on
  Linux with no graphical session (e.g. over SSH on a headless server); it
  now prints a clear error and exits instead.

## [0.3.2] - 2026-09-08

### Fixed

- Prevented the ARM64 macOS app from crashing at launch by statically linking
  `liblzma`.
- Added CI validation for external dynamic library dependencies in macOS
  bundles.

## [0.3.1] - 2026-09-08

### Fixed

- Signed and notarized macOS bundles so Gatekeeper recognizes downloaded
  releases.

## [0.3.0] - 2026-09-07

### Added

- Added appearance settings with Auto, Light, and Dark modes.
- Added a default compression codec setting. Auto selects `.tar.zst` for
  folders or multi-item bundles and `.zst` for single files.
- Added an in-app changelog viewer, available from the Help menu and version
  badge, along with links to the project website and repository.
- Added File and Edit menu commands for common archive operations, search, and
  job cancellation.

### Changed

- Redesigned the extraction view with inline destination controls, archive
  details, checksum status, and file search.
- Redesigned the compose sidebar with container buttons, a separate codec
  selector, a compression-level slider, and clearer file controls.
- Added a branded macOS title bar while retaining the native window controls.
- Refreshed the app icon.

### Fixed

- Corrected extracted-size reporting for single-root archives written into
  existing directories.
- Prevented title bar dragging from scrolling the window content.
- Fixed files opened from Finder or another application being treated as new
  archive inputs instead of archives to extract.
- Corrected the process and macOS menu-bar name to “Rcomp”.
- Corrected macOS app icon sizing in the Dock and Finder.

### Removed

- Removed the title bar's New button because it could discard an active
  operation.

## [0.2.0] - 2026-06-15

### Added

- Added a cross-platform Tauri desktop app for Windows, macOS, and Linux.
- Added desktop bundles to tagged GitHub releases.

### Changed

- Unified the library, CLI, and desktop app version numbers.

## [0.1.0] - 2026-06-12

### Added

- Initial release.

[Unreleased]: https://github.com/open-southeners/rcomp/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/open-southeners/rcomp/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/open-southeners/rcomp/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/open-southeners/rcomp/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/open-southeners/rcomp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/open-southeners/rcomp/releases/tag/v0.1.0
