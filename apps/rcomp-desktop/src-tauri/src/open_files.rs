//! "Open with" file delivery from the OS to the frontend.
//!
//! Files can reach the app three ways, and they may arrive before the webview
//! has registered its event listeners:
//!
//! - **macOS** — as a [`tauri::RunEvent::Opened`] launch/runtime event.
//! - **Windows / Linux** — as process **argv** on first launch, or via the
//!   single-instance plugin's callback when a second launch is folded into the
//!   running instance.
//!
//! All three funnel through [`deliver`]. Paths that arrive before the frontend
//! is ready are buffered in [`OpenPaths`]; the frontend drains them once on
//! startup via [`get_launch_paths`] (which also flips the "ready" flag), after
//! which later deliveries are emitted live on the `open-paths` event.

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tauri::{AppHandle, Emitter as _, Manager as _};

/// The event name emitted to the webview carrying `Vec<String>` of file paths.
pub const OPEN_PATHS_EVENT: &str = "open-paths";

/// Buffer + readiness flag for OS-delivered file paths.
///
/// Registered as Tauri managed state.  Before the frontend calls
/// [`get_launch_paths`], delivered paths accumulate in `pending`; afterwards
/// they are emitted live (see [`deliver`]).
#[derive(Default)]
pub struct OpenPaths {
    pending: Mutex<Vec<String>>,
    ready: AtomicBool,
}

impl OpenPaths {
    /// Buffer `paths` for the frontend to drain on startup.
    pub fn push<I: IntoIterator<Item = String>>(&self, paths: I) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        guard.extend(paths);
    }

    /// Mark the frontend ready and return (clearing) any buffered paths.
    pub fn take_and_ready(&self) -> Vec<String> {
        self.ready.store(true, Ordering::SeqCst);
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    /// Whether the frontend has drained startup paths and can receive live events.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
}

/// Keep only the paths that currently exist on disk.
///
/// OS "open with" invocations pass real file paths; this filters out CLI flags
/// and the program name so they are never mistaken for inputs.  Relative paths
/// are resolved against `base` (the launching process's working directory).
pub fn existing_paths<I: IntoIterator<Item = String>>(items: I, base: &Path) -> Vec<String> {
    items
        .into_iter()
        .map(|s| {
            let p = Path::new(&s);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.join(p)
            }
        })
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Deliver `paths` to the frontend: emit live when ready, else buffer them.
pub fn deliver(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let state = app.state::<OpenPaths>();
    if state.is_ready() {
        let _ = app.emit(OPEN_PATHS_EVENT, paths);
    } else {
        state.push(paths);
    }
}

/// Drain the file paths the app was launched with (and mark the frontend ready
/// for live `open-paths` events).
///
/// Called once by the frontend on startup.  Returns an empty vector when the
/// app was launched normally (no associated files).
#[tauri::command]
pub fn get_launch_paths(state: tauri::State<'_, OpenPaths>) -> Vec<String> {
    state.take_and_ready()
}

/// Convert macOS `RunEvent::Opened` URLs into existing on-disk file paths.
pub fn paths_from_urls<I: IntoIterator<Item = tauri::Url>>(urls: I) -> Vec<String> {
    urls.into_iter()
        .filter_map(|u| u.to_file_path().ok())
        .filter(|p| p.exists())
        .map(|p: PathBuf| p.to_string_lossy().into_owned())
        .collect()
}
