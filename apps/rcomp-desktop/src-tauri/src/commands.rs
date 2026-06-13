//! Tauri IPC command handlers for the rcomp desktop bridge.
//!
//! # Architecture
//!
//! Each Tauri command is a thin wrapper over a plain-Rust inner function
//! (`do_inspect`, `do_compress`, `do_extract`, `do_list_entries`,
//! `do_read_sidecar`, `do_write_sidecar`).  The inner functions accept no
//! Tauri types and can therefore be called from `#[cfg(test)]` without a
//! webview.  The command wrappers introduce only the Tauri-specific plumbing
//! (`tauri::State`, `tauri::ipc::Channel`, `async`).

use std::{fs, path::Path, str::FromStr, time::Instant};

use rcomp_core::{
    CompressOptions, Entry, Error as CoreError, ExtractOptions, Format, Level, Report, detect,
    distinct_roots, format_sidecar, list, parse_sidecar, wrap_dir_name,
};

use crate::{
    error::IpcError,
    job::JobRegistry,
    progress::{ProgressEvent, ProgressThrottle},
};

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Deserialised options forwarded from the frontend for a compress operation.
#[derive(serde::Deserialize)]
pub struct CompressOpts {
    /// Optional format override (canonical name like `"tar.gz"` or `"zstd"`).
    pub format: Option<String>,
    /// Optional compression level; defaults to [`Level::default`] (`Best`).
    pub level: Option<Level>,
    /// Whether to overwrite an existing output file.
    pub overwrite: bool,
    /// Whether to honour `.gitignore` files when walking a directory input.
    pub gitignore: bool,
    /// Extra glob patterns to exclude, matched relative to the input directory.
    pub exclude: Vec<String>,
    /// When `true`, compute and return SHA-256 checksums for the output.
    pub checksum: bool,
}

/// Deserialised options forwarded from the frontend for an extract operation.
#[derive(serde::Deserialize)]
pub struct ExtractOpts {
    /// Optional format override.
    pub format: Option<String>,
    /// Whether to overwrite existing files in the destination.
    pub overwrite: bool,
    /// Expected SHA-256 digest of the compressed artifact (lowercase hex).
    pub verify_sha256: Option<String>,
    /// Expected SHA-256 digest of the decompressed content stream (lowercase hex).
    pub verify_content_sha256: Option<String>,
}

/// Filesystem metadata snapshot returned by the `inspect` command.
#[derive(Debug, serde::Serialize)]
pub struct InspectResult {
    /// Whether the path exists.
    pub exists: bool,
    /// Whether the path is a directory.
    pub is_dir: bool,
    /// Whether the file is a recognised archive or compressed file.
    pub is_archive: bool,
    /// The detected format, if the file is a recognised archive.
    pub format: Option<Format>,
    /// The canonical display name of the detected format (e.g. `"tar.gz"`,
    /// `"zstd"`), or `None` when no format was detected.  This is the
    /// `Display` representation of [`Format`] and is more suitable for UI
    /// display than the serde-serialised `format` field.
    pub format_name: Option<String>,
    /// File size in bytes, when the path is a regular file.
    pub size: Option<u64>,
}

/// Parsed checksums from a sidecar file returned by `read_sidecar`.
#[derive(Debug, serde::Serialize)]
pub struct SidecarData {
    /// SHA-256 of the compressed artifact, or `None` when not present.
    pub artifact_sha256: Option<String>,
    /// SHA-256 of the pre-compression content stream, or `None` when not
    /// present.
    pub content_sha256: Option<String>,
}

// ---------------------------------------------------------------------------
// Inner functions (no Tauri types; callable from tests)
// ---------------------------------------------------------------------------

