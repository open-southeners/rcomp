//! RAR archive backend (extract + list only).
//!
//! RAR creation is proprietary; no encoder is provided.  This module
//! implements [`extract`] and [`list`] only.  It requires the `rar` cargo
//! feature to be enabled at compile time; the feature gates both the module
//! declaration in [`super`] and all call sites in `ops.rs`.
//!
//! # Security
//!
//! Every entry path (`FileHeader::filename`) is passed through
//! [`sanitize_entry_path`] **before** any byte is written to disk.
//! [`unrar`]'s own extraction paths are never used unchecked; we always pass
//! our sanitized destination path to [`OpenArchive::extract_to`].  Archives
//! with malicious paths are rejected with [`Error::PathTraversal`].
//!
//! Note: a malicious RAR fixture (with crafted `../` path components) cannot
//! easily be created without a RAR encoder.  The traversal call-path
//! (`sanitize_entry_path`) is shared with the tar and zip backends and is
//! covered by its own unit tests in [`super::sanitize`].
//!
//! # Unix permissions and symlinks
//!
//! `unrar`'s `FileHeader` does not expose unix mode bits or symlink metadata
//! in a cross-platform way.  This backend does not restore unix permissions
//! and does not create symlink entries.  See `CURRENT_ISSUES.md` for follow-up
//! tracking.
//!
//! # Progress
//!
//! `bytes_total` is set to `None` by the caller (see `ops.rs`) because unrar
//! drives its own I/O and there is no single compressed-byte stream to count.
//! `bytes_done` advances by the **unpacked size** of each entry after
//! extraction completes.  Invariant: `bytes_total.is_none()` for all progress
//! callbacks emitted during RAR extraction (consistent with zip and 7z).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use unrar::Archive;

use crate::{Error, Result, progress::Entry};

