//! Integration tests for SHA-256 checksum support in compress/extract.
//!
//! Tests are organised as follows:
//!
//! 1. Artifact digest matches `sha256sum` for `.zst`, `.tar.gz`, `.zip`.
//! 2. Content digest present and correct for codec/tar paths; `None` for zip/7z.
//! 3. Plain `.tar` → both artifact and content digests are equal.
//! 4. `checksum: false` → both fields are `None`.
//! 5. Roundtrip: compress with checksum, extract with both verify fields → Ok.
//! 6. Flip one byte in the artifact → `ChecksumMismatch { kind: "artifact" }`,
//!    dest dir still has no files.
//! 7. Wrong content digest → `ChecksumMismatch { kind: "content" }`.
//! 8. `zip + verify_content_sha256` → `UnsupportedOperation`.

use std::path::Path;

use rcomp_core::{CompressOptions, Error, ExtractOptions, compress, extract};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn nop(_: &rcomp_core::Progress) {}

/// Build a small single-file input.
fn make_file(root: &Path, name: &str, content: &[u8]) {
    std::fs::write(root.join(name), content).unwrap();
}

/// Build a small directory tree.
fn make_tree(root: &Path) {
    std::fs::write(root.join("a.txt"), b"hello from a").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b.txt"), b"hello from b").unwrap();
}

/// Compute SHA-256 of a file using sha2 directly (for cross-checking without
/// spawning sha256sum).
fn sha256_of_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).expect("read file for sha256");
    let mut h = Sha256::new();
    h.update(&bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute SHA-256 of raw bytes.
fn sha256_of_bytes(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Helper: verify with the system `sha256sum` tool (unix-only)
// ---------------------------------------------------------------------------

/// Run `sha256sum` on `file` and return the lowercase hex digest.
///
/// Returns `None` if the tool is not available (so the test is skipped on
/// systems that lack it rather than failing).
#[cfg(unix)]
fn sha256sum_of(file: &Path) -> Option<String> {
    let output = std::process::Command::new("sha256sum")
        .arg(file)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    // sha256sum output: "<hex>  <filename>"
    stdout
        .split_whitespace()
        .next()
        .map(|s| s.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// 1. Artifact digest matches sha256sum (.zst single file)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn artifact_digest_matches_sha256sum_zst() {
    let src = TempDir::new().unwrap();
    make_file(src.path(), "data.bin", b"the quick brown fox");
    let work = TempDir::new().unwrap();
    let archive = work.path().join("data.bin.zst");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path().join("data.bin").as_path(), &archive, &opts, nop)
        .expect("compress should succeed");

    let hex = report
        .sha256
        .expect("sha256 should be Some when checksum=true");

    // Cross-check with the system sha256sum tool.
    if let Some(sys_hex) = sha256sum_of(&archive) {
        assert_eq!(hex, sys_hex, "artifact digest should match sha256sum");
    }

    // Also verify by re-hashing with sha2.
    let recomputed = sha256_of_file(&archive);
    assert_eq!(
        hex, recomputed,
        "artifact digest must match re-hash of file"
    );
}

// ---------------------------------------------------------------------------
// 1b. Artifact digest for .tar.gz directory
// ---------------------------------------------------------------------------

#[test]
fn artifact_digest_matches_rehash_tar_gz_dir() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    let hex = report.sha256.expect("sha256 should be Some");
    let recomputed = sha256_of_file(&archive);
    assert_eq!(
        hex, recomputed,
        "artifact digest must match re-hash of .tar.gz"
    );
}

// ---------------------------------------------------------------------------
// 1c. Artifact digest for .zip directory
// ---------------------------------------------------------------------------

#[test]
fn artifact_digest_matches_rehash_zip_dir() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.zip");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    let hex = report.sha256.expect("sha256 should be Some for zip");
    let recomputed = sha256_of_file(&archive);
    assert_eq!(
        hex, recomputed,
        "artifact digest must match re-hash of .zip"
    );
}

// ---------------------------------------------------------------------------
// 2. Content digest present + correct for codec path
// ---------------------------------------------------------------------------

#[test]
fn content_digest_codec_only_equals_input_sha256() {
    let src = TempDir::new().unwrap();
    let input_data = b"the quick brown fox jumps over the lazy dog";
    make_file(src.path(), "data.txt", input_data);
    let input_path = src.path().join("data.txt");

    let work = TempDir::new().unwrap();
    let archive = work.path().join("data.txt.zst");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(&input_path, &archive, &opts, nop).expect("compress should succeed");

    let content_hex = report
        .content_sha256
        .expect("content_sha256 should be Some for codec-only");

    // The content digest must equal the SHA-256 of the original input file.
    let expected = sha256_of_bytes(input_data);
    assert_eq!(
        content_hex, expected,
        "content digest must equal sha256 of original input"
    );
}

