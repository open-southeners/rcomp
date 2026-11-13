//! The main window, created on demand.
//!
//! `tauri.conf.json` declares the window with `"create": false` so that a
//! launch for a context-menu action (see `background.rs`) never shows it.
//! Everything that needs the window — a normal launch, "Open with", clicking
//! the Dock icon, launching the app again — goes through [`show`].

use tauri::{AppHandle, Manager as _, WebviewWindowBuilder};

/// Label of the main window in `tauri.conf.json`.
pub const MAIN: &str = "main";

/// Whether the main window exists.
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN).is_some()
}

/// Create the main window, or bring it to the front if it exists.
pub fn show(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show_now(&handle));
}

fn show_now(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app.show();
    }
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let Some(config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN)
        .cloned()
    else {
        return;
    };
    match WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(e) => eprintln!("Rcomp: could not create the main window: {e}"),
    }
}
