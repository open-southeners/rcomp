/// Entry point for the Tauri application.
///
/// Builds and runs the Tauri app instance. No IPC commands are registered
/// yet — those come in a later milestone.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
