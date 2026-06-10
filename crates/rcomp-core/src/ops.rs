//! Public top-level operations: [`compress`], [`extract`], and [`list`].
//!
//! These functions are the primary public API of `rcomp-core`.  They accept
//! high-level options, dispatch to the right backend (tar, zip, or a raw codec
//! stream), apply the silent-tar rule, handle progress reporting and
//! cancellation, and report [`Report`] statistics on success.
//!
//! # Silent-tar rule
//!
//! When the requested output format is **codec-only** (e.g. `.bz2`) and the
//! input is a **directory**, the library automatically wraps the directory in a
//! tar stream before applying the codec.  This matches the conventional Unix
//! behaviour of `tar cjf archive.bz2 folder/`.  The output file name is kept
//! verbatim — the caller (CLI) is responsible for warning the user.
//!
//! # Extraction sniff
//!
//! When extracting a codec-only format (e.g. a plain `.bz2` file), the first
//! 512 decompressed bytes are inspected.  If the POSIX `ustar` magic is present
//! at byte offset 257, the stream is treated as tar-inside-codec and passed
//! directly to the tar backend.  Otherwise the stream is extracted as a single
//! file into `dest`.
//!
//! # Output naming (single-file codec extraction)
//!
//! 1. **gzip only** — if the gzip header contains an embedded filename, take
//!    only the final [`Path::file_name`] component (header is attacker-controlled).
//! 2. Strip the codec extension from the input file stem using the codec's
//!    [`Codec::short_ext`] (e.g. `data.bz2` → `data`).
//! 3. Fall back to `<input_file_name>.out`.
//!
//! # Encoder-finish after tar
//!
//! `tar::create` accepts `Box<dyn Write + 'w>` and returns the inner writer so
//! the caller can finalise it.  For the tar-with-codec case we pass a
//! [`WriteRef`] wrapper that holds a shared `&mut Box<dyn Encoder>`.  `tar`
//! writes through the reference; after `tar::create` returns we still own the
//! encoder in the outer scope and call `Box::new(enc).finish()` on it.  No
//! type-erasure downcast is required.

use std::{
    fs,
    io::{self, BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicU64, Ordering}},
    time::Instant,
};

use crate::{
    Codec, Container, Error, Format, Level, Result,
    archive::{
        OpCtx,
        sevenz,
        tar,
        zip,
    },
    codec::{Encoder, new_decoder, new_encoder},
    detect::{detect, detect_from_extension},
    progress::{CancelToken, Entry, Progress, Report, copy_with_progress},
};

#[cfg(feature = "rar")]
use crate::archive::rar;

// ---------------------------------------------------------------------------
// Public option structs
// ---------------------------------------------------------------------------

/// Options for a [`compress`] operation.
///
/// All fields implement `Default`; use struct-update syntax or set only the
/// fields you need.
///
/// ```no_run
/// use rcomp_core::CompressOptions;
/// let opts = CompressOptions { overwrite: true, ..Default::default() };
/// ```
#[derive(Debug, Clone, Default)]
pub struct CompressOptions {
    /// Override the output format.  When `None`, the format is inferred from
    /// the output file extension.
    pub format: Option<Format>,
    /// Compression level.  Defaults to [`Level::Best`].
    pub level: Level,
    /// Whether to overwrite an existing output file.
    pub overwrite: bool,
    /// Cancellation handle.  Cloning shares the same underlying flag.
    pub cancel: CancelToken,
}

/// Options for an [`extract`] operation.
///
/// All fields implement `Default`; use struct-update syntax or set only the
/// fields you need.
///
/// ```no_run
/// use rcomp_core::ExtractOptions;
/// let opts = ExtractOptions { overwrite: true, ..Default::default() };
/// ```
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Override the input format.  When `None`, the format is detected from
    /// magic bytes and/or file extension.
    pub format: Option<Format>,
    /// Whether to overwrite existing files in the destination.
    pub overwrite: bool,
    /// Cancellation handle.  Cloning shares the same underlying flag.
    pub cancel: CancelToken,
}

// ---------------------------------------------------------------------------
// compress
// ---------------------------------------------------------------------------

