//! The native top-bar menu (macOS/Windows menu bar) and its additions.
//!
//! [`build`] starts from Tauri's platform-default menu ([`Menu::default`]) —
//! App/File/Edit/View/Window/Help — and appends rcomp-specific items:
//!
//! - **Help** — located via the well-known [`HELP_SUBMENU_ID`]: two external
//!   links (opened directly, no frontend involvement) and a changelog viewer.
//! - **File** — located by display text (Tauri gives it no well-known id):
//!   the workspace's open/new/extract/compress/close actions.
//! - **Edit** — also located by text: find and cancel-job.
//!
//! Every File/Edit item (and the changelog item) emits an event for the
//! frontend to act on rather than calling into it directly; each interested
//! component subscribes to its own event and no-ops when the action doesn't
//! currently apply (e.g. "Extract Now" while no archive is open), so these
//! items are always enabled rather than needing state synced from the
//! frontend just to grey them out.

use tauri::{
    AppHandle, Emitter as _, Runtime,
    menu::{HELP_SUBMENU_ID, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};
use tauri_plugin_opener::OpenerExt as _;

/// URL of the project's public source repository.
const REPOSITORY_URL: &str = "https://github.com/open-southeners/rcomp";

/// URL of the Open Southeners website.
const WEBSITE_URL: &str = "https://open-southeners.com";

/// Event emitted to the webview when the user picks **What's New**; the
/// frontend opens the changelog overlay in response.
pub const SHOW_CHANGELOG_EVENT: &str = "show-changelog";

/// Event emitted for **File → Open Archive…**.
pub const OPEN_ARCHIVE_EVENT: &str = "menu-open-archive";
/// Event emitted for **File → New Archive from Files…**.
pub const NEW_ARCHIVE_FILES_EVENT: &str = "menu-new-archive-files";
/// Event emitted for **File → New Archive from Folder…**.
pub const NEW_ARCHIVE_FOLDER_EVENT: &str = "menu-new-archive-folder";
/// Event emitted for **File → Extract Now**.
pub const EXTRACT_NOW_EVENT: &str = "menu-extract-now";
/// Event emitted for **File → Compress Archive**.
pub const COMPRESS_EVENT: &str = "menu-compress";
/// Event emitted for **File → Close Archive**.
pub const CLOSE_ARCHIVE_EVENT: &str = "menu-close-archive";
/// Event emitted for **Edit → Find**.
pub const FIND_EVENT: &str = "menu-find";
/// Event emitted for **Edit → Cancel Job**.
pub const CANCEL_JOB_EVENT: &str = "menu-cancel-job";

const REPOSITORY_ITEM_ID: &str = "help-repository";
const WEBSITE_ITEM_ID: &str = "help-website";
const CHANGELOG_ITEM_ID: &str = "help-changelog";
const OPEN_ARCHIVE_ITEM_ID: &str = "file-open-archive";
const NEW_ARCHIVE_FILES_ITEM_ID: &str = "file-new-archive-files";
const NEW_ARCHIVE_FOLDER_ITEM_ID: &str = "file-new-archive-folder";
const EXTRACT_NOW_ITEM_ID: &str = "file-extract-now";
const COMPRESS_ITEM_ID: &str = "file-compress";
const CLOSE_ARCHIVE_ITEM_ID: &str = "file-close-archive";
const FIND_ITEM_ID: &str = "edit-find";
const CANCEL_JOB_ITEM_ID: &str = "edit-cancel-job";

/// Build the app menu: Tauri's platform default, plus rcomp's Help links and
/// changelog viewer, and its File/Edit workspace actions.
///
/// Each addition silently leaves the default menu untouched if its target
/// submenu isn't found (e.g. Linux has no default File submenu), rather than
/// failing app startup over a cosmetic addition.
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

    if let Some(file) = find_submenu(&menu, "File")? {
        file.prepend_items(&[
            &MenuItem::with_id(
                app,
                NEW_ARCHIVE_FILES_ITEM_ID,
                "New Archive from Files…",
                true,
                Some("CmdOrCtrl+N"),
            )?,
            &MenuItem::with_id(
                app,
                NEW_ARCHIVE_FOLDER_ITEM_ID,
                "New Archive from Folder…",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                OPEN_ARCHIVE_ITEM_ID,
                "Open Archive…",
                true,
                Some("CmdOrCtrl+O"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, EXTRACT_NOW_ITEM_ID, "Extract Now", true, None::<&str>)?,
            &MenuItem::with_id(
                app,
                COMPRESS_ITEM_ID,
                "Compress Archive",
                true,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                CLOSE_ARCHIVE_ITEM_ID,
                "Close Archive",
                true,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
        ])?;
    }

    if let Some(edit) = find_submenu(&menu, "Edit")? {
        edit.append_items(&[
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, FIND_ITEM_ID, "Find", true, Some("CmdOrCtrl+F"))?,
            &MenuItem::with_id(app, CANCEL_JOB_ITEM_ID, "Cancel Job", true, None::<&str>)?,
        ])?;
    }

    Ok(menu)
}

/// Find a top-level submenu of `menu` by its display text.
///
/// Neither `File` nor `Edit` get a well-known id from [`Menu::default`]
/// (unlike `Window`/`Help`), so text is the only way to locate them.
fn find_submenu<R: Runtime>(menu: &Menu<R>, text: &str) -> tauri::Result<Option<Submenu<R>>> {
    for kind in menu.items()? {
        if let Some(submenu) = kind.as_submenu()
            && submenu.text()? == text
        {
            return Ok(Some(submenu.clone()));
        }
    }
    Ok(None)
}

/// Dispatch a click on one of the items [`build`] added: open an external
/// link directly, or emit the matching event for the frontend to act on.
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
    } else if id == OPEN_ARCHIVE_ITEM_ID {
        let _ = app.emit(OPEN_ARCHIVE_EVENT, ());
    } else if id == NEW_ARCHIVE_FILES_ITEM_ID {
        let _ = app.emit(NEW_ARCHIVE_FILES_EVENT, ());
    } else if id == NEW_ARCHIVE_FOLDER_ITEM_ID {
        let _ = app.emit(NEW_ARCHIVE_FOLDER_EVENT, ());
    } else if id == EXTRACT_NOW_ITEM_ID {
        let _ = app.emit(EXTRACT_NOW_EVENT, ());
    } else if id == COMPRESS_ITEM_ID {
        let _ = app.emit(COMPRESS_EVENT, ());
    } else if id == CLOSE_ARCHIVE_ITEM_ID {
        let _ = app.emit(CLOSE_ARCHIVE_EVENT, ());
    } else if id == FIND_ITEM_ID {
        let _ = app.emit(FIND_EVENT, ());
    } else if id == CANCEL_JOB_ITEM_ID {
        let _ = app.emit(CANCEL_JOB_EVENT, ());
    }
}
