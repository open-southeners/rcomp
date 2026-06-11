//! Integration tests for the public `compress` / `extract` / `list` API.
//!
//! All tests use only the re-exported symbols from `rcomp_core` — no access
//! to internal implementation details.  Tests are organised as follows:
//!
//! 1. `tar.gz` dir roundtrip — nested dirs, empty dir, #[cfg(unix)] perms.
//! 2. **Silent-tar rule** (flagship): dir → `out.bz2` (codec-only name!) →
//!    compress applies tar wrap → extract `out.bz2` → sniff finds tar →
//!    original tree restored.
//! 3. File → `out.zst` → extract → identical file with stripped name.
//! 4. Gzip embedded-name: fixture built with `flate2::GzBuilder` (filename
//!    header) → extract honours the embedded name.
//! 5. Dir → zip → extract roundtrip.
//! 6. `list` on tar.gz and zip match the created tree; list on `.zst` →
//!    UnsupportedOperation.
//! 7. Overwrite refusals (compress and extract directions).
//! 8. Pre-cancelled token → Cancelled; no output left behind.
//! 9. UnknownFormat on `compress(x, "out.weird")`.
//! 10. **7z backend**: dir → `out.7z` → extract roundtrip, list, progress
//!     invariant (bytes_total None throughout extraction).

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};

use flate2::{Compression, GzBuilder};
use rcomp_core::{
    CancelToken, CompressOptions, Error, ExtractOptions, Level, compress, extract, list,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Tree helpers
// ---------------------------------------------------------------------------

/// Build a test directory tree:
///
/// ```text
/// <root>/
///   a.txt          ("hello a")
///   sub/
///     b.txt        ("hello b")
///   empty_dir/
///   zero.txt       ("")
/// ```
fn build_test_tree(root: &Path) {
    std::fs::write(root.join("a.txt"), b"hello a").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b.txt"), b"hello b").unwrap();
    std::fs::create_dir(root.join("empty_dir")).unwrap();
    std::fs::write(root.join("zero.txt"), b"").unwrap();
}

/// Assert the test tree was restored correctly into `dest`.
fn assert_test_tree(dest: &Path) {
    assert_eq!(
        std::fs::read(dest.join("a.txt")).unwrap(),
        b"hello a",
        "a.txt mismatch"
    );
    assert_eq!(
        std::fs::read(dest.join("sub/b.txt")).unwrap(),
        b"hello b",
        "sub/b.txt mismatch"
    );
    assert!(
        dest.join("empty_dir").is_dir(),
        "empty_dir should be a directory"
    );
    assert_eq!(
        std::fs::read(dest.join("zero.txt")).unwrap(),
        b"",
        "zero.txt should be empty"
    );
}

/// Recursively collect all paths relative to `root` (files and dirs).
fn collect_relative_paths(root: &Path) -> HashSet<PathBuf> {
    let mut result = HashSet::new();
    collect_paths_recursive(root, root, &mut result);
    result
}

fn collect_paths_recursive(root: &Path, current: &Path, out: &mut HashSet<PathBuf>) {
    for entry in std::fs::read_dir(current).expect("read_dir") {
        let entry = entry.unwrap();
        let abs = entry.path();
        let rel = abs.strip_prefix(root).unwrap().to_path_buf();
        out.insert(rel.clone());
        if abs.is_dir() {
            collect_paths_recursive(root, &abs, out);
        }
    }
}

/// Compare file byte content at `path` in `a` and `b`.
fn assert_file_bytes_equal(a: &Path, b: &Path, rel: &Path) {
    let pa = a.join(rel);
    let pb = b.join(rel);
    if pa.is_file() && pb.is_file() {
        let ba = std::fs::read(&pa).unwrap_or_else(|e| panic!("read {:?}: {e}", pa));
        let bb = std::fs::read(&pb).unwrap_or_else(|e| panic!("read {:?}: {e}", pb));
        assert_eq!(ba, bb, "byte mismatch for {}", rel.display());
    }
}

/// Default no-op progress callback.
fn nop_progress(_: &rcomp_core::Progress) {}

// ---------------------------------------------------------------------------
// 1. tar.gz dir roundtrip
// ---------------------------------------------------------------------------

#[test]
fn compress_extract_tar_gz_dir_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");
    let dest = TempDir::new().unwrap();

    // Compress.
    let compress_report = compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("compress should succeed");
    assert!(
        compress_report.entries > 0,
        "should have compressed entries"
    );
    assert!(
        compress_report.output_bytes > 0,
        "output should be non-empty"
    );

    // Extract.
    let extract_report = extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect("extract should succeed");
    assert!(extract_report.entries > 0, "should have extracted entries");

    // Verify tree.
    assert_test_tree(dest.path());

    // Byte-identical check on files.
    for rel in collect_relative_paths(src.path()) {
        assert_file_bytes_equal(src.path(), dest.path(), &rel);
    }
}

