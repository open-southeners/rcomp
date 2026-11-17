//! App windows, created on demand.
//!
//! `tauri.conf.json` declares the window template with `"create": false` so a
//! launch for a context-menu action (see `background.rs`) never shows one.
//! The app can have several windows, each opening or composing its own
//! archive: the first is labelled `main`, later ones `main-2`, `main-3`, ….
//!
//! - [`show`] brings a window to the front, creating one if there is none (a
//!   normal launch, clicking the Dock icon, launching the app again).
//! - [`new_window`] always creates one (**File → New Window**).
//! - [`for_open`] picks the window an "Open with" file goes to: a window that
//!   is still empty (its frontend reports this with [`set_window_idle`]), or
//!   a new one.

use std::{
    collections::HashSet,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU32, Ordering},
    },
};

use tauri::{AppHandle, LogicalPosition, Manager as _, WebviewWindow, WebviewWindowBuilder};

/// Label of the window template in `tauri.conf.json`, and of the first window.
pub const MAIN: &str = "main";

/// How far each new window is offset from the focused one.
const CASCADE: f64 = 24.0;

static NEXT_ID: AtomicU32 = AtomicU32::new(2);

/// Window bookkeeping (Tauri managed state).
#[derive(Default)]
pub struct Windows {
    /// Windows showing nothing yet, which can take an "Open with" file.
    idle: Mutex<HashSet<String>>,
    /// Windows being built: they don't exist in Tauri yet but already count
    /// as open, so a launch doesn't create a second one meanwhile.
    pending: Mutex<HashSet<String>>,
}

impl Windows {
    fn idle(&self) -> MutexGuard<'_, HashSet<String>> {
        self.idle.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn pending(&self) -> MutexGuard<'_, HashSet<String>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forget a window that was closed.
    pub fn forget(&self, label: &str) {
        self.idle().remove(label);
        self.pending().remove(label);
    }
}

/// Whether any window exists or is being created.
pub fn is_open(app: &AppHandle) -> bool {
    !app.webview_windows().is_empty() || !app.state::<Windows>().pending().is_empty()
}

/// The focused window, else any window.
pub fn focused(app: &AppHandle) -> Option<WebviewWindow> {
    let all = app.webview_windows();
    all.values()
        .find(|w| w.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| all.into_values().next())
}

/// Bring a window to the front, creating one if there is none.
pub fn show(app: &AppHandle) {
    if let Some(window) = focused(app) {
        bring_to_front(app, &window);
    } else if app.state::<Windows>().pending().is_empty() {
        let label = create(app);
        app.state::<Windows>().idle().insert(label);
    }
}

/// Create a new, empty window.
pub fn new_window(app: &AppHandle) {
    let label = create(app);
    app.state::<Windows>().idle().insert(label);
}

/// The window an "Open with" file should go to: an empty one (brought to
/// the front), or a new one.  The returned window is no longer idle.
pub fn for_open(app: &AppHandle) -> String {
    let windows = app.state::<Windows>();
    let reusable = {
        let mut idle = windows.idle();
        let pending = windows.pending();
        let label = idle
            .iter()
            .find(|l| pending.contains(*l) || app.get_webview_window(l).is_some())
            .cloned();
        if let Some(label) = &label {
            idle.remove(label);
        }
        label
    };
    match reusable {
        Some(label) => {
            if let Some(window) = app.get_webview_window(&label) {
                bring_to_front(app, &window);
            }
            label
        }
        None => create(app),
    }
}

/// Record whether a window is empty (frontend → backend).
#[tauri::command]
pub fn set_window_idle(window: WebviewWindow, idle: bool, windows: tauri::State<'_, Windows>) {
    let label = window.label().to_owned();
    if idle {
        windows.idle().insert(label);
    } else {
        windows.idle().remove(&label);
    }
}

fn bring_to_front(app: &AppHandle, window: &WebviewWindow) {
    activate(app);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// macOS: leave the background-only mode the app starts in (see `lib.rs`).
fn activate(_app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = _app.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = _app.show();
    }
}

/// Start building a window and return its label right away.
///
/// The window is built on a separate thread: building one from the main
/// thread (event-loop callbacks, menu handlers) can deadlock on Windows.
fn create(app: &AppHandle) -> String {
    let label = if app.get_webview_window(MAIN).is_none()
        && !app.state::<Windows>().pending().contains(MAIN)
    {
        MAIN.to_owned()
    } else {
        format!("{MAIN}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
    };
    app.state::<Windows>().pending().insert(label.clone());

    // Cascade from the focused window so a new one doesn't hide it exactly.
    let origin = focused(app).and_then(|w| {
        let scale = w.scale_factor().ok()?;
        let pos = w.outer_position().ok()?.to_logical::<f64>(scale);
        Some(LogicalPosition::new(pos.x + CASCADE, pos.y + CASCADE))
    });

    activate(app);
    let app = app.clone();
    let thread_label = label.clone();
    std::thread::spawn(move || {
        let label = thread_label;
        let Some(mut config) = app
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == MAIN)
            .cloned()
        else {
            app.state::<Windows>().forget(&label);
            return;
        };
        config.label = label.clone();
        let built = WebviewWindowBuilder::from_config(&app, &config).and_then(|builder| {
            match origin {
                Some(p) => builder.position(p.x, p.y),
                None => builder,
            }
            .build()
        });
        app.state::<Windows>().pending().remove(&label);
        match built {
            Ok(window) => {
                let _ = window.set_focus();
            }
            Err(e) => {
                app.state::<Windows>().forget(&label);
                eprintln!("Rcomp: could not create a window: {e}");
            }
        }
    });
    label
}
