//! Integration tests for partial extraction ([`ExtractOptions::entries`]).
//!
//! Coverage:
//!
//! 1. Partial extraction of a nested file and a nested folder from zip,
//!    tar.gz and 7z (including a solid 7z, where skipped entries share the
//!    decoder stream with selected ones) writes only the selection, re-rooted
//!    under its own name.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rcomp_core::{CompressOptions, ExtractOptions, compress_many, extract, list};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an archive at `<dir>/archive.<ext>` holding:
///
/// ```text
/// readme.txt          ("readme")
/// docs/
///   guide/intro.md    ("intro")
///   guide/deep/x.md   ("x")
///   notes.md          ("notes")
/// ```
fn build_archive(dir: &Path, ext: &str) -> PathBuf {
    let src = dir.join("src");
    fs::create_dir_all(src.join("docs/guide/deep")).unwrap();
    fs::write(src.join("readme.txt"), b"readme").unwrap();
    fs::write(src.join("docs/guide/intro.md"), b"intro").unwrap();
    fs::write(src.join("docs/guide/deep/x.md"), b"x").unwrap();
    fs::write(src.join("docs/notes.md"), b"notes").unwrap();

    let archive = dir.join(format!("archive.{ext}"));
    let opts = CompressOptions {
        follow_gitignore: false,
        ..Default::default()
    };
    compress_many(
        &[src.join("readme.txt"), src.join("docs")],
        &archive,
        &opts,
        |_| {},
    )
    .expect("build archive");
    fs::remove_dir_all(&src).unwrap();
    archive
}

/// Every regular file under `root`, as sorted `/`-separated relative paths.
fn files_under(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                let parts: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
                out.push(parts.join("/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn extract_only(archive: &Path, entries: &[&str]) -> TempDir {
    let dest = TempDir::new().unwrap();
    let opts = ExtractOptions {
        entries: entries.iter().map(PathBuf::from).collect(),
        ..Default::default()
    };
    extract(archive, dest.path(), &opts, |_| {}).expect("partial extract");
    dest
}

// ---------------------------------------------------------------------------
// Partial extraction
// ---------------------------------------------------------------------------

fn partial_extract_roundtrip(ext: &str) {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), ext);

    let dest = extract_only(&archive, &["docs/guide/intro.md"]);
    assert_eq!(files_under(dest.path()), vec!["intro.md"]);
    assert_eq!(fs::read(dest.path().join("intro.md")).unwrap(), b"intro");

    let dest = extract_only(&archive, &["docs/guide/", "readme.txt"]);
    assert_eq!(
        files_under(dest.path()),
        vec!["guide/deep/x.md", "guide/intro.md", "readme.txt"]
    );
    assert_eq!(fs::read(dest.path().join("guide/deep/x.md")).unwrap(), b"x");
}

#[test]
fn partial_extract_zip() {
    partial_extract_roundtrip("zip");
}

#[test]
fn partial_extract_tar_gz() {
    partial_extract_roundtrip("tar.gz");
}

#[test]
fn partial_extract_7z() {
    partial_extract_roundtrip("7z");
}

/// Selecting an entry that `list` reports must work verbatim, whatever
/// spelling the backend uses.
#[test]
fn partial_extract_accepts_listed_paths() {
    for ext in ["zip", "tar.zst", "7z"] {
        let dir = TempDir::new().unwrap();
        let archive = build_archive(dir.path(), ext);
        let notes = list(&archive)
            .unwrap()
            .into_iter()
            .find(|e| e.path.ends_with("notes.md"))
            .expect("notes.md listed");
        let dest = TempDir::new().unwrap();
        let opts = ExtractOptions {
            entries: vec![notes.path],
            ..Default::default()
        };
        extract(&archive, dest.path(), &opts, |_| {}).unwrap();
        assert_eq!(files_under(dest.path()), vec!["notes.md"], "{ext}");
    }
}

#[test]
fn partial_extract_of_unknown_entry_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "zip");
    let dest = extract_only(&archive, &["missing.txt"]);
    assert!(files_under(dest.path()).is_empty());
}
