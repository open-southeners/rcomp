//! Pure policy helpers for sidecar (sha256sum-compatible) files and archive
//! wrap-folder naming.
//!
//! These helpers contain **no filesystem I/O**.  The CLI and GUI consumers are
//! responsible for reading/writing files; they delegate the pure text-processing
//! and path logic to this module.

use std::path::PathBuf;

use crate::detect::split_format_suffix;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// An error produced while parsing a sidecar file's *text content*.
///
/// The caller (typically a CLI wrapper that owns the filesystem read) should
/// attach context such as the sidecar's file path before propagating to the
/// user.
#[derive(Debug)]
pub enum SidecarError {
    /// The `# content-sha256:` comment line contains an invalid hex digest.
    ///
    /// `line` is the raw text of the offending comment line.
    MalformedContentDigest {
        /// The raw comment line that contained the malformed digest.
        line: String,
    },
    /// An artifact line for the expected filename contains an invalid hex digest.
    ///
    /// `hex` is the malformed hex string; `file_name` is the archive file name
    /// that was being looked for.
    MalformedArtifactDigest {
        /// The malformed hex token found in the sidecar.
        hex: String,
        /// The archive file name for which the lookup was performed.
        file_name: String,
    },
    /// No artifact line in the sidecar matched the requested file name.
    ///
    /// `file_name` is the archive file name that was expected.
    NoMatchingEntry {
        /// The archive file name that had no corresponding sidecar entry.
        file_name: String,
    },
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidecarError::MalformedContentDigest { line } => {
                write!(
                    f,
                    "sidecar has a malformed content-sha256 comment: `{line}`"
                )
            }
            SidecarError::MalformedArtifactDigest { hex, file_name } => {
                write!(
                    f,
                    "sidecar contains a malformed SHA-256 digest for `{file_name}`: `{hex}`"
                )
            }
            SidecarError::NoMatchingEntry { file_name } => {
                write!(
                    f,
                    "sidecar exists but contains no line for `{file_name}` \
                     — the sidecar is required when present"
                )
            }
        }
    }
}

impl std::error::Error for SidecarError {}

// ---------------------------------------------------------------------------
// format_sidecar
// ---------------------------------------------------------------------------

/// Build the text content of a `sha256sum`-compatible sidecar file.
///
/// The returned string uses the standard `sha256sum` two-space separator so
/// that `sha256sum -c <sidecar>` works directly.
///
/// # Format
///
/// ```text
/// # content-sha256: <content_hex>     ← only when content_hex is Some
/// <artifact_hex>  <file_name>
/// ```
///
/// # Arguments
///
/// - `artifact_hex` — lowercase hex SHA-256 of the compressed artifact on disk.
/// - `content_hex` — optional lowercase hex SHA-256 of the pre-compression
///   content stream (codec-only and tar paths).
/// - `file_name` — the archive's base file name (not a full path).
///
/// # Examples
///
/// ```
/// use rcomp_core::sidecar::format_sidecar;
///
/// let hex = "a".repeat(64);
/// let text = format_sidecar(&hex, None, "archive.tar.gz");
/// assert!(text.contains(&hex));
/// assert!(text.contains("archive.tar.gz"));
/// assert!(!text.contains("content-sha256"));
/// ```
pub fn format_sidecar(artifact_hex: &str, content_hex: Option<&str>, file_name: &str) -> String {
    let mut text = String::new();
    if let Some(chex) = content_hex {
        text.push_str(&format!("# content-sha256: {chex}\n"));
    }
    text.push_str(&format!("{artifact_hex}  {file_name}\n"));
    text
}

// ---------------------------------------------------------------------------
// parse_sidecar
// ---------------------------------------------------------------------------

