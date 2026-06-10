//! tar archive backend.
//!
//! Implements [`create`], [`extract`], and [`list`] for the tar container
//! format.  tar is stream-based (`Read`/`Write` trait objects) so it composes
//! naturally with the codec layer for `tar.*` formats (e.g. `tar.gz`,
//! `tar.xz`).
//!
//! # Security
//!
//! Every entry path is validated through [`sanitize_entry_path`] and every
//! symlink / hardlink target through [`sanitize_link_target`] before any byte
//! is written to disk.  Archives with malicious paths are rejected with
//! [`Error::PathTraversal`].
//!
//! # mtime restoration
//!
//! For regular files, the mtime stored in the tar header is restored after
//! extraction using the [`filetime`] crate.  Only nonzero header mtimes are
//! applied; if the header has no mtime the file receives the current time.
//! Symlinks and directories do **not** have their mtime restored — those
//! entries are processed by the OS and their mtime changes as children are
//! written underneath them anyway.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicU64, Ordering}},
};

use filetime::FileTime;
use tar::{Archive, Builder, EntryType, Header};

use crate::{
    Error, Result,
    progress::Entry,
    walk::WalkEntry,
};

use super::{
    OpCtx,
    sanitize::{sanitize_entry_path, sanitize_link_target},
};

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// Create a tar archive from `src`, writing bytes to `w`.
///
/// - If `src` is a **directory**, its contents are archived recursively.  The
///   directory itself is *not* included as an entry; its children are stored
///   relative to `src`.  For example, archiving `photos/` yields entries
///   `a.jpg` and `sub/b.jpg`, not `photos/a.jpg`.
///   When `walk_entries` is `Some`, those pre-computed entries are used
///   directly (filtering already applied) instead of doing a fresh recursive
///   `read_dir` walk.
/// - If `src` is a **file**, `walk_entries` is ignored and a single entry is
///   written using the file's name.
/// - **Symlinks** are preserved as symlink entries (not dereferenced).
/// - Entries within each directory are processed in sorted order for
///   deterministic archives across runs.
///
/// Per entry:
/// 1. [`OpCtx::set_entry`] records the entry name in progress.
/// 2. [`OpCtx::check_cancel`] aborts if the cancel token has fired.
/// 3. For file bodies, [`OpCtx::add_bytes`] is advanced as bytes are written.
///
/// Returns `(entry_count, inner_writer)`.  The caller is responsible for
/// finishing any encoder wrapping the writer (e.g. calling
/// [`crate::codec::Encoder::finish`] on a codec encoder).  The writer is
/// returned rather than dropped so that the caller can flush trailers after the
/// tar stream ends.
///
/// # Errors
///
/// Returns [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`]
/// for underlying I/O failures.
pub(crate) fn create<'w>(
    src: &Path,
    w: Box<dyn Write + 'w>,
    ctx: &mut OpCtx<'_>,
    walk_entries: Option<&[WalkEntry]>,
) -> Result<(u64, Box<dyn Write + 'w>)> {
    let mut builder = Builder::new(w);
    // Preserve symlinks as symlink entries rather than dereferencing them.
    builder.follow_symlinks(false);

    let meta = fs::symlink_metadata(src)?;
    let mut count: u64 = 0;

    if meta.is_dir() {
        if let Some(entries) = walk_entries {
            // Use the pre-computed filtered entries (from the shared walker).
            for we in entries {
                let entry_name = we.rel.to_string_lossy().into_owned();
                ctx.set_entry(&entry_name);
                ctx.check_cancel()?;
                append_entry(&mut builder, &we.abs, &we.rel, ctx)?;
                count += 1;
            }
        } else {
            // Fallback: collect and sort all entries under src for determinism.
            let entries = collect_dir_entries(src)?;
            for (rel_path, abs_path) in entries {
                let entry_name = rel_path.to_string_lossy().into_owned();
                ctx.set_entry(&entry_name);
                ctx.check_cancel()?;
                append_entry(&mut builder, &abs_path, &rel_path, ctx)?;
                count += 1;
            }
        }
    } else {
        // Single file: use just the file name as the archive entry name.
        let name = src.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name")
        })?;
        let rel_path = PathBuf::from(name);
        let entry_name = rel_path.to_string_lossy().into_owned();
        ctx.set_entry(&entry_name);
        ctx.check_cancel()?;
        append_entry(&mut builder, src, &rel_path, ctx)?;
        count += 1;
    }

    let inner = builder.into_inner()?;
    Ok((count, inner))
}

