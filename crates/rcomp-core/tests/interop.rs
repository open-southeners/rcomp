//! Interop tests: prove that rcomp reads real-world archives created by
//! reference tools, not just its own output.
//!
//! Every test opens a committed fixture from `tests/fixtures/` and exercises
//! at least one of:
//!
//! * `detect()` — returns the expected [`Format`].
//! * `detect()` on a copy with its extension stripped — detects via magic bytes.
//! * `extract()` — decompresses to exactly the known content.
//! * `list()` — returns the expected entries (archives only).
//!
//! Fixture tiers are documented in `tests/fixtures/FIXTURES.md`.

use std::path::{Path, PathBuf};

use rcomp_core::{Codec, Container, Format, detect, extract, list};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Known content constants
// ---------------------------------------------------------------------------

/// Exact content of `sample.txt` used to build the tier-(a) codec fixtures.
///
/// Created with:
/// ```text
/// printf '%s\n' \
///   "rcomp interop test fixture" \
///   "line 1: hello from the reference tool" \
///   "line 2: deterministic content for byte comparison" \
///   "line 3: end of sample" \
///   > /tmp/sample.txt
/// ```
const SAMPLE_TXT: &[u8] = b"rcomp interop test fixture\n\
line 1: hello from the reference tool\n\
line 2: deterministic content for byte comparison\n\
line 3: end of sample\n";

/// Content of `sample_dir/sub/nested.txt` inside the tar and zip archives.
const NESTED_TXT: &[u8] = b"nested file content\n";

/// Content of `file.txt` inside `sample.7z` (from sevenz-rust2 test data).
const SEVENZ_FILE_TXT: &[u8] = b"this is a file\n";

/// First few bytes of the decompressed `ipsum.br` fixture (from
/// brotli-decompressor-5.0.1 bundled test data — created by the reference
/// Brotli encoder, not by rcomp).
const IPSUM_BR_PREFIX: &[u8] = b"Lorem ipsum dolor sit amet, consectetur adipiscing";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Absolute path to a fixture file under `tests/fixtures/`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Copy `src` to `dest_dir/<new_name>` and return the new path.
fn copy_to(src: &Path, dest_dir: &Path, new_name: &str) -> PathBuf {
    let dst = dest_dir.join(new_name);
    std::fs::copy(src, &dst).unwrap_or_else(|e| panic!("copy {:?} → {:?}: {e}", src, dst));
    dst
}

/// Default no-op progress callback.
fn nop_progress(_: &rcomp_core::Progress) {}

// ---------------------------------------------------------------------------
// Tier (a): sample.txt.gz — gzip with embedded original filename
// ---------------------------------------------------------------------------

#[test]
fn detect_gz_fixture_by_extension() {
    let path = fixture("sample.txt.gz");
    let fmt = detect(&path).expect("detect sample.txt.gz");
    assert_eq!(
        fmt,
        Format::codec(Codec::Gzip),
        "sample.txt.gz should detect as gzip"
    );
}

#[test]
fn detect_gz_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.txt.gz"), work.path(), "sample_gz_noext");
    let fmt = detect(&stripped).expect("detect extensionless gz");
    assert_eq!(
        fmt,
        Format::codec(Codec::Gzip),
        "extensionless gzip file should detect via magic bytes"
    );
}

#[test]
fn extract_gz_fixture_to_known_content() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.txt.gz"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.txt.gz");

    // gzip embedded filename is "sample.txt" — the output file should use it.
    let out = dest.path().join("sample.txt");
    assert!(out.exists(), "sample.txt should be extracted (embedded filename)");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        SAMPLE_TXT,
        "sample.txt content should match SAMPLE_TXT"
    );
}

#[test]
fn extract_gz_extensionless_honours_embedded_filename() {
    // A copy with no extension: the gzip header stores "sample.txt" so the
    // extracted file should carry that name, not fall back to stem-based naming.
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.txt.gz"), work.path(), "mystery_download");
    let dest = TempDir::new().unwrap();
    extract(&stripped, dest.path(), &Default::default(), nop_progress)
        .expect("extract extensionless gz");

    // The embedded filename "sample.txt" takes priority over stripping the
    // extension from "mystery_download" (which has no extension anyway).
    let out = dest.path().join("sample.txt");
    assert!(
        out.exists(),
        "should extract using gzip embedded filename 'sample.txt'; dest contents: {:?}",
        std::fs::read_dir(dest.path()).unwrap().collect::<Vec<_>>()
    );
    assert_eq!(std::fs::read(&out).unwrap(), SAMPLE_TXT);
}

