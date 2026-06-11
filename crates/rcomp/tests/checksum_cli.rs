//! CLI integration tests for `--checksum`, sidecar auto-verify, and related
//! error paths.
//!
//! Uses `assert_cmd` to drive the `rcomp` binary end-to-end.  Each test is
//! self-contained and uses a `tempfile::TempDir` for isolation.
//!
//! The `sha256sum_available` guard gates tests that invoke `sha256sum` so they
//! are skipped gracefully on platforms where the tool is absent.

use std::{fs, path::PathBuf, process::Command as StdCommand};

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

/// Return `true` when `sha256sum` is available on `$PATH`.
///
/// Used to gate tests that verify the sidecar with the real coreutils tool.
fn sha256sum_available() -> bool {
    StdCommand::new("sha256sum")
        .arg("--version")
        .output()
        .is_ok()
}

/// Create a temporary directory with two known files.
///
/// Layout:
/// ```text
/// <parent>/src_dir/
///   hello.txt ("hello\n")
///   world.txt ("world\n")
/// ```
fn make_two_file_dir(parent: &TempDir) -> PathBuf {
    let dir = parent.path().join("src_dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("hello.txt"), b"hello\n").unwrap();
    fs::write(dir.join("world.txt"), b"world\n").unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Test: --checksum writes a sidecar and summary shows the digest
// ---------------------------------------------------------------------------

#[test]
fn checksum_writes_sidecar_and_summary_shows_digest() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"checksum sidecar test content").unwrap();
    let out = tmp.path().join("data.txt.gz");

    // Compress with --checksum.
    let stdout = rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Sidecar must exist.
    let sidecar = tmp.path().join("data.txt.gz.sha256");
    assert!(
        sidecar.exists(),
        "sidecar should be written next to the output"
    );

    // Stdout must contain a `sha256: <hex>` line (artifact digest).
    let text = String::from_utf8(stdout).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("sha256: ")),
        "stdout should contain a sha256 line; got: {text:?}"
    );

    // Extract the hex from the sha256 line and verify it is 64 hex chars.
    let hex_line = text.lines().find(|l| l.starts_with("sha256: ")).unwrap();
    let hex = hex_line.strip_prefix("sha256: ").unwrap().trim();
    assert_eq!(hex.len(), 64, "digest should be 64 hex chars; got: {hex:?}");
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "digest should be hex; got: {hex:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: sidecar is valid `sha256sum -c` input on Unix
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn sidecar_passes_sha256sum_check() {
    if !sha256sum_available() {
        eprintln!("sha256sum not found — skipping sidecar verification test");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("payload.txt");
    fs::write(&src, b"content for sha256sum verification").unwrap();
    let out = tmp.path().join("payload.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    let sidecar = tmp.path().join("payload.txt.gz.sha256");
    assert!(sidecar.exists(), "sidecar must exist");

    // Run `sha256sum -c payload.txt.gz.sha256` from the temp dir so that the
    // relative filename in the sidecar resolves to the archive.
    let check = StdCommand::new("sha256sum")
        .arg("--check")
        .arg(sidecar.to_str().unwrap())
        .current_dir(tmp.path())
        .output()
        .expect("sha256sum --check failed to launch");

    assert!(
        check.status.success(),
        "sha256sum --check failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
}

// ---------------------------------------------------------------------------
// Test: -q suppresses stdout digest line but still writes the sidecar
// ---------------------------------------------------------------------------

#[test]
fn quiet_checksum_no_stdout_but_sidecar_written() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("q.txt");
    fs::write(&src, b"quiet checksum test").unwrap();
    let out = tmp.path().join("q.txt.gz");

    rcomp()
        .args([
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            "--checksum",
            "-q",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let sidecar = tmp.path().join("q.txt.gz.sha256");
    assert!(sidecar.exists(), "sidecar must be written even with -q");
}

// ---------------------------------------------------------------------------
// Test: sidecar collision without --force → exit 1 + hint
// ---------------------------------------------------------------------------

#[test]
fn sidecar_collision_without_force_exits_1_with_hint() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("col.txt");
    fs::write(&src, b"collision test").unwrap();
    let out = tmp.path().join("col.txt.gz");

    // First compress with --checksum succeeds.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    // Second attempt without --force must fail because the sidecar exists.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("--force").or(predicate::str::contains("force")));
}

// ---------------------------------------------------------------------------
// Test: --force allows overwriting an existing sidecar
// ---------------------------------------------------------------------------

#[test]
fn sidecar_collision_with_force_overwrites() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("over.txt");
    fs::write(&src, b"overwrite test").unwrap();
    let out = tmp.path().join("over.txt.gz");

    // First compress.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    // Second compress with --force must succeed.
    rcomp()
        .args([
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            "--checksum",
            "--force",
        ])
        .assert()
        .success();

    let sidecar = tmp.path().join("over.txt.gz.sha256");
    assert!(
        sidecar.exists(),
        "sidecar must exist after --force overwrite"
    );
}

// ---------------------------------------------------------------------------
// Test: extract with a valid sidecar succeeds and mentions verification
// ---------------------------------------------------------------------------

#[test]
fn extract_with_sidecar_succeeds_and_notes_verification() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("verify.txt");
    fs::write(&src, b"auto-verify roundtrip content").unwrap();
    let out = tmp.path().join("verify.txt.gz");

    // Compress with --checksum.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    let sidecar = tmp.path().join("verify.txt.gz.sha256");
    assert!(sidecar.exists(), "sidecar must exist");

    // Extract — the sidecar is present so auto-verify fires.
    let dest = tmp.path().join("extracted");
    let stderr = rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    assert!(
        err_text.contains("verified sha256"),
        "stderr should mention verification; got: {err_text:?}"
    );

    // The original file must be restored.
    assert!(
        dest.join("verify.txt").exists(),
        "extracted file should exist in dest"
    );
    assert_eq!(
        fs::read(dest.join("verify.txt")).unwrap(),
        b"auto-verify roundtrip content"
    );
}

// ---------------------------------------------------------------------------
// Test: corrupted archive + sidecar → exit 1 with mismatch message
//
// We use a `.zip` archive: its `list()` reads the central directory at the
// END of the file, so flipping a byte in the middle of the file still allows
// listing to succeed.  The artifact checksum check inside `extract()` then
// fires before any data is written, producing the mismatch message.
// ---------------------------------------------------------------------------

#[test]
fn corrupted_archive_with_sidecar_exits_1_mismatch_message() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    let out = tmp.path().join("corrupt.zip");

    // Compress with --checksum → sidecar records the artifact digest.
    rcomp()
        .args([dir.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    // Read the archive and identify the start of the local file data
    // (past the first local file header at offset 0).  We flip a byte there
    // so the central directory (at the end) remains intact and list() succeeds,
    // but the artifact digest no longer matches.
    let mut bytes = fs::read(&out).unwrap();
    // ZIP local file header starts at byte 0: signature 4 bytes, then 26 bytes
    // of fixed fields.  The file name length and extra field length are at
    // offsets 26 and 28.  We just flip a byte well into the data area.
    let flip_pos = bytes.len() / 4;
    bytes[flip_pos] ^= 0xff;
    fs::write(&out, bytes).unwrap();

    // Extract must fail because the artifact digest no longer matches.
    let dest = tmp.path().join("corrupt_out");
    rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("mismatch")
                .or(predicate::str::contains("checksum"))
                .or(predicate::str::contains("artifact")),
        );
}

// ---------------------------------------------------------------------------
// Test: corrupt / garbage sidecar → exit 1
// ---------------------------------------------------------------------------

#[test]
fn garbage_sidecar_exits_1() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("gs.txt");
    fs::write(&src, b"garbage sidecar test").unwrap();
    let out = tmp.path().join("gs.txt.gz");

    // Compress WITHOUT --checksum so no sidecar is written automatically.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Write a garbage sidecar manually.
    let sidecar = tmp.path().join("gs.txt.gz.sha256");
    fs::write(&sidecar, b"not a valid sidecar\n").unwrap();

    let dest = tmp.path().join("gs_out");
    // Extract should fail because the sidecar exists but has no matching line.
    rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

// ---------------------------------------------------------------------------
// Test: malformed hex in sidecar → exit 1
// ---------------------------------------------------------------------------

#[test]
fn malformed_hex_in_sidecar_exits_1() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("mh.txt");
    fs::write(&src, b"malformed hex test").unwrap();
    let out = tmp.path().join("mh.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Write a sidecar with an incorrect-length hex (not 64 chars).
    let sidecar = tmp.path().join("mh.txt.gz.sha256");
    fs::write(&sidecar, b"deadbeef  mh.txt.gz\n").unwrap();

    let dest = tmp.path().join("mh_out");
    rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

// ---------------------------------------------------------------------------
// Test: --checksum on an extract invocation → exit 2
// ---------------------------------------------------------------------------

#[test]
fn checksum_flag_on_extract_exits_2() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("xtract.txt");
    fs::write(&src, b"extract flag test").unwrap();
    let out = tmp.path().join("xtract.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Using --checksum while extracting is a usage error.
    rcomp()
        .args([out.to_str().unwrap(), "--checksum"])
        .assert()
        .failure()
        .code(2);
}

// ---------------------------------------------------------------------------
// Test: sidecar content matches the archive's actual SHA-256 digest
//       (verified via `sha256sum` tool on Unix)
// ---------------------------------------------------------------------------

/// Verify that the artifact hex in the sidecar matches what `sha256sum`
/// independently computes for the archive.  Gated on Unix + sha256sum.
#[cfg(unix)]
#[test]
fn sidecar_artifact_hex_matches_file_digest_via_sha256sum() {
    if !sha256sum_available() {
        eprintln!("sha256sum not found — skipping digest cross-check");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("xcheck.txt");
    fs::write(&src, b"deterministic content for digest cross-check").unwrap();
    let out = tmp.path().join("xcheck.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success();

    // Read the sidecar and extract the artifact hex.
    let sidecar = tmp.path().join("xcheck.txt.gz.sha256");
    let raw = fs::read_to_string(&sidecar).unwrap();
    let sidecar_hex = raw
        .lines()
        .find(|l| !l.starts_with('#') && !l.trim().is_empty())
        .and_then(|l| l.split_whitespace().next())
        .expect("sidecar must have an artifact line");

    // Compute independently with sha256sum.
    let output = StdCommand::new("sha256sum")
        .arg(out.to_str().unwrap())
        .output()
        .expect("sha256sum failed to launch");
    let tool_line = String::from_utf8(output.stdout).unwrap();
    let tool_hex = tool_line.split_whitespace().next().unwrap_or("");

    assert_eq!(
        sidecar_hex, tool_hex,
        "sidecar digest must match sha256sum output"
    );
}

// ---------------------------------------------------------------------------
// Test: roundtrip with --checksum and content digest (tar.gz dir)
// ---------------------------------------------------------------------------

#[test]
fn checksum_dir_tar_gz_roundtrip_with_content_digest() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    let out = tmp.path().join("rt.tar.gz");

    let stdout = rcomp()
        .args([dir.to_str().unwrap(), out.to_str().unwrap(), "--checksum"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(stdout).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("sha256: ")),
        "stdout should have sha256 line; got: {text:?}"
    );

    let sidecar = tmp.path().join("rt.tar.gz.sha256");
    assert!(sidecar.exists(), "sidecar must exist for dir tar.gz");

    // The sidecar should have a `# content-sha256:` comment line for tar+codec.
    let raw = fs::read_to_string(&sidecar).unwrap();
    assert!(
        raw.lines().any(|l| l.starts_with("# content-sha256:")),
        "sidecar for tar.gz should have content-sha256 comment; got:\n{raw}"
    );

    // Extract with sidecar present → auto-verify.
    let dest = tmp.path().join("rt_dest");
    let stderr = rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    assert!(
        err_text.contains("verified sha256"),
        "stderr should confirm verification; got: {err_text:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: no sidecar → extraction succeeds silently (no mention of verification)
// ---------------------------------------------------------------------------

#[test]
fn no_sidecar_extraction_is_silent() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("ns.txt");
    fs::write(&src, b"no sidecar content").unwrap();
    let out = tmp.path().join("ns.txt.gz");

    // Compress WITHOUT --checksum.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    let sidecar = tmp.path().join("ns.txt.gz.sha256");
    assert!(!sidecar.exists(), "sidecar must not exist");

    let dest = tmp.path().join("ns_dest");
    let stderr = rcomp()
        .args([out.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    assert!(
        !err_text.contains("verified"),
        "stderr must not mention verification when no sidecar; got: {err_text:?}"
    );
}