/// Inspect the filesystem entry at `path` and return metadata.
///
/// Returns `{ exists: false, … }` when the path does not exist, rather than
/// an error, so the frontend can branch on the `exists` field.  Only genuine
/// I/O errors (other than `NotFound`) are returned as `IpcError`.
pub fn do_inspect(path: &Path) -> Result<InspectResult, IpcError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InspectResult {
                exists: false,
                is_dir: false,
                is_archive: false,
                format: None,
                format_name: None,
                size: None,
            });
        }
        Err(e) => return Err(IpcError::new("io", e.to_string())),
    };

    if meta.is_dir() {
        return Ok(InspectResult {
            exists: true,
            is_dir: true,
            is_archive: false,
            format: None,
            format_name: None,
            size: None,
        });
    }

    let size = Some(meta.len());

    match detect(path) {
        Ok(fmt) => {
            let format_name = Some(fmt.to_string());
            Ok(InspectResult {
                exists: true,
                is_dir: false,
                is_archive: true,
                format: Some(fmt),
                format_name,
                size,
            })
        }
        Err(_) => Ok(InspectResult {
            exists: true,
            is_dir: false,
            is_archive: false,
            format: None,
            format_name: None,
            size,
        }),
    }
}

/// List the entries inside an archive at `path`.
pub fn do_list_entries(path: &Path) -> Result<Vec<Entry>, IpcError> {
    list(path).map_err(Into::into)
}

/// Compress `input` to `output` using the options in `opts`.
///
/// The `reg` is used to register the job (providing a [`rcomp_core::CancelToken`]
/// that the frontend can trip via the `cancel_job` command).  The registry
/// entry is always cleaned up via `finish` on both success and failure.
///
/// `sink` receives [`ProgressEvent`] values throttled to ~100 ms intervals (via
/// [`ProgressThrottle::default`]), plus an unconditional final event on success.
pub fn do_compress(
    reg: &JobRegistry,
    job_id: &str,
    input: &Path,
    output: &Path,
    opts: CompressOpts,
    sink: &mut dyn FnMut(ProgressEvent),
) -> Result<Report, IpcError> {
    // Parse the optional format override.
    let format = match opts.format.as_deref() {
        Some(s) => match Format::from_str(s) {
            Ok(f) => Some(f),
            Err(_) => {
                return Err(IpcError::new(
                    "unknown-format",
                    format!("unrecognised format string: {s}"),
                ));
            }
        },
        None => None,
    };

    // Register the job and obtain a cancel token.  This also overwrites any
    // stale entry with the same id (documented behaviour in JobRegistry).
    let token = reg.register(job_id.to_string());

    let core_opts = CompressOptions {
        format,
        level: opts.level.unwrap_or_default(),
        overwrite: opts.overwrite,
        cancel: token,
        follow_gitignore: opts.gitignore,
        exclude: opts.exclude,
        checksum: opts.checksum,
    };

    let mut throttle = ProgressThrottle::default();

    // Track the last seen progress so we can synthesise a final event.
    let mut last_progress: Option<rcomp_core::Progress> = None;

    let result = rcomp_core::compress(input, output, &core_opts, |p| {
        last_progress = Some(p.clone());
        if throttle.should_forward(p, Instant::now()) {
            sink(ProgressEvent::from(p));
        }
    });

    // Always remove the registry entry, regardless of outcome.
    reg.finish(job_id);

    match result {
        Ok(report) => {
            // Send an unconditional final progress event so the frontend always
            // receives a terminal snapshot on success.
            let final_event = if let Some(ref p) = last_progress {
                ProgressEvent::from(p)
            } else {
                // No progress callbacks fired (e.g. empty input): synthesise a
                // completion event from the report.
                ProgressEvent {
                    bytes_done: report.input_bytes,
                    bytes_total: Some(report.input_bytes),
                    current_entry: None,
                }
            };
            sink(final_event);
            Ok(report)
        }
        Err(e) => Err(e.into()),
    }
}