// ---------------------------------------------------------------------------
// Tier (a): sample.txt.bz2
// ---------------------------------------------------------------------------

#[test]
fn detect_bz2_fixture_by_extension() {
    let fmt = detect(&fixture("sample.txt.bz2")).expect("detect sample.txt.bz2");
    assert_eq!(fmt, Format::codec(Codec::Bzip2));
}

#[test]
fn detect_bz2_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.txt.bz2"), work.path(), "sample_bz2_noext");
    let fmt = detect(&stripped).expect("detect extensionless bz2");
    assert_eq!(fmt, Format::codec(Codec::Bzip2));
}

#[test]
fn extract_bz2_fixture_to_known_content() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.txt.bz2"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.txt.bz2");
    // Extension stripping: "sample.txt.bz2" → "sample.txt"
    let out = dest.path().join("sample.txt");
    assert!(out.exists(), "sample.txt should exist after bz2 extraction");
    assert_eq!(std::fs::read(&out).unwrap(), SAMPLE_TXT);
}

// ---------------------------------------------------------------------------
// Tier (a): sample.txt.xz
// ---------------------------------------------------------------------------

#[test]
fn detect_xz_fixture_by_extension() {
    let fmt = detect(&fixture("sample.txt.xz")).expect("detect sample.txt.xz");
    assert_eq!(fmt, Format::codec(Codec::Xz));
}

#[test]
fn detect_xz_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.txt.xz"), work.path(), "sample_xz_noext");
    let fmt = detect(&stripped).expect("detect extensionless xz");
    assert_eq!(fmt, Format::codec(Codec::Xz));
}

#[test]
fn extract_xz_fixture_to_known_content() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.txt.xz"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.txt.xz");
    let out = dest.path().join("sample.txt");
    assert!(out.exists(), "sample.txt should exist after xz extraction");
    assert_eq!(std::fs::read(&out).unwrap(), SAMPLE_TXT);
}

// ---------------------------------------------------------------------------
// Tier (a): sample.txt.zst
// ---------------------------------------------------------------------------

#[test]
fn detect_zst_fixture_by_extension() {
    let fmt = detect(&fixture("sample.txt.zst")).expect("detect sample.txt.zst");
    assert_eq!(fmt, Format::codec(Codec::Zstd));
}

#[test]
fn detect_zst_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.txt.zst"), work.path(), "sample_zst_noext");
    let fmt = detect(&stripped).expect("detect extensionless zst");
    assert_eq!(fmt, Format::codec(Codec::Zstd));
}

#[test]
fn extract_zst_fixture_to_known_content() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.txt.zst"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.txt.zst");
    let out = dest.path().join("sample.txt");
    assert!(out.exists(), "sample.txt should exist after zst extraction");
    assert_eq!(std::fs::read(&out).unwrap(), SAMPLE_TXT);
}

// ---------------------------------------------------------------------------
// Tier (a): sample.tar — GNU tar, ≥2 entries including a subdirectory
// ---------------------------------------------------------------------------

#[test]
fn detect_tar_fixture_by_extension() {
    let fmt = detect(&fixture("sample.tar")).expect("detect sample.tar");
    assert_eq!(fmt, Format::container(Container::Tar));
}

#[test]
fn detect_tar_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.tar"), work.path(), "sample_tar_noext");
    let fmt = detect(&stripped).expect("detect extensionless tar");
    assert_eq!(
        fmt,
        Format::container(Container::Tar),
        "extensionless tar should detect via ustar magic at offset 257"
    );
}

#[test]
fn list_tar_fixture_returns_expected_entries() {
    let entries = list(&fixture("sample.tar")).expect("list sample.tar");
    let paths: std::collections::HashSet<PathBuf> =
        entries.iter().map(|e| e.path.clone()).collect();

    // Verify that both the subdirectory and files are present.
    assert!(
        paths.iter().any(|p| p.ends_with("sample.txt")),
        "sample.tar list should contain sample_dir/sample.txt; got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("nested.txt")),
        "sample.tar list should contain sample_dir/sub/nested.txt; got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("sub") || p.ends_with("sub/")),
        "sample.tar list should contain the sub/ directory; got {paths:?}"
    );
}

