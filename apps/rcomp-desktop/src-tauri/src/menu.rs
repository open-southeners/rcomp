//! The native top-bar menu (macOS menu bar) and its Help-menu additions.
//!
//! [`build`] starts from Tauri's platform-default menu ([`Menu::default`]) —
//! App/File/Edit/View/Window/Help — and appends rcomp-specific items to the
//! generated Help submenu, located via its well-known [`HELP_SUBMENU_ID`]
//! rather than by matching on its display text. [`handle_event`] dispatches
//! clicks on those items: the two link items open an external URL directly
//! (no frontend involvement), while the changelog item emits
//! [`SHOW_CHANGELOG_EVENT`] for the frontend to open its viewer overlay.

use tauri::{
    AppHandle, Emitter as _, Runtime,
    menu::{HELP_SUBMENU_ID, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use tauri_plugin_opener::OpenerExt as _;

/// URL of the project's public source repository.
const REPOSITORY_URL: &str = "https://github.com/open-southeners/rcomp";

/// URL of the Open Southeners website.
const WEBSITE_URL: &str = "https://open-southeners.com";

/// Event emitted to the webview when the user picks **What's New**; the
/// frontend opens the changelog overlay in response.
pub const SHOW_CHANGELOG_EVENT: &str = "show-changelog";

const REPOSITORY_ITEM_ID: &str = "help-repository";
const WEBSITE_ITEM_ID: &str = "help-website";
const CHANGELOG_ITEM_ID: &str = "help-changelog";

/// Build the app menu: Tauri's platform default, plus rcomp's Help-menu
/// links and changelog viewer.
///
/// Silently leaves the default menu untouched if a future Tauri version ever
/// stops generating a Help submenu, rather than failing app startup over a
/// cosmetic addition.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;

    if let Some(help) = menu
        .get(HELP_SUBMENU_ID)
        .and_then(|kind| kind.as_submenu().cloned())
    {
        help.append_items(&[
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                REPOSITORY_ITEM_ID,
                "Project Repository",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                WEBSITE_ITEM_ID,
                "Open Southeners Website",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, CHANGELOG_ITEM_ID, "What's New", true, None::<&str>)?,
        ])?;
    }

    Ok(menu)
}

/// Dispatch a click on one of the items [`build`] added: open the matching
/// external link, or emit [`SHOW_CHANGELOG_EVENT`] for the frontend.
///
/// Unrecognised ids (every other default menu item) are ignored — Tauri's
/// predefined items (quit, undo, etc.) handle themselves natively.
pub fn handle_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id();
    if id == REPOSITORY_ITEM_ID {
        let _ = app.opener().open_url(REPOSITORY_URL, None::<&str>);
    } else if id == WEBSITE_ITEM_ID {
        let _ = app.opener().open_url(WEBSITE_URL, None::<&str>);
    } else if id == CHANGELOG_ITEM_ID {
        let _ = app.emit(SHOW_CHANGELOG_EVENT, ());
    }
}
