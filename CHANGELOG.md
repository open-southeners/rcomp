# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `rcomp-desktop`: a **Settings** screen with an **Appearance** setting
  (Auto/Light/Dark). Auto follows the OS light/dark preference live, as
  before; Light/Dark now let you pin a theme regardless of the OS setting.
- `rcomp-desktop`: Settings also gains a **Default Compression Codec**
  picker. Auto (the default) picks by content — a folder or multi-item
  bundle gets `.tar.zst`, a single file gets `.zst` — or you can pin one
  format for every new archive.

### Changed

- `rcomp-desktop`: extracting an opened archive no longer uses a side panel —
  a bar above the file list shows the archive's format/codec, whether a
  `.sha256` sidecar is available to verify against, and the space saved, plus
  a destination field (editable, or **Choose…** for the native picker) and an
  **Extract Now** button. Multi-root archives are still wrapped into a
  subfolder automatically, matching the CLI. The file list also gains a
  footer with the total uncompressed size and average file size, and a search
  box to filter entries by name.
- `rcomp-desktop`: replaced the plain OS title bar (macOS) with a branded one
  showing the app icon, name, and version, while keeping the native
  close/minimize/zoom controls. The bar remains draggable, same as the
  native title bar it replaces.
- `rcomp-desktop`: refreshed the app icon artwork.

## [0.2.0] - 2026-06-15

### Added

- `rcomp-desktop`: a cross-platform Tauri desktop app, the second consumer of
  `rcomp-core` alongside the CLI.
- Release CI now builds and attaches the desktop app bundles for Windows, macOS
  (universal), and Linux to each tagged GitHub Release. Bundles are currently
  unsigned.

### Changed

- The desktop bundle version now inherits the workspace version instead of being
  hardcoded, so `rcomp-core`, `rcomp`, and `rcomp-desktop` all release under one
  version.

## [0.1.0] - 2026-06-12

### Added

- Initial release.

[Unreleased]: https://github.com/opensoutheners/rcomp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/opensoutheners/rcomp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/opensoutheners/rcomp/releases/tag/v0.1.0
