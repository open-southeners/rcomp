//! Archive entry-path sanitizer.
//!
//! Validates archive entry paths and symlink targets against path-traversal
//! attacks (zip-slip / tar-slip) before any bytes are written to disk.
//!
//! # Security model
//!
//! All resolved paths **must** remain inside `dest`.  Any entry or link target
//! that would escape `dest` is rejected with [`Error::PathTraversal`].
//!
//! [`sanitize_entry_path`] and [`sanitize_link_target`] are **lexical** — no
//! filesystem I/O is performed — so they are safe to call before creating any
//! files.  A lexical check alone cannot see symlinks already on disk, though:
//! an archive can plant `d/up -> ..` and `s -> d/up/..` (each lexically
//! inside `dest`) and then write `s/file`, landing outside `dest`.  The
//! filesystem guards [`prepare_dir`] and [`prepare_leaf`] close that gap by
//! refusing to create anything *through* a symlink below `dest`, so backends
//! must call them before every write.
//!
//! [`safe_mode`] strips setuid/setgid/sticky bits from archive-supplied
//! permissions.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Entry-path sanitization
// ---------------------------------------------------------------------------

/// Sanitize an archive entry path against path-traversal attacks.
///
/// Takes `dest` (the extraction root) and the raw `entry` path as stored in
/// the archive, and returns the absolute path that the entry should be written
/// to.
///
/// # Algorithm
///
/// 1. Iterate the components of `entry`.
/// 2. Reject [`Component::RootDir`] and [`Component::Prefix`] (absolute
///    paths).
/// 3. Reject [`Component::ParentDir`] (`..`) unconditionally.
/// 4. Skip [`Component::CurDir`] (`.`).
/// 5. Collect the remaining [`Component::Normal`] components.
/// 6. An empty component list (no normal components) is rejected.
/// 7. Return `dest.join(collected)`.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] for absolute paths, `..` components, or
/// empty paths.
pub(crate) fn sanitize_entry_path(dest: &Path, entry: &Path) -> Result<PathBuf> {
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();

    for component in entry.components() {
        match component {
            // Reject absolute-path indicators.
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::PathTraversal {
                    entry: entry.to_path_buf(),
                });
            }
            // Reject parent-directory traversal.
            Component::ParentDir => {
                return Err(Error::PathTraversal {
                    entry: entry.to_path_buf(),
                });
            }
            // Skip current-directory components.
            Component::CurDir => {}
            // Collect normal path segments.
            Component::Normal(name) => {
                components.push(name);
            }
        }
    }

    // An empty path after filtering would resolve to `dest` itself, which is
    // not a valid entry destination.
    if components.is_empty() {
        return Err(Error::PathTraversal {
            entry: entry.to_path_buf(),
        });
    }

    let mut result = dest.to_path_buf();
    for segment in components {
        result.push(segment);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Symlink-target sanitization
// ---------------------------------------------------------------------------

/// Sanitize a symlink target stored in an archive.
///
/// Given:
/// - `dest` — the extraction root directory.
/// - `link_path` — the **absolute** path of the symlink file on disk (already
///   validated by [`sanitize_entry_path`]).
/// - `target` — the raw symlink target as stored in the archive.
///
/// Verifies that the symlink, when resolved lexically from `link_path`'s
/// parent, stays inside `dest`.
///
/// # Algorithm
///
/// 1. Absolute `target` → [`Error::PathTraversal`].
/// 2. Build `base = link_path.parent()` (or `dest` if `link_path` has no
///    parent, which should not happen after sanitize_entry_path).
/// 3. Walk `target`'s components, tracking a depth counter (starts at the
///    number of components in `base` minus those in `dest`, i.e. 0 for a
///    top-level link):
///    - [`Component::Normal`] → push component, increment depth.
///    - [`Component::CurDir`] → skip.
///    - [`Component::ParentDir`] → depth would go negative → error; else
///      decrement and pop last component.
///    - Absolute components → unreachable (handled in step 1).
/// 4. Require that the final resolved path starts with `dest`.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] if the target is absolute or resolves
/// outside `dest`.
pub(crate) fn sanitize_link_target(dest: &Path, link_path: &Path, target: &Path) -> Result<()> {
    // Step 1: absolute symlink targets are always rejected.
    if target.is_absolute() {
        return Err(Error::PathTraversal {
            entry: target.to_path_buf(),
        });
    }

    // Step 2: start lexical resolution from the symlink's containing directory.
    let base = link_path.parent().unwrap_or(dest);

    // Collect the initial path as a sequence of components we can pop.
    let mut resolved: Vec<&std::ffi::OsStr> = base
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Step 3: walk target components.
    for component in target.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {
                // Already rejected above; unreachable in practice.
                return Err(Error::PathTraversal {
                    entry: target.to_path_buf(),
                });
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop one level.  If there is nothing left to pop we would
                // escape the root — reject.
                if resolved.is_empty() {
                    return Err(Error::PathTraversal {
                        entry: target.to_path_buf(),
                    });
                }
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
            }
        }
    }

    // Step 4: rebuild the resolved path and require it starts with dest.
    let mut resolved_path = PathBuf::new();
    // Preserve the leading slash (or prefix) from `base` so that
    // starts_with(dest) works correctly on absolute dest paths.
    for component in base.components() {
        match component {
            Component::RootDir => {
                resolved_path.push("/");
            }
            Component::Prefix(p) => {
                resolved_path.push(p.as_os_str());
            }
            _ => break,
        }
    }
    for segment in &resolved {
        resolved_path.push(segment);
    }

    if resolved_path.starts_with(dest) {
        Ok(())
    } else {
        Err(Error::PathTraversal {
            entry: target.to_path_buf(),
        })
    }
}

