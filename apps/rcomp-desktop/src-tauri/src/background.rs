//! File-manager context-menu actions run in the background, without a window.
//!
//! "Compress with Rcomp" and "Decompress with Rcomp" (Finder Quick Actions,
//! Explorer verbs, Dolphin/Nemo/Nautilus actions) reach the app as
//! [`LaunchRequest`]s with [`LaunchAction::Compress`] / [`LaunchAction::Decompress`].
//! Instead of opening the main window they are queued here and run one at a
//! time on a worker thread with the user's saved defaults:
//!
//! - **Decompress** extracts each archive next to itself. An archive with one
//!   top-level item is unpacked as is; one with several (or whose single item
//!   would clash with something already there) goes into a new folder named
//!   after the archive, made unique (`photos 2`, `photos 3`, …). Nothing is
//!   ever overwritten.
//! - **Compress** bundles the selection into one archive beside the first item
//!   with the default format from Settings (or zstd / tar.zst), named after the
//!   item (or `archive`) and made unique the same way.  Selections that arrive
//!   close together (Windows launches once per selected item) form one archive.
//!
//! Each job ends with a system notification.  While jobs run, a tray / menu bar
//! icon (macOS and Windows) shows progress and offers **Cancel**.  When the app
//! was started only for these jobs and no window is open, it quits once the
//! queue is empty.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rcomp_core::{
    AccelerationPreference, Codec, CompressOptions, Container, Error as CoreError, ExtractOptions,
    Format, Level, Progress, detect, list, wrap_dir_name,
};
use tauri::{AppHandle, Manager as _};
use tauri_plugin_notification::NotificationExt as _;

use crate::open_files::{LaunchAction, LaunchRequest};

/// How long to wait for more compress requests before starting the archive:
/// Explorer launches the app once per selected item.
const BATCH_WINDOW: Duration = Duration::from_millis(400);

/// A job shows the tray / menu bar progress icon once it has run this long.
const TRAY_DELAY: Duration = Duration::from_secs(1);
/// How often the progress icon's text is refreshed.
const TRAY_REFRESH: Duration = Duration::from_millis(500);

/// File (in the app's config directory) holding the defaults the frontend
/// saves from Settings, so background jobs use the same ones.
const PREFERENCES_FILE: &str = "context-menu.json";

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

/// The Settings defaults background jobs follow.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Pinned compression format (canonical name such as `"tar.gz"`), or
    /// `None` for the automatic choice.
    #[serde(default)]
    pub default_format: Option<String>,
    /// Hardware-acceleration preference.
    #[serde(default)]
    pub acceleration: AccelerationPreference,
}

fn preferences_path(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join(PREFERENCES_FILE))
}

fn load_preferences(app: &AppHandle) -> Preferences {
    preferences_path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Save the Settings defaults for context-menu jobs.  Called by the frontend
/// on startup and whenever the user changes them.
#[tauri::command]
pub async fn save_background_preferences(
    app: AppHandle,
    preferences: Preferences,
) -> Result<(), crate::error::IpcError> {
    let path = preferences_path(&app)
        .ok_or_else(|| crate::error::IpcError::new("io", "no app config directory"))?;
    if load_preferences(&app) == preferences {
        return Ok(());
    }
    let io = |e: std::io::Error| crate::error::IpcError::new("io", e.to_string());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let json = serde_json::to_vec_pretty(&preferences).expect("preferences serialise");
    std::fs::write(&path, json).map_err(io)
}

// ---------------------------------------------------------------------------
// Planning (pure, no Tauri)
// ---------------------------------------------------------------------------

/// `dir/<stem><ext>`, or `dir/<stem> 2<ext>`, `dir/<stem> 3<ext>`, … — the
/// first that does not exist.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}{ext}"));
    if first.symlink_metadata().is_err() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|p| p.symlink_metadata().is_err())
        .expect("some name is free")
}

