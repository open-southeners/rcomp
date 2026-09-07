//! Overwrite-policy edge-case tests.
//!
//! Exercises the `AlreadyExists`-style error paths for both `compress` and
//! `extract`, and pins the actual behaviour of several corner cases so that
//! any future policy change is immediately visible in CI.
//!
//! All tests use only the public API (`compress`, `extract`, `Error`).

use rcomp_core::{CompressOptions, Error, ExtractOptions, compress, extract};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// No-op progress callback.
fn nop(_: &rcomp_core::Progress) {}

/// Build a minimal two-file source directory.
///
/// ```text
/// <dir>/
///   a.txt  ("content a")
///   b.txt  ("content b")
/// ```
fn make_two_file_dir(parent: &TempDir) -> std::path::PathBuf {
    let dir = parent.path().join("src");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"content a").unwrap();
    std::fs::write(dir.join("b.txt"), b"content b").unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Compress — refuse to overwrite an existing output file
// ---------------------------------------------------------------------------

/// `compress` to an existing file without `overwrite` must return
/// `Error::AlreadyExists` AND leave the pre-existing file byte-identical.
#[test]
fn compress_no_overwrite_returns_already_exists_and_preserves_content() {
    let work = TempDir::new().unwrap();
    let src = work.path().join("data.txt");
    std::fs::write(&src, b"original data").unwrap();

    // Write a sentinel to the output path so that it already exists.
    let out = work.path().join("out.tar.gz");
    let sentinel = b"i am the pre-existing file, do not touch me";
    std::fs::write(&out, sentinel).unwrap();

    // Compress without overwrite — must refuse.
    let err = compress(&src, &out, &Default::default(), nop)
        .expect_err("compress must fail when output already exists");

    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists, got {err:?}"
    );

    // The pre-existing file must be byte-identical — not truncated, not clobbered.
    let after = std::fs::read(&out).unwrap();
    assert_eq!(
        after, sentinel,
        "pre-existing output file must be untouched after a refused overwrite"
    );
}

// ---------------------------------------------------------------------------
// Compress — overwrite enabled replaces the existing file
// ---------------------------------------------------------------------------

/// With `overwrite = true`, `compress` must succeed and the output must be a
/// valid archive (we verify by re-extracting it).
#[test]
fn compress_with_overwrite_replaces_existing_file() {
    let work = TempDir::new().unwrap();
    let src = work.path().join("data.txt");
    std::fs::write(&src, b"new data for overwrite test").unwrap();

    // Write a sentinel to the output path.
    let out = work.path().join("out.tar.gz");
    std::fs::write(&out, b"old content").unwrap();

    let opts = CompressOptions {
        overwrite: true,
        ..Default::default()
    };
    compress(&src, &out, &opts, nop).expect("compress with overwrite=true must succeed");

    // The sentinel must be gone — the file must be a real archive.
    let after = std::fs::read(&out).unwrap();
    assert_ne!(after, b"old content", "output must have been replaced");
    assert!(!after.is_empty(), "new output must be non-empty");

    // Verify by extracting.
    let dest = TempDir::new().unwrap();
    extract(&out, dest.path(), &Default::default(), nop)
        .expect("re-extracting the overwritten archive must succeed");
    assert_eq!(
        std::fs::read(dest.path().join("data.txt")).unwrap(),
        b"new data for overwrite test"
    );
}

// ---------------------------------------------------------------------------
// Compress — output path is an existing DIRECTORY
// ---------------------------------------------------------------------------

/// When the output path exists as a **directory** (not a file), `compress`
/// must return an error rather than silently clobbering the directory.
///
/// Current behaviour (pinned):
/// - Without `overwrite`: `Error::AlreadyExists` (the guard fires because
///   `output.exists()` is `true` for directories too).
/// - With `overwrite`: `Error::Io` because `fs::File::create` on a directory
///   path fails with an OS error (IsADirectory / PermissionDenied).
///
/// Both are acceptable non-clobbering outcomes; this test pins them so a
/// regression from silent-clobber is detected immediately.
#[test]
fn compress_output_is_existing_directory_returns_error() {
    let work = TempDir::new().unwrap();
    let src = work.path().join("data.txt");
    std::fs::write(&src, b"hello").unwrap();

    // Create the output path as a directory, not a file.
    let out_dir = work.path().join("i_am_a_directory.tar.gz");
    std::fs::create_dir(&out_dir).unwrap();

    // Without overwrite: expect AlreadyExists because `out_dir.exists()` is true.
    let err = compress(&src, &out_dir, &Default::default(), nop)
        .expect_err("compress to an existing directory must fail");
    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists when output is an existing directory; got {err:?}"
    );

    // Directory must still be a directory (not replaced by a file).
    assert!(
        out_dir.is_dir(),
        "output directory must remain a directory after a refused overwrite"
    );

    // With overwrite: the guard is bypassed but File::create on a directory
    // must return an IO error — it must NOT silently clobber the directory.
    let opts = CompressOptions {
        overwrite: true,
        ..Default::default()
    };
    let err2 = compress(&src, &out_dir, &opts, nop)
        .expect_err("compress with overwrite=true to a directory must still fail with Io");
    assert!(
        matches!(err2, Error::Io(_)),
        "expected Error::Io when creating a file over a directory; got {err2:?}"
    );

    // Directory must still be intact.
    assert!(
        out_dir.is_dir(),
        "output directory must remain a directory after a failed overwrite attempt"
    );
}

