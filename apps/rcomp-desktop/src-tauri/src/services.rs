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

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObject,
};
use objc2_app_kit::{NSApplication, NSPasteboard};
use objc2_foundation::NSString;
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
        let paths = open_files::paths_from_urls(file_urls(pboard));
        open_files::deliver(&self.ivars().app, LaunchRequest { action, paths });
    }
}

/// The file URLs on a services pasteboard, one per Finder selection item.
fn file_urls(pboard: &NSPasteboard) -> Vec<tauri::Url> {
    let file_url = NSString::from_str("public.file-url");
    let Some(items) = pboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.stringForType(&file_url))
        .filter_map(|s| tauri::Url::parse(&s.to_string()).ok())
        .collect()
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
