pub mod changelog;
pub mod commands;
pub mod error;
pub mod job;
pub mod menu;
pub mod open_files;
pub mod progress;

use tauri::Manager as _;

/// Entry point for the Tauri application.
///
/// Builds the Tauri app instance, registers managed state (the [`job::JobRegistry`]
/// for in-flight cancel tokens and [`open_files::OpenPaths`] for OS "open with"
/// delivery), wires all IPC command handlers, installs a window-close hook that
/// cancels every in-flight job before the webview closes, and runs the event loop
/// handling macOS `Opened` file events.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .manage(job::JobRegistry::default())
        .manage(open_files::OpenPaths::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());

    // Desktop single-instance: a second launch (e.g. "Open with" on
    // Windows/Linux) forwards its argv to this running instance instead of
    // spawning a new window.  The plugin must be registered first so it can
    // intercept the duplicate launch before any window is created.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let base = std::path::PathBuf::from(&cwd);
            let paths = open_files::existing_paths(argv.into_iter().skip(1), &base);
            open_files::deliver(app, paths);
        }));
    }

    // Custom top-bar menu (macOS menu bar): Tauri's platform default plus
    // rcomp's Help-menu links and changelog viewer — see `menu::build`.
    #[cfg(desktop)]
    {
        builder = builder.menu(menu::build).on_menu_event(menu::handle_event);
    }

    let app = builder
        .invoke_handler(tauri::generate_handler![
            commands::inspect,
            commands::list_entries,
            commands::compress,
            commands::compress_many,
            commands::extract,
            commands::cancel_job,
            commands::read_sidecar,
            commands::write_sidecar,
            commands::wrap_info,
            open_files::get_launch_paths,
            changelog::get_changelog,
        ])
        // Cancel all in-flight jobs when the last window requests close.
        // `cancel_all` trips every registered CancelToken; worker threads call
        // `reg.finish(id)` as they unwind, removing their entries.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.state::<job::JobRegistry>().cancel_all();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Capture file paths passed as launch arguments (Windows/Linux "open with"
    // on first launch).  Buffered until the frontend drains them on startup.
    #[cfg(desktop)]
    {
        if let Ok(cwd) = std::env::current_dir() {
            let paths = open_files::existing_paths(std::env::args().skip(1), &cwd);
            if !paths.is_empty() {
                app.state::<open_files::OpenPaths>().push(paths);
            }
        }
    }

    app.run(|_app_handle, _event| {
        // macOS delivers "open with" files as an Opened event, both at launch
        // (possibly before the webview is ready) and while running.  This
        // RunEvent variant only exists on macOS; Windows/Linux deliver the
        // paths via argv (handled above) and the single-instance plugin.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = _event {
            let paths = open_files::paths_from_urls(urls);
            open_files::deliver(_app_handle, paths);
        }
    });
}
