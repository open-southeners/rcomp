//! CLI-level overwrite-policy tests.
//!
//! Uses `assert_cmd` to drive the `rcomp` binary end-to-end, exercising
//! the `--force` flag and the `AlreadyExists`→stderr hint path.
//!
//! Each test is self-contained: it builds its own archive (or input file) via
//! prior `rcomp` invocations rather than constructing archives by hand so that
//! the test does not depend on internal archive format details.

use std::{fs, path::PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return an [`assert_cmd::Command`] configured to run the `rcomp` binary.
fn rcomp() -> Command {
    Command::cargo_bin("rcomp").expect("rcomp binary must exist")
}

/// Walk `dir` recursively and return the first path whose file name equals `name`.
fn find_file_recursive(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// compress — refused overwrite: exit 1 + --force hint in stderr
// ---------------------------------------------------------------------------

/// Compressing to an existing output without `--force` must exit 1 and emit a
/// `--force` hint to stderr so that users know how to resolve the issue.
#[test]
fn compress_refused_overwrite_exits_1_with_force_hint() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello overwrite test").unwrap();
    let out = tmp.path().join("data.txt.gz");

    // First compress: must succeed.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    assert!(out.exists(), "output archive must exist after first compress");

    // Second compress without --force: must fail with exit code 1 and hint.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("--force").or(predicate::str::contains("force")),
        );
}

// ---------------------------------------------------------------------------
// compress — with --force: exit 0
// ---------------------------------------------------------------------------

/// Compressing to an existing output *with* `--force` must succeed (exit 0).
#[test]
fn compress_with_force_exits_0() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"content for force test").unwrap();
    let out = tmp.path().join("data.txt.gz");

    // First compress.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Second compress with --force: must succeed.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--force"])
        .assert()
        .success()
        .code(0);
}

// ---------------------------------------------------------------------------
// compress — --force output is a valid archive
// ---------------------------------------------------------------------------

/// After a `--force` compress the output must be extractable (not corrupted).
///
/// This test uses the same source file name both times so that the extracted
/// name is predictable (the archive stem is stripped from the archive name,
/// yielding the original source basename in both cases).
#[test]
fn compress_force_produces_valid_archive() {
    let tmp = TempDir::new().unwrap();

    // Source file: "payload.txt" → archive "payload.txt.gz".
    // The archive stem is "payload.txt" (strip ".gz"), so extract produces
    // "payload.txt" in the destination.
    let src = tmp.path().join("payload.txt");
    fs::write(&src, b"force-overwrite payload v1").unwrap();
    let out = tmp.path().join("payload.txt.gz");

    // First compress.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Overwrite with updated content (same file name so extraction name is stable).
    fs::write(&src, b"updated payload v2").unwrap();
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--force"])
        .assert()
        .success();

    // Extract and verify.
    let dest = tmp.path().join("extract_dest");
    rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Name after extraction: strip ".gz" from "payload.txt.gz" → "payload.txt".
    let restored = dest.join("payload.txt");
    assert!(restored.exists(), "payload.txt must be present after extract");
    assert_eq!(
        fs::read(&restored).unwrap(),
        b"updated payload v2",
        "extracted content must reflect the overwritten (v2) source"
    );
}

// ---------------------------------------------------------------------------
// extract — wrap-folder collision on second run: pin actual behavior
// ---------------------------------------------------------------------------
//
// Scenario: extract a multi-root archive into dest/ once (this creates the
// wrap folder dest/<stem>/), then extract it a second time into the same dest/.
//
// Expected (per spec): second run without `--force` → error; with `--force` → success.
//
// This test PINS THE ACTUAL BEHAVIOR.  If the actual behavior differs from the
// spec (e.g. the second run silently overwrites without --force, or the second
// run with --force still fails), the test captures the actual behavior with a
// comment so a human can review.