/// Parse the *text content* of a `sha256sum`-style sidecar file.
///
/// The caller is responsible for reading the file from disk and passing its
/// contents as `text`.  `input_name` is the **base file name** of the archive
/// being verified (not a full path).
///
/// # Accepted line forms
///
/// - `# content-sha256: <hex>` — optional comment carrying the pre-compression
///   content digest.
/// - `<hex>  <filename>` or `<hex> *<filename>` — artifact line.  Lines whose
///   filename does not match `input_name` are silently ignored (multi-archive
///   sidecars are tolerated).
/// - `#` lines that are not `content-sha256` comments are silently ignored.
/// - Blank lines are silently ignored.
///
/// # Returns
///
/// `Ok((verify_sha256, verify_content_sha256))` where each field is `Some` when
/// the corresponding digest was present and valid.
///
/// # Errors
///
/// - [`SidecarError::MalformedContentDigest`] — the `# content-sha256:` comment
///   is present but its hex value is not exactly 64 lowercase hex characters.
/// - [`SidecarError::MalformedArtifactDigest`] — an artifact line for
///   `input_name` is present but its hex value is malformed.
/// - [`SidecarError::NoMatchingEntry`] — no artifact line matches `input_name`.
pub fn parse_sidecar(
    text: &str,
    input_name: &str,
) -> Result<(Option<String>, Option<String>), SidecarError> {
    let mut artifact_hex: Option<String> = None;
    let mut content_hex: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Comment line: `# content-sha256: <hex>`
        if let Some(rest) = line.strip_prefix("# content-sha256:") {
            let hex = rest.trim().to_owned();
            if !is_sha256_hex(&hex) {
                return Err(SidecarError::MalformedContentDigest {
                    line: line.to_owned(),
                });
            }
            content_hex = Some(hex);
            continue;
        }

        // Skip other comment lines.
        if line.starts_with('#') {
            continue;
        }

        // Artifact line: `<64-hex>  <filename>` or `<64-hex> *<filename>`.
        //
        // The standard sha256sum format uses two spaces for text mode and
        // `<hex> *<name>` for binary mode.  We accept both.
        let Some((hex_part, rest)) = line.split_once(' ') else {
            // Not a recognised artifact line format — skip silently.
            continue;
        };
        let name_part = if let Some(n) = rest.strip_prefix('*') {
            n
        } else {
            rest.trim_start_matches(' ')
        };

        if name_part != input_name {
            // Different file — a multi-archive sidecar; tolerate and skip.
            continue;
        }

        // This line matches our input file.
        if !is_sha256_hex(hex_part) {
            return Err(SidecarError::MalformedArtifactDigest {
                hex: hex_part.to_owned(),
                file_name: input_name.to_owned(),
            });
        }
        artifact_hex = Some(hex_part.to_owned());
    }

    match artifact_hex {
        None => Err(SidecarError::NoMatchingEntry {
            file_name: input_name.to_owned(),
        }),
        Some(hex) => Ok((Some(hex), content_hex)),
    }
}

// ---------------------------------------------------------------------------
// is_sha256_hex
// ---------------------------------------------------------------------------

/// Return `true` if `s` is exactly 64 ASCII hexadecimal characters.
///
/// Accepts both upper- and lower-case hex digits (`is_ascii_hexdigit`).
///
/// ```
/// use rcomp_core::sidecar::is_sha256_hex;
///
/// assert!(is_sha256_hex(&"a".repeat(64)));
/// assert!(is_sha256_hex(&"0".repeat(64)));
/// assert!(!is_sha256_hex("short"));
/// assert!(!is_sha256_hex(&"g".repeat(64))); // 'g' is not hex
/// ```
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// distinct_roots
// ---------------------------------------------------------------------------