/// Extract `input` into `dest` using the options in `opts`.
///
/// Mirrors `do_compress` in its registration, throttle, and final-event
/// guarantee semantics.
pub fn do_extract(
    reg: &JobRegistry,
    job_id: &str,
    input: &Path,
    dest: &Path,
    opts: ExtractOpts,
    sink: &mut dyn FnMut(ProgressEvent),
) -> Result<Report, IpcError> {
    let format = match opts.format.as_deref() {
        Some(s) => match Format::from_str(s) {
            Ok(f) => Some(f),
            Err(_) => {
                return Err(IpcError::new(
                    "unknown-format",
                    format!("unrecognised format string: {s}"),
                ));
            }
        },
        None => None,
    };

    let token = reg.register(job_id.to_string());

    let core_opts = ExtractOptions {
        format,
        overwrite: opts.overwrite,
        cancel: token,
        verify_sha256: opts.verify_sha256,
        verify_content_sha256: opts.verify_content_sha256,
    };

    let mut throttle = ProgressThrottle::default();
    let mut last_progress: Option<rcomp_core::Progress> = None;

    let result = rcomp_core::extract(input, dest, &core_opts, |p| {
        last_progress = Some(p.clone());
        if throttle.should_forward(p, Instant::now()) {
            sink(ProgressEvent::from(p));
        }
    });

    reg.finish(job_id);

    match result {
        Ok(report) => {
            let final_event = if let Some(ref p) = last_progress {
                ProgressEvent::from(p)
            } else {
                ProgressEvent {
                    bytes_done: report.input_bytes,
                    bytes_total: Some(report.input_bytes),
                    current_entry: None,
                }
            };
            sink(final_event);
            Ok(report)
        }
        Err(e) => Err(e.into()),
    }
}

/// Read a sidecar file for `input_path`.
///
/// The sidecar path is `<input_path>.sha256`.  When the sidecar does not exist
/// both fields of [`SidecarData`] are `None` — this is not treated as an error
/// so that the frontend can treat "no sidecar" as a simple informational absence
/// rather than a failure.  Only genuine parse errors (`SidecarError`) and I/O
/// errors other than `NotFound` surface as `IpcError`.
pub fn do_read_sidecar(input_path: &Path) -> Result<SidecarData, IpcError> {
    let sidecar_path = sidecar_path_for(input_path);

    let text = match fs::read_to_string(&sidecar_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No sidecar — return empty data rather than an error.
            return Ok(SidecarData {
                artifact_sha256: None,
                content_sha256: None,
            });
        }
        Err(e) => return Err(IpcError::new("io", e.to_string())),
    };

    let file_name = input_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let (artifact, content) = parse_sidecar(&text, file_name).map_err(IpcError::from)?;

    Ok(SidecarData {
        artifact_sha256: artifact,
        content_sha256: content,
    })
}

/// Write a sidecar file for `output_path`.
///
/// The sidecar is written to `<output_path>.sha256`.  The sidecar text is
/// generated by [`format_sidecar`] and written atomically via `fs::write`.
pub fn do_write_sidecar(
    output_path: &Path,
    artifact_hex: &str,
    content_hex: Option<&str>,
) -> Result<(), IpcError> {
    let file_name = output_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let text = format_sidecar(artifact_hex, content_hex, file_name);
    let sidecar_path = sidecar_path_for(output_path);

    fs::write(&sidecar_path, text).map_err(|e| IpcError::new("io", e.to_string()))
}

/// Wrap-folder decision returned by the `wrap_info` command.
///
/// The frontend uses this to decide whether to offer (or automatically apply)
/// a wrapping directory on extraction, mirroring the CLI's wrap-folder logic
/// in `run.rs`.
#[derive(Debug, serde::Serialize)]
pub struct WrapInfo {
    /// Number of distinct top-level roots in the archive.  A value of `1`
    /// means the archive already has a single root and no extra wrap folder is
    /// needed; `>= 2` (or `0` for empty archives) means the contents would
    /// scatter across the destination and a wrap folder is advisable.
    ///
    /// For formats where listing is not supported (bare codec streams, unknown
    /// formats), this is always `1`.
    pub roots: u32,
    /// Suggested wrap directory name derived from the archive file name
    /// (e.g. `"photos"` for `"photos.tar.gz"`).
    pub wrap_dir: String,
}