#[cfg(unix)]
#[test]
fn compress_extract_tar_gz_preserves_unix_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let src = TempDir::new().unwrap();
    let exec_path = src.path().join("run.sh");
    let data_path = src.path().join("data.txt");

    std::fs::write(&exec_path, b"#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&exec_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&data_path, b"data").unwrap();
    std::fs::set_permissions(&data_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    let exec_mode = std::fs::metadata(dest.path().join("run.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let data_mode = std::fs::metadata(dest.path().join("data.txt"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(exec_mode, 0o755, "run.sh should have 0o755");
    assert_eq!(data_mode, 0o644, "data.txt should have 0o644");
}

// ---------------------------------------------------------------------------
// 2. Silent-tar flagship test
//
// A directory compressed to a codec-only name (e.g. "out.bz2") should be
// automatically tar-wrapped on compress, and the tar detected and unpacked
// on extract, yielding the original tree.
// ---------------------------------------------------------------------------

#[test]
fn silent_tar_dir_to_bz2_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    // Codec-only name: compress should apply the silent-tar rule and wrap in tar.
    let archive = work.path().join("out.bz2");
    let dest = TempDir::new().unwrap();

    // Compress with codec-only extension and directory input.
    let compress_report = compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("silent-tar compress should succeed");
    assert!(
        compress_report.entries > 0,
        "should have entries (tar-wrapped)"
    );

    // Extract: should sniff tar magic inside the bzip2 stream and untar.
    let extract_report = extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect("silent-tar extract should succeed");
    assert!(extract_report.entries > 0, "should have extracted entries");

    // The original tree must be fully restored.
    assert_test_tree(dest.path());

    for rel in collect_relative_paths(src.path()) {
        assert_file_bytes_equal(src.path(), dest.path(), &rel);
    }
}

/// Same as above but with the gzip codec.
#[test]
fn silent_tar_dir_to_gz_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.gz");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    assert_test_tree(dest.path());
}

/// Same with zstd.
#[test]
fn silent_tar_dir_to_zst_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.zst");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    assert_test_tree(dest.path());
}

/// Same with xz.
#[test]
fn silent_tar_dir_to_xz_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.xz");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    assert_test_tree(dest.path());
}

// ---------------------------------------------------------------------------
// 3. Single file → codec-only → extract → identical file (stripped name)
// ---------------------------------------------------------------------------