/// File extension (with the dot) for a compression format.
pub fn extension(format: Format) -> String {
    let codec = |c: Codec| match c {
        Codec::Gzip => "gz",
        Codec::Bzip2 => "bz2",
        Codec::Xz => "xz",
        Codec::Zstd => "zst",
        Codec::Lz4 => "lz4",
        Codec::Brotli => "br",
    };
    match (format.container, format.codec) {
        (Some(Container::Tar), Some(c)) => format!(".tar.{}", codec(c)),
        (None, Some(c)) => format!(".{}", codec(c)),
        (Some(Container::Tar), None) => ".tar".into(),
        (Some(Container::Zip), _) => ".zip".into(),
        (Some(Container::SevenZ), _) => ".7z".into(),
        (Some(Container::Rar), _) => ".rar".into(),
        (None, None) => String::new(),
    }
}

/// Where a compress job writes, and with which format.
#[derive(Debug, PartialEq, Eq)]
pub struct CompressPlan {
    /// Format to write.
    pub format: Format,
    /// The new archive; never an existing path.
    pub output: PathBuf,
}

/// Plan compressing `inputs` with the pinned format `preferred` (or the
/// automatic choice: zstd for one file, tar.zst otherwise).
///
/// A single-stream codec cannot hold a folder or several items, so for those
/// it is layered over tar (`zstd` → `tar.zst`) and the name says so, rather
/// than writing a `.zst` that is secretly a tarball.
pub fn plan_compress(inputs: &[PathBuf], preferred: Option<&str>) -> Result<CompressPlan, String> {
    let first = inputs.first().ok_or("nothing to compress")?;
    let bundle = inputs.len() > 1 || first.is_dir();
    let format = match preferred.and_then(|s| Format::from_str(s).ok()) {
        Some(f) if bundle && f.container.is_none() => Format::layered(
            Container::Tar,
            f.codec.expect("a format without a container has a codec"),
        ),
        Some(f) => f,
        None if bundle => Format::layered(Container::Tar, Codec::Zstd),
        None => Format::codec(Codec::Zstd),
    };
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = if inputs.len() == 1 {
        first
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".into())
    } else {
        "archive".into()
    };
    Ok(CompressPlan {
        format,
        output: unique_path(dir, &stem, &extension(format)),
    })
}

/// Where an extract job writes.
#[derive(Debug, PartialEq, Eq)]
pub struct ExtractPlan {
    /// Directory handed to the extractor.
    pub dest: PathBuf,
    /// What the user will find afterwards: the archive's single top-level
    /// item, the decompressed file, or the new wrap folder.
    pub result: PathBuf,
}

/// Plan extracting `input` next to itself without overwriting anything.
///
/// One top-level item (or a single compressed file) lands beside the archive
/// unless that name is taken; several items, or a taken name, go into a new
/// folder named after the archive (`photos`, `photos 2`, …).
pub fn plan_extract(input: &Path) -> Result<ExtractPlan, CoreError> {
    let parent = input.parent().unwrap_or(Path::new(".")).to_path_buf();
    let file_name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = wrap_dir_name(&file_name).to_string_lossy().into_owned();

    let single = match list(input) {
        Ok(entries) => {
            let mut roots: Vec<PathBuf> = Vec::new();
            for entry in &entries {
                use std::path::Component;
                let first = entry.path.components().find(|c| *c != Component::CurDir);
                if let Some(Component::Normal(name)) = first {
                    let root = PathBuf::from(name);
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                }
            }
            (roots.len() == 1).then(|| roots.remove(0))
        }
        // A single compressed file (or a stream that can't be listed): it
        // decompresses to one file named after the archive.
        Err(CoreError::UnsupportedOperation { .. } | CoreError::UnknownFormat { .. }) => {
            Some(PathBuf::from(&stem))
        }
        Err(e) => return Err(e),
    };

    if let Some(root) = single {
        let target = parent.join(&root);
        if target.symlink_metadata().is_err() {
            return Ok(ExtractPlan {
                dest: parent,
                result: target,
            });
        }
    }
    let folder = unique_path(&parent, &stem, "");
    Ok(ExtractPlan {
        dest: folder.clone(),
        result: folder,
    })
}

