//! CLI integration tests for `.gitignore`-aware compression and `--all` /
//! `--exclude` flags.
//!
//! Each test builds a scratch directory tree with a `.gitignore` file and
//! drives the `rcomp` binary via `assert_cmd`, then verifies the resulting
//! archive contents using `rcomp ls`.
//!
//! Conventions mirror `tests/cli.rs`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return an [`assert_cmd::Command`] configured to run the `rcomp` binary.
fn rcomp() -> Command {
    Command::cargo_bin("rcomp").expect("rcomp binary must exist")
}

/// Build a tree with a `.gitignore` that ignores `*.log` and `target/`.
///
/// Layout:
/// ```text
/// <root>/
///   .gitignore      ("target/\n*.log\n")
///   keep.txt        ("keep\n")
///   ignore.log      ("ignored\n")
///   target/
///     build.out     ("ignored dir entry\n")
///   sub/
///     nested.txt    ("nested\n")
/// ```
fn make_gitignore_tree(parent: &TempDir) -> PathBuf {
    let root = parent.path().join("gitignore_tree");
    fs::create_dir(&root).unwrap();
    fs::write(root.join(".gitignore"), b"target/\n*.log\n").unwrap();
    fs::write(root.join("keep.txt"), b"keep\n").unwrap();
    fs::write(root.join("ignore.log"), b"ignored\n").unwrap();
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("build.out"), b"ignored dir entry\n").unwrap();
    let sub = root.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("nested.txt"), b"nested\n").unwrap();
    root
}