/// Count the number of distinct first path-components across all archive entries.
///
/// A single-file or single-root archive returns `1` — no wrap folder is needed.
/// Multiple distinct first components indicate a "loose" archive that benefits
/// from wrapping in a folder during extraction.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use rcomp_core::{Entry, sidecar::distinct_roots};
///
/// let single = vec![
///     Entry { path: PathBuf::from("src/main.rs"), size: 100, is_dir: false },
///     Entry { path: PathBuf::from("src/lib.rs"),  size:  80, is_dir: false },
/// ];
/// assert_eq!(distinct_roots(&single), 1);
///
/// let multi = vec![
///     Entry { path: PathBuf::from("a/file.txt"), size: 10, is_dir: false },
///     Entry { path: PathBuf::from("b/file.txt"), size: 10, is_dir: false },
/// ];
/// assert_eq!(distinct_roots(&multi), 2);
/// ```
pub fn distinct_roots(entries: &[crate::Entry]) -> usize {
    use std::collections::HashSet;
    let mut roots: HashSet<&str> = HashSet::new();
    for entry in entries {
        if let Some(first) = entry.path.components().next() {
            use std::path::Component;
            if let Component::Normal(name) = first
                && let Some(s) = name.to_str()
            {
                roots.insert(s);
            }
        }
    }
    roots.len()
}

// ---------------------------------------------------------------------------
// wrap_dir_name
// ---------------------------------------------------------------------------