// ---------------------------------------------------------------------------
// Queue and worker
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Job {
    Compress(Vec<PathBuf>),
    Extract(PathBuf),
}

impl Job {
    fn label(&self) -> String {
        match self {
            Job::Compress(paths) if paths.len() == 1 => format!("Compressing {}", name(&paths[0])),
            Job::Compress(paths) => format!("Compressing {} items", paths.len()),
            Job::Extract(path) => format!("Extracting {}", name(path)),
        }
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<Job>,
    /// Compress paths waiting for [`BATCH_WINDOW`] to pass.
    batch: Vec<PathBuf>,
    /// Bumped on every compress request so only the last timer starts the job.
    batch_generation: u64,
    worker_running: bool,
    /// Whether any context-menu request has arrived since launch.
    received: bool,
    /// Cancel token of the job in progress.
    current: Option<rcomp_core::CancelToken>,
}

/// Background job queue (Tauri managed state).
#[derive(Default)]
pub struct Background {
    state: Mutex<State>,
}

impl Background {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether any job is queued, batching or running.
    pub fn is_busy(&self) -> bool {
        let s = self.lock();
        s.worker_running || !s.queue.is_empty() || !s.batch.is_empty()
    }

    /// Whether a context-menu request has arrived since the app launched.
    pub fn has_received_requests(&self) -> bool {
        self.lock().received
    }

    /// Cancel the running job and drop the queued ones.
    pub fn cancel_all(&self) {
        let mut s = self.lock();
        s.queue.clear();
        s.batch.clear();
        s.batch_generation += 1;
        if let Some(token) = &s.current {
            token.cancel();
        }
    }
}

/// Queue a compress or decompress request.  `Open` requests are ignored here
/// (they go to the window; see `open_files::dispatch`).
pub fn submit(app: &AppHandle, request: LaunchRequest) {
    let bg = app.state::<Background>();
    bg.lock().received = true;
    let paths = request.paths.into_iter().map(PathBuf::from);
    match request.action {
        LaunchAction::Open => {}
        LaunchAction::Decompress => {
            bg.lock().queue.extend(paths.map(Job::Extract));
            ensure_worker(app);
        }
        LaunchAction::Compress => {
            let generation = {
                let mut s = bg.lock();
                for p in paths {
                    if !s.batch.contains(&p) {
                        s.batch.push(p);
                    }
                }
                s.batch_generation += 1;
                s.batch_generation
            };
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(BATCH_WINDOW);
                let bg = app.state::<Background>();
                {
                    let mut s = bg.lock();
                    if s.batch_generation != generation || s.batch.is_empty() {
                        return;
                    }
                    let batch = std::mem::take(&mut s.batch);
                    s.queue.push_back(Job::Compress(batch));
                }
                ensure_worker(&app);
            });
        }
    }
}

fn ensure_worker(app: &AppHandle) {
    {
        let bg = app.state::<Background>();
        let mut s = bg.lock();
        if s.worker_running || s.queue.is_empty() {
            return;
        }
        s.worker_running = true;
    }
    let app = app.clone();
    std::thread::spawn(move || worker(app));
}