/// Build a multi-root archive (two loose top-level files) and return its path.
fn build_multi_root_archive(tmp: &TempDir) -> PathBuf {
    let dir = tmp.path().join("src_dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("alpha.txt"), b"alpha content").unwrap();
    fs::write(dir.join("beta.txt"), b"beta content").unwrap();

    let archive = tmp.path().join("multi.tar.gz");
    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    archive
}

/// Extracting the same multi-root archive a second time into the same dest
/// (same wrap folder collision) without `--force` must fail with exit 1.
///
/// NOTE (pinned behavior): if this test FAILS meaning the actual exit code is
/// 0 (the second run succeeds without --force), that means the wrap-folder
/// collision path does NOT enforce the no-overwrite policy.  See the Issues
/// section of the test report.
#[test]
fn extract_wrap_folder_collision_without_force_fails() {
    let tmp = TempDir::new().unwrap();
    let archive = build_multi_root_archive(&tmp);

    let dest = tmp.path().join("dest");
    fs::create_dir(&dest).unwrap();

    // First extraction: must succeed and create the wrap folder.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Wrap folder must exist after first extraction.
    // The wrap folder is named after the archive stem ("multi").
    let wrap_folder = dest.join("multi");
    assert!(
        wrap_folder.exists(),
        "wrap folder 'multi' must exist after first extraction"
    );

    // Second extraction into the same dest without --force:
    // PINNED: expected to fail with exit 1 because the archive entries inside
    // the wrap folder already exist.
    //
    // If this assertion fails (actual exit is 0), the overwrite guard is not
    // enforced for the wrap-folder collision scenario — report this as a bug.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

/// Extracting the same multi-root archive a second time with `--force` must
/// succeed (exit 0) and replace the existing entries.
///
/// NOTE (pinned behavior): if this test FAILS meaning the actual exit code is
/// non-zero, the --force flag is not being wired correctly into the extract
/// path for wrap-folder scenarios.
#[test]
fn extract_wrap_folder_collision_with_force_succeeds() {
    let tmp = TempDir::new().unwrap();
    let archive = build_multi_root_archive(&tmp);

    let dest = tmp.path().join("dest");
    fs::create_dir(&dest).unwrap();

    // First extraction: must succeed.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Second extraction with --force: must succeed (exit 0).
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap(), "--force"])
        .assert()
        .success()
        .code(0);

    // Files must be present and correct after the overwrite.
    let alpha = find_file_recursive(&dest, "alpha.txt");
    let beta = find_file_recursive(&dest, "beta.txt");
    assert!(alpha.is_some(), "alpha.txt must be present after --force extract");
    assert!(beta.is_some(), "beta.txt must be present after --force extract");
    assert_eq!(fs::read(alpha.unwrap()).unwrap(), b"alpha content");
    assert_eq!(fs::read(beta.unwrap()).unwrap(), b"beta content");
}

// ---------------------------------------------------------------------------
// extract — refused overwrite emits a --force hint
// ---------------------------------------------------------------------------

/// When extract refuses to overwrite (no --force), the error message on stderr
/// must contain a `--force` hint so users know what to do.
#[test]
fn extract_refused_overwrite_stderr_contains_force_hint() {
    let tmp = TempDir::new().unwrap();

    let src = tmp.path().join("file.txt");
    fs::write(&src, b"extract overwrite hint test").unwrap();
    let archive = tmp.path().join("file.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("dest");

    // First extraction.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Second extraction without --force: expect force hint in stderr.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("--force").or(predicate::str::contains("force")),
        );
}

// ---------------------------------------------------------------------------
// extract — --force allows re-extracting an archive into its own dest
// ---------------------------------------------------------------------------

/// Re-extracting into the same dest with `--force` must succeed.
#[test]
fn extract_with_force_allows_reextract_single_file() {
    let tmp = TempDir::new().unwrap();

    let src = tmp.path().join("note.txt");
    fs::write(&src, b"note content").unwrap();
    let archive = tmp.path().join("note.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("dest");

    // First extraction.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Second extraction with --force.
    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap(), "--force"])
        .assert()
        .success()
        .code(0);

    let note = dest.join("note.txt");
    assert!(note.exists(), "note.txt must exist after --force re-extract");
    assert_eq!(fs::read(&note).unwrap(), b"note content");
}
