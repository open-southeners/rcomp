//! IPC error type for the Tauri bridge.
//!
//! [`IpcError`] is the single error surface returned from every Tauri command.
//! It serialises to JSON so the frontend receives a consistent `{ kind, message,
//! data? }` shape regardless of the underlying failure.
//!
//! This module is intentionally free of `tauri::` imports so it can be compiled
//! and tested as plain Rust.

/// A serialisable error that Tauri commands return to the frontend.
///
/// The `kind` field is a stable kebab-case discriminant that the UI can branch
/// on.  `message` is a human-readable English string.  `data` carries any
/// variant-specific structured payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IpcError {
    /// Stable, kebab-case discriminant (e.g. `"already-exists"`).
    pub kind: String,
    /// Human-readable error description.
    pub message: String,
    /// Optional structured payload; omitted from JSON when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl IpcError {
    /// Construct a new `IpcError` with no structured data payload.
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            data: None,
        }
    }
}

impl From<rcomp_core::Error> for IpcError {
    fn from(err: rcomp_core::Error) -> Self {
        let message = err.to_string();
        match err {
            rcomp_core::Error::AlreadyExists { path } => Self {
                kind: "already-exists".into(),
                message,
                data: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            rcomp_core::Error::ChecksumMismatch {
                kind,
                expected,
                actual,
            } => Self {
                kind: "checksum-mismatch".into(),
                message,
                data: Some(
                    serde_json::json!({ "digest": kind, "expected": expected, "actual": actual }),
                ),
            },
            rcomp_core::Error::Cancelled => Self::new("cancelled", message),
            rcomp_core::Error::UnknownFormat { .. } => Self::new("unknown-format", message),
            rcomp_core::Error::UnsupportedOperation { .. } => {
                Self::new("unsupported-operation", message)
            }
            rcomp_core::Error::PathTraversal { .. } => Self::new("path-traversal", message),
            rcomp_core::Error::InvalidGlob { .. } => Self::new("invalid-glob", message),
            rcomp_core::Error::Io(_) => Self::new("io", message),
        }
    }
}

impl From<rcomp_core::SidecarError> for IpcError {
    fn from(err: rcomp_core::SidecarError) -> Self {
        Self::new("invalid-sidecar", err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn already_exists_kind_and_data() {
        let err = rcomp_core::Error::AlreadyExists {
            path: PathBuf::from("out.tar.gz"),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "already-exists");
        assert!(!ipc.message.is_empty());
        let data = ipc.data.expect("data should be present");
        assert_eq!(
            data["path"].as_str().unwrap(),
            "out.tar.gz",
            "data.path should contain the file name"
        );
    }

    #[test]
    fn checksum_mismatch_kind_and_data() {
        let err = rcomp_core::Error::ChecksumMismatch {
            kind: "artifact",
            expected: "aabbcc".into(),
            actual: "112233".into(),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "checksum-mismatch");
        assert!(!ipc.message.is_empty());
        let data = ipc.data.expect("data should be present");
        assert_eq!(data["digest"].as_str().unwrap(), "artifact");
        assert_eq!(data["expected"].as_str().unwrap(), "aabbcc");
        assert_eq!(data["actual"].as_str().unwrap(), "112233");
    }

    #[test]
    fn cancelled_kind() {
        let ipc: IpcError = rcomp_core::Error::Cancelled.into();
        assert_eq!(ipc.kind, "cancelled");
        assert!(!ipc.message.is_empty());
        assert!(ipc.data.is_none());
    }

    #[test]
    fn unknown_format_kind() {
        let err = rcomp_core::Error::UnknownFormat {
            path: PathBuf::from("mystery.bin"),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "unknown-format");
        assert!(ipc.message.contains("mystery.bin"));
        assert!(ipc.data.is_none());
    }

    #[test]
    fn unsupported_operation_kind() {
        let err = rcomp_core::Error::UnsupportedOperation {
            format: "rar".into(),
            operation: "compress".into(),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "unsupported-operation");
        assert!(ipc.message.contains("rar"));
        assert!(ipc.data.is_none());
    }

    #[test]
    fn path_traversal_kind() {
        let err = rcomp_core::Error::PathTraversal {
            entry: PathBuf::from("../../etc/passwd"),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "path-traversal");
        assert!(!ipc.message.is_empty());
        assert!(ipc.data.is_none());
    }

    #[test]
    fn invalid_glob_kind() {
        let err = rcomp_core::Error::InvalidGlob {
            pattern: "[bad".into(),
            message: "unclosed character class".into(),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "invalid-glob");
        assert!(ipc.message.contains("[bad"));
        assert!(ipc.data.is_none());
    }

    #[test]
    fn io_kind() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err = rcomp_core::Error::Io(io_err);
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "io");
        assert!(ipc.message.contains("no such file"));
        assert!(ipc.data.is_none());
    }

    #[test]
    fn sidecar_error_maps_to_invalid_sidecar() {
        let err = rcomp_core::SidecarError::NoMatchingEntry {
            file_name: "archive.tar.gz".into(),
        };
        let ipc: IpcError = err.into();
        assert_eq!(ipc.kind, "invalid-sidecar");
        assert!(!ipc.message.is_empty());
        assert!(ipc.data.is_none());
    }
}
