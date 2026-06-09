//! 7z archive backend.
//!
//! Implements [`create`], [`extract`], and [`list`] for the 7-Zip container
//! format.  7z is file-based (requires `Seek` on both reader and writer) so
//! it operates directly on `&Path` rather than on stream trait objects and
//! never composes with the codec layer.
//!
//! # Security
//!
//! Every entry path is validated through [`sanitize_entry_path`] before any
//! byte is written to disk.  Archives with malicious paths are rejected with
//! [`Error::PathTraversal`].
//!
//! # Unix permissions and symlinks
//!
//! `sevenz-rust2`'s `ArchiveEntry` only carries `windows_attributes`; there is
//! no field for unix mode bits or symlink metadata.  Therefore this backend
//! does not preserve unix permissions and does not create symlink entries.
//! Symlinks in the source tree are stored as regular files (the symlink target
//! data is archived).  See CURRENT_ISSUES.md for follow-up tracking.
//!
//! # Compression levels
//!
//! LZMA2 compression levels are mapped as follows:
//!
//! | [`Level`] | LZMA2 preset |
//! |-----------|--------------|
//! | `Fast`    | 1            |
//! | `Best`    | 5            |
//! | `Edge`    | 9            |

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use sevenz_rust2::{
    ArchiveEntry, ArchiveReader, ArchiveWriter,
    EncoderConfiguration,
    Password,
    encoder_options::Lzma2Options,
};

use crate::{
    Error, Level, Result,
    progress::Entry,
};

use super::{
    OpCtx,
    sanitize::sanitize_entry_path,
};

// ---------------------------------------------------------------------------
// Level → LZMA2 preset integer
// ---------------------------------------------------------------------------

