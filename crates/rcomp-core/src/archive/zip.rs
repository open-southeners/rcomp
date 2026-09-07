//! zip archive backend.
//!
//! Implements [`create`], [`extract`], and [`list`] for the ZIP container
//! format.  zip is file-based (requires `Seek` on both reader and writer) so
//! it operates directly on `&Path` rather than on stream trait objects and
//! never composes with the codec layer.
//!
//! # Security
//!
//! Every entry path is validated through [`sanitize_entry_path`] and every
//! symlink target through [`sanitize_link_target`] before any byte is written
//! to disk.  Archives with malicious paths are rejected with
//! [`Error::PathTraversal`].
//!
//! # Compression levels
//!
//! Deflate compression levels are mapped as follows:
//!
//! | [`Level`] | deflate level |
//! |-----------|---------------|
//! | `Fast`    | 1             |
//! | `Best`    | 6             |
//! | `Edge`    | 9             |

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    Error, Level, Result,
    progress::{Entry, copy_with_progress},
    walk::WalkEntry,
};

use super::{
    OpCtx,
    sanitize::{sanitize_entry_path, sanitize_link_target},
};

// ---------------------------------------------------------------------------
// Level → deflate integer
// ---------------------------------------------------------------------------

/// Map a [`Level`] to a deflate compression level integer (0–9).
const fn deflate_level(level: Level) -> i64 {
    match level {
        Level::Fast => 1,
        Level::Best => 6,
        Level::Edge => 9,
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Convert a relative [`Path`] to a forward-slash zip entry name string.
///
/// ZIP archives always use `/` as the path separator.
fn rel_path_to_zip_name(rel: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// Create a zip archive from `src`, writing the output to `out`.
///
/// - When `walk_entries` is `Some`, it is the **authoritative** entry list and
///   `src` is ignored: each [`WalkEntry::rel`] is stored verbatim as the entry
///   name. This covers both a single pre-walked directory and a multi-input
///   bundle (where each input's basename is preserved as a root). Filtering has
///   already been applied by the walker.
/// - When `walk_entries` is `None`, `src` drives traversal: a **directory** is
///   archived recursively with its children stored relative to `src` (the
///   directory itself is not an entry — archiving `photos/` yields `a.jpg`,
///   `sub/b.jpg`, not `photos/a.jpg`); a **file** is written as a single entry
///   using the file's name.
/// - On unix, **symlinks** are preserved as symlink entries (not dereferenced).
///   On non-unix the symlink target file is stored as a regular file.
/// - Entries within each directory are processed in sorted order for
///   deterministic archives across runs.
///
/// `level` controls deflate compression aggressiveness:
/// `Fast`=1, `Best`=6, `Edge`=9.
///
/// Per entry:
/// 1. [`OpCtx::set_entry`] records the entry name in progress.
/// 2. [`OpCtx::check_cancel`] aborts if the cancel token has fired.
/// 3. For file bodies, bytes are copied via [`copy_with_progress`] so that
///    `bytes_done` advances as the file is written.
///
/// Returns the number of entries written.
///
/// # Errors
///
/// Returns [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`]
/// for underlying I/O failures.
pub(crate) fn create(
    src: &Path,
    out: &Path,
    level: Level,
    ctx: &mut OpCtx<'_>,
    walk_entries: Option<&[WalkEntry]>,
) -> Result<u64> {
    let clevel = deflate_level(level);
    let file = fs::File::create(out)?;
    let mut zip = ZipWriter::new(file);
    let mut count: u64 = 0;

    if let Some(entries) = walk_entries {
        // Authoritative pre-computed entry list (single dir or multi-input).
        for we in entries {
            let entry_name = rel_path_to_zip_name(&we.rel);
            ctx.set_entry(&entry_name);
            ctx.check_cancel()?;
            append_entry(&mut zip, &we.abs, &entry_name, clevel, ctx)?;
            count += 1;
        }
    } else {
        let meta = fs::symlink_metadata(src)?;
        if meta.is_dir() {
            let entries = collect_dir_entries(src)?;
            for (rel_path, abs_path) in entries {
                let entry_name = rel_path_to_zip_name(&rel_path);
                ctx.set_entry(&entry_name);
                ctx.check_cancel()?;
                append_entry(&mut zip, &abs_path, &entry_name, clevel, ctx)?;
                count += 1;
            }
        } else {
            // Single file: use just the file name as the archive entry name.
            let name = src.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name")
            })?;
            let rel_path = PathBuf::from(name);
            let entry_name = rel_path_to_zip_name(&rel_path);
            ctx.set_entry(&entry_name);
            ctx.check_cancel()?;

            append_entry(&mut zip, src, &entry_name, clevel, ctx)?;
            count += 1;
        }
    }

    zip.finish().map_err(|e| io::Error::other(e.to_string()))?;
    Ok(count)
}

/// Recursively collect all filesystem entries under `dir`, returning
/// `(relative_path, absolute_path)` pairs sorted depth-first alphabetically.
///
/// Symlinks are included as-is (not followed).
fn collect_dir_entries(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut result: Vec<(PathBuf, PathBuf)> = Vec::new();
    collect_dir_entries_recursive(dir, dir, &mut result)?;
    Ok(result)
}

fn collect_dir_entries_recursive(
    root: &Path,
    current: &Path,
    result: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    let mut entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let abs = entry.path();
        let rel = abs
            .strip_prefix(root)
            .map_err(|_| io::Error::other("failed to strip root prefix"))?;
        entries.push((rel.to_path_buf(), abs));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (rel, abs) in entries {
        let meta = fs::symlink_metadata(&abs)?;
        result.push((rel.clone(), abs.clone()));
        // Recurse into real directories, not into symlinks-to-dirs.
        if meta.is_dir() {
            collect_dir_entries_recursive(root, &abs, result)?;
        }
    }
    Ok(())
}

/// Append a single filesystem entry (file, directory, or symlink) to `zip`.
///
/// `abs_path` is the on-disk path; `entry_name` is the zip-internal path
/// (forward-slash separated, already computed).
fn append_entry<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    abs_path: &Path,
    entry_name: &str,
    clevel: i64,
    ctx: &mut OpCtx<'_>,
) -> Result<()> {
    let meta = fs::symlink_metadata(abs_path)?;

    if meta.is_symlink() {
        append_symlink(zip, abs_path, entry_name, &meta)?;
    } else if meta.is_dir() {
        let options = dir_options(&meta);
        zip.add_directory(entry_name, options)
            .map_err(|e| io::Error::other(e.to_string()))?;
    } else {
        // Regular file: stream bytes through copy_with_progress.
        let options = file_options(&meta, clevel);
        zip.start_file(entry_name, options)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut file = fs::File::open(abs_path)?;
        copy_with_progress(&mut file, zip, ctx)?;
    }
    Ok(())
}