// ---------------------------------------------------------------------------
// Extract — one colliding file causes AlreadyExists; colliding file preserved
// ---------------------------------------------------------------------------

/// When extracting a multi-file archive into a destination where ONE target
/// file already exists (and `overwrite = false`), `extract` must return
/// `Error::AlreadyExists` and the pre-existing file must keep its original
/// content.
#[test]
fn extract_no_overwrite_single_collision_preserves_file() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    // Build the archive.
    let archive = work.path().join("multi.tar.gz");
    compress(&src, &archive, &Default::default(), nop).expect("initial compress must succeed");

    // Set up a destination with a pre-existing file that will collide.
    let dest = TempDir::new().unwrap();
    let collider = dest.path().join("a.txt");
    let original_content = b"i was here first";
    std::fs::write(&collider, original_content).unwrap();

    // Extract without overwrite — must fail.
    let err = extract(&archive, dest.path(), &Default::default(), nop)
        .expect_err("extract must fail when a target file already exists");
    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists, got {err:?}"
    );

    // The pre-existing file must be byte-identical.
    let after = std::fs::read(&collider).unwrap();
    assert_eq!(
        after, original_content,
        "pre-existing file must be untouched after a refused extract"
    );
}

// ---------------------------------------------------------------------------
// Extract — unrelated pre-existing files are untouched on failure
// ---------------------------------------------------------------------------

/// Files in the destination that are **not** archive entries must not be
/// touched by a failed extraction.
#[test]
fn extract_no_overwrite_unrelated_files_untouched_on_failure() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.tar.gz");
    compress(&src, &archive, &Default::default(), nop).unwrap();

    let dest = TempDir::new().unwrap();

    // Pre-place a file that collides with an archive entry.
    std::fs::write(dest.path().join("a.txt"), b"original a").unwrap();

    // Pre-place a file that does NOT collide with any archive entry.
    let unrelated = dest.path().join("unrelated.bin");
    let unrelated_content = b"unrelated pre-existing file, must not be touched";
    std::fs::write(&unrelated, unrelated_content).unwrap();

    // Extract should fail on the collision.
    let err = extract(&archive, dest.path(), &Default::default(), nop)
        .expect_err("extract must fail on collision");
    assert!(matches!(err, Error::AlreadyExists { .. }));

    // Unrelated file must be untouched.
    let after_unrelated = std::fs::read(&unrelated).unwrap();
    assert_eq!(
        after_unrelated, unrelated_content,
        "unrelated pre-existing file must be untouched after a failed extract"
    );
}

// ---------------------------------------------------------------------------
// Extract — unrelated pre-existing files are untouched after SUCCESS
// ---------------------------------------------------------------------------

/// Files in the destination that are not archive entries must not be touched
/// by a *successful* extraction either.
#[test]
fn extract_with_overwrite_unrelated_files_untouched_on_success() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.tar.gz");
    compress(&src, &archive, &Default::default(), nop).unwrap();

    let dest = TempDir::new().unwrap();

    // Pre-place a file that collides AND a file that does not.
    let collider = dest.path().join("a.txt");
    std::fs::write(&collider, b"old a content").unwrap();

    let unrelated = dest.path().join("unrelated.bin");
    let unrelated_content = b"untouched unrelated file";
    std::fs::write(&unrelated, unrelated_content).unwrap();

    // Extract with overwrite — must succeed.
    let opts = ExtractOptions {
        overwrite: true,
        ..Default::default()
    };
    extract(&archive, dest.path(), &opts, nop).expect("extract with overwrite=true must succeed");

    // Unrelated file must be untouched.
    let after_unrelated = std::fs::read(&unrelated).unwrap();
    assert_eq!(
        after_unrelated, unrelated_content,
        "unrelated pre-existing file must be untouched after a successful overwrite extract"
    );
}

// ---------------------------------------------------------------------------
// Extract — overwrite replaces the colliding file with correct content
// ---------------------------------------------------------------------------

/// With `overwrite = true`, a colliding file must be replaced by the archive
/// entry's correct content.
#[test]
fn extract_with_overwrite_replaces_colliding_file() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.tar.gz");
    compress(&src, &archive, &Default::default(), nop).unwrap();

    let dest = TempDir::new().unwrap();

    // Pre-place a colliding file with stale content.
    let collider = dest.path().join("a.txt");
    std::fs::write(&collider, b"stale content to be overwritten").unwrap();

    // Extract with overwrite.
    let opts = ExtractOptions {
        overwrite: true,
        ..Default::default()
    };
    extract(&archive, dest.path(), &opts, nop).expect("extract with overwrite=true must succeed");

    // Colliding file must now have the archive's content.
    let after = std::fs::read(&collider).unwrap();
    assert_eq!(
        after, b"content a",
        "colliding file must be replaced with archive entry content"
    );

    // Non-colliding entry must also be present.
    let b = std::fs::read(dest.path().join("b.txt")).unwrap();
    assert_eq!(b, b"content b", "b.txt must be extracted normally");
}