/// Compress or archive `input` and write the result to `output`.
///
/// # Format selection
///
/// 1. `opts.format` overrides everything.
/// 2. Otherwise the format is inferred from the `output` file extension via
///    [`detect_from_extension`].
/// 3. If the extension is unrecognised, [`Error::UnknownFormat`] is returned.
///
/// # Silent-tar rule
///
/// If the resolved format is **codec-only** and `input` is a directory, the
/// library automatically wraps the directory in tar before applying the codec.
///
/// # Progress
///
/// `bytes_total` is set to the sum of all input file sizes before work begins.
/// `bytes_done` advances as input bytes are read.
///
/// # Errors
///
/// - [`Error::UnknownFormat`] — no format could be inferred.
/// - [`Error::AlreadyExists`] — output exists and `opts.overwrite` is false.
/// - [`Error::UnsupportedOperation`] — format is rar (rar creation is always
///   unsupported; enable the `rar` feature for extraction only).
/// - [`Error::Cancelled`] — cancel token fired.
/// - [`Error::Io`] — underlying I/O failure.
pub fn compress(
    input: &Path,
    output: &Path,
    opts: &CompressOptions,
    mut on_progress: impl FnMut(&Progress),
) -> Result<Report> {
    let start = Instant::now();

    // --- Input must exist ---
    let input_meta = fs::symlink_metadata(input)?;

    // --- Resolve format ---
    let mut format = if let Some(f) = opts.format {
        f
    } else {
        let name = output
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        detect_from_extension(name).ok_or_else(|| Error::UnknownFormat {
            path: output.to_path_buf(),
        })?
    };

    // --- Silent-tar rule ---
    // Codec-only format + directory input → treat as tar-inside-codec.
    if format.container.is_none() && input_meta.is_dir() && let Some(codec) = format.codec {
        format = Format::layered(Container::Tar, codec);
    }

    // --- Unsupported formats ---
    // RAR creation is proprietary and always unsupported.
    // SevenZ is now supported via the sevenz backend.
    if let Some(Container::Rar) = format.container {
        return Err(Error::UnsupportedOperation {
            format: format.to_string(),
            operation: "compress".into(),
        });
    }

    // --- Overwrite check ---
    if output.exists() && !opts.overwrite {
        return Err(Error::AlreadyExists {
            path: output.to_path_buf(),
        });
    }

    // --- Pre-scan input sizes for progress total ---
    let bytes_total = scan_input_size(input)?;

    let progress = Progress {
        bytes_done: 0,
        bytes_total: Some(bytes_total),
        current_entry: None,
    };

    let mut ctx = OpCtx {
        cancel: opts.cancel.clone(),
        on_progress: &mut on_progress,
        progress,
    };

    // --- Dispatch ---
    let result = do_compress(input, output, format, opts.level, &mut ctx, bytes_total);

    // --- Best-effort cleanup on failure ---
    if result.is_err() {
        let _ = fs::remove_file(output);
    }

    let (entries, input_bytes) = result?;

    let output_bytes = fs::metadata(output).map(|m| m.len()).unwrap_or(0);

    Ok(Report {
        input_bytes,
        output_bytes,
        entries,
        duration: start.elapsed(),
    })
}