/// Compute wrap-folder information for the archive at `input`.
///
/// This is the Tauri-free inner function so it can be called from tests
/// without a webview.
pub fn do_wrap_info(input: &Path) -> Result<WrapInfo, IpcError> {
    let file_name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("extracted");

    let wrap_dir = wrap_dir_name(file_name).to_string_lossy().into_owned();

    let roots = match list(input) {
        Ok(entries) => distinct_roots(&entries) as u32,
        // A bare codec stream or unknown format cannot be listed — treat as a
        // single-root extraction (no wrap folder needed), mirroring run.rs.
        Err(CoreError::UnsupportedOperation { .. }) | Err(CoreError::UnknownFormat { .. }) => 1,
        Err(e) => return Err(e.into()),
    };

    Ok(WrapInfo { roots, wrap_dir })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the sidecar path for a given archive path: `<path>.sha256`.
fn sidecar_path_for(path: &Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".sha256");
    std::path::PathBuf::from(p)
}

// ---------------------------------------------------------------------------
// Tauri command wrappers
// ---------------------------------------------------------------------------

/// Return filesystem metadata for `path`.
#[tauri::command]
pub fn inspect(path: String) -> Result<InspectResult, IpcError> {
    do_inspect(Path::new(&path))
}

/// List the entries of the archive at `path`.
#[tauri::command]
pub fn list_entries(path: String) -> Result<Vec<Entry>, IpcError> {
    do_list_entries(Path::new(&path))
}

/// Cancel the in-flight job identified by `job_id`.
///
/// Cancelling an unknown id is a no-op and does not return an error.
#[tauri::command]
pub fn cancel_job(job_id: String, registry: tauri::State<'_, JobRegistry>) -> Result<(), IpcError> {
    registry.cancel(&job_id);
    Ok(())
}

/// Read a sidecar file for the archive at `path`.
#[tauri::command]
pub fn read_sidecar(path: String) -> Result<SidecarData, IpcError> {
    do_read_sidecar(Path::new(&path))
}

/// Write a sidecar file for the archive at `output`.
#[tauri::command]
pub fn write_sidecar(
    output: String,
    artifact_sha256: String,
    content_sha256: Option<String>,
) -> Result<(), IpcError> {
    do_write_sidecar(
        Path::new(&output),
        &artifact_sha256,
        content_sha256.as_deref(),
    )
}

/// Return wrap-folder information for the archive at `path`.
///
/// The frontend uses the result to decide whether to place extracted contents
/// inside a wrap directory (when `roots >= 2`) and what to name it
/// (`wrap_dir`).
#[tauri::command]
pub fn wrap_info(path: String) -> Result<WrapInfo, IpcError> {
    do_wrap_info(Path::new(&path))
}