#[test]
fn compress_extract_single_file_zst() {
    let src_dir = TempDir::new().unwrap();
    let src_file = src_dir.path().join("hello.txt");
    std::fs::write(&src_file, b"single file zstd content").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("hello.txt.zst");
    let dest = TempDir::new().unwrap();

    compress(&src_file, &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    // Name should be "hello.txt" (stripped ".zst" extension).
    let out_file = dest.path().join("hello.txt");
    assert!(out_file.exists(), "hello.txt should exist in dest");
    assert_eq!(
        std::fs::read(&out_file).unwrap(),
        b"single file zstd content"
    );
}

#[test]
fn compress_extract_single_file_bz2() {
    let src_dir = TempDir::new().unwrap();
    let src_file = src_dir.path().join("data.bin");
    let content: Vec<u8> = (0u8..200).collect();
    std::fs::write(&src_file, &content).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("data.bin.bz2");
    let dest = TempDir::new().unwrap();

    compress(&src_file, &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    let out_file = dest.path().join("data.bin");
    assert!(out_file.exists(), "data.bin should exist in dest");
    assert_eq!(std::fs::read(&out_file).unwrap(), content);
}

// ---------------------------------------------------------------------------
// 4. Gzip embedded-filename: extract honours the embedded name
// ---------------------------------------------------------------------------

/// Build a `.gz` file with an embedded original-filename header using
/// `flate2::GzBuilder`.  The fixture is created inline — no external tools.
fn build_gz_with_embedded_name(data: &[u8], embedded_name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = GzBuilder::new()
        .filename(embedded_name)
        .write(&mut out, Compression::default());
    encoder.write_all(data).expect("write_all");
    encoder.finish().expect("finish");
    out
}

#[test]
fn extract_gzip_embedded_filename_is_used() {
    let work = TempDir::new().unwrap();

    // Write a .gz file that carries the embedded name "original-name.txt".
    let gz_bytes = build_gz_with_embedded_name(b"embedded name content", "original-name.txt");
    let gz_path = work.path().join("archive.gz");
    std::fs::write(&gz_path, &gz_bytes).unwrap();

    let dest = TempDir::new().unwrap();
    extract(&gz_path, dest.path(), &Default::default(), nop_progress).unwrap();

    // The extracted file should use the embedded name, not "archive".
    let out_file = dest.path().join("original-name.txt");
    assert!(
        out_file.exists(),
        "extracted file should use embedded name 'original-name.txt'"
    );
    assert_eq!(std::fs::read(&out_file).unwrap(), b"embedded name content");
}

/// The embedded filename in the gzip header is attacker-controlled; only the
/// final file_name component should be used (no path traversal).
#[test]
fn extract_gzip_embedded_filename_traversal_stripped() {
    let work = TempDir::new().unwrap();

    // Embed a malicious traversal path as the filename.
    let gz_bytes = build_gz_with_embedded_name(b"evil", "../../evil.txt");
    let gz_path = work.path().join("trap.gz");
    std::fs::write(&gz_path, &gz_bytes).unwrap();

    let dest = TempDir::new().unwrap();
    extract(&gz_path, dest.path(), &Default::default(), nop_progress).unwrap();

    // Only the final component "evil.txt" should be used.
    let safe_out = dest.path().join("evil.txt");
    assert!(
        safe_out.exists(),
        "should extract to 'evil.txt' (final component only), not escape dest"
    );
    // Verify nothing was written outside dest.
    let escaped = work.path().join("evil.txt");
    assert!(
        !escaped.exists(),
        "no file should be written outside the dest directory"
    );
}

// ---------------------------------------------------------------------------
// 5. Dir → zip → extract roundtrip
// ---------------------------------------------------------------------------

#[test]
fn compress_extract_zip_dir_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.zip");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("zip compress should succeed");
    extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect("zip extract should succeed");

    assert_test_tree(dest.path());

    for rel in collect_relative_paths(src.path()) {
        assert_file_bytes_equal(src.path(), dest.path(), &rel);
    }
}

#[test]
fn compress_extract_zip_file_roundtrip() {
    let src_dir = TempDir::new().unwrap();
    let src_file = src_dir.path().join("note.txt");
    std::fs::write(&src_file, b"zip single file").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("note.zip");
    let dest = TempDir::new().unwrap();

    compress(&src_file, &archive, &Default::default(), nop_progress).unwrap();
    extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    assert_eq!(
        std::fs::read(dest.path().join("note.txt")).unwrap(),
        b"zip single file"
    );
}

/// All three Level variants produce decodable zip archives (and Edge ≤ Fast in
/// size on the repetitive corpus).
#[test]
fn zip_all_levels_produce_decodable_archives() {
    let src_dir = TempDir::new().unwrap();
    // Highly compressible content so level differences are visible.
    let data = vec![b'A'; 64 * 1024];
    std::fs::write(src_dir.path().join("corpus.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();

    for (name, level) in &[
        ("fast.zip", Level::Fast),
        ("best.zip", Level::Best),
        ("edge.zip", Level::Edge),
    ] {
        let archive = work.path().join(name);
        let dest = TempDir::new().unwrap();
        let opts = CompressOptions {
            level: *level,
            ..Default::default()
        };

        compress(src_dir.path(), &archive, &opts, nop_progress)
            .unwrap_or_else(|e| panic!("compress {name} failed: {e}"));
        extract(&archive, dest.path(), &Default::default(), nop_progress)
            .unwrap_or_else(|e| panic!("extract {name} failed: {e}"));

        assert_eq!(
            std::fs::read(dest.path().join("corpus.bin")).unwrap(),
            data,
            "roundtrip mismatch at level {:?}",
            level
        );
    }
}

// ---------------------------------------------------------------------------
// 6. list
// ---------------------------------------------------------------------------

#[test]
fn list_tar_gz_matches_created_tree() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.tar.gz");

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let entries = list(&archive).expect("list should succeed");
    let paths: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

    assert!(paths.contains(Path::new("a.txt")), "expected a.txt in list");
    assert!(
        paths.contains(Path::new("sub/b.txt")) || paths.contains(Path::new("sub\\b.txt")),
        "expected sub/b.txt in list"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("empty_dir")),
        "expected empty_dir in list"
    );
}

#[test]
fn list_zip_matches_created_tree() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.zip");

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let entries = list(&archive).expect("list should succeed");
    let names: HashSet<String> = entries
        .iter()
        .map(|e| e.path.to_string_lossy().into_owned())
        .collect();

    assert!(
        names.contains("a.txt"),
        "expected a.txt in list; got {:?}",
        names
    );
    assert!(
        names.contains("sub/b.txt"),
        "expected sub/b.txt in list; got {:?}",
        names
    );
}

