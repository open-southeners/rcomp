//! Progress reporting, cancellation token, and operation report types.

use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::archive::OpCtx;

/// A snapshot of progress for a running compress or extract operation.
///
/// Delivered to the caller's `on_progress` callback after each chunk.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Number of bytes processed so far.
    pub bytes_done: u64,
    /// Total expected bytes, when known in advance.
    pub bytes_total: Option<u64>,
    /// The archive entry currently being processed, if applicable.
    pub current_entry: Option<String>,
}

/// A lightweight cancellation handle that can be shared across threads.
///
/// Cloning a `CancelToken` shares the **same** underlying flag — cancelling
/// one clone cancels all of them.
///
/// ```
/// # use rcomp_core::progress::CancelToken;
/// let token = CancelToken::default();
/// let handle = token.clone();
/// token.cancel();
/// assert!(handle.is_cancelled());
/// ```
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// Signal cancellation.  All clones sharing this token will observe the
    /// cancellation on their next call to [`is_cancelled`](Self::is_cancelled).
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Returns `true` if cancellation has been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Summary statistics reported when an operation completes successfully.
#[derive(Debug, Clone)]
pub struct Report {
    /// Size of the input data, in bytes.
    pub input_bytes: u64,
    /// Size of the output data, in bytes.
    pub output_bytes: u64,
    /// Number of archive entries processed.
    pub entries: u64,
    /// Number of filesystem entries that were excluded from the archive.
    ///
    /// Non-zero only when a directory input was compressed with
    /// [`crate::CompressOptions::follow_gitignore`] or
    /// [`crate::CompressOptions::exclude`] active and at least one path was
    /// filtered out.  Always `0` for extraction and file-input compression.
    pub entries_excluded: u64,
    /// Wall-clock time the operation took.
    pub duration: Duration,
    /// SHA-256 digest of the compressed output artifact, as a lowercase hex
    /// string.
    ///
    /// `Some` only when [`crate::CompressOptions::checksum`] was `true`.
    /// `None` otherwise and for all extract operations.
    pub sha256: Option<String>,
    /// SHA-256 digest of the pre-compression content stream, as a lowercase
    /// hex string.
    ///
    /// `Some` only when [`crate::CompressOptions::checksum`] was `true` AND
    /// the operation has a single identifiable pre-compression byte stream:
    ///
    /// - **codec-only, file input** — digest of the input file bytes.
    /// - **tar + codec** — digest of the tar byte stream (before encoding).
    /// - **plain tar** — same bytes as `sha256` (both are always equal for
    ///   plain tar since the content *is* the artifact).
    ///
    /// `None` for zip, 7z, and when `checksum` is `false`.
    pub content_sha256: Option<String>,
}

impl Report {
    /// Compression ratio: `output_bytes / input_bytes`.
    ///
    /// A value below 1.0 means the output is smaller than the input (good).
    /// Returns `0.0` when `input_bytes` is zero to avoid a division by zero.
    pub fn ratio(&self) -> f64 {
        if self.input_bytes == 0 {
            0.0
        } else {
            self.output_bytes as f64 / self.input_bytes as f64
        }
    }
}

/// Metadata for a single entry inside an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path of the entry as stored in the archive.
    pub path: PathBuf,
    /// Uncompressed size of the entry, in bytes.
    pub size: u64,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

// ---------------------------------------------------------------------------
// copy_with_progress
// ---------------------------------------------------------------------------

