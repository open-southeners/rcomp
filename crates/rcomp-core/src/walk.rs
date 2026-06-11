//! Shared filtered directory walker for [`crate::ops::compress`].
//!
//! All three archive backends (tar, zip, 7z) and the silent-tar codec path
//! delegate directory traversal to this module.  A single call to [`collect`]
//! produces a deterministic, sorted, filtered list of entries together with
//! the total included byte count — so every backend uses the same snapshot and
//! `bytes_total` in progress reporting reflects *only* the files that will
//! actually be archived.
//!
//! # Filtering
//!
//! Two independent filter sources are supported:
//!
//! 1. **`.gitignore` rules** (controlled by [`WalkOptions::follow_gitignore`]):
//!    when `true` (the default) the walker reads the origin folder's own
//!    `.gitignore` and any nested `.gitignore` files, following full git
//!    semantics via the [`ignore`] crate.  The `.git` directory is also pruned.
//!    Parent-directory gitignore files, the global gitignore, and
//!    `.git/info/exclude` are deliberately **not** consulted — archives must
//!    be reproducible from the folder alone.
//!
//! 2. **Exclude globs** ([`WalkOptions::exclude`]): gitignore-style patterns
//!    matched relative to the origin folder, applied independently of
//!    `follow_gitignore`.  An invalid glob returns [`Error::InvalidGlob`].
//!
//! # Entry order
//!
//! Entries are yielded in deterministic sorted order (by file name, depth-first)
//! via [`ignore::WalkBuilder::sort_by_file_name`].
//!
//! # Excluded count
//!
//! When any filtering is active, [`collect`] does a second unfiltered walk to
//! count the total entries and subtracts the included count.  When no filtering
//! is in effect (no `.gitignore` anywhere and no exclude patterns), the second
//! walk is skipped and [`WalkResult::excluded`] is `0`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use ignore::{WalkBuilder, overrides::OverrideBuilder};

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options that control how [`collect`] walks a directory.
#[derive(Debug)]
pub struct WalkOptions<'a> {
    /// When `true`, honour `.gitignore` files and exclude the `.git` directory.
    ///
    /// Setting to `false` is equivalent to `--all`: every file is included
    /// regardless of `.gitignore` rules, though [`exclude`] patterns still
    /// apply.
    ///
    /// [`exclude`]: Self::exclude
    pub follow_gitignore: bool,
    /// Additional gitignore-style glob patterns to exclude, matched relative
    /// to the root directory.  Applied even when `follow_gitignore` is `false`.
    ///
    /// An invalid glob causes [`collect`] to return [`Error::InvalidGlob`].
    pub exclude: &'a [String],
}

/// A single entry produced by [`collect`].
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Absolute path on disk.
    pub abs: PathBuf,
    /// Path relative to the walk root (the root itself is never included).
    pub rel: PathBuf,
    /// `true` for directories, `false` for files and symlinks.
    pub is_dir: bool,
    /// Size of the file in bytes; `0` for directories and symlinks.
    pub size: u64,
}

/// Output of [`collect`].
#[derive(Debug)]
pub struct WalkResult {
    /// All included entries, sorted deterministically.
    pub entries: Vec<WalkEntry>,
    /// Sum of [`WalkEntry::size`] for included files only (not dirs/symlinks).
    ///
    /// This is the value used to initialise `bytes_total` in progress
    /// reporting so that filtered-out artefacts do not inflate the denominator.
    pub bytes_total: u64,
    /// Number of filesystem entries under the root that were *not* included.
    ///
    /// Computed as the unfiltered entry count minus the included entry count.
    /// When no filtering is active, this is `0` and no second walk is done.
    pub excluded: u64,
}

// ---------------------------------------------------------------------------
// collect
// ---------------------------------------------------------------------------