use super::{OpCtx, sanitize::sanitize_entry_path};

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract a RAR archive at `archive` into `dest`.
///
/// # Overwrite policy
///
/// If `overwrite` is `false` and an output file already exists,
/// [`Error::AlreadyExists`] is returned immediately.
///
/// # Entry handling
///
/// | Entry type | Action |
/// |---|---|
/// | Directory | [`fs::create_dir_all`] |
/// | Regular file | Extracted via `OpenArchive::extract_to` with our sanitized path. |
///
/// # Security
///
/// The raw `FileHeader::filename` path is passed through [`sanitize_entry_path`]
/// before any write occurs.  The sanitized destination is passed explicitly to
/// `extract_to` so that unrar never derives the output path unchecked.
///
/// # Progress
///
/// `bytes_done` advances by `FileHeader::unpacked_size` after each entry is
/// written.  `bytes_total` remains `None` (set by the caller in `ops.rs`).
///
/// Returns the number of entries extracted.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] for malicious paths,
/// [`Error::AlreadyExists`] if overwrite is disabled and a file exists,
/// [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`] for
/// I/O failures.
pub(crate) fn extract(
    archive: &Path,
    dest: &Path,
    overwrite: bool,
    ctx: &mut OpCtx<'_>,
) -> Result<u64> {
    ctx.check_cancel()?;

    let mut open = Archive::new(archive)
        .open_for_processing()
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut count: u64 = 0;

    loop {
        ctx.check_cancel()?;

        // Transition: CursorBeforeHeader → Option<CursorBeforeFile>
        let header = open
            .read_header()
            .map_err(|e| io::Error::other(e.to_string()))?;

        let header = match header {
            Some(h) => h,
            None => break, // end of archive
        };

        // Collect metadata before consuming the header via extract_to / skip.
        let raw_filename: PathBuf = header.entry().filename.clone();
        let unpacked_size: u64 = header.entry().unpacked_size;
        let is_directory: bool = header.entry().is_directory();

        // Sanitize the raw path first.
        let out_path = sanitize_entry_path(dest, &raw_filename)?;

        let entry_name = raw_filename.to_string_lossy().into_owned();
        ctx.set_entry(&entry_name);
        ctx.check_cancel()?;

        if is_directory {
            // Skip the payload (no bytes to extract for a directory entry).
            // create_dir_all is called after skip so the archive stays in sync.
            open = header.skip().map_err(|e| io::Error::other(e.to_string()))?;
            fs::create_dir_all(&out_path)?;
        } else {
            // Ensure parent directory exists before writing.
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Overwrite check.
            if !overwrite && out_path.exists() {
                // Skip the payload to keep the archive in a valid state, then
                // return the error.  (We need to skip before returning because
                // the unrar handle must be driven to completion or dropped.)
                let _ = header.skip();
                return Err(Error::AlreadyExists { path: out_path });
            }

            // Extract using our sanitized path — unrar will not infer the
            // destination; we supply it explicitly via extract_to.
            open = header
                .extract_to(&out_path)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }

        ctx.add_bytes(unpacked_size);
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List the entries of a RAR archive without extracting anything.
///
/// Returns a [`Vec<Entry>`] where each [`Entry`] contains:
/// - `path` — the raw path as stored in the archive (no sanitization applied).
/// - `size` — uncompressed byte size of the entry body.
/// - `is_dir` — `true` for directory entries.
///
/// # Errors
///
/// Returns [`Error::Io`] if the archive cannot be opened or read.
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>> {
    let open = Archive::new(archive)
        .open_for_listing()
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut entries = Vec::new();

    for header_result in open {
        let header = header_result.map_err(|e| io::Error::other(e.to_string()))?;
        entries.push(Entry {
            path: header.filename.clone(),
            size: header.unpacked_size,
            is_dir: header.is_directory(),
        });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use crate::{
        Error,
        archive::OpCtx,
        progress::{CancelToken, Progress},
    };

    use super::{extract, list};

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    macro_rules! make_ctx {
        ($token:expr, $cb:expr) => {
            OpCtx {
                cancel: $token.clone(),
                on_progress: $cb,
                progress: Progress {
                    bytes_done: 0,
                    bytes_total: None,
                    current_entry: None,
                },
            }
        };
    }

    /// Path to the test fixture.
    ///
    /// `tests/fixtures/sample.rar` is a copy of `version.rar` from the
    /// `unrar` crate's test data (origin:
    /// `~/.cargo/registry/src/.../unrar-0.5.8/data/version.rar`).
    ///
    /// Contents: one file named `VERSION` with content `"unrar-0.4.0"`.
    fn fixture() -> std::path::PathBuf {
        // Integration test binaries run from the crate root.
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.rar")
    }

    // -----------------------------------------------------------------------
    // list fixture
    // -----------------------------------------------------------------------

    #[test]
    fn list_fixture_returns_version_entry() {
        let entries = list(&fixture()).expect("list should succeed");
        assert_eq!(entries.len(), 1, "expected 1 entry; got {:?}", entries);
        assert_eq!(entries[0].path, Path::new("VERSION"));
        assert!(!entries[0].is_dir, "VERSION should not be a directory");
        // The uncompressed size matches the content "unrar-0.4.0" (11 bytes).
        assert_eq!(entries[0].size, 11, "VERSION size should be 11 bytes");
    }

    // -----------------------------------------------------------------------
    // extract fixture
    // -----------------------------------------------------------------------

    #[test]
    fn extract_fixture_produces_correct_file() {
        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let n = extract(&fixture(), dest.path(), false, &mut ctx).expect("extract should succeed");
        assert_eq!(n, 1, "expected 1 entry extracted");

        let content = std::fs::read(dest.path().join("VERSION")).unwrap();
        assert_eq!(content, b"unrar-0.4.0", "VERSION content mismatch");
    }

    // -----------------------------------------------------------------------
    // extract fixture matches list output
    // -----------------------------------------------------------------------

    #[test]
    fn extract_matches_list() {
        let listed = list(&fixture()).expect("list should succeed");

        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        extract(&fixture(), dest.path(), false, &mut ctx).expect("extract should succeed");

        // Every non-dir path from list() should exist in dest after extraction.
        for entry in &listed {
            if !entry.is_dir {
                let out = dest.path().join(&entry.path);
                assert!(out.exists(), "expected {:?} to exist after extract", out);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Overwrite=false on pre-existing file → AlreadyExists
    // -----------------------------------------------------------------------

    #[test]
    fn extract_overwrite_false_returns_already_exists() {
        let dest = TempDir::new().unwrap();

        // Pre-create the file that the archive would write.
        std::fs::write(dest.path().join("VERSION"), b"old content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(&fixture(), dest.path(), false, &mut ctx)
            .expect_err("should return AlreadyExists");

        assert!(
            matches!(err, Error::AlreadyExists { .. }),
            "expected AlreadyExists, got {err:?}"
        );

        // The pre-existing content should be untouched.
        assert_eq!(
            std::fs::read(dest.path().join("VERSION")).unwrap(),
            b"old content"
        );
    }

    // -----------------------------------------------------------------------
    // Overwrite=true replaces the pre-existing file
    // -----------------------------------------------------------------------

    #[test]
    fn extract_overwrite_true_replaces_file() {
        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("VERSION"), b"old content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        extract(&fixture(), dest.path(), true, &mut ctx).expect("extract should succeed");

        assert_eq!(
            std::fs::read(dest.path().join("VERSION")).unwrap(),
            b"unrar-0.4.0"
        );
    }

    // -----------------------------------------------------------------------
    // Progress: bytes_total remains None, bytes_done advances
    // -----------------------------------------------------------------------

    #[test]
    fn extract_progress_bytes_total_is_none() {
        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut snapshots: Vec<Progress> = Vec::new();
        let mut ctx = OpCtx {
            cancel: token.clone(),
            on_progress: &mut |p: &Progress| {
                snapshots.push(p.clone());
            },
            progress: Progress {
                bytes_done: 0,
                bytes_total: None,
                current_entry: None,
            },
        };

        extract(&fixture(), dest.path(), false, &mut ctx).expect("extract should succeed");

        assert!(
            !snapshots.is_empty(),
            "at least one progress snapshot must be emitted"
        );
        for snap in &snapshots {
            assert!(
                snap.bytes_total.is_none(),
                "rar extraction: bytes_total should be None but was {:?}",
                snap.bytes_total
            );
        }
    }

    // -----------------------------------------------------------------------
    // Traversal note: we cannot craft a malicious RAR without a RAR encoder.
    // The sanitize_entry_path call-path is shared with tar and zip backends
    // and is covered by unit tests in super::sanitize.
    // -----------------------------------------------------------------------
}