#[test]
fn extract_tar_fixture_produces_correct_tree() {
    let dest = TempDir::new().unwrap();
    let report = extract(&fixture("sample.tar"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.tar");

    assert!(report.entries > 0, "should have extracted at least one entry");

    let sample_txt = dest.path().join("sample_dir/sample.txt");
    assert!(sample_txt.exists(), "sample_dir/sample.txt should exist");
    assert_eq!(
        std::fs::read(&sample_txt).unwrap(),
        SAMPLE_TXT,
        "sample.txt content mismatch"
    );

    let nested_txt = dest.path().join("sample_dir/sub/nested.txt");
    assert!(nested_txt.exists(), "sample_dir/sub/nested.txt should exist");
    assert_eq!(
        std::fs::read(&nested_txt).unwrap(),
        NESTED_TXT,
        "nested.txt content mismatch"
    );
}

// ---------------------------------------------------------------------------
// Tier (a): sample.tar.gz
// ---------------------------------------------------------------------------

#[test]
fn detect_tar_gz_fixture_by_extension() {
    let fmt = detect(&fixture("sample.tar.gz")).expect("detect sample.tar.gz");
    assert_eq!(fmt, Format::layered(Container::Tar, Codec::Gzip));
}

#[test]
fn detect_tar_gz_fixture_extensionless_by_magic() {
    // The extensionless copy looks like a plain gzip stream (magic = 1F 8B);
    // without the .tar.gz extension, rcomp cannot refine to the layered format.
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.tar.gz"), work.path(), "sample_tar_gz_noext");
    let fmt = detect(&stripped).expect("detect extensionless tar.gz");
    // Magic bytes identify gzip; extension refinement to tar.gz is not possible
    // without the extension, so the result is codec-only gzip.
    assert_eq!(
        fmt,
        Format::codec(Codec::Gzip),
        "extensionless tar.gz detects as gzip (magic only, no extension to refine)"
    );
}

#[test]
fn list_tar_gz_fixture_returns_expected_entries() {
    let entries = list(&fixture("sample.tar.gz")).expect("list sample.tar.gz");
    let paths: std::collections::HashSet<PathBuf> =
        entries.iter().map(|e| e.path.clone()).collect();

    assert!(
        paths.iter().any(|p| p.ends_with("sample.txt")),
        "sample.tar.gz list should contain sample.txt; got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("nested.txt")),
        "sample.tar.gz list should contain nested.txt; got {paths:?}"
    );
}

#[test]
fn extract_tar_gz_fixture_produces_correct_tree() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.tar.gz"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.tar.gz");

    let sample_txt = dest.path().join("sample_dir/sample.txt");
    assert!(sample_txt.exists(), "sample_dir/sample.txt should exist after tar.gz extract");
    assert_eq!(std::fs::read(&sample_txt).unwrap(), SAMPLE_TXT);

    let nested_txt = dest.path().join("sample_dir/sub/nested.txt");
    assert!(nested_txt.exists(), "sample_dir/sub/nested.txt should exist after tar.gz extract");
    assert_eq!(std::fs::read(&nested_txt).unwrap(), NESTED_TXT);
}

// ---------------------------------------------------------------------------
// Tier (a): sample.zip — Info-ZIP, ≥2 entries including a subdirectory
// ---------------------------------------------------------------------------

#[test]
fn detect_zip_fixture_by_extension() {
    let fmt = detect(&fixture("sample.zip")).expect("detect sample.zip");
    assert_eq!(fmt, Format::container(Container::Zip));
}

#[test]
fn detect_zip_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.zip"), work.path(), "sample_zip_noext");
    let fmt = detect(&stripped).expect("detect extensionless zip");
    assert_eq!(
        fmt,
        Format::container(Container::Zip),
        "extensionless zip should detect via PK magic bytes"
    );
}

#[test]
fn list_zip_fixture_returns_expected_entries() {
    let entries = list(&fixture("sample.zip")).expect("list sample.zip");
    let names: std::collections::HashSet<String> = entries
        .iter()
        .map(|e| e.path.to_string_lossy().into_owned())
        .collect();

    assert!(
        names.contains("sample_dir/sample.txt"),
        "sample.zip list should contain sample_dir/sample.txt; got {names:?}"
    );
    assert!(
        names.contains("sample_dir/sub/nested.txt"),
        "sample.zip list should contain sample_dir/sub/nested.txt; got {names:?}"
    );
}