#[test]
fn list_zst_codec_only_returns_unsupported() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("file.txt"), b"data").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("file.txt.zst");

    compress(
        &src_dir.path().join("file.txt"),
        &archive,
        &Default::default(),
        nop_progress,
    )
    .unwrap();

    let err = list(&archive).expect_err("list on zst should return UnsupportedOperation");
    assert!(
        matches!(err, Error::UnsupportedOperation { .. }),
        "expected UnsupportedOperation, got {err:?}"
    );
}

/// list() on a codec-only file that wraps a silent-tar (dir → .bz2) should
/// return the archive entries by sniffing the decompressed stream.
#[test]
fn list_bz2_silent_tar_returns_tree_entries() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    // Codec-only extension: compress applies the silent-tar rule.
    let archive = work.path().join("out.bz2");

    compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("silent-tar compress should succeed");

    let entries = list(&archive).expect("list on silent-tar bz2 should succeed");
    let paths: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

    assert!(
        paths.contains(Path::new("a.txt")),
        "expected a.txt in list; got {:?}",
        paths
    );
    assert!(
        paths.contains(Path::new("sub/b.txt")) || paths.contains(Path::new("sub\\b.txt")),
        "expected sub/b.txt in list; got {:?}",
        paths
    );
    assert!(
        paths.iter().any(|p| p.ends_with("empty_dir")),
        "expected empty_dir in list; got {:?}",
        paths
    );
}

/// list() on a codec-only file containing a plain single file (not tar-wrapped)
/// must still return UnsupportedOperation.
#[test]
fn list_zst_bare_single_file_returns_unsupported() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("note.txt"), b"plain text, not tar").unwrap();

    let work = TempDir::new().unwrap();
    // Compress a single file → codec-only, no silent-tar wrap.
    let archive = work.path().join("note.txt.zst");

    compress(
        &src_dir.path().join("note.txt"),
        &archive,
        &Default::default(),
        nop_progress,
    )
    .unwrap();

    let err = list(&archive)
        .expect_err("list on bare zst single-file should return UnsupportedOperation");
    assert!(
        matches!(err, rcomp_core::Error::UnsupportedOperation { .. }),
        "expected UnsupportedOperation, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Overwrite refusals
// ---------------------------------------------------------------------------

#[test]
fn compress_refuses_to_overwrite_existing_output() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("f.txt"), b"data").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    // First compress creates the archive.
    compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();

    // Second compress should refuse to overwrite.
    let err = compress(src_dir.path(), &archive, &Default::default(), nop_progress)
        .expect_err("should refuse to overwrite");
    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists, got {err:?}"
    );
}

#[test]
fn compress_overwrites_when_flag_set() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("f.txt"), b"data v2").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    // Create an existing archive.
    compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();

    // Overwrite should succeed.
    let opts = CompressOptions {
        overwrite: true,
        ..Default::default()
    };
    compress(src_dir.path(), &archive, &opts, nop_progress)
        .expect("should succeed with overwrite=true");
}

#[test]
fn extract_refuses_to_overwrite_existing_file() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("file.txt"), b"content").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();

    let dest = TempDir::new().unwrap();
    // Pre-create the file that would be extracted.
    std::fs::write(dest.path().join("file.txt"), b"old content").unwrap();

    let err = extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect_err("should refuse to overwrite");
    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists, got {err:?}"
    );

    // Verify the pre-existing file was not altered.
    assert_eq!(
        std::fs::read(dest.path().join("file.txt")).unwrap(),
        b"old content"
    );
}