/// Inner dispatch for compress (after guards/progress setup).
///
/// `input_bytes_total` is the pre-scanned total input size (already computed
/// by the caller to initialise `ctx.progress.bytes_total`).  It is reused
/// here as the `input_bytes` field of the returned tuple, avoiding a second
/// filesystem walk for all container formats.
///
/// Returns `(entry_count, input_bytes_read)`.
fn do_compress(
    input: &Path,
    output: &Path,
    format: Format,
    level: Level,
    ctx: &mut OpCtx<'_>,
    input_bytes_total: u64,
) -> Result<(u64, u64)> {
    match (format.container, format.codec) {
        // 7z container (no codec layer — 7z carries its own LZMA2 codec).
        (Some(Container::SevenZ), None) => {
            let entries = sevenz::create(input, output, level, ctx)?;
            Ok((entries, input_bytes_total))
        }

        // Zip container (no codec layer).
        (Some(Container::Zip), None) => {
            let entries = zip::create(input, output, level, ctx)?;
            Ok((entries, input_bytes_total))
        }

        // Tar container with a codec: write tar data through the encoder.
        //
        // Pattern for encoder-finish after tar:
        //   1. Create `Box<dyn Encoder>` wrapping the output file.
        //   2. Wrap it in a `WriteRef` that holds `&mut Box<dyn Encoder>`.
        //   3. Pass `Box<WriteRef>` (a `Box<dyn Write>`) to `tar::create`.
        //   4. After `tar::create` returns, call `Box::new(encoder).finish()`
        //      on the encoder we still own in this scope.
        //   No downcast is required because we never move the encoder into the
        //   box — only a mutable reference to it.
        (Some(Container::Tar), Some(codec)) => {
            let out_file = fs::File::create(output)?;
            let mut encoder: Box<dyn Encoder> = new_encoder(codec, Box::new(out_file), level)?;
            let entries = {
                let write_ref = WriteRef::new(&mut encoder);
                // tar::create takes ownership of the Box<dyn Write> for the
                // duration of the archive build.  After it returns, write_ref
                // (and the mutable borrow on encoder) is released.
                let (n, _) = tar::create(input, Box::new(write_ref), ctx)?;
                n
            };
            // encoder borrow ends here; safe to call finish.
            encoder.finish()?;
            Ok((entries, input_bytes_total))
        }

        // Plain tar (no codec).
        (Some(Container::Tar), None) => {
            let out_file = fs::File::create(output)?;
            let (entries, _) = tar::create(input, Box::new(out_file), ctx)?;
            Ok((entries, input_bytes_total))
        }

        // Codec-only + file input (silent-tar already resolved dirs above).
        (None, Some(codec)) => {
            let mut in_file = fs::File::open(input)?;
            let out_file = fs::File::create(output)?;
            let mut encoder = new_encoder(codec, Box::new(out_file), level)?;
            let input_bytes = copy_with_progress(&mut in_file, &mut *encoder, ctx)?;
            Box::new(encoder).finish()?;
            Ok((1, input_bytes))
        }

        _ => {
            // Should not reach here given guard above.
            Err(Error::UnsupportedOperation {
                format: format.to_string(),
                operation: "compress".into(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract or decompress `input` into `dest`.
///
/// `dest` is created if it does not exist.
///
/// # Format selection
///
/// 1. `opts.format` overrides everything.
/// 2. Otherwise the format is detected via [`detect`] (magic bytes first,
///    extension as tiebreaker).
///
/// # Extraction of codec-only streams
///
/// The first 512 decompressed bytes are inspected:
///
/// - `ustar` at byte offset 257 → tar-inside-codec.  The sniffed bytes are
///   re-chained with the remaining decoder stream for a single-pass
///   decompression + untar.
/// - Otherwise → single-file decode into `dest`.  The output file name is
///   determined by: gzip embedded filename (final component only) → strip
///   codec extension from input stem → `<input_file_name>.out`.
///
/// # Progress
///
/// **tar / tar+codec / codec-only:** `bytes_total` is set to the compressed
/// input file size.  `bytes_done` advances over **compressed** bytes consumed
/// (the input file is wrapped in a counting reader), giving a true percentage
/// of the compressed stream processed.  Invariant: every [`Progress`] callback
/// satisfies `bytes_total.is_none() || bytes_done <= bytes_total`.
///
/// **zip / 7z:** Because these formats are random-access (file-based) there is
/// no single compressed-byte stream to count.  `bytes_total` is set to `None`
/// for zip and 7z extraction so the invariant above is preserved even though
/// `bytes_done` reflects decompressed bytes in that case.
///
/// # Errors
///
/// - [`Error::UnknownFormat`] — format cannot be detected.
/// - [`Error::AlreadyExists`] — an output file exists and `opts.overwrite` is false.
/// - [`Error::UnsupportedOperation`] — format is rar (without the `rar`
///   feature enabled).
/// - [`Error::Cancelled`] — cancel token fired.
/// - [`Error::Io`] — underlying I/O failure.
pub fn extract(
    input: &Path,
    dest: &Path,
    opts: &ExtractOptions,
    mut on_progress: impl FnMut(&Progress),
) -> Result<Report> {
    let start = Instant::now();

    // --- Detect format ---
    let format = if let Some(f) = opts.format {
        f
    } else {
        detect(input)?
    };

    // --- Unsupported formats (feature-gated) ---
    // When the `rar` feature is disabled, RAR extraction is unsupported.
    // When enabled, extraction is dispatched below via do_extract.
    #[cfg(not(feature = "rar"))]
    if let Some(Container::Rar) = format.container {
        return Err(Error::UnsupportedOperation {
            format: format.to_string(),
            operation: "extract — enable the `rar` feature to extract RAR archives".into(),
        });
    }

    // --- Create destination ---
    fs::create_dir_all(dest)?;

    // --- Progress total = compressed input size ---
    let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);

    let progress = Progress {
        bytes_done: 0,
        bytes_total: Some(input_size),
        current_entry: None,
    };

    let mut ctx = OpCtx {
        cancel: opts.cancel.clone(),
        on_progress: &mut on_progress,
        progress,
    };

    // --- Dispatch ---
    let (entries, input_bytes) = do_extract(input, dest, format, opts.overwrite, &mut ctx)?;

    let output_bytes: u64 = sum_dir_size(dest).unwrap_or(0);

    Ok(Report {
        input_bytes,
        output_bytes,
        entries,
        duration: start.elapsed(),
    })
}

/// Inner dispatch for extract.
///
/// Returns `(entry_count, input_bytes_consumed)`.
///
/// # Progress accounting
///
/// For tar (with or without codec) and codec-only streams:
/// `bytes_total` is set to the compressed file size before calling this
/// function.  Inside, we wrap the raw file in an `AtomicCountingReader` so
/// that compressed bytes consumed are accumulated in an `Arc<AtomicU64>`.
/// Per-chunk progress updates are driven by reading the counter, ensuring
/// `bytes_done` never exceeds `bytes_total` (the compressed size).
///
/// For zip: the format is random-access (no counting reader), so compressed
/// progress is not trackable without deep surgery.  `bytes_total` is reset to
/// `None` for zip so that the invariant `bytes_done <= bytes_total` always
/// holds even though `bytes_done` advances by decompressed bytes.
fn do_extract(
    input: &Path,
    dest: &Path,
    format: Format,
    overwrite: bool,
    ctx: &mut OpCtx<'_>,
) -> Result<(u64, u64)> {
    match (format.container, format.codec) {
        // RAR container (extract-only; creation is always unsupported).
        //
        // unrar drives its own file I/O so there is no compressed-byte stream
        // to wrap.  Reset bytes_total to None consistent with zip and 7z.
        #[cfg(feature = "rar")]
        (Some(Container::Rar), None) => {
            ctx.progress.bytes_total = None;
            let entries = rar::extract(input, dest, overwrite, ctx)?;
            let input_bytes = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok((entries, input_bytes))
        }

        // 7z container.
        //
        // 7z is random-access (file-based) so we cannot wrap the stream in a
        // counting reader.  Reset bytes_total to None to prevent bytes_done
        // (decompressed) from ever exceeding it.  Consistent with zip.
        (Some(Container::SevenZ), None) => {
            ctx.progress.bytes_total = None;
            let entries = sevenz::extract(input, dest, overwrite, ctx)?;
            let input_bytes = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok((entries, input_bytes))
        }

        // Zip container.
        //
        // Zip is random-access (file-based) so we cannot wrap the stream in a
        // counting reader.  Reset bytes_total to None to prevent bytes_done
        // (decompressed) from ever exceeding it.
        (Some(Container::Zip), None) => {
            ctx.progress.bytes_total = None;
            let entries = zip::extract(input, dest, overwrite, ctx)?;
            let input_bytes = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            Ok((entries, input_bytes))
        }

        // Tar container (possibly with a codec).
        //
        // Compressed bytes are counted by an AtomicCountingReader.  The
        // counter is passed into tar::extract so each per-chunk write syncs
        // progress.bytes_done from the atomic counter instead of adding the
        // (larger) decompressed count.  This guarantees bytes_done ≤ bytes_total
        // throughout extraction.
        (Some(Container::Tar), codec_opt) => {
            let file = fs::File::open(input)?;
            let counter = Arc::new(AtomicU64::new(0));
            let counting = AtomicCountingReader::new(file, Arc::clone(&counter));
            let reader: Box<dyn Read> = if let Some(codec) = codec_opt {
                new_decoder(codec, Box::new(counting))?
            } else {
                Box::new(counting)
            };
            let entries = tar::extract(reader, dest, overwrite, ctx, Some(Arc::clone(&counter)))?;
            // Emit a final progress update at the exact compressed byte count.
            let compressed = counter.load(Ordering::Relaxed);
            ctx.progress.bytes_done = compressed;
            (ctx.on_progress)(&ctx.progress);
            Ok((entries, compressed))
        }

        // Codec-only stream.
        (None, Some(codec)) => {
            let file = fs::File::open(input)?;
            let counter = Arc::new(AtomicU64::new(0));
            let counting = AtomicCountingReader::new(file, Arc::clone(&counter));
            let mut decoder = new_decoder(codec, Box::new(counting))?;

            // Sniff the first 512 decompressed bytes for a tar ustar magic.
            let (is_tar, sniff_bytes) = sniff_tar_prefix(&mut *decoder)?;

            if is_tar {
                // Re-chain the sniffed prefix with the remaining decoder stream.
                let prefix = Cursor::new(sniff_bytes);
                let chained: Box<dyn Read + '_> = Box::new(prefix.chain(decoder));
                let entries = tar::extract(
                    chained,
                    dest,
                    overwrite,
                    ctx,
                    Some(Arc::clone(&counter)),
                )?;
                // Emit a final progress update at the exact compressed byte count.
                let compressed = counter.load(Ordering::Relaxed);
                ctx.progress.bytes_done = compressed;
                (ctx.on_progress)(&ctx.progress);
                Ok((entries, compressed))
            } else {
                // Single-file decode: determine the output file name.
                let out_name = output_name_for_codec_file(input, codec)?;
                let out_path = dest.join(&out_name);

                if out_path.exists() && !overwrite {
                    return Err(Error::AlreadyExists { path: out_path });
                }

                ctx.set_entry(out_name.to_string_lossy().as_ref());
                ctx.check_cancel()?;

                let mut out_file = fs::File::create(&out_path)?;
                // Write the already-sniffed bytes first, then copy the rest,
                // syncing bytes_done from the compressed-bytes counter per
                // chunk so bytes_done never exceeds bytes_total.
                out_file.write_all(&sniff_bytes)?;
                copy_decoder_synced(&mut *decoder, &mut out_file, ctx, &counter)?;

                let compressed = counter.load(Ordering::Relaxed);
                ctx.progress.bytes_done = compressed;
                (ctx.on_progress)(&ctx.progress);
                Ok((1, compressed))
            }
        }

        _ => Err(Error::UnsupportedOperation {
            format: format.to_string(),
            operation: "extract".into(),
        }),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List the entries of `archive` without extracting anything.
///
/// # Format support
///
/// | Format | Supported |
/// |---|---|
/// | `.zip` | yes |
/// | `.tar`, `.tar.*` | yes |
/// | `.7z` | yes |
/// | codec-only (`.gz`, `.bz2`, …) | [`Error::UnsupportedOperation`] |
/// | `.rar` | [`Error::UnsupportedOperation`] (enable the `rar` feature) |
///
/// # Errors
///
/// Returns [`Error::UnknownFormat`], [`Error::UnsupportedOperation`], or
/// [`Error::Io`].
pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    let format = detect(archive)?;

    match (format.container, format.codec) {
        // RAR container (list-only; creation is always unsupported).
        #[cfg(feature = "rar")]
        (Some(Container::Rar), None) => rar::list(archive),

        // 7z container.
        (Some(Container::SevenZ), None) => sevenz::list(archive),

        // Zip container.
        (Some(Container::Zip), None) => zip::list(archive),

        // Tar (with or without codec).
        (Some(Container::Tar), codec_opt) => {
            let file = fs::File::open(archive)?;
            let reader: Box<dyn Read> = if let Some(codec) = codec_opt {
                new_decoder(codec, Box::new(file))?
            } else {
                Box::new(file)
            };
            tar::list(reader)
        }

        // Codec-only stream: decode and sniff for tar ustar magic.
        // If tar is found, list through the decoder; otherwise UnsupportedOperation.
        (None, Some(codec)) => {
            let file = fs::File::open(archive)?;
            let mut decoder = new_decoder(codec, Box::new(file))?;

            let (is_tar, sniff_bytes) = sniff_tar_prefix(&mut *decoder)?;

            if is_tar {
                let prefix = Cursor::new(sniff_bytes);
                let chained: Box<dyn Read> = Box::new(prefix.chain(decoder));
                tar::list(chained)
            } else {
                Err(Error::UnsupportedOperation {
                    format: format.to_string(),
                    operation: "list".into(),
                })
            }
        }

        // Unsupported container formats (RAR without the rar feature falls through here).
        _ => {
            if let Some(Container::Rar) = format.container {
                Err(Error::UnsupportedOperation {
                    format: format.to_string(),
                    operation: "list — enable the `rar` feature to list RAR archives".into(),
                })
            } else {
                Err(Error::UnsupportedOperation {
                    format: format.to_string(),
                    operation: "list".into(),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read up to 512 decompressed bytes from `reader` and check for a POSIX
/// `ustar` tar magic at byte offset 257.
///
/// Returns `(is_tar, sniffed_bytes)`.  The returned `Vec` contains the bytes
/// that were already consumed from the reader so the caller can re-chain them
/// with the remaining stream using [`std::io::Cursor::chain`].
///
/// This is the single canonical implementation of the tar-inside-codec sniff
/// used by both [`do_extract`] (extract path) and [`list`].
fn sniff_tar_prefix(reader: &mut dyn Read) -> Result<(bool, Vec<u8>)> {
    let mut sniff = [0u8; 512];
    let mut sniff_read = 0usize;
    while sniff_read < 512 {
        match reader.read(&mut sniff[sniff_read..]) {
            Ok(0) => break,
            Ok(n) => sniff_read += n,
            Err(e) => return Err(Error::Io(e)),
        }
    }
    let sniff_buf = &sniff[..sniff_read];
    // Check for ustar magic at offset 257 (POSIX tar header).
    let is_tar = sniff_buf.len() >= 262 && &sniff_buf[257..262] == b"ustar";
    Ok((is_tar, sniff_buf.to_vec()))
}

/// Recursively sum the size of all regular files under `path`.
///
/// If `path` is a regular file, returns its size.  Returns `Ok(0)` for
/// empty or unreadable directories rather than failing.
fn scan_input_size(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        let mut total: u64 = 0;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            total = total.saturating_add(scan_input_size(&entry.path()).unwrap_or(0));
        }
        Ok(total)
    } else if meta.is_file() {
        Ok(meta.len())
    } else {
        // Symlinks and other special entries contribute 0 to the input size.
        Ok(0)
    }
}

/// Recursively sum the sizes of all files under a directory.
///
/// Used to compute `output_bytes` after extraction.
fn sum_dir_size(dir: &Path) -> Result<u64> {
    let mut total: u64 = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total = total.saturating_add(sum_dir_size(&entry.path()).unwrap_or(0));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

/// Copy bytes from `r` to `w` in 64 KiB chunks, checking for cancellation
/// each iteration and syncing `ctx.progress.bytes_done` from `counter` (the
/// compressed-bytes [`AtomicCountingReader`] counter) rather than adding the
/// decompressed count.
///
/// Used by the single-file codec-only extraction path to keep `bytes_done`
/// within the compressed `bytes_total` bound.
fn copy_decoder_synced(
    r: &mut dyn Read,
    w: &mut dyn Write,
    ctx: &mut OpCtx<'_>,
    counter: &AtomicU64,
) -> Result<()> {
    const BUF_SIZE: usize = 64 * 1024;
    let mut buf = [0u8; BUF_SIZE];

    loop {
        ctx.check_cancel()?;
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        ctx.progress.bytes_done = counter.load(Ordering::Relaxed);
        (ctx.on_progress)(&ctx.progress);
    }
    Ok(())
}

/// Determine the output file name when extracting a codec-only stream as a
/// single file.
///
/// Priority:
///
/// 1. **gzip only** — gzip embedded filename header, taking only the final
///    `file_name` component (header is attacker-controlled).
/// 2. Strip the codec extension from the input file stem.
/// 3. Fall back to `<input_file_name>.out`.
fn output_name_for_codec_file(input: &Path, codec: Codec) -> Result<PathBuf> {
    // 1. Gzip embedded filename (if applicable).
    if codec == Codec::Gzip && let Ok(file) = fs::File::open(input) {
        let gz = flate2::read::GzDecoder::new(BufReader::new(file));
        if let Some(header) = gz.header()
            && let Some(raw) = header.filename()
            && let Ok(s) = std::str::from_utf8(raw)
        {
            let p = Path::new(s);
            // Take only the final component to prevent traversal.
            if let Some(name) = p.file_name() {
                let name_path = PathBuf::from(name);
                if !name_path.as_os_str().is_empty() {
                    return Ok(name_path);
                }
            }
        }
    }

    // 2. Strip the codec extension from the input file stem.
    if let Some(file_name) = input.file_name().and_then(|s| s.to_str()) {
        let ext = format!(".{}", codec.short_ext());
        if let Some(stem) = file_name.strip_suffix(&ext) && !stem.is_empty() {
            return Ok(PathBuf::from(stem));
        }
    }

    // 3. Fallback: append ".out" to the input file name.
    let base = input
        .file_name()
        .map(|s| {
            let mut b = s.to_os_string();
            b.push(".out");
            b
        })
        .unwrap_or_else(|| "output.out".into());
    Ok(PathBuf::from(base))
}

// ---------------------------------------------------------------------------
// WriteRef — mutable reference wrapper enabling encoder-finish after tar
// ---------------------------------------------------------------------------

/// A [`Write`] wrapper that borrows a `&mut W` instead of owning the writer.
///
/// This is the key to finishing a codec encoder after `tar::create`:
///
/// 1. Create `Box<dyn Encoder>` wrapping the output file.
/// 2. Wrap it in `WriteRef::new(&mut encoder)` — a concrete type that
///    implements `Write` by forwarding to the encoder.
/// 3. Box the `WriteRef` and pass it to `tar::create` as `Box<dyn Write>`.
/// 4. `tar::create` returns the `Box<dyn Write>` (the boxed `WriteRef`).
///    Drop it to release the `&mut encoder` borrow.
/// 5. Call `encoder.finish()` — the encoder is owned by the outer scope and
///    has never moved into the box, so this is safe.
///
/// No downcast or `unsafe` is required.
struct WriteRef<'a, W: Write + ?Sized> {
    inner: &'a mut W,
}

impl<'a, W: Write + ?Sized> WriteRef<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self { inner }
    }
}

impl<W: Write + ?Sized> Write for WriteRef<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------
// AtomicCountingReader — counts compressed bytes without borrowing OpCtx
// ---------------------------------------------------------------------------

/// A [`Read`] wrapper that accumulates bytes read into an [`Arc<AtomicU64>`]
/// counter.
///
/// Unlike a direct `&mut OpCtx` approach, this does not borrow `ctx`, so the
/// caller can use `ctx` freely after creating the reader (e.g. to pass to
/// backend functions).  After the operation completes the caller reads the
/// final counter value and calls `ctx.add_bytes(compressed)` once to emit a
/// final progress update reflecting the true compressed bytes consumed.
///
/// # Cancellation
///
/// Cancellation is NOT checked inside this reader.  The backend
/// (`tar::extract`, `copy_with_progress`) already checks `ctx.check_cancel()`
/// per entry/chunk.  Returning `Interrupted` from inside a decoder would cause
/// some decoders to loop indefinitely since `Interrupted` conventionally means
/// "retry the syscall".
struct AtomicCountingReader<R: Read> {
    inner: R,
    counter: Arc<AtomicU64>,
}

impl<R: Read> AtomicCountingReader<R> {
    fn new(inner: R, counter: Arc<AtomicU64>) -> Self {
        Self { inner, counter }
    }
}

impl<R: Read> Read for AtomicCountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.counter.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(n)
    }
}
