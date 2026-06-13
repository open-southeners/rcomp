pub mod commands;
pub mod error;
pub mod job;
pub mod progress;

use tauri::Manager as _;

/// Entry point for the Tauri application.
///
/// Builds the Tauri app instance, registers managed state (the [`job::JobRegistry`]
/// for in-flight cancel tokens), wires all IPC command handlers, and installs a
/// window-close hook that cancels every in-flight job before the webview closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(job::JobRegistry::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::inspect,
            commands::list_entries,
            commands::compress,
            commands::extract,
            commands::cancel_job,
            commands::read_sidecar,
            commands::write_sidecar,
            commands::wrap_info,
        ])
        // Cancel all in-flight jobs when the last window requests close.
        // `cancel_all` trips every registered CancelToken; worker threads call
        // `reg.finish(id)` as they unwind, removing their entries.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.state::<job::JobRegistry>().cancel_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