#[test]
fn extract_zip_fixture_produces_correct_tree() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.zip"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.zip");

    let sample_txt = dest.path().join("sample_dir/sample.txt");
    assert!(sample_txt.exists(), "sample_dir/sample.txt should exist after zip extract");
    assert_eq!(std::fs::read(&sample_txt).unwrap(), SAMPLE_TXT);

    let nested_txt = dest.path().join("sample_dir/sub/nested.txt");
    assert!(nested_txt.exists(), "sample_dir/sub/nested.txt should exist after zip extract");
    assert_eq!(std::fs::read(&nested_txt).unwrap(), NESTED_TXT);
}

// ---------------------------------------------------------------------------
// Tier (b): sample.7z — sevenz-rust2 bundled test fixture (LZMA)
//           Origin: sevenz-rust2-0.21.0/tests/resources/single_file_with_content_lzma.7z
//           Known content: file.txt = "this is a file\n"
// ---------------------------------------------------------------------------

#[test]
fn detect_7z_fixture_by_extension() {
    let fmt = detect(&fixture("sample.7z")).expect("detect sample.7z");
    assert_eq!(fmt, Format::container(Container::SevenZ));
}

#[test]
fn detect_7z_fixture_extensionless_by_magic() {
    let work = TempDir::new().unwrap();
    let stripped = copy_to(&fixture("sample.7z"), work.path(), "sample_7z_noext");
    let fmt = detect(&stripped).expect("detect extensionless 7z");
    assert_eq!(
        fmt,
        Format::container(Container::SevenZ),
        "extensionless 7z should detect via 7z magic bytes"
    );
}

#[test]
fn list_7z_fixture_returns_expected_entry() {
    let entries = list(&fixture("sample.7z")).expect("list sample.7z");
    assert_eq!(entries.len(), 1, "sample.7z should list exactly 1 entry; got {entries:?}");
    assert_eq!(
        entries[0].path,
        Path::new("file.txt"),
        "entry should be named file.txt"
    );
    assert!(!entries[0].is_dir, "file.txt should not be a directory");
    // size = len("this is a file\n") = 15
    assert_eq!(entries[0].size, 15, "file.txt should be 15 bytes");
}

#[test]
fn extract_7z_fixture_to_known_content() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("sample.7z"), dest.path(), &Default::default(), nop_progress)
        .expect("extract sample.7z");

    let out = dest.path().join("file.txt");
    assert!(out.exists(), "file.txt should exist after 7z extraction");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        SEVENZ_FILE_TXT,
        "file.txt content should be 'this is a file\\n'"
    );
}

// ---------------------------------------------------------------------------
// Tier (b): ipsum.br — brotli-decompressor-5.0.1 bundled fixture
//           Origin: brotli-decompressor-5.0.1/src/bin/ipsum.brotli
//           (created by the reference Brotli encoder, not by rcomp)
//           Brotli has no magic bytes — no extensionless magic-byte test.
// ---------------------------------------------------------------------------

/// Brotli has no magic bytes, so extension-based detection is the only
/// reliable path.  The fixture carries the `.br` extension.
#[test]
fn detect_br_fixture_by_extension_only() {
    let fmt = detect(&fixture("ipsum.br")).expect("detect ipsum.br");
    assert_eq!(
        fmt,
        Format::codec(Codec::Brotli),
        "ipsum.br should detect as brotli via extension"
    );
}

#[test]
fn extract_br_fixture_to_known_prefix() {
    let dest = TempDir::new().unwrap();
    extract(&fixture("ipsum.br"), dest.path(), &Default::default(), nop_progress)
        .expect("extract ipsum.br");

    // Extension stripped: "ipsum.br" → "ipsum"
    let out = dest.path().join("ipsum");
    assert!(out.exists(), "'ipsum' should exist after .br extraction");

    let content = std::fs::read(&out).unwrap();
    // Verify the decompressed content starts with the expected Lorem ipsum prefix.
    assert!(
        content.starts_with(IPSUM_BR_PREFIX),
        "decompressed ipsum.br should start with Lorem ipsum prefix; got {:?}",
        &content[..IPSUM_BR_PREFIX.len().min(content.len())]
    );
    // Full decompressed size as measured from ipsum.raw is 5018 bytes.
    assert_eq!(
        content.len(),
        5018,
        "decompressed ipsum.br should be 5018 bytes"
    );
}
