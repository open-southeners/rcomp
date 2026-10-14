//! Integration tests for partial extraction ([`ExtractOptions::entries`]) and
//! [`add_to_archive`].
//!
//! Coverage:
//!
//! 1. Partial extraction of a nested file and a nested folder from zip,
//!    tar.gz and 7z (including a solid 7z, where skipped entries share the
//!    decoder stream with selected ones) writes only the selection, re-rooted
//!    under its own name.
//! 2. Adding files and folders at the root and into a folder, for zip,
//!    tar.zst and 7z, keeps every existing entry.
//! 3. Name clashes fail without `replace` (archive untouched) and replace
//!    the entry with it.
//! 4. A codec-only tarball is grown as a tarball; a single-stream file is
//!    refused.
//! 5. A cancelled add leaves the original archive byte-for-byte intact and no
//!    staging folder behind.
//! 6. Adding a folder that contains the archive, or using a prefix that
//!    escapes the root, is refused.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rcomp_core::{
    AddOptions, CancelToken, CompressOptions, Error, ExtractOptions, add_to_archive, compress_many,
    extract, list,
};
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

fn extract_all(archive: &Path) -> TempDir {
    let dest = TempDir::new().unwrap();
    extract(archive, dest.path(), &ExtractOptions::default(), |_| {}).expect("extract");
    dest
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

// ---------------------------------------------------------------------------
// add_to_archive
// ---------------------------------------------------------------------------

/// Create new inputs `extra.txt` and `photos/{a,b}.jpg` under `dir/new`.
fn build_additions(dir: &Path) -> Vec<PathBuf> {
    let new = dir.join("new");
    fs::create_dir_all(new.join("photos")).unwrap();
    fs::write(new.join("extra.txt"), b"extra").unwrap();
    fs::write(new.join("photos/a.jpg"), b"a").unwrap();
    fs::write(new.join("photos/b.jpg"), b"b").unwrap();
    vec![new.join("extra.txt"), new.join("photos")]
}

fn add_at_root(ext: &str) {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), ext);
    let inputs = build_additions(dir.path());

    add_to_archive(&archive, &inputs, &AddOptions::default(), |_| {}).expect("add");

    let dest = extract_all(&archive);
    assert_eq!(
        files_under(dest.path()),
        vec![
            "docs/guide/deep/x.md",
            "docs/guide/intro.md",
            "docs/notes.md",
            "extra.txt",
            "photos/a.jpg",
            "photos/b.jpg",
            "readme.txt",
        ],
        "{ext}"
    );
    // Inputs added from the root are read in place, never moved.
    assert!(inputs.iter().all(|p| p.exists()));
}

#[test]
fn add_to_zip() {
    add_at_root("zip");
}

#[test]
fn add_to_tar_zst() {
    add_at_root("tar.zst");
}

#[test]
fn add_to_7z() {
    add_at_root("7z");
}

#[test]
fn add_into_existing_and_new_folders() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "zip");
    let inputs = build_additions(dir.path());

    let opts = AddOptions {
        prefix: Some(PathBuf::from("docs/guide/")),
        ..Default::default()
    };
    add_to_archive(&archive, &inputs[..1], &opts, |_| {}).unwrap();
    let opts = AddOptions {
        prefix: Some(PathBuf::from("media/2026")),
        ..Default::default()
    };
    add_to_archive(&archive, &inputs[1..], &opts, |_| {}).unwrap();

    let dest = extract_all(&archive);
    assert_eq!(
        files_under(dest.path()),
        vec![
            "docs/guide/deep/x.md",
            "docs/guide/extra.txt",
            "docs/guide/intro.md",
            "docs/notes.md",
            "media/2026/photos/a.jpg",
            "media/2026/photos/b.jpg",
            "readme.txt",
        ]
    );
}

