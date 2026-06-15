# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