// ---------------------------------------------------------------------------
// Filesystem guards
// ---------------------------------------------------------------------------

/// Reject `path` if it, or any directory between `dest` and it, is an
/// existing symlink.
///
/// `dest` itself is trusted (the caller chose it).  Components that do not
/// exist yet are fine: they will be created as real directories.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] if a symlink is found or `path` is not
/// under `dest`, or [`Error::Io`] if a component cannot be inspected.
pub(crate) fn reject_symlinks_below(dest: &Path, path: &Path) -> Result<()> {
    let rel = path.strip_prefix(dest).map_err(|_| Error::PathTraversal {
        entry: path.to_path_buf(),
    })?;
    let mut current = dest.to_path_buf();
    for component in rel.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::PathTraversal {
                    entry: rel.to_path_buf(),
                });
            }
            Ok(_) => {}
            // Nothing deeper can exist either.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Create the directory for a directory entry at `out_path` (already
/// sanitized), without following any symlink below `dest`.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] if `out_path` or one of its ancestors
/// below `dest` is a symlink, or [`Error::Io`] if creation fails.
pub(crate) fn prepare_dir(dest: &Path, out_path: &Path) -> Result<()> {
    reject_symlinks_below(dest, out_path)?;
    fs::create_dir_all(out_path)?;
    Ok(())
}

/// Get `out_path` (already sanitized) ready for a new file, symlink, or hard
/// link: create its parent directories and deal with anything already there.
///
/// Never writes through a symlink: ancestors below `dest` must be real
/// directories, and an existing symlink at `out_path` itself is replaced
/// (with `overwrite`) rather than followed.  With `overwrite`, any existing
/// non-directory is removed so the caller creates a fresh inode; an existing
/// directory is left for the caller's create call to fail on.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] if an ancestor below `dest` is a symlink,
/// [`Error::AlreadyExists`] if something exists at `out_path` and `overwrite`
/// is false, or [`Error::Io`] on filesystem failure.
pub(crate) fn prepare_leaf(dest: &Path, out_path: &Path, overwrite: bool) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        reject_symlinks_below(dest, parent)?;
        fs::create_dir_all(parent)?;
    }
    match fs::symlink_metadata(out_path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) if !overwrite => Err(Error::AlreadyExists {
            path: out_path.to_path_buf(),
        }),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => {
            fs::remove_file(out_path)?;
            Ok(())
        }
    }
}

