# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
