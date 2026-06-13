pub mod error;
pub mod job;
pub mod progress;

use tauri::Manager as _;

/// Entry point for the Tauri application.
///
/// Builds the Tauri app instance, registers managed state (the [`job::JobRegistry`]
/// for in-flight cancel tokens), and wires in the IPC command handlers and
/// window-close hook added in the next stage.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(job::JobRegistry::default())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
