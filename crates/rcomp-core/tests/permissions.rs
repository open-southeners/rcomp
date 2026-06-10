//! Integration tests for directory-permission restoration on extraction.
//!
//! These tests verify that the two-phase chmod approach correctly restores
//! unix directory modes after extracting tar, zip, and tar.gz archives.
//! Directories with restrictive permissions (e.g. 0o550) must not block their
//! own children from being written during extraction; permissions are applied
//! only after all entries are fully written.
//!
//! The entire file is gated on `#[cfg(unix)]` since unix mode bits are a
//! unix-only concept.

#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
};

use rcomp_core::{CompressOptions, ExtractOptions, compress, extract};
use tempfile::TempDir;

/// No-op progress callback.
fn nop(_: &rcomp_core::Progress) {}

// ---------------------------------------------------------------------------
// Source-tree construction helpers
// ---------------------------------------------------------------------------

/// Build a source tree that exercises both a restricted (0o750) and a
/// read-only-with-child (0o550) directory:
///
/// ```text
/// <root>/
///   normal_dir/          mode 0o750
///     file_in_normal.txt
///   readonly_dir/        mode 0o550 (set AFTER its child is written)
///     file_in_readonly.txt
///   top_file.txt
/// ```
///
/// Returns `(src_dir, readonly_dir_path)`.  The caller must restore write
/// permissions on `readonly_dir_path` before `src_dir` is dropped, so that
/// tempfile's cleanup can remove it.
fn build_perm_tree(root: &Path) {
    // Top-level file.
    fs::write(root.join("top_file.txt"), b"top level").unwrap();

    // normal_dir with mode 0o750.
    let normal = root.join("normal_dir");
    fs::create_dir(&normal).unwrap();
    fs::write(normal.join("file_in_normal.txt"), b"in normal").unwrap();
    fs::set_permissions(&normal, fs::Permissions::from_mode(0o750)).unwrap();

    // readonly_dir: create the child file FIRST, then restrict the dir.
    let ro = root.join("readonly_dir");
    fs::create_dir(&ro).unwrap();
    fs::write(ro.join("file_in_readonly.txt"), b"in readonly").unwrap();
    // Now restrict — the child is already present so creating it is fine.
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o550)).unwrap();
}

/// Restore write permissions on the read-only directory so that tempfile can
/// clean up the source tree.
fn restore_write_perms(readonly_dir: &Path) {
    // Best-effort: if the dir was already removed or never existed, ignore.
    let _ = fs::set_permissions(readonly_dir, fs::Permissions::from_mode(0o755));
}

/// Assert that both directory modes are exactly what we stored.
fn assert_dir_modes(dest: &Path) {
    let normal_mode = fs::metadata(dest.join("normal_dir"))
        .expect("normal_dir must exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        normal_mode, 0o750,
        "normal_dir: expected 0o750, got 0o{normal_mode:o}"
    );

    let ro_mode = fs::metadata(dest.join("readonly_dir"))
        .expect("readonly_dir must exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        ro_mode, 0o550,
        "readonly_dir: expected 0o550, got 0o{ro_mode:o}"
    );
}

/// Assert that files inside the restricted directories were extracted.
fn assert_children(dest: &Path) {
    let content = fs::read(dest.join("normal_dir/file_in_normal.txt"))
        .expect("file_in_normal.txt must exist");
    assert_eq!(content, b"in normal");

    let content = fs::read(dest.join("readonly_dir/file_in_readonly.txt"))
        .expect("file_in_readonly.txt must exist after extraction through readonly_dir");
    assert_eq!(content, b"in readonly");

    let top = fs::read(dest.join("top_file.txt")).expect("top_file.txt must exist");
    assert_eq!(top, b"top level");
}

// ---------------------------------------------------------------------------
// tar roundtrip
// ---------------------------------------------------------------------------

#[test]
fn tar_dir_permissions_restored() {
    let src = TempDir::new().unwrap();
    build_perm_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.tar");
    let dest = TempDir::new().unwrap();

    // Compress.
    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("tar compress should succeed");

    // Restore write permissions before extracting (extraction is to a fresh dest,
    // so src write permissions don't matter for extraction, but we need to be
    // able to clean up the src tempdir afterward).
    restore_write_perms(&src.path().join("readonly_dir"));

    // Extract.
    extract(&archive, dest.path(), &ExtractOptions::default(), nop)
        .expect("tar extract should succeed — read-only dir must not block children");

    // Assert modes were correctly restored.
    assert_dir_modes(dest.path());

    // Assert children were written despite the read-only dir.
    assert_children(dest.path());

    // Restore dest write perms for cleanup.
    restore_write_perms(&dest.path().join("readonly_dir"));
}

// ---------------------------------------------------------------------------
// zip roundtrip
// ---------------------------------------------------------------------------

#[test]
fn zip_dir_permissions_restored() {
    let src = TempDir::new().unwrap();
    build_perm_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.zip");
    let dest = TempDir::new().unwrap();

    // Compress.
    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("zip compress should succeed");

    restore_write_perms(&src.path().join("readonly_dir"));

    // Extract.
    extract(&archive, dest.path(), &ExtractOptions::default(), nop)
        .expect("zip extract should succeed — read-only dir must not block children");

    assert_dir_modes(dest.path());
    assert_children(dest.path());

    restore_write_perms(&dest.path().join("readonly_dir"));
}

// ---------------------------------------------------------------------------
// tar.gz roundtrip — proves the codec-wrapped tar path inherits the fix
// ---------------------------------------------------------------------------

#[test]
fn tar_gz_dir_permissions_restored() {
    let src = TempDir::new().unwrap();
    build_perm_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.tar.gz");
    let dest = TempDir::new().unwrap();

    // Compress.
    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("tar.gz compress should succeed");

    restore_write_perms(&src.path().join("readonly_dir"));

    // Extract.
    extract(&archive, dest.path(), &ExtractOptions::default(), nop)
        .expect("tar.gz extract should succeed — read-only dir must not block children");

    assert_dir_modes(dest.path());
    assert_children(dest.path());

    restore_write_perms(&dest.path().join("readonly_dir"));
}
