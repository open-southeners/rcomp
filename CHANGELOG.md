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
- `rcomp-desktop`: the macOS **Help** menu now links to the project's
  repository and the Open Southeners website, and adds a **What's New** item
  that opens an in-app viewer of this changelog — no need to visit GitHub to
  see what changed.

### Changed

- `rcomp-desktop`: extracting an opened archive no longer uses a side panel —
  a bar above the file list shows the archive's format/codec, whether a
  `.sha256` sidecar is available to verify against, and the space saved, plus
  a destination field (editable, or **Choose…** for the native picker) and an
  **Extract Now** button. Multi-root archives are still wrapped into a
  subfolder automatically, matching the CLI. The file list's status bar shows
  the archive's original and on-disk size and its file size (averaged, for
  more than one file), next to a search box for filtering entries by name
  and the archive's path.
- `rcomp-desktop`: reworked the compose sidebar to match the app's design —
  the format picker is now a container button-grid (None/TAR/ZIP/7-Zip) with
  a separate compression-algorithm dropdown, disabled with a hint when the
  container manages its own compression (ZIP, 7-Zip); the Fast/Best/Edge
  level picker is now a 3-stop slider. The file list also gains its own
  **Add Files** button and a drag & drop hint for building a bundle.
- `rcomp-desktop`: replaced the plain OS title bar (macOS) with a branded one
  showing the app icon, name, and version, with a divider separating it from
  the native traffic lights, while keeping the native close/minimize/zoom
  controls. The bar remains draggable, same as the native title bar it
  replaces.
- `rcomp-desktop`: refreshed the app icon artwork.

### Fixed

- Extracting an archive whose contents don't get wrapped in a subfolder (a
  single-root archive extracted straight into an existing, non-empty
  destination) no longer reports a wildly inflated extracted size — the
  reported `output_bytes` used to include every unrelated file already
  sitting in the destination folder. Affects both the `rcomp` CLI summary and
  `rcomp-desktop`'s extraction summary.
- `rcomp-desktop`: dragging the title bar no longer scrolls the whole window
  out of view — only the file list and compose sidebar scroll, as intended.

### Removed

- `rcomp-desktop`: removed the title bar's **New** button. It reset the
  workspace without cancelling or warning about an in-progress compress or
  extract job, so a stray click mid-operation could lose work; use **New
  operation** on the completion screen once a job finishes, or start a fresh
  operation from an empty workspace instead.

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

[Unreleased]: https://github.com/open-southeners/rcomp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/open-southeners/rcomp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/open-southeners/rcomp/releases/tag/v0.1.0
