//! Error types and `Result` alias for `rcomp-core`.

use std::path::PathBuf;

/// All errors that `rcomp-core` operations can produce.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file extension and magic bytes did not match any known format.
    #[error("unrecognized or unsupported format: {path}")]
    UnknownFormat { path: PathBuf },

    /// The requested operation is not available for the given format (e.g. rar
    /// compression).
    #[error("{operation} is not supported for {format}")]
    UnsupportedOperation { format: String, operation: String },

    /// The operation was cancelled via a [`crate::progress::CancelToken`].
    #[error("operation cancelled")]
    Cancelled,

    /// The output path already exists and the caller did not enable overwrite.
    #[error("output already exists: {path}")]
    AlreadyExists { path: PathBuf },

    /// An archive entry's path resolves outside the destination directory
    /// (zip-slip / path-traversal attack).
    #[error("archive entry escapes destination: {entry}")]
    PathTraversal { entry: PathBuf },

    /// An underlying I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience `Result` alias that pins the error type to [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_format_displays_path() {
        let err = Error::UnknownFormat {
            path: PathBuf::from("mystery.bin"),
        };
        assert!(err.to_string().contains("mystery.bin"));
    }

    #[test]
    fn unsupported_operation_displays_format_and_op() {
        let err = Error::UnsupportedOperation {
            format: "rar".into(),
            operation: "compress".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("rar"));
        assert!(msg.contains("compress"));
    }

    #[test]
    fn cancelled_displays_message() {
        assert_eq!(Error::Cancelled.to_string(), "operation cancelled");
    }

    #[test]
    fn already_exists_displays_path() {
        let err = Error::AlreadyExists {
            path: PathBuf::from("output.tar.gz"),
        };
        assert!(err.to_string().contains("output.tar.gz"));
    }

    #[test]
    fn io_error_is_transparent() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err: Error = io_err.into();
        assert!(err.to_string().contains("no such file"));
    }
}
