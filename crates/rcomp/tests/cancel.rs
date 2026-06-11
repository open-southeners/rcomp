//! Ctrl-C cancellation integration test.
//!
//! Spawns the built `rcomp` binary compressing a large incompressible file,
//! sends SIGINT mid-operation, and asserts that:
//!
//! - The process exits with a non-zero status (exit code 1).
//! - stderr mentions cancellation.
//! - No partial output file remains (core's `Cancelled` cleanup ran).
//!
//! **Codec choice**: bzip2 is used rather than xz because the xz backend uses
//! liblzma's multi-threaded stream encoder (`MtStreamBuilder`), which buffers
//! all input asynchronously before compressing.  This means the
//! `copy_with_progress` cancel-check loop finishes in milliseconds (all writes
//! return immediately) and the actual compression happens inside
//! `encoder.finish()` with no cancel hook.  bzip2's encoder is strictly
//! synchronous — each 64 KiB `write()` blocks until that block is compressed —
//! so the cancel-check fires reliably between blocks.
//!
//! Only enabled on Unix because signal delivery via `kill -INT` is a Unix
//! concept.

#![cfg(unix)]

use std::{
    fs,
    io::{self, Write as _},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a file of `size` bytes from `/dev/urandom` at `path`.
///
/// `/dev/urandom` data is incompressible (high entropy), ensuring that the
/// bzip2 encoder cannot finish quickly even at `--best`.
fn write_random_file(path: &PathBuf, size: usize) -> io::Result<()> {
    let mut src = fs::File::open("/dev/urandom")?;
    let mut dst = fs::File::create(path)?;
    let mut remaining = size;
    let mut buf = [0u8; 65536];
    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = io::Read::read(&mut src, &mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        remaining -= n;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Compress a large incompressible file with bzip2 `--best`, send SIGINT after
/// a short delay, and verify:
///
/// 1. The process exits with a non-zero status.
/// 2. stderr contains the word "cancelled".
/// 3. The partial output file (`out.bz2`) does **not** remain in the tempdir
///    (core's `Cancelled` cleanup deleted it).
///
/// 96 MiB of incompressible random data at bzip2 `--best` takes roughly 8–10 s
/// on this hardware; 1 s of delay reliably interrupts mid-operation while
/// keeping total test time well under 10 s.
#[test]
fn ctrl_c_cancels_compress_and_cleans_up() {
    let tmp = TempDir::new().expect("tempdir");

    // 96 MiB of random, incompressible data.
    let input = tmp.path().join("random.bin");
    write_random_file(&input, 96 * 1024 * 1024).expect("write random file");

    let output = tmp.path().join("out.bz2");

    // Spawn the rcomp binary directly (not via assert_cmd) so we get a Child
    // handle and can send signals to it.
    let child = Command::new(cargo_bin("rcomp"))
        .args([
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--best", // bzip2 is single-threaded and synchronous: cancel fires between chunks
            "-q",     // suppress progress bar output so stderr only shows our message
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rcomp");

    let pid = child.id();

    // Give bzip2 a moment to start compressing several blocks, then send SIGINT.
    // 1 s is enough to process several 64 KiB chunks; the test stays under ~3 s
    // (the process exits immediately once the next cancel check fires).
    thread::sleep(Duration::from_millis(1000));

    // Send SIGINT to the child process.
    Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("kill -INT");

    // Wait for the child to finish (it should exit quickly after SIGINT).
    let output_result = child.wait_with_output().expect("wait_with_output");

    // 1. Non-zero exit status.
    assert!(
        !output_result.status.success(),
        "rcomp should exit with non-zero status after SIGINT; \
         got: {:?}",
        output_result.status
    );

    // 2. stderr mentions cancellation.
    let stderr = String::from_utf8_lossy(&output_result.stderr);
    assert!(
        stderr.contains("cancelled"),
        "stderr should mention 'cancelled'; got: {stderr:?}"
    );

    // 3. No partial output file remains.
    assert!(
        !output.exists(),
        "partial output file should have been deleted by core's Cancelled cleanup; \
         found: {}",
        output.display()
    );
}
