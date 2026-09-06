# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `rcomp-desktop`: extracting an opened archive no longer uses a side panel —
  a bar above the file list shows the archive's format/codec, whether a
  `.sha256` sidecar is available to verify against, and the space saved, plus
  a destination field (editable, or **Choose…** for the native picker) and an
  **Extract Now** button. Multi-root archives are still wrapped into a
  subfolder automatically, matching the CLI. The file list also gains a
  footer with the total uncompressed size and average file size, and a search
  box to filter entries by name.

## [0.2.0] - 2026-06-15

### Added

- `rcomp-desktop`: a cross-platform Tauri desktop app, the second consumer of
  `rcomp-core` alongside the CLI.
- Release CI now builds and attaches the desktop app bundles for Windows, macOS
  (universal), and Linux to each tagged GitHub Release. Bundles are currently
  unsigned.

### Changed
- Initial release.

[Unreleased]: https://github.com/opensoutheners/rcomp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/opensoutheners/rcomp/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/opensoutheners/rcomp/releases/tag/v0.1.0