/// Copy bytes from `r` to `w` in 64 KiB chunks, reporting progress and
/// honouring cancellation on every iteration.
///
/// After each successful read chunk the function:
///
/// 1. Checks [`OpCtx::check_cancel`]; returns [`crate::Error::Cancelled`] if
///    the token has been signalled.
/// 2. Writes the chunk to `w`.
/// 3. Calls [`OpCtx::add_bytes`] to advance `bytes_done` and fire the
///    `on_progress` callback.
///
/// Returns the total number of bytes copied.
///
/// # Errors
///
/// Returns [`crate::Error::Cancelled`] if the cancel token fires between
/// iterations, or [`crate::Error::Io`] for underlying I/O failures.
pub(crate) fn copy_with_progress(
    r: &mut dyn Read,
    w: &mut dyn Write,
    ctx: &mut OpCtx<'_>,
) -> crate::Result<u64> {
    const BUF_SIZE: usize = 64 * 1024; // 64 KiB
    let mut buf = [0u8; BUF_SIZE];
    let mut total: u64 = 0;

    loop {
        // Check cancellation before attempting a read.
        ctx.check_cancel()?;

        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        ctx.add_bytes(n as u64);
        total += n as u64;
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CancelToken ---

    #[test]
    fn cancel_token_clone_shares_state() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled());

        let handle = token.clone();
        assert!(!handle.is_cancelled());

        // Cancel via the original; the clone must observe it.
        token.cancel();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn cancel_token_cancel_via_clone_seen_by_original() {
        let token = CancelToken::default();
        let handle = token.clone();

        handle.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_is_idempotent() {
        let token = CancelToken::default();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    // --- Report::ratio ---

    #[test]
    fn ratio_normal_case() {
        let report = Report {
            input_bytes: 1000,
            output_bytes: 250,
            entries: 1,
            entries_excluded: 0,
            duration: Duration::from_millis(10),
            sha256: None,
            content_sha256: None,
        };
        let ratio = report.ratio();
        assert!(
            (ratio - 0.25).abs() < f64::EPSILON,
            "expected 0.25, got {ratio}"
        );
    }

    #[test]
    fn ratio_zero_input_returns_zero() {
        let report = Report {
            input_bytes: 0,
            output_bytes: 0,
            entries: 0,
            entries_excluded: 0,
            duration: Duration::ZERO,
            sha256: None,
            content_sha256: None,
        };
        assert_eq!(report.ratio(), 0.0);
    }

    #[test]
    fn ratio_output_larger_than_input() {
        // Compressing tiny incompressible data can expand slightly.
        let report = Report {
            input_bytes: 10,
            output_bytes: 18,
            entries: 1,
            entries_excluded: 0,
            duration: Duration::from_nanos(1),
            sha256: None,
            content_sha256: None,
        };
        assert!(report.ratio() > 1.0);
    }

    // --- copy_with_progress ---

    #[test]
    fn copy_with_progress_pre_cancelled_returns_cancelled() {
        use crate::archive::OpCtx;

        let token = CancelToken::default();
        // Signal cancellation before any bytes are available.
        token.cancel();

        let mut progress_calls: u64 = 0;
        let mut ctx = OpCtx {
            cancel: token,
            on_progress: &mut |_p| {
                progress_calls += 1;
            },
            progress: Progress {
                bytes_done: 0,
                bytes_total: None,
                current_entry: None,
            },
        };

        let data = b"hello world";
        let mut src: &[u8] = data;
        let mut dst = Vec::new();

        let result = super::copy_with_progress(&mut src, &mut dst, &mut ctx);
        assert!(
            matches!(result, Err(crate::Error::Cancelled)),
            "expected Cancelled, got {result:?}"
        );
        // No bytes must have been written.
        assert!(
            dst.is_empty(),
            "no bytes should be written after a pre-cancel"
        );
        // No progress callbacks should have fired.
        assert_eq!(progress_calls, 0);
    }

    #[test]
    fn copy_with_progress_copies_all_bytes() {
        use crate::archive::OpCtx;

        let token = CancelToken::default();
        let mut bytes_reported: u64 = 0;
        let mut ctx = OpCtx {
            cancel: token,
            on_progress: &mut |p| {
                bytes_reported = p.bytes_done;
            },
            progress: Progress {
                bytes_done: 0,
                bytes_total: None,
                current_entry: None,
            },
        };

        let data = b"hello world - copy_with_progress test payload";
        let mut src: &[u8] = data;
        let mut dst = Vec::new();

        let n = super::copy_with_progress(&mut src, &mut dst, &mut ctx).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(&dst, data);
        assert_eq!(bytes_reported, data.len() as u64);
    }
}