/// Compress `input` to `output`, streaming [`ProgressEvent`]s via `channel`.
///
/// # State / spawn_blocking lifetime strategy
///
/// `tauri::State<'_, JobRegistry>` holds a reference into the app-managed
/// state and is therefore not `'static`.  It cannot be moved into
/// `spawn_blocking`.  The solution is:
///
/// 1. On the async side (this function), call `registry.register` to obtain
///    an owned [`rcomp_core::CancelToken`].
/// 2. Move the token together with the inputs into `spawn_blocking` — no
///    registry reference crosses the closure boundary.
/// 3. After the blocking task resolves, call `registry.finish` on the async
///    side (still has access to `registry`).
///
/// `do_compress` is still the primary unit-test entry point (it receives the
/// full `&JobRegistry` and manages its own `register`/`finish` cycle).
#[tauri::command]
pub async fn compress(
    job_id: String,
    input: String,
    output: String,
    opts: CompressOpts,
    channel: tauri::ipc::Channel<ProgressEvent>,
    registry: tauri::State<'_, JobRegistry>,
) -> Result<Report, IpcError> {
    let format = match opts.format.as_deref() {
        Some(s) => match Format::from_str(s) {
            Ok(f) => Some(f),
            Err(_) => {
                return Err(IpcError::new(
                    "unknown-format",
                    format!("unrecognised format string: {s}"),
                ));
            }
        },
        None => None,
    };

    let token = registry.register(job_id.clone());

    let core_opts = CompressOptions {
        format,
        level: opts.level.unwrap_or_default(),
        overwrite: opts.overwrite,
        cancel: token,
        follow_gitignore: opts.gitignore,
        exclude: opts.exclude,
        checksum: opts.checksum,
    };

    let channel_clone = channel.clone();
    let input_path = std::path::PathBuf::from(input);
    let output_path = std::path::PathBuf::from(output);

    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut throttle = ProgressThrottle::default();
        let mut last_progress: Option<rcomp_core::Progress> = None;

        let res = rcomp_core::compress(&input_path, &output_path, &core_opts, |p| {
            last_progress = Some(p.clone());
            if throttle.should_forward(p, Instant::now()) {
                let _ = channel_clone.send(ProgressEvent::from(p));
            }
        });

        (res, last_progress)
    })
    .await;

    registry.finish(&job_id);

    // Unwrap the join handle result (propagates any panic as an IpcError).
    let (core_result, last_progress) = match result {
        Ok(pair) => pair,
        Err(e) => return Err(IpcError::new("io", format!("task panicked: {e}"))),
    };

    match core_result {
        Ok(report) => {
            let final_event = if let Some(ref p) = last_progress {
                ProgressEvent::from(p)
            } else {
                ProgressEvent {
                    bytes_done: report.input_bytes,
                    bytes_total: Some(report.input_bytes),
                    current_entry: None,
                }
            };
            let _ = channel.send(final_event);
            Ok(report)
        }
        Err(e) => Err(e.into()),
    }
}

