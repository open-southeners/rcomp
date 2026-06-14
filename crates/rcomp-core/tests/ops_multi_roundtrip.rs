//! Integration tests for the multi-input public API [`compress_many`].
//!
//! These exercise the bundle-into-one-archive semantics: each input becomes a
//! top-level root in the resulting archive, named by its final path component.
//!
//! Coverage:
//!
//! 1. Bundle (2 files + 1 dir) → `tar.zst` / `zip` / `7z` → extract → roots and
//!    contents are preserved verbatim.
//! 2. `list` reports the basename-prefixed roots for each container.
//! 3. Codec-only + multiple inputs → auto tar-layering (silent-tar) → extract
//!    via the tar sniff restores every root.
//! 4. Codec-only + a single file → plain codec (no tar wrapping).
//! 5. Duplicate basenames → [`Error::DuplicateInput`].
//! 6. Empty input list → an I/O error.
//! 7. Content checksum is computed for a tar+codec bundle.

use std::path::{Path, PathBuf};

use rcomp_core::{CompressOptions, Error, ExtractOptions, compress_many, extract, list};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a source tree and return the three inputs to bundle:
///
/// ```text
/// <root>/
///   one.txt          ("one")
///   two.txt          ("two")
///   pics/
///     a.txt          ("a")
///     sub/b.txt      ("b")
/// ```
fn build_inputs(root: &Path) -> Vec<PathBuf> {
    std::fs::write(root.join("one.txt"), b"one").unwrap();
    std::fs::write(root.join("two.txt"), b"two").unwrap();
    std::fs::create_dir(root.join("pics")).unwrap();
    std::fs::write(root.join("pics/a.txt"), b"a").unwrap();
    std::fs::create_dir(root.join("pics/sub")).unwrap();
    std::fs::write(root.join("pics/sub/b.txt"), b"b").unwrap();

    vec![
        root.join("one.txt"),
        root.join("two.txt"),
        root.join("pics"),
    ]
}

/// Assert that an extraction destination contains all bundled roots verbatim.
fn assert_bundle_restored(dest: &Path) {
    assert_eq!(std::fs::read(dest.join("one.txt")).unwrap(), b"one");
    assert_eq!(std::fs::read(dest.join("two.txt")).unwrap(), b"two");
    assert_eq!(std::fs::read(dest.join("pics/a.txt")).unwrap(), b"a");
    assert_eq!(std::fs::read(dest.join("pics/sub/b.txt")).unwrap(), b"b");
}

/// Compress-many into `output`, then extract into a fresh dir and assert the
/// bundle round-trips. `gitignore` is disabled so the test tree is archived
/// verbatim regardless of any ambient configuration.
fn roundtrip(output_name: &str) {
    let src = TempDir::new().unwrap();
    let inputs = build_inputs(src.path());
    let output = src.path().join(output_name);

    let opts = CompressOptions {
        follow_gitignore: false,
        ..Default::default()
    };
    let report = compress_many(&inputs, &output, &opts, |_| {}).expect("compress_many");
    assert!(
        report.entries >= 4,
        "expected >= 4 entries, got {}",
        report.entries
    );
    assert!(output.is_file(), "output archive should exist");

    let dest = TempDir::new().unwrap();
    extract(&output, dest.path(), &ExtractOptions::default(), |_| {}).expect("extract");
    assert_bundle_restored(dest.path());
}

// ---------------------------------------------------------------------------
// 1. Container round-trips
// ---------------------------------------------------------------------------

#[test]
fn bundle_tar_zst_roundtrip() {
    roundtrip("bundle.tar.zst");
}

#[test]
fn bundle_zip_roundtrip() {
    roundtrip("bundle.zip");
}

#[test]
fn bundle_7z_roundtrip() {
    roundtrip("bundle.7z");
}

#[test]
fn bundle_plain_tar_roundtrip() {
    roundtrip("bundle.tar");
}

// ---------------------------------------------------------------------------
// 2. list reports the basename-prefixed roots
// ---------------------------------------------------------------------------

