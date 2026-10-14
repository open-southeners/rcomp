//! Finder Quick Actions ("Compress with Rcomp" / "Decompress with Rcomp").
//!
//! The menu items are declared as `NSServices` in the bundle's `Info.plist`;
//! macOS lists them under **Quick Actions** in Finder's context menu and sends
//! the chosen message (`compressFiles:userData:error:` or
//! `decompressFiles:userData:error:`) to the object registered here, launching
//! the app first when needed.  The selected files arrive on a pasteboard as
//! file URLs and are delivered to the frontend like any other launch request.
//!
//! Services are only registered for a bundled app that LaunchServices knows
//! about (e.g. installed in `/Applications`); `tauri dev` does not show them.

use std::path::Path;

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObject,
};
use objc2_app_kit::{NSApplication, NSPasteboard};
use objc2_foundation::{NSString, NSURL};
use tauri::AppHandle;

use crate::open_files::{self, LaunchAction, LaunchRequest};

/// Instance state: the app to deliver requests to.
pub struct Ivars {
    app: AppHandle,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RcompServicesProvider"]
    #[ivars = Ivars]
    pub struct ServicesProvider;

    impl ServicesProvider {
        /// `NSMessage` = `compressFiles` in `Info.plist`.
        #[unsafe(method(compressFiles:userData:error:))]
        fn compress_files(
            &self,
            pboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            self.deliver(LaunchAction::Compress, pboard);
        }

        /// `NSMessage` = `decompressFiles` in `Info.plist`.
        #[unsafe(method(decompressFiles:userData:error:))]
        fn decompress_files(
            &self,
            pboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            self.deliver(LaunchAction::Decompress, pboard);
        }
    }
);

impl ServicesProvider {
    fn new(mtm: MainThreadMarker, app: AppHandle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { app });
        // SAFETY: `init` on an NSObject subclass with initialised ivars.
        unsafe { msg_send![super(this), init] }
    }

    fn deliver(&self, action: LaunchAction, pboard: &NSPasteboard) {
        let paths = open_files::existing_paths(file_paths(pboard), Path::new("/"));
        open_files::deliver(&self.ivars().app, LaunchRequest { action, paths });
    }
}

/// The file paths on a services pasteboard, one per Finder selection item.
///
/// Finder sends file *reference* URLs (`file:///.file/id=<volume>.<inode>`),
/// whose literal path does not exist, so each URL is resolved to its real
/// path by Foundation rather than parsed as text.
fn file_paths(pboard: &NSPasteboard) -> Vec<String> {
    let file_url = NSString::from_str("public.file-url");
    let Some(items) = pboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.stringForType(&file_url))
        .filter_map(|s| resolve_file_url(&s))
        .collect()
}

/// Resolve a `file:` URL string, including a file reference URL, to a path.
fn resolve_file_url(url: &NSString) -> Option<String> {
    let path = NSURL::URLWithString(url)?.filePathURL()?.path()?;
    Some(path.to_string())
}

/// Register the services provider with AppKit.  Must run on the main thread,
/// before the run loop delivers the launching service request.
pub fn register(app: &AppHandle) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let provider = ServicesProvider::new(mtm, app.clone());
    // SAFETY: the provider implements the selectors `Info.plist` names, and
    // it is intentionally leaked below so it outlives the application.
    unsafe {
        NSApplication::sharedApplication(mtm).setServicesProvider(Some(&provider));
    }
    std::mem::forget(provider);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_file_reference_urls_resolve_to_real_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Informe año 1.txt");
        std::fs::write(&file, b"x").unwrap();
        let real = std::fs::canonicalize(&file).unwrap();

        let url = NSURL::fileURLWithPath(&NSString::from_str(&real.to_string_lossy()));
        let reference = url.fileReferenceURL().unwrap().absoluteString().unwrap();
        assert!(
            reference.to_string().starts_with("file:///.file/id="),
            "expected a file reference URL, got {reference}"
        );

        // Foundation returns the name as stored (decomposed Unicode), so
        // compare file identity rather than the string.
        for input in [reference, url.absoluteString().unwrap()] {
            let resolved = resolve_file_url(&input).expect("resolves");
            assert!(
                same_file(Path::new(&resolved), &real),
                "{input} -> {resolved}"
            );
        }
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
}
