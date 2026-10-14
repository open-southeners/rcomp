//! Dragging archive entries out of the app into a file manager.
//!
//! A webview cannot hand files to Finder / Explorer by itself, and the entries
//! do not exist on disk yet.  [`drag_out`] therefore extracts the selected
//! entries into a fresh folder under [`staging_root`], then starts a native OS
//! drag session carrying those real files; the drop target copies them from
//! there.
//!
//! Staged folders cannot be removed when the drag ends — the drop target may
//! still be copying from them — so they are swept by age instead
//! ([`purge_stale`]) at startup and before each new drag.
//!
//! The staging root is in the per-user app cache folder, never a fixed name
//! in a shared temp directory: another user could plant that name as a
//! symlink and have the sweep delete their target's contents.  The root is
//! created owner-only and refused when it is a symlink ([`ensure_root`]), and
//! the sweep only removes real `drag-*` folders.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime},
};

use rcomp_core::{ExtractOptions, extract};

use tauri::Manager as _;

use crate::{error::IpcError, job::JobRegistry};

/// Name prefix of every staging folder; the sweep touches nothing else.
const STAGE_PREFIX: &str = "drag-";

/// Staged folders older than this are removed by [`purge_stale`].
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Preview image shown under the cursor while dragging.
const DRAG_ICON: &[u8] = include_bytes!("../icons/64x64.png");

/// The folder all drag-out staging folders live under: `drag` in the
/// per-user app cache directory.
///
/// The frontend asks for it (via [`drag_staging_root`]) so a drag that ends
/// back over the app's own window is not mistaken for files dropped in.
pub fn staging_root(app: &tauri::AppHandle) -> Result<PathBuf, IpcError> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("drag"))
        .map_err(|e| IpcError::new("io", format!("no cache directory: {e}")))
}

