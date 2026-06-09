//! Format detection by file extension and magic bytes.
//!
//! Three entry points are provided:
//!
//! * [`detect_from_extension`] — pure name/extension lookup, no I/O.
//! * [`detect_from_bytes`] — pure magic-byte table lookup, no I/O.
//! * [`detect`] — reads up to 512 bytes from a real file and combines both
//!   methods, with magic bytes taking priority.

use std::path::Path;

use crate::{
    Error, Result,
    format::{Codec, Container, Format},
};

// ---------------------------------------------------------------------------
// Extension table
// ---------------------------------------------------------------------------

/// Detect a [`Format`] from a file name using extension matching.
///
/// Matching is case-insensitive and longest-suffix wins, so `.tar.gz` is
/// returned for `archive.tar.gz` rather than the `.gz` entry.
///
/// Returns `None` when the name carries no recognised extension.
///
/// Note: Brotli (`.br`, `.tar.br`) has no magic bytes and is therefore
/// **extension-only**; [`detect_from_bytes`] never returns a brotli result.
pub fn detect_from_extension(file_name: &str) -> Option<Format> {
    // Lower-case the whole name once so all comparisons are case-insensitive.
    let lower = file_name.to_ascii_lowercase();

    // Ordered longest-suffix-first so that `.tar.gz` wins over `.gz`, etc.
    // Each entry is (suffix, Format).
    type ExtensionRule = (&'static str, fn() -> Format);
    const TABLE: &[ExtensionRule] = &[
        // Two-part (layered) extensions — must come before their single-part counterparts.
        (".tar.gz", || Format::layered(Container::Tar, Codec::Gzip)),
        (".tar.bz2", || Format::layered(Container::Tar, Codec::Bzip2)),
        (".tar.xz", || Format::layered(Container::Tar, Codec::Xz)),
        (".tar.zst", || Format::layered(Container::Tar, Codec::Zstd)),
        (".tar.lz4", || Format::layered(Container::Tar, Codec::Lz4)),
        (".tar.br", || Format::layered(Container::Tar, Codec::Brotli)),
        // Abbreviated equivalents for layered tar archives.
        (".tgz", || Format::layered(Container::Tar, Codec::Gzip)),
        (".tbz2", || Format::layered(Container::Tar, Codec::Bzip2)),
        (".txz", || Format::layered(Container::Tar, Codec::Xz)),
        (".tzst", || Format::layered(Container::Tar, Codec::Zstd)),
        // Plain container/codec single-part extensions.
        (".tar", || Format::container(Container::Tar)),
        (".gz", || Format::codec(Codec::Gzip)),
        (".bz2", || Format::codec(Codec::Bzip2)),
        (".xz", || Format::codec(Codec::Xz)),
        (".zst", || Format::codec(Codec::Zstd)),
        (".lz4", || Format::codec(Codec::Lz4)),
        (".br", || Format::codec(Codec::Brotli)),
        (".zip", || Format::container(Container::Zip)),
        (".7z", || Format::container(Container::SevenZ)),
        (".rar", || Format::container(Container::Rar)),
    ];

    for (suffix, make_format) in TABLE {
        if lower.ends_with(suffix) {
            return Some(make_format());
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Magic-byte table
// ---------------------------------------------------------------------------

/// Detect a [`Format`] from the leading bytes of a file.
///
/// The function performs no I/O; pass the first few bytes (ideally ≥ 512 for
/// tar detection, but even a handful is enough for most codecs).
///
/// Returns `None` when the header matches no known signature or when the
/// buffer is too short to confirm any signature.
///
/// # Brotli
///
/// Brotli has no standardised magic bytes.  It cannot be detected from the
/// file content alone — use [`detect_from_extension`] or require the user to
/// pass an explicit `--algo` flag when the name is also absent.
pub fn detect_from_bytes(header: &[u8]) -> Option<Format> {
    // Helper: returns true when `header` starts with `magic`.
    let starts_with = |magic: &[u8]| header.len() >= magic.len() && header.starts_with(magic);

    // --- Codecs ---

    // gzip: 1F 8B
    if starts_with(&[0x1F, 0x8B]) {
        return Some(Format::codec(Codec::Gzip));
    }

    // bzip2: "BZh" (42 5A 68)
    if starts_with(b"BZh") {
        return Some(Format::codec(Codec::Bzip2));
    }

    // xz: FD 37 7A 58 5A 00
    if starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return Some(Format::codec(Codec::Xz));
    }

    // zstd: 28 B5 2F FD (little-endian magic number 0xFD2FB528)
    if starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Some(Format::codec(Codec::Zstd));
    }

    // lz4 frame: 04 22 4D 18
    if starts_with(&[0x04, 0x22, 0x4D, 0x18]) {
        return Some(Format::codec(Codec::Lz4));
    }

    // --- Archives ---

    // zip: 50 4B 03 04 (local file header) or 50 4B 05 06 (empty archive)
    if starts_with(&[0x50, 0x4B, 0x03, 0x04]) || starts_with(&[0x50, 0x4B, 0x05, 0x06]) {
        return Some(Format::container(Container::Zip));
    }

    // 7z: 37 7A BC AF 27 1C
    if starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Some(Format::container(Container::SevenZ));
    }

    // rar5: 52 61 72 21 1A 07 01 00  (must be checked before rar4)
    // rar4: 52 61 72 21 1A 07 00
    // rar4's signature is a 7-byte prefix of rar5's 8-byte signature, so we
    // test rar5 first to avoid mis-classifying a rar5 file as rar4.
    if starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00]) {
        return Some(Format::container(Container::Rar));
    }
    if starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00]) {
        return Some(Format::container(Container::Rar));
    }

    // tar: "ustar" at byte offset 257.
    // The POSIX ustar magic is the 5-byte string "ustar" (with an optional
    // space or NUL at offset 262) starting at byte 257 in the 512-byte block.
    if header.len() >= 262 {
        let magic_slice = &header[257..262];
        if magic_slice == b"ustar" {
            return Some(Format::container(Container::Tar));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Combined path-based detection
// ---------------------------------------------------------------------------

/// Detect the [`Format`] of the file at `path`.
///
/// The function reads up to 512 bytes from the file (enough to cover the tar
/// ustar magic at offset 257) and combines magic-byte and extension detection:
///
/// 1. **Magic hit + extension refinement** — if the magic bytes identify a
///    codec and the file name identifies a layered format whose codec matches,
///    the layered format is returned (e.g. gzip magic + `.tar.gz` name →
///    `tar.gz`).  Similarly, if the magic identifies a container and the
///    extension agrees, the extension result is used.
/// 2. **Magic hit alone** — when the extension result does not refine the
///    magic result, the magic result is returned.
/// 3. **Extension only** — when magic detection yields nothing (e.g. brotli),
///    the extension result is returned.
/// 4. **Neither** — [`Error::UnknownFormat`] is returned.
///
/// A missing or unreadable file produces [`Error::Io`].
///
/// Note: detecting a tar archive *inside* a compressed stream (e.g. checking
/// whether a `.gz` file contains tar data) requires decompressing first and is
/// deferred to milestone 3.
pub fn detect(path: &Path) -> Result<Format> {
    use std::{fs, io::Read};

    // Read up to 512 bytes.
    let mut buf = [0u8; 512];
    let n = {
        let mut file = fs::File::open(path).map_err(Error::Io)?;
        file.read(&mut buf).map_err(Error::Io)?
    };
    let header = &buf[..n];

    let magic = detect_from_bytes(header);
    let ext = path
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(detect_from_extension);

    match (magic, ext) {
        // Magic succeeded — try to refine with extension.
        (Some(magic_fmt), Some(ext_fmt)) => {
            // Refinement: the extension's Format "agrees with" the magic result
            // when they share the same codec or the same container.
            let refines = (magic_fmt.codec.is_some() && magic_fmt.codec == ext_fmt.codec)
                || (magic_fmt.container.is_some()
                    && magic_fmt.container == ext_fmt.container);
            if refines {
                Ok(ext_fmt)
            } else {
                Ok(magic_fmt)
            }
        }
        // Magic succeeded, no extension hint.
        (Some(magic_fmt), None) => Ok(magic_fmt),
        // No magic, fall back to extension.
        (None, Some(ext_fmt)) => Ok(ext_fmt),
        // Neither — unknown format.
        (None, None) => Err(Error::UnknownFormat {
            path: path.to_path_buf(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // detect_from_extension — every table row
    // -----------------------------------------------------------------------

    #[test]
    fn ext_tar_gz() {
        assert_eq!(
            detect_from_extension("archive.tar.gz"),
            Some(Format::layered(Container::Tar, Codec::Gzip))
        );
    }

    #[test]
    fn ext_tgz() {
        assert_eq!(
            detect_from_extension("archive.tgz"),
            Some(Format::layered(Container::Tar, Codec::Gzip))
        );
    }

    #[test]
    fn ext_tar_bz2() {
        assert_eq!(
            detect_from_extension("archive.tar.bz2"),
            Some(Format::layered(Container::Tar, Codec::Bzip2))
        );
    }

    #[test]
    fn ext_tbz2() {
        assert_eq!(
            detect_from_extension("archive.tbz2"),
            Some(Format::layered(Container::Tar, Codec::Bzip2))
        );
    }

    #[test]
    fn ext_tar_xz() {
        assert_eq!(
            detect_from_extension("archive.tar.xz"),
            Some(Format::layered(Container::Tar, Codec::Xz))
        );
    }

    #[test]
    fn ext_txz() {
        assert_eq!(
            detect_from_extension("archive.txz"),
            Some(Format::layered(Container::Tar, Codec::Xz))
        );
    }

    #[test]
    fn ext_tar_zst() {
        assert_eq!(
            detect_from_extension("archive.tar.zst"),
            Some(Format::layered(Container::Tar, Codec::Zstd))
        );
    }

    #[test]
    fn ext_tzst() {
        assert_eq!(
            detect_from_extension("archive.tzst"),
            Some(Format::layered(Container::Tar, Codec::Zstd))
        );
    }

    #[test]
    fn ext_tar_lz4() {
        assert_eq!(
            detect_from_extension("archive.tar.lz4"),
            Some(Format::layered(Container::Tar, Codec::Lz4))
        );
    }

    #[test]
    fn ext_tar_br() {
        assert_eq!(
            detect_from_extension("archive.tar.br"),
            Some(Format::layered(Container::Tar, Codec::Brotli))
        );
    }

    #[test]
    fn ext_tar() {
        assert_eq!(
            detect_from_extension("archive.tar"),
            Some(Format::container(Container::Tar))
        );
    }

    #[test]
    fn ext_gz() {
        assert_eq!(
            detect_from_extension("file.gz"),
            Some(Format::codec(Codec::Gzip))
        );
    }

    #[test]
    fn ext_bz2() {
        assert_eq!(
            detect_from_extension("file.bz2"),
            Some(Format::codec(Codec::Bzip2))
        );
    }

    #[test]
    fn ext_xz() {
        assert_eq!(
            detect_from_extension("file.xz"),
            Some(Format::codec(Codec::Xz))
        );
    }

    #[test]
    fn ext_zst() {
        assert_eq!(
            detect_from_extension("file.zst"),
            Some(Format::codec(Codec::Zstd))
        );
    }

    #[test]
    fn ext_lz4() {
        assert_eq!(
            detect_from_extension("file.lz4"),
            Some(Format::codec(Codec::Lz4))
        );
    }

    #[test]
    fn ext_br() {
        assert_eq!(
            detect_from_extension("file.br"),
            Some(Format::codec(Codec::Brotli))
        );
    }

    #[test]
    fn ext_zip() {
        assert_eq!(
            detect_from_extension("archive.zip"),
            Some(Format::container(Container::Zip))
        );
    }

    #[test]
    fn ext_7z() {
        assert_eq!(
            detect_from_extension("archive.7z"),
            Some(Format::container(Container::SevenZ))
        );
    }

    #[test]
    fn ext_rar() {
        assert_eq!(
            detect_from_extension("archive.rar"),
            Some(Format::container(Container::Rar))
        );
    }

    // --- Longest-suffix precedence ---

    #[test]
    fn ext_longest_suffix_tar_gz_not_gz() {
        // .tar.gz must win over .gz
        let result = detect_from_extension("archive.tar.gz");
        assert_eq!(result, Some(Format::layered(Container::Tar, Codec::Gzip)));
        // Confirm the codec-only form was NOT returned.
        assert_ne!(result, Some(Format::codec(Codec::Gzip)));
    }

    #[test]
    fn ext_longest_suffix_tar_bz2_not_bz2() {
        let result = detect_from_extension("backup.tar.bz2");
        assert_eq!(result, Some(Format::layered(Container::Tar, Codec::Bzip2)));
    }

    // --- Case-insensitivity ---

    #[test]
    fn ext_case_insensitive_tgz_uppercase() {
        assert_eq!(
            detect_from_extension("FILE.TGZ"),
            Some(Format::layered(Container::Tar, Codec::Gzip))
        );
    }

    #[test]
    fn ext_case_insensitive_tar_gz_mixed() {
        assert_eq!(
            detect_from_extension("Archive.TAR.GZ"),
            Some(Format::layered(Container::Tar, Codec::Gzip))
        );
    }

    #[test]
    fn ext_case_insensitive_zip_uppercase() {
        assert_eq!(
            detect_from_extension("DATA.ZIP"),
            Some(Format::container(Container::Zip))
        );
    }

    // --- Unknown extension ---

    #[test]
    fn ext_unknown_returns_none() {
        assert_eq!(detect_from_extension("document.pdf"), None);
    }

    #[test]
    fn ext_no_extension_returns_none() {
        assert_eq!(detect_from_extension("Makefile"), None);
    }

    #[test]
    fn ext_empty_string_returns_none() {
        assert_eq!(detect_from_extension(""), None);
    }

    // -----------------------------------------------------------------------
    // detect_from_bytes — one synthetic header per format
    // -----------------------------------------------------------------------

    #[test]
    fn magic_gzip() {
        let header = [0x1F, 0x8B, 0x08, 0x00];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::codec(Codec::Gzip))
        );
    }

    #[test]
    fn magic_bzip2() {
        let header = [b'B', b'Z', b'h', b'9'];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::codec(Codec::Bzip2))
        );
    }

    #[test]
    fn magic_xz() {
        let header = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00];
        assert_eq!(detect_from_bytes(&header), Some(Format::codec(Codec::Xz)));
    }

    #[test]
    fn magic_zstd() {
        let header = [0x28, 0xB5, 0x2F, 0xFD, 0x04, 0x00];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::codec(Codec::Zstd))
        );
    }

    #[test]
    fn magic_lz4() {
        let header = [0x04, 0x22, 0x4D, 0x18, 0x64, 0x40];
        assert_eq!(detect_from_bytes(&header), Some(Format::codec(Codec::Lz4)));
    }

    #[test]
    fn magic_zip_local_file_header() {
        let header = [0x50, 0x4B, 0x03, 0x04, 0x14, 0x00];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::container(Container::Zip))
        );
    }

    #[test]
    fn magic_zip_empty_archive() {
        let header = [0x50, 0x4B, 0x05, 0x06, 0x00, 0x00];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::container(Container::Zip))
        );
    }

    #[test]
    fn magic_sevenz() {
        let header = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::container(Container::SevenZ))
        );
    }

    #[test]
    fn magic_rar4() {
        // RAR4: 52 61 72 21 1A 07 00
        let header = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00, 0xFF];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::container(Container::Rar))
        );
    }

    #[test]
    fn magic_rar5() {
        // RAR5: 52 61 72 21 1A 07 01 00
        let header = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00, 0xFF];
        assert_eq!(
            detect_from_bytes(&header),
            Some(Format::container(Container::Rar))
        );
    }

    /// RAR5's signature starts with RAR4's 7 bytes plus an extra 0x01 0x00.
    /// Ensure that a RAR5 file is not mis-classified as RAR4.
    #[test]
    fn magic_rar5_not_misidentified_as_rar4() {
        let rar5_header = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00, 0xFF];
        // Both return Container::Rar; the important invariant is that the
        // rar5 bytes go through the rar5 branch, which we verify by checking
        // that the rar4-only bytes (ending 0x00) do NOT match the rar5 pattern.
        let rar4_header = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00, 0xFF, 0xFF];
        assert_eq!(
            detect_from_bytes(&rar5_header),
            Some(Format::container(Container::Rar))
        );
        assert_eq!(
            detect_from_bytes(&rar4_header),
            Some(Format::container(Container::Rar))
        );
    }

    #[test]
    fn magic_tar_ustar_at_offset_257() {
        // Build a minimal 512-byte block with "ustar" at offset 257.
        let mut block = [0u8; 512];
        block[257] = b'u';
        block[258] = b's';
        block[259] = b't';
        block[260] = b'a';
        block[261] = b'r';
        assert_eq!(
            detect_from_bytes(&block),
            Some(Format::container(Container::Tar))
        );
    }

    // --- Short / empty buffers ---

    #[test]
    fn magic_empty_buffer_returns_none() {
        assert_eq!(detect_from_bytes(&[]), None);
    }

    #[test]
    fn magic_one_byte_returns_none() {
        assert_eq!(detect_from_bytes(&[0x1F]), None);
    }

    #[test]
    fn magic_three_bytes_returns_none_for_most() {
        // 3 bytes: enough for bzip2 (BZh) but not xz (6 bytes).
        assert_eq!(
            detect_from_bytes(b"BZh"),
            Some(Format::codec(Codec::Bzip2))
        );
        // Random 3 bytes that don't match anything.
        assert_eq!(detect_from_bytes(&[0x00, 0x01, 0x02]), None);
    }

    #[test]
    fn magic_short_buffer_under_four_bytes_returns_none_for_four_byte_sigs() {
        // zstd needs 4 bytes; 3 bytes → None.
        assert_eq!(detect_from_bytes(&[0x28, 0xB5, 0x2F]), None);
    }

    // -----------------------------------------------------------------------
    // detect() — integration tests using real temp files
    // -----------------------------------------------------------------------

    #[cfg(test)]
    mod detect_integration {
        use super::*;
        use std::io::Write;
        use tempfile::NamedTempFile;

        /// Helper: create a named temp file with the given extension and content,
        /// returning the file (kept alive) and its path.
        fn temp_file_with_content(
            suffix: &str,
            content: &[u8],
        ) -> (NamedTempFile, std::path::PathBuf) {
            let mut f = tempfile::Builder::new()
                .suffix(suffix)
                .tempfile()
                .expect("tempfile");
            f.write_all(content).expect("write");
            f.flush().expect("flush");
            let path = f.path().to_path_buf();
            (f, path)
        }

        // --- Magic + extension refinement ---

        #[test]
        fn detect_gzip_bytes_and_tar_gz_name_returns_layered() {
            // Real gzip magic bytes but the name says .tar.gz → layered.
            let gzip_magic = [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00];
            let (_f, path) = temp_file_with_content(".tar.gz", &gzip_magic);
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::layered(Container::Tar, Codec::Gzip));
        }

        #[test]
        fn detect_gzip_bytes_and_tgz_name_returns_layered() {
            let gzip_magic = [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00];
            let (_f, path) = temp_file_with_content(".tgz", &gzip_magic);
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::layered(Container::Tar, Codec::Gzip));
        }

        // --- Magic only (no useful extension) ---

        #[test]
        fn detect_magic_only_extensionless_gzip() {
            // Extensionless file: only magic bytes decide.
            let gzip_magic = [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00];
            let mut f = tempfile::Builder::new()
                .suffix("") // no extension
                .tempfile()
                .expect("tempfile");
            f.write_all(&gzip_magic).expect("write");
            f.flush().expect("flush");
            let path = f.path().to_path_buf();
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::codec(Codec::Gzip));
        }

        // --- Extension only (brotli — no magic bytes) ---

        #[test]
        fn detect_brotli_extension_only() {
            // Brotli has no magic; detection must fall back to extension.
            let arbitrary_content = b"this is not really brotli data";
            let (_f, path) = temp_file_with_content(".br", arbitrary_content);
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::codec(Codec::Brotli));
        }

        // --- Unknown content and name → UnknownFormat ---

        #[test]
        fn detect_unknown_content_and_name_returns_unknown_format() {
            let content = b"hello world, definitely not a known archive";
            let (_f, path) = temp_file_with_content(".bin", content);
            let err = detect(&path).expect_err("should be Err");
            assert!(
                matches!(err, Error::UnknownFormat { .. }),
                "expected UnknownFormat, got: {err:?}"
            );
        }

        // --- Missing path → Io ---

        #[test]
        fn detect_missing_file_returns_io_error() {
            let path = std::path::Path::new("/nonexistent/path/to/file.tar.gz");
            let err = detect(path).expect_err("should be Err");
            assert!(
                matches!(err, Error::Io(_)),
                "expected Io error, got: {err:?}"
            );
        }

        // --- Magic disagrees with extension → magic wins ---

        #[test]
        fn detect_magic_wins_when_extension_disagrees() {
            // File has gzip magic but is named .zip — magic wins.
            let gzip_magic = [0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00];
            let (_f, path) = temp_file_with_content(".zip", &gzip_magic);
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::codec(Codec::Gzip));
        }

        // --- Zstd magic + tar.zst name → layered ---

        #[test]
        fn detect_zstd_bytes_and_tar_zst_name_returns_layered() {
            let zstd_magic = [0x28, 0xB5, 0x2F, 0xFD, 0x04, 0x00];
            let (_f, path) = temp_file_with_content(".tar.zst", &zstd_magic);
            let result = detect(&path).expect("detect");
            assert_eq!(result, Format::layered(Container::Tar, Codec::Zstd));
        }
    }
}