fn worker(app: AppHandle) {
    let prefs = load_preferences(&app);
    let tray = Arc::new(Mutex::new(tray::Tray::new()));
    loop {
        let (job, cancel) = {
            let bg = app.state::<Background>();
            let mut s = bg.lock();
            let Some(job) = s.queue.pop_front() else {
                s.worker_running = false;
                s.current = None;
                break;
            };
            let cancel = rcomp_core::CancelToken::default();
            s.current = Some(cancel.clone());
            (job, cancel)
        };

        // Codecs report progress irregularly (some only at the end), so a
        // timer, not the callback, drives the tray icon.
        let label = job.label();
        let latest: Arc<Mutex<Option<Progress>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let ticker = {
            let (app, tray, latest, done) = (
                app.clone(),
                Arc::clone(&tray),
                Arc::clone(&latest),
                Arc::clone(&done),
            );
            let label = label.clone();
            std::thread::spawn(move || {
                std::thread::sleep(TRAY_DELAY);
                while !done.load(Ordering::Relaxed) {
                    let text = match &*latest.lock().unwrap_or_else(|e| e.into_inner()) {
                        Some(p) => status_text(&label, p),
                        None => format!("{label}…"),
                    };
                    tray.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .update(&app, &text);
                    std::thread::sleep(TRAY_REFRESH);
                }
            })
        };
        let mut on_progress = |p: &Progress| {
            *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
        };

        let outcome = run_job(&job, &prefs, cancel, &mut on_progress);
        done.store(true, Ordering::Relaxed);
        let _ = ticker.join();
        notify_outcome(&app, &job, outcome);
    }
    tray.lock().unwrap_or_else(|e| e.into_inner()).remove(&app);
    quit_if_idle(&app);
}

fn status_text(label: &str, p: &Progress) -> String {
    match p.bytes_total {
        Some(total) if total > 0 => {
            let pct = (p.bytes_done.min(total) * 100) / total;
            format!("{label} — {pct}%")
        }
        _ => format!("{label}…"),
    }
}

enum Outcome {
    Compressed { output: PathBuf, bytes: u64 },
    Extracted { result: PathBuf },
    Cancelled,
    Failed(String),
}