/// Create `root` if needed (owner-only on Unix) and confirm it is a real
/// directory rather than a symlink to somewhere else.
pub fn ensure_root(root: &Path) -> std::io::Result<()> {
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(root)?;
    if !meta.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a plain directory",
            root.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        // The root must belong to whoever owns its parent, the per-user cache
        // directory; anything else was created by another account.
        let probe = fs::metadata(root.parent().unwrap_or(root))?;
        if meta.uid() != probe.uid() {
            return Err(std::io::Error::other(format!(
                "{} is owned by another user",
                root.display()
            )));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Remove staging folders older than [`STALE_AFTER`].  Best-effort; only
/// real directories named `drag-*` are touched, never symlinks.
pub fn purge_stale(root: &Path) {
    if ensure_root(root).is_err() {
        return;
    }
    let Ok(dir) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in dir.flatten() {
        let is_stage = entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
            && entry.file_type().is_ok_and(|t| t.is_dir());
        if !is_stage {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= STALE_AFTER);
        if old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Extract `entries` of `archive` into a new folder under `root` and return
/// the top-level paths created there (what the drag session should carry).
///
/// Tauri-free so it can be tested without a webview.
pub fn stage(
    root: &Path,
    archive: &Path,
    entries: &[String],
    cancel: rcomp_core::CancelToken,
) -> Result<Vec<PathBuf>, IpcError> {
    if entries.is_empty() {
        return Err(IpcError::new("io", "no entries selected to drag"));
    }
    ensure_root(root).map_err(|e| IpcError::new("io", e.to_string()))?;
    purge_stale(root);
    let dir = tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .tempdir_in(root)
        .map_err(|e| IpcError::new("io", e.to_string()))?
        .keep();

    let opts = ExtractOptions {
        cancel,
        entries: entries.iter().map(PathBuf::from).collect(),
        ..Default::default()
    };
    if let Err(e) = extract(archive, &dir, &opts, |_| {}) {
        let _ = fs::remove_dir_all(&dir);
        return Err(e.into());
    }

    let mut items: Vec<PathBuf> = fs::read_dir(&dir)
        .and_then(|d| d.map(|e| e.map(|e| e.path())).collect())
        .map_err(|e| IpcError::new("io", e.to_string()))?;
    items.sort();
    if items.is_empty() {
        let _ = fs::remove_dir_all(&dir);
        return Err(IpcError::new(
            "io",
            "the selected entries produced no files",
        ));
    }
    Ok(items)
}

/// Return [`staging_root`] as a string for the frontend's drop filter.
#[tauri::command]
pub fn drag_staging_root(app: tauri::AppHandle) -> Result<String, IpcError> {
    Ok(staging_root(&app)?.to_string_lossy().into_owned())
}

/// Extract `entries` of `archive` and start a native drag carrying them.
///
/// Called from the webview's `dragstart`, while the mouse button is still
/// held: the OS drag session takes over the cursor once extraction finishes.
/// Large selections therefore show a short delay before the drag begins.
/// The job is registered under `job_id` so it can be cancelled.
#[tauri::command]
pub async fn drag_out(
    app: tauri::AppHandle,
    window: tauri::Window,
    job_id: String,
    archive: String,
    entries: Vec<String>,
    registry: tauri::State<'_, JobRegistry>,
) -> Result<(), IpcError> {
    let root = staging_root(&app)?;
    let cancel = registry.register(job_id.clone());
    let archive = PathBuf::from(archive);
    let staged =
        tauri::async_runtime::spawn_blocking(move || stage(&root, &archive, &entries, cancel))
            .await;
    registry.finish(&job_id);
    let items =
        staged.unwrap_or_else(|e| Err(IpcError::new("io", format!("task panicked: {e}"))))?;

    // Native drag APIs must be driven from the main (UI) thread.
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        #[cfg(target_os = "linux")]
        let handle = window.gtk_window();
        #[cfg(not(target_os = "linux"))]
        let handle = tauri::Result::Ok(window);

        let started = match handle {
            Ok(handle) => drag::start_drag(
                &handle,
                drag::DragItem::Files(items),
                drag::Image::Raw(DRAG_ICON.to_vec()),
                |_result, _cursor| {},
                drag::Options::default(),
            )
            .map_err(|e| IpcError::new("io", format!("could not start the drag: {e}"))),
            Err(e) => Err(IpcError::new("io", e.to_string())),
        };
        let _ = tx.send(started);
    })
    .map_err(|e| IpcError::new("io", e.to_string()))?;

    rx.recv()
        .unwrap_or_else(|_| Err(IpcError::new("io", "the drag was never started")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(dir: &Path) -> PathBuf {
        let src = dir.join("src");
        fs::create_dir_all(src.join("docs/guide")).unwrap();
        fs::write(src.join("docs/guide/a.md"), b"a").unwrap();
        fs::write(src.join("top.txt"), b"top").unwrap();
        let archive = dir.join("a.zip");
        rcomp_core::compress_many(
            &[src.join("docs"), src.join("top.txt")],
            &archive,
            &rcomp_core::CompressOptions {
                follow_gitignore: false,
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();
        archive
    }

    #[test]
    fn stage_writes_only_the_selection_under_its_own_names() {
        let dir = tempfile::tempdir().unwrap();
        let archive = zip_with(dir.path());
        let root = dir.path().join("staging");

        let items = stage(
            &root,
            &archive,
            &["docs/guide/".into(), "top.txt".into()],
            Default::default(),
        )
        .unwrap();

        let names: Vec<_> = items
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["guide", "top.txt"]);
        assert!(items.iter().all(|p| p.starts_with(&root)));
        assert_eq!(fs::read(items[0].join("a.md")).unwrap(), b"a");
    }

    #[test]
    fn stage_rejects_an_empty_selection_and_cleans_up_on_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let archive = zip_with(dir.path());
        let root = dir.path().join("staging");

        assert!(stage(&root, &archive, &[], Default::default()).is_err());
        assert!(stage(&root, &archive, &["missing".into()], Default::default()).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[test]
    fn purge_keeps_fresh_staging_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("drag-fresh")).unwrap();
        purge_stale(dir.path());
        assert!(dir.path().join("drag-fresh").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_is_refused_and_never_purged() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::create_dir_all(victim.join("drag-old")).unwrap();
        let root = dir.path().join("root");
        std::os::unix::fs::symlink(&victim, &root).unwrap();

        assert!(ensure_root(&root).is_err());
        // Even a stale-looking entry behind the symlink must survive.
        let old = SystemTime::now() - STALE_AFTER * 2;
        fs::File::open(victim.join("drag-old"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        purge_stale(&root);
        assert!(victim.join("drag-old").exists());
    }

    #[cfg(unix)]
    #[test]
    fn root_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache/drag");
        ensure_root(&root).unwrap();
        let mode = fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    // Setting a directory's mtime through `File::open` is Unix-only.
    #[cfg(unix)]
    #[test]
    fn purge_only_removes_stale_stage_folders() {
        let dir = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - STALE_AFTER * 2;
        for name in ["drag-old", "keep-me"] {
            fs::create_dir(dir.path().join(name)).unwrap();
            fs::File::open(dir.path().join(name))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        purge_stale(dir.path());
        assert!(!dir.path().join("drag-old").exists());
        assert!(dir.path().join("keep-me").exists());
    }
}
