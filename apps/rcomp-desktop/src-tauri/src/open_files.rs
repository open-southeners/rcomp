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
//! All of them funnel through [`dispatch`]: compress / decompress requests run
//! in the background without a window (see `background.rs`); each file to open
//! goes to its own window (an empty one, or a new one — see
//! `window::for_open`) through [`deliver`]. Requests that arrive before that
//! window's frontend is ready are buffered per window in [`OpenPaths`]; the
//! frontend drains them once on startup via [`get_launch_requests`] (which
//! also marks the window ready), after which later deliveries are emitted live
//! to that window on the `launch-request` event.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Mutex, MutexGuard},
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

/// Per-window buffer + readiness flags for OS-delivered requests.
///
/// Registered as Tauri managed state.  Before a window's frontend calls
/// [`get_launch_requests`], requests for it accumulate here; afterwards they
/// are emitted live to that window (see [`deliver`]).
#[derive(Default)]
pub struct OpenPaths {
    inner: Mutex<Buffers>,
}

#[derive(Default)]
struct Buffers {
    pending: HashMap<String, Vec<LaunchRequest>>,
    ready: HashSet<String>,
}

impl OpenPaths {
    fn lock(&self) -> MutexGuard<'_, Buffers> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Buffer `request` for window `label` unless that window is ready, in
    /// which case the request is handed back to be emitted live.  One lock
    /// covers both, so a request can't slip in after the window drained.
    pub fn push_unless_ready(&self, label: &str, request: LaunchRequest) -> Option<LaunchRequest> {
        let mut b = self.lock();
        if b.ready.contains(label) {
            return Some(request);
        }
        b.pending.entry(label.to_owned()).or_default().push(request);
        None
    }

    /// Mark window `label` ready and return (clearing) its buffered requests.
    pub fn take_and_ready(&self, label: &str) -> Vec<LaunchRequest> {
        let mut b = self.lock();
        b.ready.insert(label.to_owned());
        b.pending.remove(label).unwrap_or_default()
    }

    /// Forget a window that was closed.
    pub fn forget(&self, label: &str) {
        let mut b = self.lock();
        b.ready.remove(label);
        b.pending.remove(label);
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

/// Route an OS request: context-menu actions run in the background with no
/// window; each file to open goes to its own window.
pub fn dispatch(app: &AppHandle, request: LaunchRequest) {
    if request.paths.is_empty() {
        return;
    }
    match request.action {
        LaunchAction::Open => {
            for path in request.paths {
                let label = crate::window::for_open(app);
                deliver(
                    app,
                    &label,
                    LaunchRequest {
                        action: LaunchAction::Open,
                        paths: vec![path],
                    },
                );
            }
        }
        LaunchAction::Compress | LaunchAction::Decompress => {
            crate::background::submit(app, request);
        }
    }
}

/// Deliver `request` to window `label`: emit live when it is ready, else
/// buffer it.
pub fn deliver(app: &AppHandle, label: &str, request: LaunchRequest) {
    if request.paths.is_empty() {
        return;
    }
    if let Some(request) = app.state::<OpenPaths>().push_unless_ready(label, request) {
        let _ = app.emit_to(label, LAUNCH_REQUEST_EVENT, request);
    }
}

/// Drain the requests the app was launched with (and mark the frontend ready
/// for live `launch-request` events).
///
/// Called once by the frontend on startup.  Returns an empty vector when the
/// app was launched normally (no associated files).
#[tauri::command]
pub fn get_launch_requests(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, OpenPaths>,
) -> Vec<LaunchRequest> {
    state.take_and_ready(window.label())
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
        .map(|p| p.to_string_lossy().into_owned())
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