#[test]
fn bundle_list_reports_roots() {
    let src = TempDir::new().unwrap();
    let inputs = build_inputs(src.path());
    let output = src.path().join("bundle.zip");

    let opts = CompressOptions {
        follow_gitignore: false,
        ..Default::default()
    };
    compress_many(&inputs, &output, &opts, |_| {}).expect("compress_many");

    let entries = list(&output).expect("list");
    let names: Vec<String> = entries
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();

    assert!(
        names.iter().any(|p| p == "one.txt"),
        "one.txt root missing; got {names:?}"
    );
    assert!(
        names.iter().any(|p| p == "two.txt"),
        "two.txt root missing; got {names:?}"
    );
    assert!(
        names.iter().any(|p| p == "pics/a.txt"),
        "pics/a.txt missing; got {names:?}"
    );
    assert!(
        names.iter().any(|p| p == "pics/sub/b.txt"),
        "pics/sub/b.txt missing; got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Codec-only + multiple inputs → auto tar-layering
// ---------------------------------------------------------------------------

#[test]
fn bundle_codec_only_multiple_inputs_auto_tars() {
    let src = TempDir::new().unwrap();
    std::fs::write(src.path().join("one.txt"), b"one").unwrap();
    std::fs::write(src.path().join("two.txt"), b"two").unwrap();
    let inputs = vec![src.path().join("one.txt"), src.path().join("two.txt")];

    // Codec-only output name (.gz) with two inputs must be tar-wrapped.
    let output = src.path().join("bundle.gz");
    let opts = CompressOptions {
        follow_gitignore: false,
        ..Default::default()
    };
    compress_many(&inputs, &output, &opts, |_| {}).expect("compress_many");

    // The tar sniff on extract should restore both files as separate roots.
    let dest = TempDir::new().unwrap();
    extract(&output, dest.path(), &ExtractOptions::default(), |_| {}).expect("extract");
    assert_eq!(std::fs::read(dest.path().join("one.txt")).unwrap(), b"one");
    assert_eq!(std::fs::read(dest.path().join("two.txt")).unwrap(), b"two");
}

// ---------------------------------------------------------------------------
// 4. Codec-only + a single file → plain codec (no tar wrapping)
// ---------------------------------------------------------------------------

#[test]
fn bundle_codec_only_single_file_no_tar() {
    let src = TempDir::new().unwrap();
    std::fs::write(src.path().join("solo.txt"), b"solo payload").unwrap();
    let inputs = vec![src.path().join("solo.txt")];

    let output = src.path().join("solo.gz");
    let opts = CompressOptions {
        follow_gitignore: false,
        ..Default::default()
    };
    let report = compress_many(&inputs, &output, &opts, |_| {}).expect("compress_many");
    // A single codec stream is exactly one logical entry.
    assert_eq!(report.entries, 1, "codec single-file should be one entry");

    // Extract: the codec-only sniff finds no tar, so a single file is written.
    let dest = TempDir::new().unwrap();
    extract(&output, dest.path(), &ExtractOptions::default(), |_| {}).expect("extract");

    let files: Vec<_> = std::fs::read_dir(dest.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "exactly one file should be extracted");
    assert_eq!(std::fs::read(&files[0]).unwrap(), b"solo payload");
}

// ---------------------------------------------------------------------------
// 5. Duplicate basenames → DuplicateInput
// ---------------------------------------------------------------------------

#[test]
fn bundle_duplicate_basename_errors() {
    let src = TempDir::new().unwrap();
    std::fs::create_dir(src.path().join("p1")).unwrap();
    std::fs::create_dir(src.path().join("p2")).unwrap();
    std::fs::write(src.path().join("p1/data.txt"), b"x").unwrap();
    std::fs::write(src.path().join("p2/data.txt"), b"y").unwrap();

    let inputs = vec![
        src.path().join("p1/data.txt"),
        src.path().join("p2/data.txt"),
    ];
    let output = src.path().join("dup.zip");

    let err = compress_many(&inputs, &output, &CompressOptions::default(), |_| {})
        .expect_err("duplicate basenames must error");
    assert!(
        matches!(err, Error::DuplicateInput { ref name } if name == "data.txt"),
        "expected DuplicateInput(data.txt), got {err:?}"
    );
    assert!(
        !output.exists(),
        "no partial archive should be left behind on a usage error"
    );
}

// ---------------------------------------------------------------------------
// 6. Empty input list → error
// ---------------------------------------------------------------------------

#[test]
fn bundle_empty_inputs_errors() {
    let src = TempDir::new().unwrap();
    let output = src.path().join("empty.zip");
    let err = compress_many(&[], &output, &CompressOptions::default(), |_| {})
        .expect_err("empty input list must error");
    assert!(
        matches!(err, Error::Io(_)),
        "expected Io error, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Content checksum for a tar+codec bundle
// ---------------------------------------------------------------------------

#[test]
fn bundle_tar_codec_content_checksum_present() {
    let src = TempDir::new().unwrap();
    let inputs = build_inputs(src.path());
    let output = src.path().join("bundle.tar.gz");

    let opts = CompressOptions {
        follow_gitignore: false,
        checksum: true,
        ..Default::default()
    };
    let report = compress_many(&inputs, &output, &opts, |_| {}).expect("compress_many");
    let artifact = report.sha256.expect("artifact digest");
    let content = report.content_sha256.expect("content digest for tar+codec");
    assert_eq!(artifact.len(), 64, "artifact digest is 64 hex chars");
    assert_eq!(content.len(), 64, "content digest is 64 hex chars");

    // The content digest verifies on extraction (proves it matches the tar stream).
    let dest = TempDir::new().unwrap();
    let xopts = ExtractOptions {
        verify_content_sha256: Some(content),
        ..Default::default()
    };
    extract(&output, dest.path(), &xopts, |_| {}).expect("extract with content verify");
    assert_bundle_restored(dest.path());
}
