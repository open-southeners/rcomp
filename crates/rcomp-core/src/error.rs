//! Error types and `Result` alias for `rcomp-core`.

use std::path::PathBuf;

/// All errors that `rcomp-core` operations can produce.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file extension and magic bytes did not match any known format.
    #[error("unrecognized or unsupported format: {path}")]
    UnknownFormat {
        /// The path or name that could not be identified.
        path: PathBuf,
    },

    /// The requested operation is not available for the given format (e.g. rar
    /// compression).
    #[error("{operation} is not supported for {format}")]
    UnsupportedOperation {
        /// The format name (e.g. `"rar"`).
        format: String,
        /// The operation that was attempted (e.g. `"compress"`).
        operation: String,
    },

    /// The operation was cancelled via a [`crate::progress::CancelToken`].
    #[error("operation cancelled")]
    Cancelled,

    /// The output path already exists and the caller did not enable overwrite.
    #[error("output already exists: {path}")]
    AlreadyExists {
        /// The path that already exists.
        path: PathBuf,
    },

    /// An archive entry's path resolves outside the destination directory
    /// (zip-slip / path-traversal attack).
    #[error("archive entry escapes destination: {entry}")]
    PathTraversal {
        /// The offending entry path as stored in the archive.
        entry: PathBuf,
    },

    /// An exclude glob pattern passed to the walker was syntactically invalid.
    ///
    /// This is a usage error; the CLI maps it to exit 2.
    #[error("invalid glob pattern `{pattern}`: {message}")]
    InvalidGlob {
        /// The pattern that could not be parsed.
        pattern: String,
        /// A human-readable description of the parse error.
        message: String,
    },

    /// Two or more inputs to a multi-input [`crate::compress_many`] share the
    /// same final path component, which would collide as archive root entries.
    ///
    /// This is a usage error; the CLI maps it to exit 2.
    #[error("duplicate input name `{name}`: multiple inputs resolve to the same archive root")]
    DuplicateInput {
        /// The colliding final path component.
        name: String,
    },

    /// A SHA-256 digest mismatch detected during extraction.
    ///
    /// The `kind` field distinguishes which digest failed:
    ///
    /// - `"artifact"` — the compressed file's SHA-256 did not match the
    ///   expected value supplied via [`crate::ExtractOptions::verify_sha256`].
    ///   Nothing is written to the destination directory when this fires.
    /// - `"content"` — the decompressed stream's SHA-256 did not match the
    ///   expected value supplied via
    ///   [`crate::ExtractOptions::verify_content_sha256`].
    #[error("checksum mismatch ({kind}): expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Which digest failed: `"artifact"` or `"content"`.
        kind: &'static str,
        /// The expected digest (lowercase hex).
        expected: String,
        /// The actual digest computed during the operation (lowercase hex).
        actual: String,
    },

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

    #[test]
    fn invalid_glob_displays_pattern_and_message() {
        let err = Error::InvalidGlob {
            pattern: "[bad".into(),
            message: "unclosed character class".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("[bad"), "should contain the pattern");
        assert!(
            msg.contains("unclosed character class"),
            "should contain the message"
        );
    }

    #[test]
    fn duplicate_input_displays_name() {
        let err = Error::DuplicateInput {
            name: "photos".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("photos"), "should contain the colliding name");
    }

    #[test]
    fn checksum_mismatch_displays_kind_expected_actual() {
        let err = Error::ChecksumMismatch {
            kind: "artifact",
            expected: "aabbcc".into(),
            actual: "112233".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("artifact"), "should contain kind");
        assert!(msg.contains("aabbcc"), "should contain expected digest");
        assert!(msg.contains("112233"), "should contain actual digest");

        let err2 = Error::ChecksumMismatch {
            kind: "content",
            expected: "deadbeef".into(),
            actual: "cafebabe".into(),
        };
        let msg2 = err2.to_string();
        assert!(msg2.contains("content"), "should contain kind 'content'");
    }
}