/// Permission bits that extraction may apply from an archive: `rwx` for
/// user/group/other only.  Setuid, setgid, and sticky bits from an untrusted
/// archive are dropped.
pub(crate) fn safe_mode(mode: u32) -> u32 {
    mode & 0o777
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{sanitize_entry_path, sanitize_link_target};
    use crate::Error;

    fn dest() -> PathBuf {
        PathBuf::from("/extract/dest")
    }

    // --- sanitize_entry_path: rejection cases ---

    #[test]
    fn entry_parent_dir_component_rejected() {
        // `../evil` must be rejected.
        let err = sanitize_entry_path(&dest(), Path::new("../evil")).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
    }

    #[test]
    fn entry_absolute_path_rejected() {
        // `/abs/evil` is absolute and must be rejected.
        let err = sanitize_entry_path(&dest(), Path::new("/abs/evil")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    #[test]
    fn entry_nested_traversal_rejected() {
        // `foo/../../evil` contains `..` and must be rejected.
        let err = sanitize_entry_path(&dest(), Path::new("foo/../../evil")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    #[test]
    fn entry_empty_path_rejected() {
        // An empty path has no Normal components and must be rejected.
        let err = sanitize_entry_path(&dest(), Path::new("")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    #[test]
    fn entry_cur_dir_only_rejected() {
        // A path of only `.` yields no Normal components after filtering.
        let err = sanitize_entry_path(&dest(), Path::new(".")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    // Windows-style `C:\evil` path.  We build the path from a string so the
    // test runs on all platforms (on non-Windows the prefix component is just
    // treated as a Normal segment on the real FS, but `Path::components`
    // parses it as a Prefix on any host when the string is well-formed).
    #[test]
    fn entry_windows_prefix_style_rejected() {
        // Construct via a raw string so the Prefix component is visible on all
        // platforms (the `tar` crate can embed such strings in archives).
        // On Unix, Path::new(r"C:\evil") is a single Normal component "C:\\evil"
        // — it will pass the sanitizer, which is correct: on Unix it is not
        // really absolute.  We test the *component* level.
        //
        // To exercise the actual Prefix rejection we must parse as a Windows
        // path.  Since std::path::Path is OS-specific, we instead test via
        // an absolute path that triggers RootDir, which is the closest
        // portable equivalent.
        let err = sanitize_entry_path(&dest(), Path::new("/C:/evil")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    // --- sanitize_entry_path: acceptance cases ---

    #[test]
    fn entry_cur_dir_prefix_accepted() {
        // `./ok/file` — the leading `.` is skipped; entry resolves to dest/ok/file.
        let result = sanitize_entry_path(&dest(), Path::new("./ok/file")).unwrap();
        assert_eq!(result, PathBuf::from("/extract/dest/ok/file"));
    }

    #[test]
    fn entry_nested_path_accepted() {
        // `a/b/c` resolves to dest/a/b/c.
        let result = sanitize_entry_path(&dest(), Path::new("a/b/c")).unwrap();
        assert_eq!(result, PathBuf::from("/extract/dest/a/b/c"));
    }

    #[test]
    fn entry_simple_file_accepted() {
        let result = sanitize_entry_path(&dest(), Path::new("file.txt")).unwrap();
        assert_eq!(result, PathBuf::from("/extract/dest/file.txt"));
    }

    // --- sanitize_link_target: rejection cases ---

    #[test]
    fn link_target_absolute_rejected() {
        // Absolute symlink target must always be rejected.
        let link = dest().join("sub/link");
        let err = sanitize_link_target(&dest(), &link, Path::new("/etc/passwd")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    #[test]
    fn link_target_escapes_dest_rejected() {
        // `../../x` from dest/sub/link escapes dest.
        let link = dest().join("sub/link");
        let err = sanitize_link_target(&dest(), &link, Path::new("../../x")).unwrap_err();
        assert!(matches!(err, Error::PathTraversal { .. }));
    }

    // --- sanitize_link_target: acceptance cases ---

    #[test]
    fn link_target_relative_inside_dest_accepted() {
        // `sub/ok` from dest/link resolves to dest/sub/ok — inside dest.
        let link = dest().join("link");
        sanitize_link_target(&dest(), &link, Path::new("sub/ok")).unwrap();
    }

    #[test]
    fn link_target_sibling_inside_dest_accepted() {
        // `../sibling` from dest/sub/link resolves to dest/sibling — still
        // inside dest.
        let link = dest().join("sub/link");
        sanitize_link_target(&dest(), &link, Path::new("../sibling")).unwrap();
    }
}