/// Recursively collect all filesystem entries under `dir`, returning
/// `(relative_path, absolute_path)` pairs sorted depth-first alphabetically.
///
/// Symlinks are included as-is (not followed) — their metadata is obtained
/// via `symlink_metadata`.
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
    // Read directory entries and sort them by name for determinism.
    let mut entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let abs = entry.path();
        let rel = abs.strip_prefix(root).map_err(|_| {
            io::Error::other("failed to strip root prefix")
        })?;
        entries.push((rel.to_path_buf(), abs));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (rel, abs) in entries {
        let meta = fs::symlink_metadata(&abs)?;
        result.push((rel.clone(), abs.clone()));
        // Recurse into real directories but not symlinks-to-dirs.
        if meta.is_dir() {
            collect_dir_entries_recursive(root, &abs, result)?;
        }
    }
    Ok(())
}

/// Append a single filesystem entry (file, directory, or symlink) to `builder`.
///
/// `abs_path` is the on-disk path; `rel_path` is the name stored in the
/// archive.
fn append_entry<W: Write>(
    builder: &mut Builder<W>,
    abs_path: &Path,
    rel_path: &Path,
    ctx: &mut OpCtx<'_>,
) -> Result<()> {
    let meta = fs::symlink_metadata(abs_path)?;

    if meta.is_symlink() {
        // Preserve the symlink entry without dereferencing.
        let target = fs::read_link(abs_path)?;
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Symlink);
        header.set_size(0);
        header.set_mode(meta.permissions().mode_bits());
        builder.append_link(&mut header, rel_path, &target)?;
    } else if meta.is_dir() {
        builder.append_dir(rel_path, abs_path)?;
    } else {
        // Regular file: stream bytes through a CountingReader so that
        // ctx.add_bytes advances as the file body is written to the archive.
        let mut file = fs::File::open(abs_path)?;
        let mut header = Header::new_gnu();
        header.set_metadata(&meta);
        header.set_size(meta.len());
        header.set_cksum();
        let counting_reader = CountingReader {
            inner: &mut file,
            ctx,
        };
        builder.append_data(&mut header, rel_path, counting_reader)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CountingReader — bridges tar's append_data with OpCtx progress
// ---------------------------------------------------------------------------

/// A thin `Read` wrapper that calls [`OpCtx::add_bytes`] after each chunk and
/// checks for cancellation before each read.
struct CountingReader<'a, 'b, R: Read> {
    inner: R,
    ctx: &'a mut OpCtx<'b>,
}

impl<R: Read> Read for CountingReader<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Check for cancellation before each read so long archives are
        // responsive to cancel signals.
        if self.ctx.cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.ctx.add_bytes(n as u64);
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract a tar archive from `r` into `dest`.
///
/// # Overwrite policy
///
/// If `overwrite` is `false` and an output file already exists,
/// [`Error::AlreadyExists`] is returned immediately (no partial state is
/// cleaned up — that is the caller's responsibility).
///
/// # Entry handling
///
/// | Entry type | Action |
/// |---|---|
/// | Directory | [`fs::create_dir_all`] |
/// | Regular file | Written via [`copy_with_progress`]; unix mode restored. |
/// | Symlink | Target validated with [`sanitize_link_target`]; created on unix; skipped on non-unix. |
/// | Hard link | Target validated with [`sanitize_entry_path`]; [`fs::hard_link`] called. |
/// | Other | Skipped silently. |
///
/// # mtime
///
/// After writing a regular file the mtime stored in its tar header is restored
/// via [`filetime::set_file_mtime`].  Only nonzero header mtimes are applied.
///
/// Returns `(entry_count, reader)` where `reader` is the underlying byte
/// stream that was passed in, returned after all entries are consumed.  The
/// caller may drain any remaining bytes from it (e.g. to ensure a wrapping
/// SHA-256 hasher sees the complete decompressed stream, including trailing
/// end-of-archive blocks that the tar crate leaves unread).
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] for malicious paths or link targets,
/// [`Error::AlreadyExists`] if overwrite is disabled and a file exists,
/// [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`] for
/// I/O failures.
pub(crate) fn extract<'r>(
    r: Box<dyn Read + 'r>,
    dest: &Path,
    overwrite: bool,
    ctx: &mut OpCtx<'_>,
    counter: Option<Arc<AtomicU64>>,
) -> Result<(u64, Box<dyn Read + 'r>)> {
    let mut archive = Archive::new(r);
    let mut count: u64 = 0;

    // Collect (out_path, unix_mode) pairs for directories so we can apply
    // their permissions after all entries are written.  A read-only directory
    // would otherwise block writing its own children.
    #[cfg(unix)]
    let mut dir_modes: Vec<(PathBuf, u32)> = Vec::new();

    let entries = archive.entries()?;
    for entry_result in entries {
        let mut entry = entry_result?;

        // Retrieve the raw path from the header before processing.
        let raw_path = entry.path()?.into_owned();
        let out_path = sanitize_entry_path(dest, &raw_path)?;

        let entry_name = raw_path.to_string_lossy().into_owned();
        ctx.set_entry(&entry_name);
        ctx.check_cancel()?;

        let entry_type = entry.header().entry_type();

        if entry_type.is_dir() {
            fs::create_dir_all(&out_path)?;
            // Collect the directory mode for deferred application.
            #[cfg(unix)]
            if let Ok(mode) = entry.header().mode() {
                dir_modes.push((out_path.clone(), mode));
            }
        } else if entry_type.is_file() {
            // Ensure parent directory exists.
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            // Overwrite check.
            if !overwrite && out_path.exists() {
                return Err(Error::AlreadyExists { path: out_path });
            }
            let mode = entry.header().mode().ok();
            let mut file = fs::File::create(&out_path)?;
            // Use counter-syncing copy so that progress.bytes_done reflects
            // compressed bytes consumed (via the AtomicCountingReader upstream)
            // rather than the larger decompressed byte count.  When no counter
            // is supplied (e.g. in unit tests that call extract directly) fall
            // back to a plain cancel-checked copy that does not advance
            // bytes_done at all — the caller is responsible for syncing.
            copy_extract(&mut entry, &mut file, ctx, counter.as_ref())?;
            // Restore unix permissions where available.
            #[cfg(unix)]
            if let Some(m) = mode {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&out_path, fs::Permissions::from_mode(m))?;
            }
            // Restore mtime from the tar header when available and nonzero.
            if let Ok(mtime) = entry.header().mtime()
                && mtime != 0
            {
                let ft = FileTime::from_unix_time(mtime as i64, 0);
                // Ignore errors — mtime restoration is best-effort.
                let _ = filetime::set_file_mtime(&out_path, ft);
            }
            #[cfg(not(unix))]
            let _ = mode;
        } else if entry_type.is_symlink() {
            let link_target = entry
                .link_name()?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "symlink entry has no link name",
                    )
                })?
                .into_owned();

            // Only create symlinks on unix; skip on non-unix.
            #[cfg(unix)]
            {
                sanitize_link_target(dest, &out_path, &link_target)?;
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if !overwrite && out_path.symlink_metadata().is_ok() {
                    return Err(Error::AlreadyExists { path: out_path });
                }
                std::os::unix::fs::symlink(&link_target, &out_path)?;
            }
            #[cfg(not(unix))]
            {
                // Symlink entries are silently skipped on non-unix targets.
                let _ = link_target;
            }
        } else if entry_type.is_hard_link() {
            let link_target = entry
                .link_name()?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "hard link entry has no link name",
                    )
                })?
                .into_owned();

            // Hard link targets in tar are always relative to the archive root.
            let target_path = sanitize_entry_path(dest, &link_target)?;
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if !overwrite && out_path.exists() {
                return Err(Error::AlreadyExists { path: out_path });
            }
            fs::hard_link(&target_path, &out_path)?;
        } else {
            // Unknown / unsupported entry type (device nodes, FIFOs, etc.) —
            // skip silently without incrementing the counter.
            continue;
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
        dir_modes.sort_by(|a, b| b.0.as_os_str().len().cmp(&a.0.as_os_str().len()));
        for (dir_path, mode) in dir_modes {
            fs::set_permissions(&dir_path, fs::Permissions::from_mode(mode))?;
        }
    }

    // Return the underlying reader so the caller can drain any remaining bytes
    // (e.g. the trailing end-of-archive zero block that the tar crate stops
    // reading before EOF) through any wrapping hasher.
    let reader = archive.into_inner();
    Ok((count, reader))
}

