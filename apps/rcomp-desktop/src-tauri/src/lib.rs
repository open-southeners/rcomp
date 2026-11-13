pub mod background;
pub mod changelog;
pub mod cli_link;
pub mod commands;
#[cfg(desktop)]
pub mod drag_out;
pub mod error;
pub mod job;
pub mod menu;
pub mod open_files;
pub mod progress;
#[cfg(target_os = "macos")]
pub mod services;
pub mod window;

use tauri::Manager as _;

/// Bails out with a clean message instead of letting `tao` panic with a raw
/// GTK `BoolError` when there is no X11/Wayland session (e.g. an SSH session
/// on a headless server) — `gtk::init` fails deep inside the event loop
/// constructor with no recoverable `Result` to catch.
#[cfg(target_os = "linux")]
fn ensure_display_available() {
    let has_display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();

    if !has_display {
        eprintln!(
            "rcomp: no graphical display found (DISPLAY and WAYLAND_DISPLAY are both unset)."
        );
        eprintln!(
            "This is the Rcomp desktop app, which needs an X11 or Wayland session — not the `rcomp` command-line tool."
        );
        eprintln!("Looking for the CLI instead? Install it with: cargo install rcomp");
        std::process::exit(1);
    }
}

/// How long a macOS launch waits for a Quick Action request before showing
/// the window.
#[cfg(target_os = "macos")]
const MACOS_LAUNCH_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

/// Entry point for the Tauri application.
///
/// Builds the Tauri app instance, registers managed state (the [`job::JobRegistry`]
/// for in-flight cancel tokens and [`open_files::OpenPaths`] for OS "open with"
/// delivery), wires all IPC command handlers, installs a window-close hook that
/// cancels every in-flight job before the webview closes, and runs the event loop
/// handling macOS `Opened` file events.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The Windows uninstaller runs the app with `--unlink-cli` to take the
    // bundled CLI off PATH; do that without a window or display.
    if let Some(code) = cli_link::handle_unlink_flag() {
        std::process::exit(code);
    }

    #[cfg(target_os = "linux")]
    ensure_display_available();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .manage(job::JobRegistry::default())
        .manage(open_files::OpenPaths::default())
        .manage(background::Background::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init());

    // Desktop single-instance: a second launch (e.g. "Open with" or a
    // context-menu action on Windows/Linux) forwards its argv to this running
    // instance instead of spawning a new process.  The plugin must be
    // registered first so it can intercept the duplicate launch before any
    // window is created.  A plain second launch shows the window.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let base = std::path::PathBuf::from(&cwd);
            let requests = open_files::requests_from_args(argv.into_iter().skip(1), &base);
            if requests.is_empty() {
                window::show(app);
            }
            for request in requests {
                open_files::dispatch(app, request);
            }
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
            commands::add_to_archive,
            #[cfg(desktop)]
            drag_out::drag_out,
            #[cfg(desktop)]
            drag_out::drag_staging_root,
            open_files::get_launch_requests,
            background::save_background_preferences,
            changelog::get_changelog,
            cli_link::cli_link_status,
            cli_link::link_cli,
            cli_link::unlink_cli,
        ])
        .setup(|app| {
            // Finder Quick Actions: register the NSServices provider while
            // still launching, so a service request that started the app is
            // not lost.  Stay out of the Dock until a window is shown, so
            // context-menu jobs run quietly.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                services::register(app.handle());
            }

            // Windows/Linux pass files and context-menu actions as launch
            // arguments.  Only "Open with" (or a plain launch) needs the
            // window; `--compress` / `--decompress` run without one.
            #[cfg(not(target_os = "macos"))]
            {
                let cwd = std::env::current_dir().unwrap_or_default();
                let requests = open_files::requests_from_args(std::env::args().skip(1), &cwd);
                if requests.is_empty() {
                    window::show(app.handle());
                }
                for request in requests {
                    open_files::dispatch(app.handle(), request);
                }
            }
            Ok(())
        })
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

    // Sweep drag-out staging folders left over from earlier sessions.
    #[cfg(desktop)]
    if let Ok(root) = drag_out::staging_root(app.handle()) {
        std::thread::spawn(move || drag_out::purge_stale(&root));
    }

    app.run(|app_handle, event| match event {
        // Closing the last window while a context-menu job runs keeps the app
        // alive until the job ends (`background::quit_if_idle` then quits).
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } if app_handle.state::<background::Background>().is_busy() => {
            api.prevent_exit();
        }
        // macOS: a launch from the Dock or Finder shows the window; one for a
        // Quick Action doesn't.  The service request arrives just after the
        // app finishes launching, so wait briefly before deciding.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Ready => {
            let app = app_handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(MACOS_LAUNCH_GRACE);
                if !window::is_open(&app)
                    && !app
                        .state::<background::Background>()
                        .has_received_requests()
                {
                    window::show(&app);
                }
            });
        }
        // macOS: clicking the Dock icon with no window open.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => window::show(app_handle),
        // macOS delivers "open with" files as an Opened event, both at launch
        // (possibly before the webview is ready) and while running.  This
        // RunEvent variant only exists on macOS; Windows/Linux deliver the
        // paths via argv (handled in `setup`) and the single-instance plugin.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Opened { urls } => {
            open_files::dispatch(
                app_handle,
                open_files::LaunchRequest {
                    action: open_files::LaunchAction::Open,
                    paths: open_files::paths_from_urls(urls),
                },
            );
        }
        _ => {}
    });
}