fn run_job(
    job: &Job,
    prefs: &Preferences,
    cancel: rcomp_core::CancelToken,
    on_progress: &mut dyn FnMut(&Progress),
) -> Outcome {
    let engine = crate::commands::operation_engine(prefs.acceleration);
    let result = match job {
        Job::Compress(inputs) => {
            let plan = match plan_compress(inputs, prefs.default_format.as_deref()) {
                Ok(plan) => plan,
                Err(message) => return Outcome::Failed(message),
            };
            let opts = CompressOptions {
                format: Some(plan.format),
                level: Level::default(),
                overwrite: false,
                cancel,
                follow_gitignore: true,
                exclude: Vec::new(),
                checksum: false,
                acceleration: prefs.acceleration,
            };
            engine
                .compress_many(inputs, &plan.output, &opts, on_progress)
                .map(|report| Outcome::Compressed {
                    output: plan.output,
                    bytes: report.output_bytes,
                })
        }
        Job::Extract(input) => {
            if detect(input).is_err() {
                return Outcome::Failed(format!(
                    "\u{201c}{}\u{201d} isn't an archive or compressed file Rcomp can open.",
                    name(input)
                ));
            }
            let (verify_sha256, verify_content_sha256) = match read_sidecar(input) {
                Ok(digests) => digests,
                Err(message) => return Outcome::Failed(message),
            };
            let plan = match plan_extract(input) {
                Ok(plan) => plan,
                Err(e) => return Outcome::Failed(e.to_string()),
            };
            let opts = ExtractOptions {
                format: None,
                overwrite: false,
                cancel,
                verify_sha256,
                verify_content_sha256,
                acceleration: prefs.acceleration,
                entries: Vec::new(),
            };
            engine
                .extract(input, &plan.dest, &opts, on_progress)
                .map(|_| Outcome::Extracted {
                    result: plan.result,
                })
        }
    };
    match result {
        Ok(outcome) => outcome,
        Err(CoreError::Cancelled) => Outcome::Cancelled,
        Err(CoreError::ChecksumMismatch { .. }) => Outcome::Failed(
            "Its checksum doesn't match the .sha256 file beside it: the archive may be corrupt or tampered with."
                .into(),
        ),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Digests from `<input>.sha256`, when present, to verify against.
fn read_sidecar(input: &Path) -> Result<(Option<String>, Option<String>), String> {
    match crate::commands::do_read_sidecar(input) {
        Ok(data) => Ok((data.artifact_sha256, data.content_sha256)),
        Err(e) => Err(format!("Its .sha256 file could not be read: {}", e.message)),
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn folder_name(path: &Path) -> String {
    path.parent().map(name).unwrap_or_default()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn notify_outcome(app: &AppHandle, job: &Job, outcome: Outcome) {
    let (title, body) = match (job, outcome) {
        (_, Outcome::Cancelled) => return,
        (_, Outcome::Compressed { output, bytes }) => (
            format!("Compressed {}", name(&output)),
            format!("{} in {}", human_size(bytes), folder_name(&output)),
        ),
        (_, Outcome::Extracted { result }) => (
            format!("Extracted {}", name(&result)),
            format!("In {}", folder_name(&result)),
        ),
        (Job::Compress(paths), Outcome::Failed(message)) => (
            if paths.len() == 1 {
                format!("Couldn't compress {}", name(&paths[0]))
            } else {
                format!("Couldn't compress {} items", paths.len())
            },
            message,
        ),
        (Job::Extract(path), Outcome::Failed(message)) => {
            (format!("Couldn't extract {}", name(path)), message)
        }
    };

    let _ = app.notification().builder().title(title).body(body).show();
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// How long the app stays after its last job before quitting.  Notifications
/// are posted asynchronously, and Linux notification servers (GNOME's among
/// them) drop a notification once the app that sent it exits, so there the app
/// waits until a typical notification has been read.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const QUIT_DELAY: Duration = Duration::from_millis(1500);
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const QUIT_DELAY: Duration = Duration::from_secs(10);

/// Quit when the app has nothing left to do: no window and no jobs.
pub fn quit_if_idle(app: &AppHandle) {
    if crate::window::is_open(app) || app.state::<Background>().is_busy() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(QUIT_DELAY);
        if !crate::window::is_open(&app) && !app.state::<Background>().is_busy() {
            app.exit(0);
        }
    });
}

// ---------------------------------------------------------------------------
// Tray / menu bar progress (macOS and Windows)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray {
    use tauri::{
        AppHandle, Manager as _,
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };

    const ID: &str = "rcomp-background";
    const CANCEL_ID: &str = "rcomp-background-cancel";

    /// The progress icon, created the first time a job reports progress
    /// after running for a second.
    #[derive(Default)]
    pub struct Tray {
        status: Option<MenuItem<tauri::Wry>>,
    }

    impl Tray {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn update(&mut self, app: &AppHandle, text: &str) {
            if let Some(status) = &self.status {
                let _ = status.set_text(text);
                if let Some(icon) = app.tray_by_id(ID) {
                    let _ = icon.set_tooltip(Some(text));
                }
                return;
            }
            let Ok(status) =
                MenuItem::with_id(app, "rcomp-background-status", text, false, None::<&str>)
            else {
                return;
            };
            let Ok(cancel) = MenuItem::with_id(app, CANCEL_ID, "Cancel", true, None::<&str>) else {
                return;
            };
            let Ok(menu) = Menu::with_items(app, &[&status, &cancel]) else {
                return;
            };
            let mut builder = TrayIconBuilder::with_id(ID)
                .tooltip(text)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    if event.id() == CANCEL_ID {
                        app.state::<super::Background>().cancel_all();
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
            if builder.build(app).is_ok() {
                self.status = Some(status);
            }
        }

        pub fn remove(&mut self, app: &AppHandle) {
            if self.status.take().is_some() {
                let _ = app.remove_tray_by_id(ID);
            }
        }
    }
}

/// Linux has no tray progress: tray icons need an extra library there that
/// many desktops lack.  Notifications still report each result.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod tray {
    use tauri::AppHandle;

    pub struct Tray;

    impl Tray {
        pub fn new() -> Self {
            Self
        }

        pub fn update(&mut self, _app: &AppHandle, _text: &str) {}
        pub fn remove(&mut self, _app: &AppHandle) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_counts_up_from_two() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "a", ".zip"),
            dir.path().join("a.zip")
        );
        std::fs::write(dir.path().join("a.zip"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a", ".zip"),
            dir.path().join("a 2.zip")
        );
        std::fs::write(dir.path().join("a 2.zip"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a", ".zip"),
            dir.path().join("a 3.zip")
        );
    }

    #[test]
    fn compress_defaults_by_selection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        let folder = dir.path().join("photos");
        std::fs::write(&file, b"x").unwrap();
        std::fs::create_dir(&folder).unwrap();

        let plan = plan_compress(std::slice::from_ref(&file), None).unwrap();
        assert_eq!(plan.format, Format::codec(Codec::Zstd));
        assert_eq!(plan.output, dir.path().join("notes.txt.zst"));

        let plan = plan_compress(std::slice::from_ref(&folder), None).unwrap();
        assert_eq!(plan.format, Format::layered(Container::Tar, Codec::Zstd));
        assert_eq!(plan.output, dir.path().join("photos.tar.zst"));

        let plan = plan_compress(&[file.clone(), folder.clone()], Some("zip")).unwrap();
        assert_eq!(plan.output, dir.path().join("archive.zip"));
    }

    #[test]
    fn a_pinned_codec_is_tarred_for_folders_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("photos");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(dir.path().join("photos.tar.gz"), b"").unwrap();

        let plan = plan_compress(std::slice::from_ref(&folder), Some("gzip")).unwrap();
        assert_eq!(plan.format, Format::layered(Container::Tar, Codec::Gzip));
        assert_eq!(plan.output, dir.path().join("photos 2.tar.gz"));
    }

    fn make_zip(dir: &Path, name: &str, files: &[&str]) -> PathBuf {
        let src = dir.join(format!("{name}-src"));
        let mut inputs = Vec::new();
        for f in files {
            let p = src.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"data").unwrap();
            let top = src.join(Path::new(f).components().next().unwrap());
            if !inputs.contains(&top) {
                inputs.push(top);
            }
        }
        let out = dir.join(format!("{name}.zip"));
        rcomp_core::Engine::default()
            .compress_many(
                &inputs,
                &out,
                &CompressOptions {
                    format: Some(Format::container(Container::Zip)),
                    ..Default::default()
                },
                |_| {},
            )
            .unwrap();
        std::fs::remove_dir_all(&src).unwrap();
        out
    }

    #[test]
    fn extract_plans_beside_the_archive_or_into_a_unique_folder() {
        let dir = tempfile::tempdir().unwrap();

        // One top-level folder: unpacked as is.
        let single = make_zip(dir.path(), "single", &["photos/a.jpg", "photos/b.jpg"]);
        let plan = plan_extract(&single).unwrap();
        assert_eq!(plan.dest, dir.path());
        assert_eq!(plan.result, dir.path().join("photos"));

        // ...unless that name is taken.
        std::fs::create_dir(dir.path().join("photos")).unwrap();
        let plan = plan_extract(&single).unwrap();
        assert_eq!(plan.dest, dir.path().join("single"));

        // Several top-level items: a folder named after the archive.
        let multi = make_zip(dir.path(), "multi", &["a.txt", "b.txt"]);
        std::fs::create_dir(dir.path().join("multi")).unwrap();
        let plan = plan_extract(&multi).unwrap();
        assert_eq!(plan.dest, dir.path().join("multi 2"));
        assert_eq!(plan.result, plan.dest);
    }

    #[test]
    fn extract_plans_a_compressed_file_by_its_stem() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("notes.txt");
        std::fs::write(&input, b"hello").unwrap();
        let gz = dir.path().join("notes.txt.gz");
        rcomp_core::Engine::default()
            .compress(
                &input,
                &gz,
                &CompressOptions {
                    format: Some(Format::codec(Codec::Gzip)),
                    ..Default::default()
                },
                |_| {},
            )
            .unwrap();

        // `notes.txt` is still there, so the result goes into a new folder.
        let plan = plan_extract(&gz).unwrap();
        assert_eq!(plan.dest, dir.path().join("notes.txt 2"));
        std::fs::remove_file(&input).unwrap();
        let plan = plan_extract(&gz).unwrap();
        assert_eq!(plan.dest, dir.path());
        assert_eq!(plan.result, input);
    }
}