#[test]
fn clash_without_replace_leaves_archive_untouched() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "tar.gz");
    let before = fs::read(&archive).unwrap();

    fs::create_dir(dir.path().join("new")).unwrap();
    let clash = dir.path().join("new/readme.txt");
    fs::write(&clash, b"replaced").unwrap();

    let err = add_to_archive(
        &archive,
        std::slice::from_ref(&clash),
        &AddOptions::default(),
        |_| {},
    )
    .expect_err("clash must fail");
    match err {
        Error::AlreadyExists { path } => assert_eq!(path, Path::new("readme.txt")),
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert_eq!(fs::read(&archive).unwrap(), before);

    let opts = AddOptions {
        replace: true,
        ..Default::default()
    };
    add_to_archive(&archive, &[clash], &opts, |_| {}).unwrap();
    let dest = extract_all(&archive);
    assert_eq!(
        fs::read(dest.path().join("readme.txt")).unwrap(),
        b"replaced"
    );
    assert_eq!(files_under(dest.path()).len(), 4);
}

#[test]
fn replacing_a_folder_drops_its_old_contents() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "zip");
    let docs = dir.path().join("new/docs");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("only.md"), b"only").unwrap();

    let opts = AddOptions {
        replace: true,
        ..Default::default()
    };
    add_to_archive(&archive, &[docs], &opts, |_| {}).unwrap();
    let dest = extract_all(&archive);
    assert_eq!(files_under(dest.path()), vec!["docs/only.md", "readme.txt"]);
}

#[test]
fn codec_only_tarball_grows_as_a_tarball() {
    let dir = TempDir::new().unwrap();
    // Built as .tar.gz, then renamed so only the codec is visible.
    let tgz = build_archive(dir.path(), "tar.gz");
    let gz = dir.path().join("backup.gz");
    fs::rename(&tgz, &gz).unwrap();
    let inputs = build_additions(dir.path());

    add_to_archive(&gz, &inputs[..1], &AddOptions::default(), |_| {}).unwrap();
    let dest = extract_all(&gz);
    assert!(files_under(dest.path()).contains(&"extra.txt".to_owned()));
    assert!(files_under(dest.path()).contains(&"readme.txt".to_owned()));
}

#[test]
fn single_stream_file_is_refused() {
    let dir = TempDir::new().unwrap();
    let plain = dir.path().join("notes.txt");
    fs::write(&plain, b"hello").unwrap();
    let gz = dir.path().join("notes.txt.gz");
    rcomp_core::compress(&plain, &gz, &CompressOptions::default(), |_| {}).unwrap();
    let inputs = build_additions(dir.path());

    let err = add_to_archive(&gz, &inputs, &AddOptions::default(), |_| {}).unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedOperation { .. }),
        "got {err:?}"
    );
}

#[test]
fn cancelled_add_keeps_original_and_cleans_up() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "7z");
    let before = fs::read(&archive).unwrap();
    let inputs = build_additions(dir.path());

    let cancel = CancelToken::default();
    cancel.cancel();
    let opts = AddOptions {
        cancel,
        ..Default::default()
    };
    let err = add_to_archive(&archive, &inputs, &opts, |_| {}).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "got {err:?}");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".rcomp"))
        .collect();
    assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
}

#[test]
fn adding_the_folder_that_holds_the_archive_is_refused() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "zip");
    let err = add_to_archive(
        &archive,
        &[dir.path().to_path_buf()],
        &AddOptions::default(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io(_)), "got {err:?}");
}

#[test]
fn prefix_escaping_the_root_is_refused() {
    let dir = TempDir::new().unwrap();
    let archive = build_archive(dir.path(), "zip");
    let inputs = build_additions(dir.path());
    let opts = AddOptions {
        prefix: Some(PathBuf::from("../outside")),
        ..Default::default()
    };
    let err = add_to_archive(&archive, &inputs, &opts, |_| {}).unwrap_err();
    assert!(matches!(err, Error::PathTraversal { .. }), "got {err:?}");
}