/// List the `rcomp ls` output for an archive as lines of path strings.
fn ls_paths(archive: &Path) -> Vec<String> {
    let out = rcomp()
        .args(["ls", archive.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    text.lines()
        .filter_map(|l| {
            // Format: `{size:>12}  {path}` — skip empty lines.
            if l.len() < 14 {
                return None;
            }
            Some(l[14..].trim_end().to_owned())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test: default compress excludes .gitignore-matched paths + note on stderr
// ---------------------------------------------------------------------------

#[test]
fn default_compress_excludes_gitignored_and_shows_note() {
    let tmp = TempDir::new().unwrap();
    let root = make_gitignore_tree(&tmp);
    let out = tmp.path().join("gitignore_default.tar.gz");

    let stderr = rcomp()
        .args([root.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();

    // The excluded-count note must appear on stderr.
    assert!(
        err_text.contains("excluded") && err_text.contains(".gitignore"),
        "stderr should mention excluded paths via .gitignore; got: {err_text:?}"
    );

    // The archived paths must NOT include ignore.log or anything under target/.
    let paths = ls_paths(&out);
    assert!(
        !paths.iter().any(|p| p.contains("ignore.log")),
        "ignore.log should be excluded; paths: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("target")),
        "target/ should be excluded; paths: {paths:?}"
    );
    // keep.txt and sub/nested.txt must be present.
    assert!(
        paths.iter().any(|p| p.contains("keep.txt")),
        "keep.txt should be archived; paths: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.contains("nested.txt")),
        "nested.txt should be archived; paths: {paths:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: --all includes everything (no note emitted)
// ---------------------------------------------------------------------------

#[test]
fn all_includes_everything_no_note() {
    let tmp = TempDir::new().unwrap();
    let root = make_gitignore_tree(&tmp);
    let out = tmp.path().join("gitignore_all.tar.gz");

    let stderr = rcomp()
        .args([root.to_str().unwrap(), out.to_str().unwrap(), "--all"])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    assert!(
        !err_text.contains("excluded"),
        "stderr must not mention excluded paths with --all; got: {err_text:?}"
    );

    let paths = ls_paths(&out);
    // Everything is included.
    assert!(
        paths.iter().any(|p| p.contains("ignore.log")),
        "ignore.log should be included with --all; paths: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.contains("target")),
        "target/ should be included with --all; paths: {paths:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: --exclude alone filters matching paths (gitignore still active)
// ---------------------------------------------------------------------------

#[test]
fn exclude_alone_filters_matching_paths() {
    let tmp = TempDir::new().unwrap();
    let root = make_gitignore_tree(&tmp);
    let out = tmp.path().join("exclude_only.tar.gz");

    let stderr = rcomp()
        .args([
            root.to_str().unwrap(),
            out.to_str().unwrap(),
            "--exclude",
            "*.txt",
        ])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    // Both gitignore and --exclude are active.
    assert!(
        err_text.contains("excluded") && err_text.contains(".gitignore") && err_text.contains("--exclude"),
        "stderr should mention both .gitignore and --exclude; got: {err_text:?}"
    );

    let paths = ls_paths(&out);
    // .txt files excluded via --exclude.
    assert!(
        !paths.iter().any(|p| p.ends_with(".txt")),
        "*.txt should be excluded; paths: {paths:?}"
    );
    // .log excluded via gitignore.
    assert!(
        !paths.iter().any(|p| p.ends_with(".log")),
        ".log files should be gitignore-excluded; paths: {paths:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: --all --exclude works (gitignore off, but exclude still applied)
// ---------------------------------------------------------------------------

#[test]
fn all_with_exclude_applies_exclude_without_gitignore() {
    let tmp = TempDir::new().unwrap();
    let root = make_gitignore_tree(&tmp);
    let out = tmp.path().join("all_exclude.tar.gz");

    let stderr = rcomp()
        .args([
            root.to_str().unwrap(),
            out.to_str().unwrap(),
            "--all",
            "--exclude",
            "*.log",
        ])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    // Only --exclude is active (gitignore off via --all).
    assert!(
        err_text.contains("excluded") && err_text.contains("--exclude"),
        "stderr should mention --exclude; got: {err_text:?}"
    );
    assert!(
        !err_text.contains(".gitignore"),
        "stderr must not mention .gitignore when --all is given; got: {err_text:?}"
    );

    let paths = ls_paths(&out);
    // .log excluded via --exclude.
    assert!(
        !paths.iter().any(|p| p.ends_with(".log")),
        "*.log should be excluded via --exclude; paths: {paths:?}"
    );
    // target/ should now be present (gitignore disabled by --all).
    assert!(
        paths.iter().any(|p| p.contains("target")),
        "target/ should be included when --all given; paths: {paths:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: no exclusions → no note on stderr
// ---------------------------------------------------------------------------

#[test]
fn no_gitignore_file_no_note() {
    let tmp = TempDir::new().unwrap();
    // A plain directory with no .gitignore → nothing excluded → no note.
    let root = tmp.path().join("plain_dir");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.txt"), b"a\n").unwrap();
    fs::write(root.join("b.txt"), b"b\n").unwrap();

    let out = tmp.path().join("plain.tar.gz");

    let stderr = rcomp()
        .args([root.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();

    let err_text = String::from_utf8(stderr).unwrap();
    assert!(
        !err_text.contains("excluded"),
        "stderr must not mention excluded paths when nothing is excluded; got: {err_text:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: bad glob → exit 2
// ---------------------------------------------------------------------------

#[test]
fn bad_glob_exits_2() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("bad_glob_dir");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), b"x").unwrap();
    let out = tmp.path().join("bad_glob.tar.gz");

    // An unclosed character class is always an invalid glob.
    rcomp()
        .args([
            root.to_str().unwrap(),
            out.to_str().unwrap(),
            "--exclude",
            "[invalid",
        ])
        .assert()
        .failure()
        .code(2);
}

// ---------------------------------------------------------------------------
// Test: --all on an extract invocation → exit 2
// ---------------------------------------------------------------------------

#[test]
fn all_on_extract_exits_2() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src.txt");
    fs::write(&src, b"all flag extract test").unwrap();
    let out = tmp.path().join("src.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // --all on an extract operation is a usage error.
    rcomp()
        .args([out.to_str().unwrap(), "--all"])
        .assert()
        .failure()
        .code(2);
}

// ---------------------------------------------------------------------------
// Test: --exclude on an extract invocation → exit 2
// ---------------------------------------------------------------------------

#[test]
fn exclude_on_extract_exits_2() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("ex.txt");
    fs::write(&src, b"exclude on extract test").unwrap();
    let out = tmp.path().join("ex.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // --exclude on an extract operation is a usage error.
    rcomp()
        .args([out.to_str().unwrap(), "--exclude", "*.txt"])
        .assert()
        .failure()
        .code(2);
}

// ---------------------------------------------------------------------------
// Test: multiple --exclude globs work together
// ---------------------------------------------------------------------------

#[test]
fn multiple_excludes_work_together() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("multi_exclude_dir");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep.rs"), b"rust file\n").unwrap();
    fs::write(root.join("drop.txt"), b"text file\n").unwrap();
    fs::write(root.join("drop.log"), b"log file\n").unwrap();

    let out = tmp.path().join("multi_excl.tar.gz");

    rcomp()
        .args([
            root.to_str().unwrap(),
            out.to_str().unwrap(),
            "--all",        // no .gitignore interference
            "--exclude",
            "*.txt",
            "--exclude",
            "*.log",
        ])
        .assert()
        .success();

    let paths = ls_paths(&out);
    assert!(
        paths.iter().any(|p| p.contains("keep.rs")),
        "keep.rs should be archived; paths: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("drop.txt")),
        "drop.txt should be excluded; paths: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("drop.log")),
        "drop.log should be excluded; paths: {paths:?}"
    );
}
