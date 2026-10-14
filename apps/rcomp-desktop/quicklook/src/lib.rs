//! C ABI over [`rcomp_core::list`] for the macOS Quick Look extension.
//!
//! The Swift extension links this crate as a static library and calls
//! [`rcomp_ql_list_json`] with the previewed file's path, receiving the
//! archive's entries as a JSON document it renders into HTML.  JSON keeps the
//! boundary to one owned C string in each direction.

use std::{
    ffi::{CStr, CString, c_char},
    path::Path,
};

use rcomp_core::{Entry, detect, list};

/// The JSON document handed to the extension.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum Listing {
    Ok {
        ok: bool,
        format: String,
        entries: Vec<Entry>,
    },
    Err {
        ok: bool,
        /// `"unsupported"` for single-stream files and formats that cannot be
        /// listed, `"error"` for anything else.
        kind: &'static str,
        message: String,
    },
}

/// List `path` into a [`Listing`].  Safe Rust, so it can be tested directly.
fn listing(path: &Path) -> Listing {
    let format = match detect(path) {
        Ok(f) => f.to_string(),
        Err(e) => return failure(e),
    };
    match list(path) {
        Ok(entries) => Listing::Ok {
            ok: true,
            format,
            entries,
        },
        Err(e) => failure(e),
    }
}

fn failure(err: rcomp_core::Error) -> Listing {
    let kind = match err {
        rcomp_core::Error::UnsupportedOperation { .. }
        | rcomp_core::Error::UnknownFormat { .. } => "unsupported",
        _ => "error",
    };
    Listing::Err {
        ok: false,
        kind,
        message: err.to_string(),
    }
}

/// List the archive at the NUL-terminated UTF-8 `path` as JSON.
///
/// Never returns null; free the result with [`rcomp_ql_free`].
///
/// # Safety
///
/// `path` must be null or point to a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rcomp_ql_list_json(path: *const c_char) -> *mut c_char {
    let doc = if path.is_null() {
        Listing::Err {
            ok: false,
            kind: "error",
            message: "no path given".into(),
        }
    } else {
        // SAFETY: the caller guarantees a valid NUL-terminated string.
        let raw = unsafe { CStr::from_ptr(path) };
        match raw.to_str() {
            Ok(p) => listing(Path::new(p)),
            Err(_) => Listing::Err {
                ok: false,
                kind: "error",
                message: "path is not valid UTF-8".into(),
            },
        }
    };
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| {
        r#"{"ok":false,"kind":"error","message":"could not encode the listing"}"#.into()
    });
    // JSON escapes NUL as \u0000, so the string never contains one.
    CString::new(json).unwrap_or_default().into_raw()
}

/// Free a string returned by [`rcomp_ql_list_json`].  Null is ignored.
///
/// # Safety
///
/// `json` must be null or a pointer returned by [`rcomp_ql_list_json`] that
/// has not been freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rcomp_ql_free(json: *mut c_char) {
    if !json.is_null() {
        // SAFETY: the pointer came from `CString::into_raw` above.
        drop(unsafe { CString::from_raw(json) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(path: &str) -> serde_json::Value {
        let c = CString::new(path).unwrap();
        // SAFETY: valid C string in, owned C string out and freed below.
        unsafe {
            let out = rcomp_ql_list_json(c.as_ptr());
            let v = serde_json::from_str(CStr::from_ptr(out).to_str().unwrap()).unwrap();
            rcomp_ql_free(out);
            v
        }
    }

    #[test]
    fn lists_a_zip_as_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.txt"), b"abc").unwrap();
        let zip = dir.path().join("x.zip");
        rcomp_core::compress_many(
            &[dir.path().join("docs")],
            &zip,
            &rcomp_core::CompressOptions {
                follow_gitignore: false,
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();

        let v = call(zip.to_str().unwrap());
        assert_eq!(v["ok"], true);
        assert_eq!(v["format"], "zip");
        let entries = v["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e["path"] == "docs/a.txt" && e["size"] == 3 && e["is_dir"] == false)
        );
    }

    #[test]
    fn single_stream_files_are_reported_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("a.txt");
        std::fs::write(&plain, b"hello").unwrap();
        let gz = dir.path().join("a.txt.gz");
        rcomp_core::compress(&plain, &gz, &Default::default(), |_| {}).unwrap();

        let v = call(gz.to_str().unwrap());
        assert_eq!(v["ok"], false);
        assert_eq!(v["kind"], "unsupported");
    }

    #[test]
    fn null_and_missing_paths_are_errors_not_crashes() {
        // SAFETY: null is explicitly allowed.
        unsafe {
            let out = rcomp_ql_list_json(std::ptr::null());
            assert!(!out.is_null());
            rcomp_ql_free(out);
        }
        assert_eq!(call("/definitely/not/here.zip")["ok"], false);
    }
}