// ---------------------------------------------------------------------------
// copy_extract — cancel-checked copy that syncs progress from a counter
// ---------------------------------------------------------------------------

/// Copy bytes from `r` to `w` in 64 KiB chunks, checking for cancellation and
/// optionally syncing `ctx.progress.bytes_done` from `counter` instead of
/// accumulating the decompressed byte count.
///
/// When `counter` is `Some`, after each chunk we load the atomic counter (which
/// reflects compressed bytes consumed by the upstream [`AtomicCountingReader`])
/// and assign it to `progress.bytes_done` before invoking the callback.  This
/// ensures that `bytes_done` never exceeds `bytes_total` (the compressed file
/// size) even though the decompressed data may be much larger.
///
/// When `counter` is `None` (direct unit-test usage), the copy still checks
/// for cancellation per chunk but does not touch `bytes_done` at all.
fn copy_extract(
    r: &mut dyn Read,
    w: &mut dyn Write,
    ctx: &mut OpCtx<'_>,
    counter: Option<&Arc<AtomicU64>>,
) -> crate::Result<()> {
    const BUF_SIZE: usize = 64 * 1024;
    let mut buf = [0u8; BUF_SIZE];

    loop {
        ctx.check_cancel()?;
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        if let Some(ctr) = counter {
            // Sync progress.bytes_done to the compressed-bytes counter so the
            // user-visible percentage is based on the compressed input size.
            ctx.progress.bytes_done = ctr.load(Ordering::Relaxed);
            (ctx.on_progress)(&ctx.progress);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List the entries of a tar archive without extracting anything.
///
/// Returns a [`Vec<Entry>`] where each [`Entry`] contains:
/// - `path` — the raw path as stored in the archive (no sanitization applied).
/// - `size` — uncompressed byte size of the entry body.
/// - `is_dir` — `true` for directory entries.
///
/// # Errors
///
/// Returns [`Error::Io`] if the stream cannot be read.
pub(crate) fn list(r: Box<dyn Read + '_>) -> Result<Vec<Entry>> {
    let mut archive = Archive::new(r);
    let mut entries = Vec::new();

    for entry_result in archive.entries()? {
        let entry = entry_result?;
        let path = entry.path()?.into_owned();
        let size = entry.size();
        let is_dir = entry.header().entry_type().is_dir();
        entries.push(Entry { path, size, is_dir });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Mode bits helper — unix returns the real mode; non-unix returns a default.
// ---------------------------------------------------------------------------

trait PermissionsModeBits {
    fn mode_bits(&self) -> u32;
}

impl PermissionsModeBits for fs::Permissions {
    #[cfg(unix)]
    fn mode_bits(&self) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        self.mode()
    }

    #[cfg(not(unix))]
    fn mode_bits(&self) -> u32 {
        0o644
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        io::Cursor,
        path::{Path, PathBuf},
    };

    use tar::{Builder, EntryType, Header};
    use tempfile::TempDir;

    use crate::{
        Error,
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

    /// Create a tar from `src` into an in-memory buffer.
    fn create_to_buf(src: &Path) -> Vec<u8> {
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        let mut buf: Vec<u8> = Vec::new();
        create(src, Box::new(&mut buf), &mut ctx, None).expect("create failed");
        // The inner writer (`&mut buf`) is returned but we don't need it here —
        // `buf` is already populated.
        buf
    }

    /// Extract a tar from `buf` into a fresh tempdir and return that tempdir.
    fn extract_buf(buf: &[u8]) -> TempDir {
        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        extract(Box::new(Cursor::new(buf)), dest.path(), false, &mut ctx, None)
            .map(|(_, _)| ())
            .expect("extract failed");
        dest
    }

    /// Round-trip a directory: create then extract, returning (buf, dest).
    fn roundtrip(src: &Path) -> (Vec<u8>, TempDir) {
        let buf = create_to_buf(src);
        let dest = extract_buf(&buf);
        (buf, dest)
    }

    // -----------------------------------------------------------------------
    // Dir roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn dir_roundtrip_nested_dirs_and_files() {
        let src = TempDir::new().unwrap();
        let sp = src.path();

        // Build tree:  a.txt, sub/b.txt, empty_dir/
        std::fs::write(sp.join("a.txt"), b"hello a").unwrap();
        std::fs::create_dir(sp.join("sub")).unwrap();
        std::fs::write(sp.join("sub/b.txt"), b"hello b").unwrap();
        std::fs::create_dir(sp.join("empty_dir")).unwrap();

        let (_buf, dest) = roundtrip(sp);
        let dp = dest.path();

        assert_eq!(std::fs::read(dp.join("a.txt")).unwrap(), b"hello a");
        assert_eq!(std::fs::read(dp.join("sub/b.txt")).unwrap(), b"hello b");
        assert!(dp.join("empty_dir").is_dir());
    }

    #[test]
    fn dir_roundtrip_empty_file() {
        let src = TempDir::new().unwrap();
        let sp = src.path();
        std::fs::write(sp.join("empty.txt"), b"").unwrap();

        let (_buf, dest) = roundtrip(sp);
        let dp = dest.path();
        assert!(dp.join("empty.txt").exists());
        assert_eq!(std::fs::read(dp.join("empty.txt")).unwrap(), b"");
    }

    #[cfg(unix)]
    #[test]
    fn dir_roundtrip_unix_permissions_preserved() {
        use std::os::unix::fs::PermissionsExt;

        let src = TempDir::new().unwrap();
        let sp = src.path();

        let exec_file = sp.join("exec.sh");
        let read_file = sp.join("data.txt");

        std::fs::write(&exec_file, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exec_file, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&read_file, b"data").unwrap();
        std::fs::set_permissions(&read_file, std::fs::Permissions::from_mode(0o644)).unwrap();

        let (_buf, dest) = roundtrip(sp);
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

        let mut buf: Vec<u8> = Vec::new();
        let (n, _) = create(&src_file, Box::new(&mut buf), &mut ctx, None).expect("create failed");
        assert_eq!(n, 1);

        let dest = TempDir::new().unwrap();
        let token2 = CancelToken::default();
        let mut cb2: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx2 = make_ctx!(token2, &mut *cb2);

        let (n2, _) = extract(Box::new(Cursor::new(&buf)), dest.path(), false, &mut ctx2, None)
            .expect("extract failed");
        assert_eq!(n2, 1);
        assert_eq!(
            std::fs::read(dest.path().join("hello.txt")).unwrap(),
            b"single file content"
        );
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

        let (_buf, dest) = roundtrip(sp);
        let dp = dest.path();

        // The symlink should have been restored.
        let meta = std::fs::symlink_metadata(dp.join("link.txt")).unwrap();
        assert!(meta.file_type().is_symlink());
        // Following the link should yield target content.
        assert_eq!(
            std::fs::read(dp.join("link.txt")).unwrap(),
            b"target content"
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

        let buf = create_to_buf(sp);
        let entries = list(Box::new(Cursor::new(&buf))).unwrap();
        let paths: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

        assert!(paths.contains(Path::new("a.txt")), "expected a.txt");
        assert!(paths.contains(Path::new("sub")), "expected sub/");
        assert!(paths.contains(Path::new("sub/b.txt")), "expected sub/b.txt");
    }

    // -----------------------------------------------------------------------
    // Overwrite policy
    // -----------------------------------------------------------------------

    #[test]
    fn overwrite_false_returns_already_exists() {
        let src = TempDir::new().unwrap();
        let sp = src.path();
        std::fs::write(sp.join("file.txt"), b"content").unwrap();

        let buf = create_to_buf(sp);

        // Pre-create the output file.
        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"existing").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(Box::new(Cursor::new(&buf)), dest.path(), false, &mut ctx, None)
            .err()
            .expect("expected an error");

        assert!(
            matches!(err, Error::AlreadyExists { .. }),
            "expected AlreadyExists, got {err:?}"
        );
    }

    #[test]
    fn overwrite_true_replaces_existing_file() {
        let src = TempDir::new().unwrap();
        let sp = src.path();
        std::fs::write(sp.join("file.txt"), b"new content").unwrap();

        let buf = create_to_buf(sp);

        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"old content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        extract(Box::new(Cursor::new(&buf)), dest.path(), true, &mut ctx, None).unwrap();

        assert_eq!(
            std::fs::read(dest.path().join("file.txt")).unwrap(),
            b"new content"
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

        let buf = create_to_buf(sp);

        // Pre-cancel the token before extraction starts.
        let token = CancelToken::default();
        token.cancel();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let dest = TempDir::new().unwrap();
        let err =
            extract(Box::new(Cursor::new(&buf)), dest.path(), false, &mut ctx, None)
                .err()
                .expect("expected an error");

        assert!(
            matches!(err, Error::Cancelled),
            "expected Cancelled, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // mtime restoration
    // -----------------------------------------------------------------------

    #[test]
    fn extract_restores_file_mtime() {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Known timestamp: 2001-09-08T21:46:40Z — 1_000_000_000 seconds since epoch.
        const KNOWN_MTIME_SECS: u64 = 1_000_000_000;

        let src = TempDir::new().unwrap();
        let sp = src.path();

        let src_file = sp.join("timestamped.txt");
        std::fs::write(&src_file, b"mtime test").unwrap();

        // Apply a known mtime to the source file via filetime before archiving.
        let ft = filetime::FileTime::from_unix_time(KNOWN_MTIME_SECS as i64, 0);
        filetime::set_file_mtime(&src_file, ft).expect("set_file_mtime failed");

        // Archive the single file and extract it.
        let buf = create_to_buf(sp);
        let dest = extract_buf(&buf);
        let dp = dest.path();

        // Read back the mtime of the extracted file.
        let meta = std::fs::metadata(dp.join("timestamped.txt")).unwrap();
        let extracted_mtime = meta
            .modified()
            .expect("platform must support mtime")
            .duration_since(UNIX_EPOCH)
            .expect("mtime is before epoch")
            .as_secs();

        // Allow ±1 second for filesystem granularity differences.
        let expected = KNOWN_MTIME_SECS;
        assert!(
            extracted_mtime.abs_diff(expected) <= 1,
            "expected mtime ~{expected}s, got {extracted_mtime}s"
        );

        // Sanity: the extracted mtime must be far from "now" (i.e. the original
        // value was preserved, not reset to the current time).
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            now_secs.saturating_sub(extracted_mtime) > 1_000_000,
            "mtime looks like 'now'; restoration likely did not take effect"
        );
    }

    // -----------------------------------------------------------------------
    // Malicious archive tests — hand-crafted archives with raw Header bytes
    // -----------------------------------------------------------------------

    /// Build a tar archive in memory with a single file entry whose path is
    /// the raw byte string `path_bytes`.  This bypasses tar's safe API so that
    /// traversal sequences like `../` can be embedded in the archive.
    fn make_malicious_tar(path_bytes: &[u8], entry_data: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut header = Header::new_gnu();
        header.set_size(entry_data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);

        // Write raw bytes into the `name` field of the header, bypassing
        // path validation.
        let name_field = &mut header.as_old_mut().name;
        let copy_len = path_bytes.len().min(name_field.len() - 1);
        name_field[..copy_len].copy_from_slice(&path_bytes[..copy_len]);
        name_field[copy_len] = 0;

        header.set_cksum();

        let mut builder = Builder::new(&mut buf);
        builder.append(&header, entry_data).expect("append failed");
        builder.into_inner().unwrap();
        buf
    }

    /// Build a tar archive with a single symlink entry.  Both the link path
    /// and link target are written as raw bytes to bypass path validation.
    fn make_malicious_symlink_tar(link_path: &[u8], link_target: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(EntryType::Symlink);

        // Write raw link path.
        let name_field = &mut header.as_old_mut().name;
        let copy_len = link_path.len().min(name_field.len() - 1);
        name_field[..copy_len].copy_from_slice(&link_path[..copy_len]);
        name_field[copy_len] = 0;

        // Write raw link target.
        let link_field = &mut header.as_old_mut().linkname;
        let target_len = link_target.len().min(link_field.len() - 1);
        link_field[..target_len].copy_from_slice(&link_target[..target_len]);
        link_field[target_len] = 0;

        header.set_cksum();

        let mut builder = Builder::new(&mut buf);
        builder
            .append(&header, std::io::empty())
            .expect("append failed");
        builder.into_inner().unwrap();
        buf
    }

    #[test]
    fn malicious_path_traversal_dot_dot_rejected() {
        let tar_bytes = make_malicious_tar(b"../evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(Box::new(Cursor::new(&tar_bytes)), dest.path(), false, &mut ctx, None)
            .err()
            .expect("expected an error");

        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        // dest must be empty — no file was written.
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty(), "dest should be clean after rejection");
    }

    #[test]
    fn malicious_absolute_path_rejected() {
        let tar_bytes = make_malicious_tar(b"/abs/evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(Box::new(Cursor::new(&tar_bytes)), dest.path(), false, &mut ctx, None)
            .err()
            .expect("expected an error");

        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty(), "dest should be clean after rejection");
    }

    #[cfg(unix)]
    #[test]
    fn malicious_symlink_absolute_target_rejected() {
        // link.txt → /etc/passwd
        let tar_bytes = make_malicious_symlink_tar(b"link.txt", b"/etc/passwd");
        let dest = TempDir::new().unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(Box::new(Cursor::new(&tar_bytes)), dest.path(), false, &mut ctx, None)
            .err()
            .expect("expected an error");

        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        assert!(
            std::fs::symlink_metadata(dest.path().join("link.txt")).is_err(),
            "link.txt must not exist in dest"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malicious_symlink_escaping_target_rejected() {
        // link.txt → ../../x (escapes dest)
        let tar_bytes = make_malicious_symlink_tar(b"link.txt", b"../../x");
        let dest = TempDir::new().unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(Box::new(Cursor::new(&tar_bytes)), dest.path(), false, &mut ctx, None)
            .err()
            .expect("expected an error");

        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        assert!(
            std::fs::symlink_metadata(dest.path().join("link.txt")).is_err(),
            "link.txt must not exist in dest"
        );
    }
}
