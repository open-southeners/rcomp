//! File delivery from the OS to the frontend: "Open with" and file-manager
//! context-menu actions.
//!
//! Requests can reach the app several ways, and they may arrive before the
//! webview has registered its event listeners:
//!
//! - **macOS** — as a [`tauri::RunEvent::Opened`] launch/runtime event
//!   ("Open with"), or through the Finder Quick Actions services provider
//!   (see `services.rs`).
//! - **Windows / Linux** — as process **argv** on first launch, or via the
//!   single-instance plugin's callback when a second launch is folded into the
//!   running instance.  Context-menu entries launch the app with
//!   `--compress` / `--decompress` followed by the selected paths
//!   (see [`requests_from_args`]).
//!
//! All of them funnel through [`deliver`]. Requests that arrive before the
//! frontend is ready are buffered in [`OpenPaths`]; the frontend drains them
//! once on startup via [`get_launch_requests`] (which also flips the "ready"
//! flag), after which later deliveries are emitted live on the
//! `launch-request` event.

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tauri::{AppHandle, Emitter as _, Manager as _};

/// The event name emitted to the webview carrying one [`LaunchRequest`].
pub const LAUNCH_REQUEST_EVENT: &str = "launch-request";

/// Launch flag that compresses the paths following it.
pub const COMPRESS_FLAG: &str = "--compress";
/// Launch flag that extracts the archives following it.
pub const DECOMPRESS_FLAG: &str = "--decompress";

/// What the OS asked the app to do with a set of paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchAction {
    /// Show the paths ("Open with" / double-click).
    Open,
    /// Bundle the paths into a new archive next to them.
    Compress,
    /// Extract each archive next to itself.
    Decompress,
}

/// One OS request: an action and the existing paths it applies to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LaunchRequest {
    /// What to do with `paths`.
    pub action: LaunchAction,
    /// Absolute paths that existed when the request arrived; never empty.
    pub paths: Vec<String>,
}

/// Buffer + readiness flag for OS-delivered requests.
///
/// Registered as Tauri managed state.  Before the frontend calls
/// [`get_launch_requests`], delivered requests accumulate in `pending`;
/// afterwards they are emitted live (see [`deliver`]).
#[derive(Default)]
pub struct OpenPaths {
    pending: Mutex<Vec<LaunchRequest>>,
    ready: AtomicBool,
}

impl OpenPaths {
    /// Buffer `request` for the frontend to drain on startup.
    pub fn push(&self, request: LaunchRequest) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(request);
    }

    /// Mark the frontend ready and return (clearing) any buffered requests.
    pub fn take_and_ready(&self) -> Vec<LaunchRequest> {
        self.ready.store(true, Ordering::SeqCst);
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    /// Whether the frontend has drained startup requests and can receive live events.
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

/// Turn launch arguments (without the program name) into requests.
///
/// Paths before any action flag are opened.  [`COMPRESS_FLAG`] /
/// [`DECOMPRESS_FLAG`] apply to every path after them, up to the next flag.
/// Paths that do not exist are dropped, as are requests left with none.
pub fn requests_from_args<I: IntoIterator<Item = String>>(
    args: I,
    base: &Path,
) -> Vec<LaunchRequest> {
    let mut groups: Vec<(LaunchAction, Vec<String>)> = vec![(LaunchAction::Open, Vec::new())];
    for arg in args {
        match arg.as_str() {
            COMPRESS_FLAG => groups.push((LaunchAction::Compress, Vec::new())),
            DECOMPRESS_FLAG => groups.push((LaunchAction::Decompress, Vec::new())),
            _ => groups.last_mut().expect("never empty").1.push(arg),
        }
    }
    groups
        .into_iter()
        .filter_map(|(action, items)| {
            let paths = existing_paths(items, base);
            (!paths.is_empty()).then_some(LaunchRequest { action, paths })
        })
        .collect()
}

/// Deliver `request` to the frontend: emit live when ready, else buffer it.
pub fn deliver(app: &AppHandle, request: LaunchRequest) {
    if request.paths.is_empty() {
        return;
    }
    let state = app.state::<OpenPaths>();
    if state.is_ready() {
        let _ = app.emit(LAUNCH_REQUEST_EVENT, request);
    } else {
        state.push(request);
    }
}

/// Drain the requests the app was launched with (and mark the frontend ready
/// for live `launch-request` events).
///
/// Called once by the frontend on startup.  Returns an empty vector when the
/// app was launched normally (no associated files).
#[tauri::command]
pub fn get_launch_requests(state: tauri::State<'_, OpenPaths>) -> Vec<LaunchRequest> {
    state.take_and_ready()
}

/// Convert macOS file URLs into existing on-disk file paths.
///
/// macOS-only: used for `RunEvent::Opened` (which does not exist on other
/// platforms, where files arrive via argv instead) and Finder services.
#[cfg(target_os = "macos")]
pub fn paths_from_urls<I: IntoIterator<Item = tauri::Url>>(urls: I) -> Vec<String> {
    urls.into_iter()
        .filter_map(|u| u.to_file_path().ok())
        .filter(|p| p.exists())
        .map(|p: PathBuf| p.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn plain_paths_are_opened_and_missing_ones_dropped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.zip"), b"").unwrap();
        let reqs = requests_from_args(args(&["a.zip", "missing.zip", "--flag"]), dir.path());
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].action, LaunchAction::Open);
        assert_eq!(
            reqs[0].paths,
            vec![dir.path().join("a.zip").to_string_lossy()]
        );
    }

    #[test]
    fn action_flags_group_the_paths_after_them() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.txt", "b.txt", "c.zip"] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        let reqs = requests_from_args(
            args(&["--compress", "a.txt", "b.txt", "--decompress", "c.zip"]),
            dir.path(),
        );
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].action, LaunchAction::Compress);
        assert_eq!(reqs[0].paths.len(), 2);
        assert_eq!(reqs[1].action, LaunchAction::Decompress);
        assert!(reqs[1].paths[0].ends_with("c.zip"));
    }

    #[test]
    fn a_flag_without_existing_paths_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(requests_from_args(args(&["--compress", "nope"]), dir.path()).is_empty());
    }
}