/// Append a symlink entry.
///
/// On unix: write a zip symlink entry whose body is the raw link target.
/// On non-unix: store the target file content as a regular file entry.
fn append_symlink<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    abs_path: &Path,
    entry_name: &str,
    meta: &fs::Metadata,
) -> Result<()> {
    #[cfg(unix)]
    {
        let target = fs::read_link(abs_path)?;
        let target_str = target.to_string_lossy().into_owned();
        let options = symlink_options(meta);
        zip.add_symlink(entry_name, &target_str, options)
            .map_err(|e| io::Error::other(e.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        // Dereference and store target content as a regular file.
        let target = fs::read_link(abs_path)?;
        let real_meta = fs::metadata(&target)?;
        let options = file_options(&real_meta, /* clevel irrelevant for small links */ 6);
        zip.start_file(entry_name, options)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut target_file = fs::File::open(&target)?;
        io::copy(&mut target_file, zip)?;
        let _ = meta;
    }
    Ok(())
}

/// Build a [`SimpleFileOptions`] for a regular file.
fn file_options(meta: &fs::Metadata, clevel: i64) -> SimpleFileOptions {
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(clevel));
    unix_perms(opts, meta)
}

/// Build a [`SimpleFileOptions`] for a directory entry.
fn dir_options(meta: &fs::Metadata) -> SimpleFileOptions {
    // Directories are always stored (no compression useful).
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    unix_perms(opts, meta)
}

/// Build a [`SimpleFileOptions`] for a symlink entry.
///
/// `add_symlink` will OR in `S_IFLNK` on top of the permission bits.
#[cfg_attr(not(unix), allow(dead_code))]
fn symlink_options(meta: &fs::Metadata) -> SimpleFileOptions {
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    unix_perms(opts, meta)
}

/// Apply unix permissions to `opts` from `meta` on unix; no-op otherwise.
fn unix_perms(opts: SimpleFileOptions, meta: &fs::Metadata) -> SimpleFileOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        opts.unix_permissions(meta.mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        opts
    }
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract a zip archive at `archive` into `dest`.
///
/// # Overwrite policy
///
/// If `overwrite` is `false` and an output file already exists,
/// [`Error::AlreadyExists`] is returned immediately.
///
/// # Entry handling
///
/// | Entry type | Action |
/// |---|---|
/// | Directory | [`fs::create_dir_all`] |
/// | Regular file | Written via [`copy_with_progress`]; unix mode restored. |
/// | Symlink | Target validated with [`sanitize_link_target`]; created on unix; skipped on non-unix. |
///
/// # Security
///
/// The raw entry name (from [`zip::read::ZipFile::name`]) is parsed as a
/// [`Path`] and passed through [`sanitize_entry_path`] before any byte is
/// written.  The crate's own `enclosed_name` is consulted only when the raw
/// name is empty or contains NULL bytes.
///
/// Returns `(entry_count, bytes_written)` where `bytes_written` is the total
/// decompressed byte count actually written to regular files under `dest`
/// (directories and symlinks contribute 0).
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] for malicious paths or link targets,
/// [`Error::AlreadyExists`] if overwrite is disabled and a file exists,
/// [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`] for
/// I/O failures.
pub(crate) fn extract(
    archive: &Path,
    dest: &Path,
    overwrite: bool,
    ctx: &mut OpCtx<'_>,
) -> Result<(u64, u64)> {
    let file = fs::File::open(archive)?;
    let mut zip = ZipArchive::new(file).map_err(|e| io::Error::other(e.to_string()))?;
    let mut count: u64 = 0;
    let mut bytes_written: u64 = 0;

    // Collect (out_path, unix_mode) pairs for directories so we can apply
    // their permissions after all entries are written.  A read-only directory
    // would otherwise block writing its own children.
    #[cfg(unix)]
    let mut dir_modes: Vec<(PathBuf, u32)> = Vec::new();

    for idx in 0..zip.len() {
        // We take the entry name first, then re-open it for reading.
        // The borrow checker requires us to close the ZipFile before calling
        // by_index again, so we collect the metadata in a first pass and the
        // body in a second.
        let (raw_name, is_dir, is_symlink, unix_mode, size) = {
            let entry = zip
                .by_index(idx)
                .map_err(|e| io::Error::other(e.to_string()))?;
            let raw_name = entry.name().to_owned();
            let is_dir = entry.is_dir();
            let is_symlink = entry.is_symlink();
            let unix_mode = entry.unix_mode();
            let size = entry.size();
            (raw_name, is_dir, is_symlink, unix_mode, size)
        };

        // Sanitize the raw entry path using OUR sanitizer — do not rely solely
        // on the zip crate's enclosed_name which silently drops `..` components.
        let raw_path = Path::new(&raw_name);
        let out_path = sanitize_entry_path(dest, raw_path)?;

        ctx.set_entry(&raw_name);
        ctx.check_cancel()?;

        if is_dir {
            fs::create_dir_all(&out_path)?;
            // Collect the directory mode for deferred application.
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                // Only restore the lower 12 bits (type + permissions).
                let perm_bits = mode & 0o7777;
                if perm_bits != 0 {
                    dir_modes.push((out_path.clone(), perm_bits));
                }
            }
        } else if is_symlink {
            // Symlink entries store the target path as the file body.
            let target_bytes = {
                let mut entry = zip
                    .by_index(idx)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                let mut buf = Vec::with_capacity(size as usize);
                entry.read_to_end(&mut buf)?;
                buf
            };
            let target_str = std::str::from_utf8(&target_bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "symlink target is not valid UTF-8",
                )
            })?;
            let link_target = Path::new(target_str);

            // Only create symlinks on unix; skip on non-unix.
            #[cfg(unix)]
            {
                sanitize_link_target(dest, &out_path, link_target)?;
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !overwrite && out_path.symlink_metadata().is_ok() {
                    return Err(Error::AlreadyExists { path: out_path });
                }
                std::os::unix::fs::symlink(link_target, &out_path)?;
            }
            #[cfg(not(unix))]
            {
                // Symlink entries silently skipped on non-unix targets.
                let _ = (link_target, overwrite, out_path, unix_mode);
                continue;
            }
        } else {
            // Regular file.
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if !overwrite && out_path.exists() {
                return Err(Error::AlreadyExists { path: out_path });
            }
            {
                let mut entry = zip
                    .by_index(idx)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                let mut out_file = fs::File::create(&out_path)?;
                bytes_written += copy_with_progress(&mut entry, &mut out_file, ctx)?;
            }
            // Restore unix permissions from the zip entry's external attributes.
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                // Only restore the lower 12 bits (type + permissions).
                let perm_bits = mode & 0o7777;
                if perm_bits != 0 {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&out_path, fs::Permissions::from_mode(perm_bits))?;
                }
            }
        }

        count += 1;
    }

    // Apply collected directory modes deepest-first (longest path first) so
    // that a read-only parent does not prevent writes to its own children.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Sort by descending component depth so deepest directories are
        // chmod'd first.  Use path length as a fast proxy for depth.
        dir_modes.sort_by_key(|b| std::cmp::Reverse(b.0.as_os_str().len()));
        for (dir_path, mode) in dir_modes {
            fs::set_permissions(&dir_path, fs::Permissions::from_mode(mode))?;
        }
    }

    Ok((count, bytes_written))
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List the entries of a zip archive without extracting anything.
///
/// Returns a [`Vec<Entry>`] where each [`Entry`] contains:
/// - `path` — the raw path as stored in the archive (no sanitization applied).
/// - `size` — uncompressed byte size of the entry body.
/// - `is_dir` — `true` for directory entries.
///
/// # Errors
///
/// Returns [`Error::Io`] if the archive cannot be read.
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>> {
    let file = fs::File::open(archive)?;
    let mut zip = ZipArchive::new(file).map_err(|e| io::Error::other(e.to_string()))?;
    let mut entries = Vec::with_capacity(zip.len());

    for idx in 0..zip.len() {
        let entry = zip
            .by_index(idx)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let path = PathBuf::from(entry.name());
        let size = entry.size();
        let is_dir = entry.is_dir();
        entries.push(Entry { path, size, is_dir });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        io::Write,
        path::{Path, PathBuf},
    };

    use tempfile::TempDir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use crate::{
        Error, Level,
        archive::OpCtx,
        progress::{CancelToken, Progress},
    };

    use super::{create, extract, list};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    macro_rules! make_ctx {
        ($token:expr, $cb:expr) => {
            OpCtx {
                cancel: $token.clone(),
                on_progress: $cb,
                progress: Progress {
                    bytes_done: 0,
                    bytes_total: None,
                    current_entry: None,
                },
            }
        };
    }

    /// Create a zip from `src` into a temp file and return (zip_path, tempdir).
    fn create_zip(src: &Path, level: Level) -> (PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let zip_path = tmp.path().join("archive.zip");
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        create(src, &zip_path, level, &mut ctx, None).expect("create failed");
        (zip_path, tmp)
    }

    /// Extract a zip from `zip_path` into a fresh tempdir and return that tempdir.
    fn extract_zip(zip_path: &Path) -> TempDir {
        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        extract(zip_path, dest.path(), false, &mut ctx).expect("extract failed");
        dest
    }

    // -----------------------------------------------------------------------
    // Dir roundtrip — nested dirs, empty dir, unicode filename
    // -----------------------------------------------------------------------

    #[test]
    fn dir_roundtrip_nested_and_unicode() {
        let src = TempDir::new().unwrap();
        let sp = src.path();

        // Build tree: a.txt, sub/b.txt, empty_dir/, héllo wörld.txt
        std::fs::write(sp.join("a.txt"), b"hello a").unwrap();
        std::fs::create_dir(sp.join("sub")).unwrap();
        std::fs::write(sp.join("sub/b.txt"), b"hello b").unwrap();
        std::fs::create_dir(sp.join("empty_dir")).unwrap();
        std::fs::write(sp.join("héllo wörld.txt"), b"unicode content").unwrap();

        let (zip_path, _tmp) = create_zip(sp, Level::Best);
        let dest = extract_zip(&zip_path);
        let dp = dest.path();

        assert_eq!(std::fs::read(dp.join("a.txt")).unwrap(), b"hello a");
        assert_eq!(std::fs::read(dp.join("sub/b.txt")).unwrap(), b"hello b");
        assert!(dp.join("empty_dir").is_dir(), "empty_dir must exist");
        assert_eq!(
            std::fs::read(dp.join("héllo wörld.txt")).unwrap(),
            b"unicode content"
        );
    }

    // -----------------------------------------------------------------------
    // Single-file roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn single_file_roundtrip() {
        let src_dir = TempDir::new().unwrap();
        let src_file = src_dir.path().join("hello.txt");
        std::fs::write(&src_file, b"single file content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let out_dir = TempDir::new().unwrap();
        let zip_path = out_dir.path().join("out.zip");
        let n = create(&src_file, &zip_path, Level::Best, &mut ctx, None).expect("create failed");
        assert_eq!(n, 1, "single file should yield 1 entry");

        let dest = TempDir::new().unwrap();
        let token2 = CancelToken::default();
        let mut cb2: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx2 = make_ctx!(token2, &mut *cb2);
        let (n2, _) = extract(&zip_path, dest.path(), false, &mut ctx2).expect("extract failed");
        assert_eq!(n2, 1);
        assert_eq!(
            std::fs::read(dest.path().join("hello.txt")).unwrap(),
            b"single file content"
        );
    }

    // -----------------------------------------------------------------------
    // Unix permissions preserved
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn unix_permissions_preserved() {
        use std::os::unix::fs::PermissionsExt;

        let src = TempDir::new().unwrap();
        let sp = src.path();

        let exec_file = sp.join("exec.sh");
        let read_file = sp.join("data.txt");

        std::fs::write(&exec_file, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exec_file, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&read_file, b"data").unwrap();
        std::fs::set_permissions(&read_file, std::fs::Permissions::from_mode(0o644)).unwrap();

        let (zip_path, _tmp) = create_zip(sp, Level::Best);
        let dest = extract_zip(&zip_path);
        let dp = dest.path();

        let exec_mode = std::fs::metadata(dp.join("exec.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let read_mode = std::fs::metadata(dp.join("data.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(exec_mode, 0o755, "exec.sh should have 0o755");
        assert_eq!(read_mode, 0o644, "data.txt should have 0o644");
    }

    // -----------------------------------------------------------------------
    // Zip-slip security tests — craft malicious archives with raw ZipWriter
    // -----------------------------------------------------------------------

    /// Build an in-memory zip with a single file entry whose name is the raw
    /// string `entry_name`.  This bypasses any safety checks.
    fn make_malicious_zip(entry_name: &str, data: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            // start_file accepts any string — including `../evil.txt`.
            zip.start_file(entry_name, options)
                .expect("start_file failed");
            zip.write_all(data).expect("write failed");
            zip.finish().expect("finish failed");
        }
        buf.into_inner()
    }

    fn extract_malicious(zip_bytes: Vec<u8>, dest: &Path) -> crate::Result<(u64, u64)> {
        let zip_file = TempDir::new().unwrap();
        let zip_path = zip_file.path().join("malicious.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        extract(&zip_path, dest, false, &mut ctx)
    }

    #[test]
    fn zip_slip_dot_dot_rejected() {
        let zip_bytes = make_malicious_zip("../evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let err = extract_malicious(zip_bytes, dest.path()).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        // dest must be clean — no file was written.
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty(), "dest should be clean after rejection");
    }

    #[test]
    fn zip_slip_absolute_name_rejected() {
        let zip_bytes = make_malicious_zip("/abs/evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let err = extract_malicious(zip_bytes, dest.path()).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty(), "dest should be clean after rejection");
    }

    // -----------------------------------------------------------------------
    // Overwrite policy
    // -----------------------------------------------------------------------

    #[test]
    fn overwrite_false_returns_already_exists() {
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("file.txt"), b"content").unwrap();

        let (zip_path, _tmp) = create_zip(src.path(), Level::Best);

        // Pre-create the output file.
        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"existing").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(&zip_path, dest.path(), false, &mut ctx).unwrap_err();
        assert!(
            matches!(err, Error::AlreadyExists { .. }),
            "expected AlreadyExists, got {err:?}"
        );
    }

    #[test]
    fn overwrite_true_replaces_existing_file() {
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("file.txt"), b"new content").unwrap();

        let (zip_path, _tmp) = create_zip(src.path(), Level::Best);

        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"old content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        extract(&zip_path, dest.path(), true, &mut ctx).unwrap();

        assert_eq!(
            std::fs::read(dest.path().join("file.txt")).unwrap(),
            b"new content"
        );
    }

    // -----------------------------------------------------------------------
    // list
    // -----------------------------------------------------------------------

    #[test]
    fn list_matches_created_entries() {
        let src = TempDir::new().unwrap();
        let sp = src.path();
        std::fs::write(sp.join("a.txt"), b"aaa").unwrap();
        std::fs::create_dir(sp.join("sub")).unwrap();
        std::fs::write(sp.join("sub/b.txt"), b"bbb").unwrap();

        let (zip_path, _tmp) = create_zip(sp, Level::Best);
        let entries = list(&zip_path).unwrap();
        let paths: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

        assert!(paths.contains(Path::new("a.txt")), "expected a.txt in list");
        // The dir entry ends with '/' in zip, so check for prefix match.
        let has_sub = paths
            .iter()
            .any(|p| p == Path::new("sub") || p == Path::new("sub/") || p.starts_with("sub"));
        assert!(has_sub, "expected sub directory in list: {paths:?}");
        assert!(
            paths.contains(Path::new("sub/b.txt")),
            "expected sub/b.txt in list"
        );
    }

    // -----------------------------------------------------------------------
    // Cancel mid-extract
    // -----------------------------------------------------------------------

    #[test]
    fn cancel_mid_extract_returns_cancelled() {
        let src = TempDir::new().unwrap();
        let sp = src.path();
        // Write enough data that the copy loop runs at least once.
        let big_data: Vec<u8> = (0..128 * 1024).map(|i| (i % 256) as u8).collect();
        std::fs::write(sp.join("big.bin"), &big_data).unwrap();

        let (zip_path, _tmp) = create_zip(sp, Level::Fast);

        // Pre-cancel the token before extraction starts.
        let token = CancelToken::default();
        token.cancel();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let dest = TempDir::new().unwrap();
        let err = extract(&zip_path, dest.path(), false, &mut ctx).unwrap_err();

        assert!(
            matches!(err, Error::Cancelled),
            "expected Cancelled, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // All three Levels produce decodable archives
    // -----------------------------------------------------------------------

    #[test]
    fn all_levels_roundtrip() {
        let content = b"repetitive repetitive repetitive content for compression testing";

        for level in [Level::Fast, Level::Best, Level::Edge] {
            let src_dir = TempDir::new().unwrap();
            let src_file = src_dir.path().join("data.txt");
            std::fs::write(&src_file, content).unwrap();

            let (zip_path, _tmp) = create_zip(src_dir.path(), level);
            let dest = extract_zip(&zip_path);

            let extracted = std::fs::read(dest.path().join("data.txt")).unwrap();
            assert_eq!(extracted, content, "roundtrip failed for level {level:?}");
        }
    }

    // -----------------------------------------------------------------------
    // Symlink roundtrip (unix only)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn symlink_roundtrip() {
        use std::os::unix::fs::symlink;

        let src = TempDir::new().unwrap();
        let sp = src.path();
        std::fs::write(sp.join("target.txt"), b"target content").unwrap();
        symlink("target.txt", sp.join("link.txt")).unwrap();

        let (zip_path, _tmp) = create_zip(sp, Level::Best);
        let dest = extract_zip(&zip_path);
        let dp = dest.path();

        // The symlink should have been restored.
        let meta = std::fs::symlink_metadata(dp.join("link.txt")).unwrap();
        assert!(meta.file_type().is_symlink(), "link.txt must be a symlink");
        // Following the link should yield target content.
        assert_eq!(
            std::fs::read(dp.join("link.txt")).unwrap(),
            b"target content"
        );
    }

    // -----------------------------------------------------------------------
    // Malicious symlink via crafted zip (unix only)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn zip_slip_symlink_absolute_target_rejected() {
        // Craft a zip with a symlink entry: link.txt → /etc/passwd
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            // add_symlink sets S_IFLNK bits and stores the target as the body.
            let options = SimpleFileOptions::default();
            zip.add_symlink("link.txt", "/etc/passwd", options).unwrap();
            zip.finish().unwrap();
        }
        let zip_bytes = buf.into_inner();

        let zip_file = TempDir::new().unwrap();
        let zip_path = zip_file.path().join("evil.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();

        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(&zip_path, dest.path(), false, &mut ctx).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal for absolute symlink target, got {err:?}"
        );
        assert!(
            std::fs::symlink_metadata(dest.path().join("link.txt")).is_err(),
            "link.txt must not exist in dest"
        );
    }

    #[cfg(unix)]
    #[test]
    fn zip_slip_symlink_escaping_target_rejected() {
        // Craft a zip with a symlink entry: link.txt → ../../x
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buf);
            let options = SimpleFileOptions::default();
            zip.add_symlink("link.txt", "../../x", options).unwrap();
            zip.finish().unwrap();
        }
        let zip_bytes = buf.into_inner();

        let zip_file = TempDir::new().unwrap();
        let zip_path = zip_file.path().join("evil.zip");
        std::fs::write(&zip_path, &zip_bytes).unwrap();

        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(&zip_path, dest.path(), false, &mut ctx).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal for escaping symlink target, got {err:?}"
        );
        assert!(
            std::fs::symlink_metadata(dest.path().join("link.txt")).is_err(),
            "link.txt must not exist in dest"
        );
    }
}