// ---------------------------------------------------------------------------
// Extract ZIP — refuse to overwrite, preserve file
// ---------------------------------------------------------------------------

/// Same collision test but for the ZIP backend, ensuring the `zip::extract`
/// overwrite guard behaves the same as the tar backend.
#[test]
fn extract_zip_no_overwrite_single_collision_preserves_file() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.zip");
    compress(&src, &archive, &Default::default(), nop).expect("initial zip compress must succeed");

    let dest = TempDir::new().unwrap();
    let collider = dest.path().join("a.txt");
    let original_content = b"zip collision - i was here first";
    std::fs::write(&collider, original_content).unwrap();

    let err = extract(&archive, dest.path(), &Default::default(), nop)
        .expect_err("zip extract must fail when a target file already exists");
    assert!(
        matches!(err, Error::AlreadyExists { .. }),
        "expected AlreadyExists for zip, got {err:?}"
    );

    let after = std::fs::read(&collider).unwrap();
    assert_eq!(
        after, original_content,
        "pre-existing file must be untouched after a refused zip extract"
    );
}

// ---------------------------------------------------------------------------
// Extract ZIP — overwrite succeeds and replaces colliding file
// ---------------------------------------------------------------------------

#[test]
fn extract_zip_with_overwrite_replaces_colliding_file() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.zip");
    compress(&src, &archive, &Default::default(), nop).unwrap();

    let dest = TempDir::new().unwrap();
    let collider = dest.path().join("a.txt");
    std::fs::write(&collider, b"stale zip content").unwrap();

    let opts = ExtractOptions {
        overwrite: true,
        ..Default::default()
    };
    extract(&archive, dest.path(), &opts, nop)
        .expect("zip extract with overwrite=true must succeed");

    let after = std::fs::read(&collider).unwrap();
    assert_eq!(
        after, b"content a",
        "zip extract with overwrite must replace the colliding file"
    );
}

// ---------------------------------------------------------------------------
// Compress — content-integrity check: refused overwrite touches nothing
// ---------------------------------------------------------------------------

/// Regression guard: the best-effort `fs::remove_file(output)` in the cleanup
/// path is only called when `do_compress` itself returns an error.  When the
/// guard fires *before* `do_compress` (i.e. for `AlreadyExists`), the cleanup
/// code is NOT reached, so the existing file must survive intact.
#[test]
fn compress_no_overwrite_cleanup_does_not_remove_existing_file() {
    let work = TempDir::new().unwrap();
    let src_dir = make_two_file_dir(&work);

    // First compress: creates the archive.
    let archive = work.path().join("existing.tar.gz");
    compress(&src_dir, &archive, &Default::default(), nop).expect("first compress must succeed");

    let size_before = std::fs::metadata(&archive).unwrap().len();
    assert!(size_before > 0, "archive must be non-empty");

    // Second compress without overwrite: must fail without touching the archive.
    let err = compress(&src_dir, &archive, &Default::default(), nop)
        .expect_err("second compress must fail");
    assert!(matches!(err, Error::AlreadyExists { .. }));

    let size_after = std::fs::metadata(&archive).unwrap().len();
    assert_eq!(
        size_before, size_after,
        "archive file size must be unchanged after a refused overwrite"
    );
}

// ---------------------------------------------------------------------------
// Extract — output_bytes must not fold in pre-existing unrelated files
// ---------------------------------------------------------------------------

/// Regression guard: extracting a small archive into a destination directory
/// that already contains a large, unrelated file must report `output_bytes`
/// for the bytes this extraction actually wrote, not the size of everything
/// already sitting in `dest` (e.g. extracting a tiny archive into `~/Downloads`
/// must not report gigabytes just because Downloads already has other files
/// in it).
#[test]
fn extract_output_bytes_excludes_preexisting_unrelated_files() {
    let work = TempDir::new().unwrap();
    let src = make_two_file_dir(&work);

    let archive = work.path().join("multi.zip");
    compress(&src, &archive, &Default::default(), nop).unwrap();

    let dest = TempDir::new().unwrap();
    // An unrelated file already in `dest`, much larger than anything the
    // archive contains ("content a" + "content b" = 18 bytes total).
    std::fs::write(dest.path().join("unrelated.bin"), vec![0u8; 1024 * 1024]).unwrap();

    let report =
        extract(&archive, dest.path(), &Default::default(), nop).expect("extract must succeed");

    assert_eq!(
        report.output_bytes, 18,
        "output_bytes must reflect only the bytes this extraction wrote, \
         not the pre-existing unrelated file already in dest"
    );
}