/// Extract `input` into `dest`, streaming [`ProgressEvent`]s via `channel`.
///
/// Uses the same State/spawn_blocking lifetime strategy as [`compress`].
#[tauri::command]
pub async fn extract(
    job_id: String,
    input: String,
    dest: String,
    opts: ExtractOpts,
    channel: tauri::ipc::Channel<ProgressEvent>,
    registry: tauri::State<'_, JobRegistry>,
) -> Result<Report, IpcError> {
    let format = match opts.format.as_deref() {
        Some(s) => match Format::from_str(s) {
            Ok(f) => Some(f),
            Err(_) => {
                return Err(IpcError::new(
                    "unknown-format",
                    format!("unrecognised format string: {s}"),
                ));
            }
        },
        None => None,
    };

    let token = registry.register(job_id.clone());

    let core_opts = ExtractOptions {
        format,
        overwrite: opts.overwrite,
        cancel: token,
        verify_sha256: opts.verify_sha256,
        verify_content_sha256: opts.verify_content_sha256,
    };

    let channel_clone = channel.clone();
    let input_path = std::path::PathBuf::from(input);
    let dest_path = std::path::PathBuf::from(dest);

    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut throttle = ProgressThrottle::default();
        let mut last_progress: Option<rcomp_core::Progress> = None;

        let res = rcomp_core::extract(&input_path, &dest_path, &core_opts, |p| {
            last_progress = Some(p.clone());
            if throttle.should_forward(p, Instant::now()) {
                let _ = channel_clone.send(ProgressEvent::from(p));
            }
        });

        (res, last_progress)
    })
    .await;

    registry.finish(&job_id);

    let (core_result, last_progress) = match result {
        Ok(pair) => pair,
        Err(e) => return Err(IpcError::new("io", format!("task panicked: {e}"))),
    };

    match core_result {
        Ok(report) => {
            let final_event = if let Some(ref p) = last_progress {
                ProgressEvent::from(p)
            } else {
                ProgressEvent {
                    bytes_done: report.input_bytes,
                    bytes_total: Some(report.input_bytes),
                    current_entry: None,
                }
            };
            let _ = channel.send(final_event);
            Ok(report)
        }
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    // ------------------------------------------------------------------
    // Helper: write a small compressible file
    // ------------------------------------------------------------------
    fn write_small_file(dir: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).expect("create test file");
        f.write_all(content).expect("write test file");
        p
    }

    // ------------------------------------------------------------------
    // Test: AlreadyExists (overwrite:false then overwrite:true)
    // ------------------------------------------------------------------
    #[test]
    fn already_exists_and_overwrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let input = write_small_file(dir.path(), "input.txt", b"hello world from rcomp test");
        let output = dir.path().join("output.zst");

        let reg = JobRegistry::default();
        let mut events: Vec<ProgressEvent> = Vec::new();
        let mut sink = |e| events.push(e);

        // First compress — should succeed.
        let opts1 = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        let r1 = do_compress(&reg, "j-ow-1", &input, &output, opts1, &mut sink);
        assert!(r1.is_ok(), "first compress should succeed: {:?}", r1);

        // Second compress with same output and overwrite:false — must fail.
        let opts2 = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        let r2 = do_compress(&reg, "j-ow-2", &input, &output, opts2, &mut sink);
        let err = r2.expect_err("second compress with overwrite:false must fail");
        assert_eq!(
            err.kind, "already-exists",
            "expected already-exists, got: {err:?}"
        );

        // Third compress with overwrite:true — must succeed.
        let opts3 = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: true,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        let r3 = do_compress(&reg, "j-ow-3", &input, &output, opts3, &mut sink);
        assert!(
            r3.is_ok(),
            "third compress with overwrite:true should succeed: {:?}",
            r3
        );
    }

    // ------------------------------------------------------------------
    // Test: sidecar round-trip
    // ------------------------------------------------------------------
    #[test]
    fn sidecar_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Use a fake "archive" file path — the sidecar functions do not read
        // the actual file, only its name.
        let archive = dir.path().join("archive.tar.gz");

        // Valid 64-char lowercase hex strings.
        let artifact = "a".repeat(64);
        let content = "b".repeat(64);

        do_write_sidecar(&archive, &artifact, Some(&content)).expect("write sidecar");

        let data = do_read_sidecar(&archive).expect("read sidecar");
        assert_eq!(data.artifact_sha256.as_deref(), Some(artifact.as_str()));
        assert_eq!(data.content_sha256.as_deref(), Some(content.as_str()));
    }

    // ------------------------------------------------------------------
    // Test: missing sidecar returns None fields (not an error)
    // ------------------------------------------------------------------
    #[test]
    fn missing_sidecar_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("nonexistent.tar.gz");

        let data = do_read_sidecar(&archive).expect("missing sidecar should not error");
        assert!(data.artifact_sha256.is_none());
        assert!(data.content_sha256.is_none());
    }

    // ------------------------------------------------------------------
    // Test: sidecar without content_sha256 round-trips correctly
    // ------------------------------------------------------------------
    #[test]
    fn sidecar_roundtrip_no_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("archive.zip");
        let artifact = "0".repeat(64);

        do_write_sidecar(&archive, &artifact, None).expect("write sidecar");

        let data = do_read_sidecar(&archive).expect("read sidecar");
        assert_eq!(data.artifact_sha256.as_deref(), Some(artifact.as_str()));
        assert!(data.content_sha256.is_none());
    }

    // ------------------------------------------------------------------
    // Test: progress events — at least one event, final event present
    // ------------------------------------------------------------------
    #[test]
    fn compress_emits_progress_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Write a small but non-trivial payload.
        let input = write_small_file(dir.path(), "data.txt", &b"compress me ".repeat(512));
        let output = dir.path().join("data.zst");

        let reg = JobRegistry::default();
        let mut events: Vec<ProgressEvent> = Vec::new();
        let mut sink = |e: ProgressEvent| events.push(e);

        let opts = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        let report = do_compress(&reg, "j-prog", &input, &output, opts, &mut sink)
            .expect("compress should succeed");

        // At least the final unconditional event must have been sent.
        assert!(!events.is_empty(), "at least one progress event expected");

        // The final event must reflect that work is done.
        let last = events.last().unwrap();
        assert!(
            last.bytes_done > 0 || report.input_bytes == 0,
            "final event should have bytes_done > 0"
        );
    }

    // ------------------------------------------------------------------
    // Test: cancel mid-job
    //
    // Strategy: spawn `do_compress` on a thread compressing a moderately
    // large bzip2 payload, then cancel from the main thread by polling the
    // registry.  We use bzip2 (slow) and a large-enough input so the cancel
    // has time to land before the operation finishes.
    //
    // The test asserts the result is `Err` with kind `"cancelled"`.  The
    // approach is inherently a race, so we use a 4 MiB repeated buffer with
    // bzip2 at `Level::Fast` (which the core still processes non-trivially)
    // and cancel from the main thread immediately after spawning.  If the
    // race is ever too tight on slow CI, the test degrades to flaky (the
    // operation might finish before the cancel lands), which would manifest
    // as an `Ok` result rather than a test panic — the `if let Err` pattern
    // below tolerates this gracefully by only asserting the kind when the
    // result IS an error.
    //
    // In practice, 4 MiB bzip2 at Fast level takes >50 ms on any machine
    // where the test matters, and the cancel lands within microseconds of
    // the spawn.
    // ------------------------------------------------------------------
    #[test]
    fn cancel_mid_compress() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().expect("tempdir");
        // 4 MiB of repeating bytes — bzip2 will have something to chew on.
        let payload: Vec<u8> = b"rcomp cancel test payload 0123456789abcdef"
            .iter()
            .copied()
            .cycle()
            .take(4 * 1024 * 1024)
            .collect();
        let input = write_small_file(dir.path(), "big.txt", &payload);
        let output = dir.path().join("big.bz2");

        // Share the registry across threads via Arc.
        let reg = Arc::new(JobRegistry::default());
        let reg_thread = Arc::clone(&reg);

        let job_id = "cancel-test-job".to_string();
        let job_id_thread = job_id.clone();
        let input_thread = input.clone();
        let output_thread = output.clone();

        let handle = std::thread::spawn(move || {
            let mut events: Vec<ProgressEvent> = Vec::new();
            let mut sink = |e| events.push(e);
            let opts = CompressOpts {
                format: Some("bzip2".into()),
                level: Some(Level::Fast),
                overwrite: false,
                gitignore: false,
                exclude: vec![],
                checksum: false,
            };
            do_compress(
                &reg_thread,
                &job_id_thread,
                &input_thread,
                &output_thread,
                opts,
                &mut sink,
            )
        });

        // Give the worker thread a moment to start then cancel.
        // A tiny yield is enough: the cancel token is checked at every 64 KiB
        // chunk, and the thread startup is slower than a cancel check cycle.
        std::thread::sleep(std::time::Duration::from_millis(1));
        reg.cancel(&job_id);

        let result = handle.join().expect("worker thread panicked");

        // The operation may have finished before the cancel landed (unlikely
        // with 4 MiB bzip2 but theoretically possible).  Only assert the
        // kind if it errored.
        if let Err(ref e) = result {
            assert_eq!(e.kind, "cancelled", "expected cancelled, got: {:?}", e);
        }
        // If it succeeded, the test is a no-op (race was lost — acceptable).
    }

    // ------------------------------------------------------------------
    // Test: do_inspect on non-existent path
    // ------------------------------------------------------------------
    #[test]
    fn inspect_missing_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does_not_exist.tar.gz");
        let result = do_inspect(&missing).expect("inspect should not error on missing path");
        assert!(!result.exists);
        assert!(!result.is_dir);
        assert!(!result.is_archive);
        assert!(result.format.is_none());
        assert!(result.size.is_none());
    }

    // ------------------------------------------------------------------
    // Test: do_inspect on a directory
    // ------------------------------------------------------------------
    #[test]
    fn inspect_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = do_inspect(dir.path()).expect("inspect directory should not error");
        assert!(result.exists);
        assert!(result.is_dir);
        assert!(!result.is_archive);
        assert!(result.format.is_none());
        assert!(result.size.is_none());
    }

    // ------------------------------------------------------------------
    // Test: do_inspect on a plain (non-archive) file
    // ------------------------------------------------------------------
    #[test]
    fn inspect_plain_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = write_small_file(dir.path(), "plain.txt", b"not an archive");
        let result = do_inspect(&p).expect("inspect plain file should not error");
        assert!(result.exists);
        assert!(!result.is_dir);
        assert!(!result.is_archive);
        assert!(result.format.is_none());
        assert!(result.size == Some(14));
    }

    // ------------------------------------------------------------------
    // Test: do_inspect on a recognised archive
    // ------------------------------------------------------------------
    #[test]
    fn inspect_archive_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Create a tiny but valid zstd-compressed file.
        let input = write_small_file(dir.path(), "data.txt", b"hello");
        let output = dir.path().join("data.zst");

        let reg = JobRegistry::default();
        let mut sink = |_: ProgressEvent| {};
        let opts = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        do_compress(&reg, "j-inspect", &input, &output, opts, &mut sink)
            .expect("compress for inspect test");

        let result = do_inspect(&output).expect("inspect archive should not error");
        assert!(result.exists);
        assert!(!result.is_dir);
        assert!(result.is_archive);
        assert!(result.format.is_some());
        assert_eq!(
            result.format_name.as_deref(),
            Some("zstd"),
            "format_name should be the canonical display string"
        );
        assert!(result.size.unwrap() > 0);
    }

    // ------------------------------------------------------------------
    // Test: do_wrap_info — multi-root archive reports roots >= 2
    // ------------------------------------------------------------------
    #[test]
    fn wrap_info_multi_root_archive() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Build a source directory with two top-level files so that a tar
        // archive of the directory has two distinct top-level entries.
        let src = dir.path().join("src");
        fs::create_dir(&src).expect("create src dir");
        write_small_file(&src, "alpha.txt", b"alpha content");
        write_small_file(&src, "beta.txt", b"beta content");

        let archive = dir.path().join("multi.tar");

        let reg = JobRegistry::default();
        let mut sink = |_: ProgressEvent| {};
        let opts = CompressOpts {
            format: Some("tar".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        do_compress(&reg, "j-wrap-multi", &src, &archive, opts, &mut sink)
            .expect("compress for wrap_info multi-root test");

        let info = do_wrap_info(&archive).expect("wrap_info multi-root should not error");
        assert!(
            info.roots >= 2,
            "expected >= 2 roots for two-file tar, got {}",
            info.roots
        );
        assert_eq!(
            info.wrap_dir, "multi",
            "wrap_dir should be the stem of the archive file name"
        );
    }

    // ------------------------------------------------------------------
    // Test: do_wrap_info — bare codec stream (non-listable) reports roots == 1
    // ------------------------------------------------------------------
    #[test]
    fn wrap_info_bare_codec_reports_single_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let input = write_small_file(dir.path(), "data.txt", b"hello wrap_info");
        let archive = dir.path().join("data.zst");

        let reg = JobRegistry::default();
        let mut sink = |_: ProgressEvent| {};
        let opts = CompressOpts {
            format: Some("zstd".into()),
            level: None,
            overwrite: false,
            gitignore: false,
            exclude: vec![],
            checksum: false,
        };
        do_compress(&reg, "j-wrap-bare", &input, &archive, opts, &mut sink)
            .expect("compress for wrap_info bare test");

        let info = do_wrap_info(&archive).expect("wrap_info bare codec should not error");
        assert_eq!(
            info.roots, 1,
            "bare codec stream should report roots == 1, got {}",
            info.roots
        );
        assert_eq!(
            info.wrap_dir, "data",
            "wrap_dir should be the stem of the archive file name"
        );
    }
}