/// Derive a wrap-folder name from an archive's base file name.
///
/// The strategy, in priority order:
///
/// 1. Strip the recognised format suffix via [`split_format_suffix`] and return
///    the stem.  E.g. `"photos.tar.gz"` → `"photos"`, `"data.zip"` → `"data"`.
/// 2. Fall back to `Path::file_stem` (the part before the *last* dot).
///    E.g. `"archive.unknown"` → `"archive"`.
/// 3. If neither yields a non-empty stem, return `"extracted"`.
///
/// # Arguments
///
/// `archive_file_name` — the **base file name** of the archive (not a full
/// path).  Passing a full path still works but may produce unexpected results
/// because `split_format_suffix` operates on the whole string.  Prefer calling
/// `path.file_name()` and converting to `&str` before passing here.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use rcomp_core::sidecar::wrap_dir_name;
///
/// assert_eq!(wrap_dir_name("photos.tar.gz"), PathBuf::from("photos"));
/// assert_eq!(wrap_dir_name("data.zip"),      PathBuf::from("data"));
/// assert_eq!(wrap_dir_name("archive.unknown"), PathBuf::from("archive"));
/// assert_eq!(wrap_dir_name(""),              PathBuf::from("extracted"));
/// ```
pub fn wrap_dir_name(archive_file_name: &str) -> PathBuf {
    if let Some((stem, _fmt)) = split_format_suffix(archive_file_name)
        && !stem.is_empty()
    {
        return PathBuf::from(stem);
    }
    // Fall back to Path::file_stem (last dot suffix).
    let stem = std::path::Path::new(archive_file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if stem.is_empty() {
        PathBuf::from("extracted")
    } else {
        PathBuf::from(stem)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // is_sha256_hex
    // -----------------------------------------------------------------------

    #[test]
    fn sha256_hex_valid_lowercase() {
        let hex = "a".repeat(64);
        assert!(is_sha256_hex(&hex));
    }

    #[test]
    fn sha256_hex_valid_uppercase() {
        let hex = "A".repeat(64);
        assert!(is_sha256_hex(&hex));
    }

    #[test]
    fn sha256_hex_valid_mixed() {
        // 64 chars mixing digits and upper/lower hex letters.
        let hex = "0123456789abcdefABCDEF0123456789abcdefABCDEF0123456789abcdef0123";
        assert_eq!(hex.len(), 64, "test fixture must be exactly 64 chars");
        assert!(is_sha256_hex(hex));
    }

    #[test]
    fn sha256_hex_too_short() {
        assert!(!is_sha256_hex(&"a".repeat(63)));
    }

    #[test]
    fn sha256_hex_too_long() {
        assert!(!is_sha256_hex(&"a".repeat(65)));
    }

    #[test]
    fn sha256_hex_invalid_char() {
        // 'g' is not a hex digit.
        let mut hex = "a".repeat(64);
        hex.replace_range(0..1, "g");
        assert!(!is_sha256_hex(&hex));
    }

    #[test]
    fn sha256_hex_empty() {
        assert!(!is_sha256_hex(""));
    }

    // -----------------------------------------------------------------------
    // format_sidecar
    // -----------------------------------------------------------------------

    fn dummy_hex() -> String {
        "a".repeat(64)
    }

    #[test]
    fn format_no_content_hex() {
        let text = format_sidecar(&dummy_hex(), None, "archive.tar.gz");
        assert_eq!(text, format!("{}  archive.tar.gz\n", dummy_hex()));
        assert!(!text.contains("content-sha256"));
    }

    #[test]
    fn format_with_content_hex() {
        let ahex = "a".repeat(64);
        let chex = "b".repeat(64);
        let text = format_sidecar(&ahex, Some(&chex), "archive.tar.gz");
        let expected = format!("# content-sha256: {chex}\n{ahex}  archive.tar.gz\n");
        assert_eq!(text, expected);
    }

    // -----------------------------------------------------------------------
    // parse_sidecar — round-trips through format_sidecar
    // -----------------------------------------------------------------------

    #[test]
    fn parse_roundtrip_no_content() {
        let hex = dummy_hex();
        let text = format_sidecar(&hex, None, "archive.tar.gz");
        let (artifact, content) = parse_sidecar(&text, "archive.tar.gz").unwrap();
        assert_eq!(artifact, Some(hex));
        assert_eq!(content, None);
    }

    #[test]
    fn parse_roundtrip_with_content() {
        let ahex = "a".repeat(64);
        let chex = "b".repeat(64);
        let text = format_sidecar(&ahex, Some(&chex), "archive.tar.gz");
        let (artifact, content) = parse_sidecar(&text, "archive.tar.gz").unwrap();
        assert_eq!(artifact, Some(ahex));
        assert_eq!(content, Some(chex));
    }

    #[test]
    fn parse_ignores_unrelated_filenames() {
        // A multi-archive sidecar — only the matching line is returned.
        let ahex1 = "1".repeat(64);
        let ahex2 = "2".repeat(64);
        let text = format!(
            "{ahex1}  other.tar.gz\n\
             {ahex2}  target.tar.gz\n"
        );
        let (artifact, _) = parse_sidecar(&text, "target.tar.gz").unwrap();
        assert_eq!(artifact, Some(ahex2));
    }

    #[test]
    fn parse_ignores_blank_lines_and_other_comments() {
        let hex = dummy_hex();
        let text = format!("\n# some other comment\n\n{hex}  archive.tar.gz\n\n");
        let (artifact, content) = parse_sidecar(&text, "archive.tar.gz").unwrap();
        assert_eq!(artifact, Some(hex));
        assert_eq!(content, None);
    }

    #[test]
    fn parse_accepts_binary_mode_star_prefix() {
        // `sha256sum` binary mode: `<hex> *<name>`
        let hex = dummy_hex();
        let text = format!("{hex} *archive.tar.gz\n");
        let (artifact, _) = parse_sidecar(&text, "archive.tar.gz").unwrap();
        assert_eq!(artifact, Some(hex));
    }

    // -----------------------------------------------------------------------
    // parse_sidecar — error paths
    // -----------------------------------------------------------------------

    #[test]
    fn parse_error_no_matching_line() {
        let hex = dummy_hex();
        let text = format!("{hex}  other.tar.gz\n");
        let err = parse_sidecar(&text, "archive.tar.gz").unwrap_err();
        assert!(
            matches!(err, SidecarError::NoMatchingEntry { .. }),
            "expected NoMatchingEntry, got: {err}"
        );
        assert!(err.to_string().contains("archive.tar.gz"));
    }

    #[test]
    fn parse_error_malformed_artifact_digest() {
        // The artifact line for the target file has a bad hex value.
        let text = "notahex  archive.tar.gz\n";
        let err = parse_sidecar(text, "archive.tar.gz").unwrap_err();
        assert!(
            matches!(err, SidecarError::MalformedArtifactDigest { .. }),
            "expected MalformedArtifactDigest, got: {err}"
        );
        assert!(err.to_string().contains("archive.tar.gz"));
    }

    #[test]
    fn parse_error_malformed_content_digest_comment() {
        // A `# content-sha256:` comment with a non-hex value.
        let hex = dummy_hex();
        let text = format!("# content-sha256: not-hex\n{hex}  archive.tar.gz\n");
        let err = parse_sidecar(&text, "archive.tar.gz").unwrap_err();
        assert!(
            matches!(err, SidecarError::MalformedContentDigest { .. }),
            "expected MalformedContentDigest, got: {err}"
        );
    }

    #[test]
    fn parse_content_comment_path() {
        // When the content-sha256 comment comes before the artifact line.
        let ahex = "c".repeat(64);
        let chex = "d".repeat(64);
        let text = format!("# content-sha256: {chex}\n{ahex}  archive.tar.gz\n");
        let (artifact, content) = parse_sidecar(&text, "archive.tar.gz").unwrap();
        assert_eq!(artifact, Some(ahex));
        assert_eq!(content, Some(chex));
    }

    // -----------------------------------------------------------------------
    // distinct_roots
    // -----------------------------------------------------------------------

    fn entry(path: &str) -> crate::Entry {
        crate::Entry {
            path: PathBuf::from(path),
            size: 0,
            is_dir: false,
        }
    }

    #[test]
    fn distinct_roots_single_root() {
        let entries = vec![
            entry("src/main.rs"),
            entry("src/lib.rs"),
            entry("src/util.rs"),
        ];
        assert_eq!(distinct_roots(&entries), 1);
    }

    #[test]
    fn distinct_roots_multi_root() {
        let entries = vec![
            entry("a/file.txt"),
            entry("b/file.txt"),
            entry("c/file.txt"),
        ];
        assert_eq!(distinct_roots(&entries), 3);
    }

    #[test]
    fn distinct_roots_empty() {
        assert_eq!(distinct_roots(&[]), 0);
    }

    #[test]
    fn distinct_roots_flat_files() {
        // Files with no directory component — each file IS its own root.
        let entries = vec![entry("README.md"), entry("Cargo.toml"), entry("README.md")];
        // Two distinct names ("README.md" and "Cargo.toml").
        assert_eq!(distinct_roots(&entries), 2);
    }

    // -----------------------------------------------------------------------
    // wrap_dir_name
    // -----------------------------------------------------------------------

    #[test]
    fn wrap_dir_name_tar_gz() {
        assert_eq!(wrap_dir_name("photos.tar.gz"), PathBuf::from("photos"));
    }

    #[test]
    fn wrap_dir_name_zip() {
        assert_eq!(wrap_dir_name("data.zip"), PathBuf::from("data"));
    }

    #[test]
    fn wrap_dir_name_tgz_abbreviation() {
        assert_eq!(wrap_dir_name("archive.tgz"), PathBuf::from("archive"));
    }

    #[test]
    fn wrap_dir_name_unknown_extension_falls_back_to_file_stem() {
        assert_eq!(wrap_dir_name("archive.unknown"), PathBuf::from("archive"));
    }

    #[test]
    fn wrap_dir_name_empty_string_returns_extracted() {
        assert_eq!(wrap_dir_name(""), PathBuf::from("extracted"));
    }

    #[test]
    fn wrap_dir_name_no_extension_returns_name() {
        // No dot at all — file_stem returns the whole name.
        assert_eq!(wrap_dir_name("Makefile"), PathBuf::from("Makefile"));
    }

    #[test]
    fn wrap_dir_name_multi_dot_stem() {
        assert_eq!(wrap_dir_name("my.data.tar.zst"), PathBuf::from("my.data"));
    }
}