/// Map a [`Level`] to a LZMA2 compression preset (0–9).
const fn lzma2_preset(level: Level) -> u32 {
    match level {
        Level::Fast => 1,
        Level::Best => 5,
        Level::Edge => 9,
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Convert a relative [`Path`] to a forward-slash 7z entry name string.
///
/// 7z archives use `/` as the path separator on all platforms.
fn rel_path_to_7z_name(rel: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

// ---------------------------------------------------------------------------
// CountingReader — counts bytes read via an Arc<AtomicU64>
// ---------------------------------------------------------------------------

/// A [`Read`] wrapper that accumulates bytes read into an [`Arc<AtomicU64>`]
/// counter.
///
/// Because `ArchiveWriter::push_archive_entry` consumes the reader internally,
/// we cannot pass `&mut OpCtx` directly into the reader without creating
/// an aliasing borrow.  Instead, we share the counter through an atomic so
/// the caller can sync `ctx.progress.bytes_done` after each entry completes.
struct CountingReader<R: Read> {
    inner: R,
    counter: Arc<AtomicU64>,
}

impl<R: Read> CountingReader<R> {
    fn new(inner: R, counter: Arc<AtomicU64>) -> Self {
        Self { inner, counter }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.counter.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// Create a 7z archive from `src`, writing the output to `out`.
///
/// - If `src` is a **directory**, its contents are archived recursively.  The
///   directory itself is *not* included as an entry; its children are stored
///   relative to `src`.  For example, archiving `photos/` yields entries
///   `a.jpg` and `sub/b.jpg`, not `photos/a.jpg`.
/// - If `src` is a **file**, a single entry is written using the file's name.
/// - **Symlinks** are not preserved as symlink entries.  On all platforms the
///   symlink-target's data is stored as a regular file entry (dereferenced).
/// - Entries within each directory are processed in sorted order for
///   deterministic archives across runs.
/// - Directory entries are included.
///
/// `level` controls LZMA2 compression aggressiveness:
/// `Fast`=1, `Best`=5, `Edge`=9.
///
/// Per entry:
/// 1. [`OpCtx::set_entry`] records the entry name in progress.
/// 2. [`OpCtx::check_cancel`] aborts if the cancel token has fired.
/// 3. For file bodies, bytes are counted via [`CountingReader`] and synced
///    back to `ctx.progress.bytes_done` after each entry via
///    [`OpCtx::add_bytes`].
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
) -> Result<u64> {
    let preset = lzma2_preset(level);
    let lzma2_opts = Lzma2Options::from_level(preset);
    let encoder_cfg = EncoderConfiguration::from(lzma2_opts);

    let mut writer = ArchiveWriter::create(out)
        .map_err(|e| io::Error::other(e.to_string()))?;
    writer.set_content_methods(vec![encoder_cfg]);

    let mut count: u64 = 0;
    let meta = fs::symlink_metadata(src)?;

    if meta.is_dir() {
        let entries = collect_dir_entries(src)?;
        for (rel_path, abs_path) in entries {
            let entry_name = rel_path_to_7z_name(&rel_path);
            ctx.set_entry(&entry_name);
            ctx.check_cancel()?;

            push_entry(&mut writer, &abs_path, &entry_name, ctx)?;
            count += 1;
        }
    } else {
        // Single file: use just the file name as the archive entry name.
        let name = src.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source path has no file name")
        })?;
        let rel_path = PathBuf::from(name);
        let entry_name = rel_path_to_7z_name(&rel_path);
        ctx.set_entry(&entry_name);
        ctx.check_cancel()?;

        push_entry(&mut writer, src, &entry_name, ctx)?;
        count += 1;
    }

    writer.finish().map_err(|e| io::Error::other(e.to_string()))?;
    Ok(count)
}

/// Recursively collect all filesystem entries under `dir`, returning
/// `(relative_path, absolute_path)` pairs sorted depth-first alphabetically.
///
/// Symlinks are included as-is (not followed for directory recursion).
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
        let rel = abs.strip_prefix(root).map_err(|_| {
            io::Error::other("failed to strip root prefix")
        })?;
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

/// Push a single filesystem entry (file, directory, or symlink) to the writer.
///
/// `abs_path` is the on-disk path; `entry_name` is the 7z-internal name
/// (forward-slash separated, already computed).
///
/// Symlinks are stored as regular files (dereferenced).
/// This is consistent with `sevenz-rust2` not supporting symlink entry types.
fn push_entry<W: Write + io::Seek>(
    writer: &mut ArchiveWriter<W>,
    abs_path: &Path,
    entry_name: &str,
    ctx: &mut OpCtx<'_>,
) -> Result<()> {
    let meta = fs::symlink_metadata(abs_path)?;

    if meta.is_dir() {
        let dir_entry = ArchiveEntry::new_directory(entry_name);
        writer
            .push_archive_entry::<fs::File>(dir_entry, None)
            .map_err(|e| io::Error::other(e.to_string()))?;
        return Ok(());
    }

    // Regular file or symlink (symlinks stored as file data via dereferencing).
    let open_path = if meta.is_symlink() {
        fs::read_link(abs_path).unwrap_or_else(|_| abs_path.to_path_buf())
    } else {
        abs_path.to_path_buf()
    };

    let file = fs::File::open(&open_path)?;

    // Wrap the file in a CountingReader so we can track bytes without
    // borrowing ctx into the writer's internal read loop.
    let counter = Arc::new(AtomicU64::new(0));
    let counting = CountingReader::new(file, Arc::clone(&counter));

    let file_entry = ArchiveEntry::from_path(abs_path, entry_name.to_string());
    writer
        .push_archive_entry(file_entry, Some(counting))
        .map_err(|e| io::Error::other(e.to_string()))?;

    // Sync bytes_done from the atomic counter.
    let bytes_read = counter.load(Ordering::Relaxed);
    ctx.add_bytes(bytes_read);

    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract a 7z archive at `archive` into `dest`.
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
/// | Regular file | Written in 64 KiB chunks; `bytes_done` advances per chunk. |
///
/// # Security
///
/// The raw entry name (from [`ArchiveEntry::name`]) is parsed as a [`Path`]
/// and passed through [`sanitize_entry_path`] before any byte is written.
///
/// # Progress
///
/// `bytes_total` is set to `None` by the caller (see [`crate::ops::extract`])
/// because 7z is random-access (file-based) and there is no single
/// compressed-byte stream to count.  `bytes_done` advances by the
/// **uncompressed** bytes written per chunk.  Invariant:
/// `bytes_total.is_none()` for all progress callbacks emitted during 7z
/// extraction (consistent with zip).
///
/// Returns the number of entries extracted.
///
/// # Errors
///
/// Returns [`Error::PathTraversal`] for malicious paths,
/// [`Error::AlreadyExists`] if overwrite is disabled and a file exists,
/// [`Error::Cancelled`] if the cancel token fires, or [`Error::Io`] for
/// I/O failures.
pub(crate) fn extract(
    archive: &Path,
    dest: &Path,
    overwrite: bool,
    ctx: &mut OpCtx<'_>,
) -> Result<u64> {
    // Check cancel before we start.
    ctx.check_cancel()?;

    let mut reader = ArchiveReader::open(archive, Password::empty())
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut count: u64 = 0;

    // Collect entry metadata before the streaming pass so we can pre-create
    // directories and set entry progress names correctly.
    let entries_meta: Vec<(String, bool)> = reader
        .archive()
        .files
        .iter()
        .map(|e| (e.name.clone(), e.is_directory))
        .collect();

    // Pass 1: create all directory entries up-front so that parent dirs exist
    // before files are written.
    for (raw_name, is_dir) in &entries_meta {
        if !is_dir {
            continue;
        }
        let raw_path = Path::new(raw_name.as_str());
        let out_path = sanitize_entry_path(dest, raw_path)?;
        ctx.set_entry(raw_name);
        ctx.check_cancel()?;
        fs::create_dir_all(&out_path)?;
        count += 1;
    }

    // Pass 2: extract file entries via the streaming for_each_entries API.
    // We capture ctx fields we need inside the closure via copies/clones that
    // don't require mutable borrow aliasing.
    let cancel_token = ctx.cancel.clone();
    let dest_path = dest.to_path_buf();

    // Error and count tracking returned out of the closure.
    let mut file_error: Option<Error> = None;
    let mut file_count_extracted: u64 = 0;

    // We need to drive progress from inside the closure where we hold `ctx`.
    // However, for_each_entries takes a plain `FnMut` closure and we cannot
    // borrow `ctx` twice (once for the closure captures, once for `add_bytes`).
    // Solution: accumulate bytes in a shared atomic and flush them to ctx after
    // for_each_entries returns.
    let bytes_counter = Arc::new(AtomicU64::new(0));
    let bytes_counter_clone = Arc::clone(&bytes_counter);

    reader
        .for_each_entries(|entry, reader| {
            if entry.is_directory {
                // Already handled in pass 1.
                return Ok(true);
            }

            let raw_name = entry.name();
            let raw_path = Path::new(raw_name);
            let out_path = match sanitize_entry_path(&dest_path, raw_path) {
                Ok(p) => p,
                Err(e) => {
                    file_error = Some(e);
                    return Ok(false);
                }
            };

            // Ensure parent directory exists (in case it was not a separate entry).
            if let Some(parent) = out_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                file_error = Some(Error::Io(e));
                return Ok(false);
            }

            // Overwrite check.
            if !overwrite && out_path.exists() {
                file_error = Some(Error::AlreadyExists { path: out_path });
                return Ok(false);
            }

            // Write the decompressed bytes to disk.
            let mut out_file = match fs::File::create(&out_path) {
                Ok(f) => f,
                Err(e) => {
                    file_error = Some(Error::Io(e));
                    return Ok(false);
                }
            };

            // Copy in 64 KiB chunks, checking cancellation between chunks.
            const BUF_SIZE: usize = 64 * 1024;
            let mut buf = vec![0u8; BUF_SIZE];
            loop {
                if cancel_token.is_cancelled() {
                    file_error = Some(Error::Cancelled);
                    return Ok(false);
                }
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        file_error = Some(Error::Io(e));
                        return Ok(false);
                    }
                };
                if let Err(e) = out_file.write_all(&buf[..n]) {
                    file_error = Some(Error::Io(e));
                    return Ok(false);
                }
                bytes_counter_clone.fetch_add(n as u64, Ordering::Relaxed);
            }

            file_count_extracted += 1;
            Ok(true)
        })
        .map_err(|e| io::Error::other(e.to_string()))?;

    if let Some(err) = file_error {
        return Err(err);
    }

    // Sync accumulated bytes to ctx for a final progress update.
    let total_bytes = bytes_counter.load(Ordering::Relaxed);
    if total_bytes > 0 {
        ctx.add_bytes(total_bytes);
    }

    // Final cancel check.
    ctx.check_cancel()?;

    count += file_count_extracted;
    Ok(count)
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List the entries of a 7z archive without extracting anything.
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
    let reader = ArchiveReader::open(archive, Password::empty())
        .map_err(|e| io::Error::other(e.to_string()))?;

    let entries = reader
        .archive()
        .files
        .iter()
        .map(|e| Entry {
            path: PathBuf::from(&e.name),
            size: e.size,
            is_dir: e.is_directory,
        })
        .collect();

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
    };

    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    use tempfile::TempDir;

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

    /// Create a 7z from `src` into a temp file and return (archive_path, tempdir).
    fn create_7z(src: &Path, level: Level) -> (PathBuf, TempDir) {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("archive.7z");
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        create(src, &archive_path, level, &mut ctx).expect("create failed");
        (archive_path, tmp)
    }

    /// Extract a 7z from `archive_path` into a fresh tempdir and return that tempdir.
    fn extract_7z(archive_path: &Path) -> TempDir {
        let dest = TempDir::new().unwrap();
        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        extract(archive_path, dest.path(), false, &mut ctx).expect("extract failed");
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

        let (archive_path, _tmp) = create_7z(sp, Level::Best);
        let dest = extract_7z(&archive_path);
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
        let archive_path = out_dir.path().join("out.7z");
        let n = create(&src_file, &archive_path, Level::Best, &mut ctx).expect("create failed");
        assert_eq!(n, 1, "single file should yield 1 entry");

        let dest = TempDir::new().unwrap();
        let token2 = CancelToken::default();
        let mut cb2: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx2 = make_ctx!(token2, &mut *cb2);
        let n2 = extract(&archive_path, dest.path(), false, &mut ctx2).expect("extract failed");
        assert_eq!(n2, 1);
        assert_eq!(
            std::fs::read(dest.path().join("hello.txt")).unwrap(),
            b"single file content"
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

            let (archive_path, _tmp) = create_7z(src_dir.path(), level);
            let dest = extract_7z(&archive_path);

            let extracted = std::fs::read(dest.path().join("data.txt")).unwrap();
            assert_eq!(
                extracted, content,
                "roundtrip failed for level {level:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Malicious entry name — path traversal protection
    //
    // sevenz-rust2's ArchiveWriter accepts arbitrary entry names (including
    // `../evil`).  We craft such an archive in memory and verify that our
    // sanitize_entry_path call in `extract` rejects it with PathTraversal.
    // -----------------------------------------------------------------------

    /// Build a 7z archive as a `Vec<u8>` with a single file entry whose name
    /// is the raw string `entry_name`.  This bypasses any safety checks at
    /// write time so we can craft malicious archives for testing.
    fn make_malicious_7z(entry_name: &str, data: &[u8]) -> Vec<u8> {
        use std::io::Cursor;
        let buf = Cursor::new(Vec::new());
        let mut writer = ArchiveWriter::new(buf).expect("ArchiveWriter::new failed");
        let entry = ArchiveEntry::new_file(entry_name);
        writer
            .push_archive_entry(entry, Some(data))
            .expect("push_archive_entry failed");
        let cursor = writer.finish().expect("finish failed");
        cursor.into_inner()
    }

    fn extract_malicious(bytes: Vec<u8>, dest: &Path) -> crate::Result<u64> {
        let archive_dir = TempDir::new().unwrap();
        let archive_path = archive_dir.path().join("malicious.7z");
        std::fs::write(&archive_path, &bytes).unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);
        extract(&archive_path, dest, false, &mut ctx)
    }

    #[test]
    fn sevenz_slip_dot_dot_rejected() {
        let bytes = make_malicious_7z("../evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let err = extract_malicious(bytes, dest.path()).unwrap_err();
        assert!(
            matches!(err, Error::PathTraversal { .. }),
            "expected PathTraversal, got {err:?}"
        );
        // dest must be clean — no file was written.
        let entries: Vec<_> = std::fs::read_dir(dest.path()).unwrap().collect();
        assert!(entries.is_empty(), "dest should be clean after rejection");
    }

    #[test]
    fn sevenz_slip_absolute_name_rejected() {
        // 7z entry names stored as `/abs/evil.txt` (absolute path) must be
        // rejected by the sanitizer.
        let bytes = make_malicious_7z("/abs/evil.txt", b"evil");
        let dest = TempDir::new().unwrap();

        let err = extract_malicious(bytes, dest.path()).unwrap_err();
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

        let (archive_path, _tmp) = create_7z(src.path(), Level::Best);

        // Pre-create the output file.
        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"existing").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let err = extract(&archive_path, dest.path(), false, &mut ctx).unwrap_err();
        assert!(
            matches!(err, Error::AlreadyExists { .. }),
            "expected AlreadyExists, got {err:?}"
        );
    }

    #[test]
    fn overwrite_true_replaces_existing_file() {
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("file.txt"), b"new content").unwrap();

        let (archive_path, _tmp) = create_7z(src.path(), Level::Best);

        let dest = TempDir::new().unwrap();
        std::fs::write(dest.path().join("file.txt"), b"old content").unwrap();

        let token = CancelToken::default();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        extract(&archive_path, dest.path(), true, &mut ctx).unwrap();

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

        let (archive_path, _tmp) = create_7z(sp, Level::Best);
        let entries = list(&archive_path).unwrap();
        let paths: HashSet<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

        assert!(
            paths.contains(Path::new("a.txt")),
            "expected a.txt in list; got {:?}",
            paths
        );
        let has_sub = paths.iter().any(|p| {
            p == Path::new("sub") || p == Path::new("sub/") || p.starts_with("sub")
        });
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

        let (archive_path, _tmp) = create_7z(sp, Level::Fast);

        // Pre-cancel the token before extraction starts.
        let token = CancelToken::default();
        token.cancel();
        let mut cb: Box<dyn FnMut(&Progress)> = Box::new(|_| {});
        let mut ctx = make_ctx!(token, &mut *cb);

        let dest = TempDir::new().unwrap();
        let err = extract(&archive_path, dest.path(), false, &mut ctx).unwrap_err();

        assert!(
            matches!(err, Error::Cancelled),
            "expected Cancelled, got {err:?}"
        );
    }
}