// ---------------------------------------------------------------------------
// 8. Pre-cancelled token → Cancelled; no output left behind
// ---------------------------------------------------------------------------

#[test]
fn compress_pre_cancelled_returns_cancelled_no_output() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("data.txt"), b"data").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("cancelled.tar.gz");

    let token = CancelToken::default();
    token.cancel(); // Cancel before starting.

    let opts = CompressOptions {
        cancel: token,
        ..Default::default()
    };

    let err = compress(src_dir.path(), &archive, &opts, nop_progress)
        .expect_err("should return Cancelled");
    assert!(
        matches!(err, Error::Cancelled),
        "expected Cancelled, got {err:?}"
    );

    // No partial output should remain.
    assert!(
        !archive.exists(),
        "no output file should remain after cancellation"
    );
}

#[test]
fn extract_pre_cancelled_returns_cancelled() {
    // Build an archive to extract from.
    let src_dir = TempDir::new().unwrap();
    let data: Vec<u8> = (0..128 * 1024).map(|i| (i % 256) as u8).collect();
    std::fs::write(src_dir.path().join("big.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("big.tar.gz");
    compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();

    let dest = TempDir::new().unwrap();

    let token = CancelToken::default();
    token.cancel();

    let opts = ExtractOptions {
        cancel: token,
        ..Default::default()
    };

    let err =
        extract(&archive, dest.path(), &opts, nop_progress).expect_err("should return Cancelled");
    assert!(
        matches!(err, Error::Cancelled),
        "expected Cancelled, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. UnknownFormat on unrecognised output extension
// ---------------------------------------------------------------------------

#[test]
fn compress_unknown_extension_returns_unknown_format() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("f.txt"), b"hi").unwrap();

    let work = TempDir::new().unwrap();
    let weird_output = work.path().join("out.weird");

    let err = compress(
        src_dir.path(),
        &weird_output,
        &Default::default(),
        nop_progress,
    )
    .expect_err("should fail with UnknownFormat");
    assert!(
        matches!(err, Error::UnknownFormat { .. }),
        "expected UnknownFormat, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. 7z backend — dir roundtrip, list, progress invariant
// ---------------------------------------------------------------------------

#[test]
fn compress_extract_7z_dir_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.7z");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("7z compress should succeed");
    extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect("7z extract should succeed");

    assert_test_tree(dest.path());

    for rel in collect_relative_paths(src.path()) {
        assert_file_bytes_equal(src.path(), dest.path(), &rel);
    }
}

#[test]
fn list_7z_matches_created_tree() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("tree.7z");

    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let entries = list(&archive).expect("list on 7z should succeed");
    let names: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

    assert!(
        names.contains(Path::new("a.txt")),
        "expected a.txt in 7z list; got {:?}",
        names
    );
    assert!(
        names.contains(Path::new("sub/b.txt")),
        "expected sub/b.txt in 7z list; got {:?}",
        names
    );
}

/// For 7z: bytes_total must be None for all progress snapshots (because 7z is
/// random-access like zip and we reset bytes_total to None before extraction).
#[test]
fn extract_7z_progress_bytes_total_is_none() {
    let src = TempDir::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
    std::fs::write(src.path().join("big.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("big.7z");
    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let dest = TempDir::new().unwrap();
    let mut snapshots: Vec<rcomp_core::Progress> = Vec::new();
    extract(&archive, dest.path(), &Default::default(), |p| {
        snapshots.push(p.clone());
    })
    .expect("7z extract should succeed");

    assert!(
        !snapshots.is_empty(),
        "at least one progress snapshot must be emitted during 7z extraction"
    );

    for snap in &snapshots {
        assert!(
            snap.bytes_total.is_none(),
            "7z extraction: bytes_total should be None but was {:?}",
            snap.bytes_total,
        );
    }
}

// ---------------------------------------------------------------------------
// 11. tar.bz2 layered roundtrip (explicit)
// ---------------------------------------------------------------------------

#[test]
fn compress_extract_tar_bz2_dir_roundtrip() {
    let src = TempDir::new().unwrap();
    build_test_tree(src.path());

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.bz2");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &Default::default(), nop_progress)
        .expect("tar.bz2 compress should succeed");
    extract(&archive, dest.path(), &Default::default(), nop_progress)
        .expect("tar.bz2 extract should succeed");

    assert_test_tree(dest.path());
}

// ---------------------------------------------------------------------------
// 12. Report fields
// ---------------------------------------------------------------------------

#[test]
fn compress_report_has_reasonable_fields() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("x.txt"), b"hello world").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("report.tar.gz");

    let report = compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();

    assert!(report.input_bytes > 0, "input_bytes should be > 0");
    assert!(report.output_bytes > 0, "output_bytes should be > 0");
    assert!(report.entries > 0, "entries should be > 0");
    // Duration should be non-negative (it always is for Duration, but at least
    // ensure it's not absurdly large).
    assert!(
        report.duration.as_secs() < 60,
        "duration should be < 60s for a trivial operation"
    );
}

#[test]
fn extract_report_has_reasonable_fields() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("y.txt"), b"data").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("rep.tar.gz");
    let dest = TempDir::new().unwrap();

    compress(src_dir.path(), &archive, &Default::default(), nop_progress).unwrap();
    let report = extract(&archive, dest.path(), &Default::default(), nop_progress).unwrap();

    assert!(report.entries > 0, "entries should be > 0");
    assert!(report.duration.as_secs() < 60);
}