// ---------------------------------------------------------------------------
// 2b. Content digest for .tar.gz: equals sha256 of decompressed tar stream
// ---------------------------------------------------------------------------

#[test]
fn content_digest_tar_gz_equals_tar_stream_sha256() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    assert!(
        report.content_sha256.is_some(),
        "content_sha256 should be Some for tar+codec"
    );

    // Cross-check: decompress the .tar.gz and hash the raw tar bytes.
    let gz_bytes = std::fs::read(&archive).expect("read archive");
    let mut decoder = flate2::read::GzDecoder::new(gz_bytes.as_slice());
    let mut tar_bytes = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut tar_bytes).expect("decompress");

    let expected = sha256_of_bytes(&tar_bytes);
    assert_eq!(
        report.content_sha256.unwrap(),
        expected,
        "content digest must equal sha256 of decompressed tar stream"
    );
}

// ---------------------------------------------------------------------------
// 2c. Content digest is None for zip
// ---------------------------------------------------------------------------

#[test]
fn content_digest_none_for_zip() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.zip");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    assert!(
        report.content_sha256.is_none(),
        "content_sha256 must be None for zip (no single content stream)"
    );
}

// ---------------------------------------------------------------------------
// 2d. Content digest is None for 7z
// ---------------------------------------------------------------------------

#[test]
fn content_digest_none_for_sevenz() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.7z");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    assert!(
        report.content_sha256.is_none(),
        "content_sha256 must be None for 7z"
    );
}

// ---------------------------------------------------------------------------
// 3. Plain tar: both digests equal, both Some
// ---------------------------------------------------------------------------

#[test]
fn plain_tar_artifact_equals_content_digest() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar");

    let opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &opts, nop).expect("compress should succeed");

    let artifact = report.sha256.expect("sha256 should be Some for plain tar");
    let content = report
        .content_sha256
        .expect("content_sha256 should be Some for plain tar");

    assert_eq!(
        artifact, content,
        "plain tar: artifact and content digests must be equal"
    );

    // Both must match the file on disk.
    let on_disk = sha256_of_file(&archive);
    assert_eq!(
        artifact, on_disk,
        "artifact must match re-hash of .tar file"
    );
}

// ---------------------------------------------------------------------------
// 4. checksum: false → both None
// ---------------------------------------------------------------------------

#[test]
fn checksum_false_both_none() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    // Default has checksum: false
    let report = compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("compress should succeed");

    assert!(
        report.sha256.is_none(),
        "sha256 must be None when checksum=false"
    );
    assert!(
        report.content_sha256.is_none(),
        "content_sha256 must be None when checksum=false"
    );
}

// ---------------------------------------------------------------------------
// 5. Roundtrip: compress with checksum, extract with both verify fields → Ok
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_verify_succeeds() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");
    let dest = TempDir::new().unwrap();

    // Compress with checksum.
    let comp_opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(src.path(), &archive, &comp_opts, nop).expect("compress should succeed");

    let artifact_hex = report.sha256.expect("sha256 should be Some");
    let content_hex = report
        .content_sha256
        .expect("content_sha256 should be Some");

    // Extract with both verify fields.
    let ext_opts = ExtractOptions {
        verify_sha256: Some(artifact_hex),
        verify_content_sha256: Some(content_hex),
        ..Default::default()
    };
    extract(&archive, dest.path(), &ext_opts, nop)
        .expect("extract with valid digests should succeed");

    // Verify tree was actually extracted.
    assert!(
        dest.path().join("a.txt").exists(),
        "a.txt should be extracted"
    );
}

// ---------------------------------------------------------------------------
// 5b. Roundtrip verify for codec-only (single file .zst)
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_verify_codec_only_succeeds() {
    let src = TempDir::new().unwrap();
    let input_data = b"test content for codec roundtrip";
    make_file(src.path(), "data.bin", input_data);
    let work = TempDir::new().unwrap();
    let archive = work.path().join("data.bin.zst");
    let dest = TempDir::new().unwrap();

    let comp_opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report = compress(
        src.path().join("data.bin").as_path(),
        &archive,
        &comp_opts,
        nop,
    )
    .expect("compress should succeed");

    let artifact_hex = report.sha256.expect("sha256 should be Some");
    let content_hex = report
        .content_sha256
        .expect("content_sha256 should be Some");

    let ext_opts = ExtractOptions {
        verify_sha256: Some(artifact_hex),
        verify_content_sha256: Some(content_hex),
        ..Default::default()
    };
    extract(&archive, dest.path(), &ext_opts, nop)
        .expect("extract with valid digests should succeed");
}