/// Walk `root`, applying `.gitignore` rules and/or exclude globs according to
/// `opts`, and return the filtered list of entries.
///
/// The root directory itself is **not** included in `entries`.
///
/// # Errors
///
/// - [`Error::InvalidGlob`] — one of `opts.exclude` patterns is syntactically
///   invalid.
/// - [`Error::Io`] — a directory entry could not be read.
pub(crate) fn collect(root: &Path, opts: &WalkOptions<'_>) -> Result<WalkResult> {
    // --- Build optional exclude override ---
    //
    // OverrideBuilder semantics (from the `ignore` docs):
    //   - A plain glob (no `!`) is a **whitelist** match.
    //   - A `!`-prefixed glob is an **ignore/exclude** match.
    //
    // So `!*.log` means "ignore files matching *.log" — exactly what we want
    // for user-supplied exclude patterns.
    let has_excludes = !opts.exclude.is_empty();
    let overrides = if has_excludes {
        let mut ob = OverrideBuilder::new(root);
        for pattern in opts.exclude {
            // Prefix with `!` to give the pattern exclude (not whitelist) semantics.
            let negated = format!("!{pattern}");
            ob.add(&negated).map_err(|e| Error::InvalidGlob {
                pattern: pattern.clone(),
                message: e.to_string(),
            })?;
        }
        Some(ob.build().map_err(|e| Error::InvalidGlob {
            pattern: opts.exclude.join(", "),
            message: e.to_string(),
        })?)
    } else {
        None
    };

    // Filtering is active when either gitignore following is on OR exclude
    // globs were supplied.  We only do the second (unfiltered) walk when at
    // least one filter is active, because only then can the count differ.
    let filtering_active = opts.follow_gitignore || has_excludes;

    // --- Build the WalkBuilder ---
    let entries = build_walk(root, opts, overrides.as_ref())?;

    // --- Second walk for excluded count ---
    let excluded = if filtering_active {
        let total = count_all_entries(root)?;
        total.saturating_sub(entries.len() as u64)
    } else {
        0
    };

    let bytes_total: u64 = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();

    Ok(WalkResult {
        entries,
        bytes_total,
        excluded,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the filtered walk and collect entries.
fn build_walk(
    root: &Path,
    opts: &WalkOptions<'_>,
    overrides: Option<&ignore::overrides::Override>,
) -> Result<Vec<WalkEntry>> {
    let follow_gitignore = opts.follow_gitignore;

    let mut builder = WalkBuilder::new(root);

    // Include dotfiles — only gitignore rules and `.git` exclude things.
    builder.hidden(false);
    // Do NOT read parent-directory .gitignore files; archives must be
    // reproducible from the folder alone.
    builder.parents(false);
    // No global gitignore (machine-local, non-reproducible).
    builder.git_global(false);
    // No .git/info/exclude (machine-local, non-reproducible).
    builder.git_exclude(false);
    // No .ignore files — only .gitignore semantics are supported.
    builder.ignore(false);
    // Honour .gitignore only when follow_gitignore is set.
    builder.git_ignore(follow_gitignore);
    // A .gitignore is respected even without a .git directory present.
    builder.require_git(false);
    // Do not follow symlinks.
    builder.follow_links(false);
    // Deterministic order.
    builder.sort_by_file_name(|a, b| a.cmp(b));

    // Apply user-supplied exclude overrides when present.
    if let Some(ov) = overrides {
        builder.overrides(ov.clone());
    }

    // Prune .git when following gitignore (under --all it stays in).
    if follow_gitignore {
        builder.filter_entry(|entry| {
            // filter_entry returns true to include, false to prune.
            entry.file_name() != ".git"
        });
    }

    let mut result: Vec<WalkEntry> = Vec::new();

    for entry_result in builder.build() {
        let entry = entry_result.map_err(|e| {
            // ignore::Error wraps std::io::Error when it is an I/O failure.
            // Preserve the original message by constructing a new io::Error
            // with the same kind and the original error as the source, so the
            // caller sees the full context rather than just the error kind.
            if let Some(io_err) = e.io_error() {
                Error::Io(io::Error::new(io_err.kind(), io_err.to_string()))
            } else {
                Error::Io(io::Error::other(e.to_string()))
            }
        })?;

        // The root itself is always the first entry; skip it.
        if entry.depth() == 0 {
            continue;
        }

        let abs = entry.path().to_path_buf();
        let rel = abs
            .strip_prefix(root)
            .map_err(|_| Error::Io(std::io::Error::other("entry path not under root")))?
            .to_path_buf();

        // Use symlink_metadata so symlinks are not followed.
        let meta = fs::symlink_metadata(&abs)?;
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };

        result.push(WalkEntry {
            abs,
            rel,
            is_dir,
            size,
        });
    }

    Ok(result)
}

/// Count every filesystem entry under `root` without any filtering.
///
/// Used to compute the excluded count.
fn count_all_entries(root: &Path) -> Result<u64> {
    let mut count: u64 = 0;
    count_recursive(root, &mut count)?;
    Ok(count)
}

fn count_recursive(dir: &Path, count: &mut u64) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        *count += 1;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            count_recursive(&entry.path(), count)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use crate::Error;

    use super::{WalkOptions, collect};

    fn make_tree(root: &Path) {
        // root/a.txt, root/.hidden, root/sub/b.txt, root/sub/c.log
        std::fs::write(root.join("a.txt"), b"aaa").unwrap();
        std::fs::write(root.join(".hidden"), b"dot").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), b"bbb").unwrap();
        std::fs::write(root.join("sub/c.log"), b"ccc").unwrap();
    }

    // -----------------------------------------------------------------------
    // Exclude-glob semantics verification
    //
    // This test is the focused verification that `!pattern` in the
    // OverrideBuilder gives exclude (not whitelist) semantics: a file matching
    // `!*.log` must be absent from the result, and all other files must be
    // present.
    // -----------------------------------------------------------------------

    #[test]
    fn exclude_glob_removes_matching_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);

        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &["*.log".to_string()],
        };
        let result = collect(root, &opts).unwrap();

        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();

        // a.txt and .hidden and sub/b.txt must be present.
        assert!(
            paths.iter().any(|p| p == Path::new("a.txt")),
            "a.txt must be included; got {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == Path::new(".hidden")),
            ".hidden must be included; got {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == Path::new("sub/b.txt")),
            "sub/b.txt must be included; got {paths:?}"
        );

        // sub/c.log must be excluded.
        assert!(
            !paths.iter().any(|p| p == Path::new("sub/c.log")),
            "sub/c.log must be excluded; got {paths:?}"
        );
    }

    #[test]
    fn exclude_glob_with_gitignore_off_removes_matching() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        // Even without gitignore, exclude globs work.
        // Use `*.txt` to exclude txt files from sub/ — this is a clear
        // file-based pattern with no ambiguity about directory entries.
        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &["sub/b.txt".to_string()],
        };
        let result = collect(root, &opts).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();
        // sub/b.txt must be excluded.
        assert!(
            !paths.iter().any(|p| p == Path::new("sub/b.txt")),
            "sub/b.txt must be excluded; got {paths:?}"
        );
        // a.txt and .hidden must still be present.
        assert!(
            paths.iter().any(|p| p == Path::new("a.txt")),
            "a.txt must be present; got {paths:?}"
        );
    }

    #[test]
    fn invalid_glob_returns_invalid_glob_error() {
        let tmp = TempDir::new().unwrap();
        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &["[bad".to_string()],
        };
        let err = collect(tmp.path(), &opts).unwrap_err();
        assert!(
            matches!(err, Error::InvalidGlob { .. }),
            "expected InvalidGlob, got {err:?}"
        );
    }

    #[test]
    fn no_filtering_excluded_is_zero() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        assert_eq!(
            result.excluded, 0,
            "excluded must be 0 when no filtering is active"
        );
    }

    #[test]
    fn hidden_files_included_by_default() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();
        assert!(
            paths.iter().any(|p| p == Path::new(".hidden")),
            ".hidden must be included; got {paths:?}"
        );
    }

    #[test]
    fn gitignore_excludes_matching_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        // Write a .gitignore that ignores *.log
        std::fs::write(root.join(".gitignore"), b"*.log\n").unwrap();

        let opts = WalkOptions {
            follow_gitignore: true,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();

        // a.txt and .hidden and sub/b.txt and .gitignore must be present.
        assert!(paths.iter().any(|p| p == Path::new("a.txt")));
        assert!(paths.iter().any(|p| p == Path::new(".hidden")));
        assert!(paths.iter().any(|p| p == Path::new("sub/b.txt")));

        // sub/c.log must be excluded via .gitignore.
        assert!(
            !paths.iter().any(|p| p == Path::new("sub/c.log")),
            "sub/c.log must be excluded by .gitignore; got {paths:?}"
        );

        // excluded count should be > 0
        assert!(
            result.excluded > 0,
            "excluded should be > 0 when .gitignore filters something"
        );
    }

    #[test]
    fn git_dir_pruned_when_follow_gitignore_true() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        // Simulate a .git dir
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();

        let opts = WalkOptions {
            follow_gitignore: true,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();

        // Nothing under .git should appear.
        assert!(
            paths.iter().all(|p| !p.starts_with(".git")),
            ".git must be pruned; got {paths:?}"
        );
    }

    #[test]
    fn git_dir_included_when_follow_gitignore_false() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        make_tree(root);
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();

        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| e.rel.clone()).collect();

        assert!(
            paths.iter().any(|p| p.starts_with(".git")),
            ".git must be included when follow_gitignore=false; got {paths:?}"
        );
    }

    #[test]
    fn bytes_total_is_sum_of_included_file_sizes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), b"hello").unwrap(); // 5 bytes
        std::fs::write(root.join("b.txt"), b"world!").unwrap(); // 6 bytes

        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        assert_eq!(result.bytes_total, 11, "bytes_total should be 5+6=11");
    }

    #[test]
    fn sorted_order_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("z.txt"), b"z").unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("m.txt"), b"m").unwrap();

        let opts = WalkOptions {
            follow_gitignore: false,
            exclude: &[],
        };
        let result = collect(root, &opts).unwrap();
        let names: Vec<_> = result
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.rel.to_string_lossy().into_owned())
            .collect();

        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "entries must be in sorted order");
    }
}