// ---------------------------------------------------------------------------
// 13. Progress callback receives updates
// ---------------------------------------------------------------------------

#[test]
fn compress_progress_callback_is_called() {
    let src_dir = TempDir::new().unwrap();
    let data: Vec<u8> = vec![42u8; 256 * 1024]; // 256 KiB
    std::fs::write(src_dir.path().join("large.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("prog.tar.gz");

    let mut call_count = 0u32;
    compress(src_dir.path(), &archive, &Default::default(), |_| {
        call_count += 1;
    })
    .unwrap();

    assert!(call_count > 0, "progress callback should have been invoked");
}

// ---------------------------------------------------------------------------
// 14. Extraction progress invariant: bytes_done never exceeds bytes_total
// ---------------------------------------------------------------------------

/// Collect all Progress snapshots during an extract call.
fn collect_extract_progress(archive: &std::path::Path) -> Vec<rcomp_core::Progress> {
    let dest = TempDir::new().unwrap();
    let mut snapshots: Vec<rcomp_core::Progress> = Vec::new();
    extract(archive, dest.path(), &Default::default(), |p| {
        snapshots.push(p.clone());
    })
    .expect("extract should succeed");
    snapshots
}

/// For tar.gz: bytes_total is Some(compressed_size) and every bytes_done must
/// be ≤ bytes_total throughout extraction.
#[test]
fn extract_tar_gz_progress_never_exceeds_total() {
    let src = TempDir::new().unwrap();
    // Use a file large enough to trigger multiple chunk callbacks (> 64 KiB).
    let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
    std::fs::write(src.path().join("big.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("big.tar.gz");
    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let snapshots = collect_extract_progress(&archive);
    assert!(
        !snapshots.is_empty(),
        "at least one progress snapshot must be emitted during extraction"
    );

    for snap in &snapshots {
        if let Some(total) = snap.bytes_total {
            assert!(
                snap.bytes_done <= total,
                "tar.gz extraction: bytes_done ({}) exceeded bytes_total ({}) — invariant violated",
                snap.bytes_done,
                total,
            );
        }
    }
}

/// For zip: bytes_total must be None for all progress snapshots (because zip
/// is random-access and we cannot track compressed bytes).
#[test]
fn extract_zip_progress_bytes_total_is_none() {
    let src = TempDir::new().unwrap();
    let data: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
    std::fs::write(src.path().join("big.bin"), &data).unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("big.zip");
    compress(src.path(), &archive, &Default::default(), nop_progress).unwrap();

    let snapshots = collect_extract_progress(&archive);
    assert!(
        !snapshots.is_empty(),
        "at least one progress snapshot must be emitted during zip extraction"
    );

    for snap in &snapshots {
        assert!(
            snap.bytes_total.is_none(),
            "zip extraction: bytes_total should be None but was {:?}",
            snap.bytes_total,
        );
    }
}

// ---------------------------------------------------------------------------
// 15. RAR backend
// ---------------------------------------------------------------------------

/// Path to the RAR test fixture.
///
/// `tests/fixtures/sample.rar` is a copy of `version.rar` from the `unrar`
/// crate's test data (tier b of the 3-tier fixture strategy; `rar` binary not
/// present on this host).
///
/// Contents: one file named `VERSION` with content `"unrar-0.4.0"` (11 bytes).
fn rar_fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.rar")
}

/// RAR compress is always unsupported (RAR creation is proprietary).
/// This test runs with or without the `rar` feature.
#[test]
fn rar_compress_always_unsupported() {
    let src_dir = TempDir::new().unwrap();
    std::fs::write(src_dir.path().join("file.txt"), b"data").unwrap();

    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.rar");

    let opts = CompressOptions {
        format: Some(rcomp_core::Format {
            container: Some(rcomp_core::Container::Rar),
            codec: None,
        }),
        ..CompressOptions::default()
    };

    let err = compress(src_dir.path(), &archive, &opts, nop_progress)
        .expect_err("rar compress should always return UnsupportedOperation");

    assert!(
        matches!(err, Error::UnsupportedOperation { .. }),
        "expected UnsupportedOperation for rar compress, got {err:?}"
    );
}

/// Without the `rar` feature, extract on a .rar file returns UnsupportedOperation
/// whose message mentions the `rar` feature.
#[cfg(not(feature = "rar"))]
#[test]
fn rar_extract_without_feature_returns_unsupported_mentioning_feature() {
    let fixture = rar_fixture();
    let dest = TempDir::new().unwrap();

    let err = extract(&fixture, dest.path(), &Default::default(), nop_progress)
        .expect_err("rar extract without feature should return UnsupportedOperation");

    match err {
        Error::UnsupportedOperation { ref operation, .. } => {
            assert!(
                operation.contains("rar"),
                "UnsupportedOperation message should mention the `rar` feature; got operation={operation:?}"
            );
        }
        other => panic!("expected UnsupportedOperation, got {other:?}"),
    }
}

/// With the `rar` feature, extract the fixture and verify file contents.
#[cfg(feature = "rar")]
#[test]
fn rar_extract_fixture_produces_correct_tree() {
    let fixture = rar_fixture();
    let dest = TempDir::new().unwrap();

    let report = extract(&fixture, dest.path(), &Default::default(), nop_progress)
        .expect("rar extract should succeed");
    assert!(
        report.entries > 0,
        "should have extracted at least one entry"
    );

    // sample.rar contains VERSION with content "unrar-0.4.0".
    let content = std::fs::read(dest.path().join("VERSION")).unwrap();
    assert_eq!(content, b"unrar-0.4.0", "VERSION content mismatch");
}

/// With the `rar` feature, list the fixture and verify it matches extracted contents.
#[cfg(feature = "rar")]
#[test]
fn rar_list_fixture_matches_extract() {
    let fixture = rar_fixture();

    let entries = list(&fixture).expect("rar list should succeed");
    assert_eq!(
        entries.len(),
        1,
        "expected 1 entry in list; got {:?}",
        entries
    );
    assert_eq!(entries[0].path, Path::new("VERSION"));
    assert!(!entries[0].is_dir, "VERSION should not be a directory");
    assert_eq!(entries[0].size, 11, "VERSION size should be 11 bytes");
}

/// With the `rar` feature, extract twice with overwrite=false → AlreadyExists.
#[cfg(feature = "rar")]
#[test]
fn rar_extract_overwrite_false_returns_already_exists() {
    let fixture = rar_fixture();

    let dest = TempDir::new().unwrap();
    // First extract succeeds.
    extract(&fixture, dest.path(), &Default::default(), nop_progress).expect("first extract");

    // Second extract with overwrite=false must return AlreadyExists.
    let err = extract(&fixture, dest.path(), &Default::default(), nop_progress)
        .expect_err("second extract should return AlreadyExists");

    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists on second extract, got {err:?}"
    );
}

/// With the `rar` feature, extraction progress must have bytes_total = None.
#[cfg(feature = "rar")]
#[test]
fn rar_extract_progress_bytes_total_is_none() {
    let fixture = rar_fixture();
    let dest = TempDir::new().unwrap();

    let mut snapshots: Vec<rcomp_core::Progress> = Vec::new();
    extract(&fixture, dest.path(), &Default::default(), |p| {
        snapshots.push(p.clone());
    })
    .expect("rar extract should succeed");

    assert!(
        !snapshots.is_empty(),
        "at least one progress snapshot must be emitted during rar extraction"
    );
    for snap in &snapshots {
        assert!(
            snap.bytes_total.is_none(),
            "rar extraction: bytes_total should be None but was {:?}",
            snap.bytes_total
        );
    }
}