// ---------------------------------------------------------------------------
// 6. Flip one byte in artifact → ChecksumMismatch{kind:"artifact"}, dest empty
// ---------------------------------------------------------------------------

#[test]
fn artifact_mismatch_returns_error_and_dest_empty() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");

    // Compress to get a valid archive.
    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("compress should succeed");

    // Flip a byte in the middle of the archive to corrupt it.
    let mut bytes = std::fs::read(&archive).expect("read archive");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&archive, &bytes).expect("write corrupted archive");

    // Compute a fresh SHA-256 of what the file looked like BEFORE corruption.
    // Actually, we need the pre-corruption digest.  Let's recompress to get
    // the expected digest, then use it with the corrupted file.
    //
    // Simpler approach: compress again to get the canonical digest, then
    // corrupt the archive and verify against the canonical digest.
    let work2 = TempDir::new().unwrap();
    let archive2 = work2.path().join("out.tar.gz");
    let comp_opts = CompressOptions {
        checksum: true,
        ..Default::default()
    };
    let report =
        compress(src.path(), &archive2, &comp_opts, nop).expect("compress2 should succeed");
    let good_hex = report.sha256.expect("sha256 should be Some");

    // Now corrupt archive2 and try to verify with the good_hex.
    let mut bytes2 = std::fs::read(&archive2).expect("read archive2");
    let mid2 = bytes2.len() / 2;
    bytes2[mid2] ^= 0xFF;
    std::fs::write(&archive2, &bytes2).expect("write corrupted archive2");

    let dest = TempDir::new().unwrap();
    let ext_opts = ExtractOptions {
        verify_sha256: Some(good_hex),
        ..Default::default()
    };
    let err = extract(&archive2, dest.path(), &ext_opts, nop)
        .expect_err("should fail with artifact mismatch");

    assert!(
        matches!(
            err,
            Error::ChecksumMismatch {
                kind: "artifact",
                ..
            }
        ),
        "expected ChecksumMismatch{{kind:artifact}}, got {err:?}"
    );

    // Dest directory must have no files (artifact check runs before unpacking).
    let dest_files: Vec<_> = walkdir_files(dest.path());
    assert!(
        dest_files.is_empty(),
        "dest must have no files after artifact mismatch; found: {dest_files:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Wrong content digest → ChecksumMismatch{kind:"content"}
// ---------------------------------------------------------------------------

#[test]
fn content_mismatch_returns_error() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.tar.gz");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("compress should succeed");

    let ext_opts = ExtractOptions {
        verify_content_sha256: Some(
            "0000000000000000000000000000000000000000000000000000000000000000".into(),
        ),
        ..Default::default()
    };
    let err = extract(&archive, dest.path(), &ext_opts, nop)
        .expect_err("should fail with content mismatch");

    assert!(
        matches!(
            err,
            Error::ChecksumMismatch {
                kind: "content",
                ..
            }
        ),
        "expected ChecksumMismatch{{kind:content}}, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 8. zip + verify_content_sha256 → UnsupportedOperation
// ---------------------------------------------------------------------------

#[test]
fn zip_content_verify_unsupported() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.zip");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("compress should succeed");

    let ext_opts = ExtractOptions {
        verify_content_sha256: Some(
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".into(),
        ),
        ..Default::default()
    };
    let err = extract(&archive, dest.path(), &ext_opts, nop)
        .expect_err("should fail with UnsupportedOperation");

    assert!(
        matches!(err, Error::UnsupportedOperation { .. }),
        "expected UnsupportedOperation for zip + verify_content_sha256, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 8b. 7z + verify_content_sha256 → UnsupportedOperation
// ---------------------------------------------------------------------------

#[test]
fn sevenz_content_verify_unsupported() {
    let src = TempDir::new().unwrap();
    make_tree(src.path());
    let work = TempDir::new().unwrap();
    let archive = work.path().join("out.7z");
    let dest = TempDir::new().unwrap();

    compress(src.path(), &archive, &CompressOptions::default(), nop)
        .expect("compress should succeed");

    let ext_opts = ExtractOptions {
        verify_content_sha256: Some(
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".into(),
        ),
        ..Default::default()
    };
    let err = extract(&archive, dest.path(), &ext_opts, nop)
        .expect_err("should fail with UnsupportedOperation");

    assert!(
        matches!(err, Error::UnsupportedOperation { .. }),
        "expected UnsupportedOperation for 7z + verify_content_sha256, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Helper: recursively list all files under a directory
// ---------------------------------------------------------------------------

fn walkdir_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    walkdir_files_recursive(dir, &mut out);
    out
}

fn walkdir_files_recursive(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walkdir_files_recursive(&path, out);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
}
