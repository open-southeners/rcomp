//! Two-layer format model: `Codec` (stream compressor) and `Container` (archive).
//!
//! The two layers can appear independently or together:
//!
//! | Example         | `container`          | `codec`          |
//! |-----------------|----------------------|------------------|
//! | `file.gz`       | `None`               | `Some(Gzip)`     |
//! | `archive.tar`   | `Some(Tar)`          | `None`           |
//! | `archive.tar.gz`| `Some(Tar)`          | `Some(Gzip)`     |
//! | `archive.zip`   | `Some(Zip)`          | `None`           |
//!
//! Use the provided constructors rather than struct literals to ensure the
//! invariant that at least one of the two fields is `Some`.

use std::{fmt, str::FromStr};

/// A single-stream codec (compressor/decompressor).
///
/// Each variant corresponds to one byte-stream compression algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// GNU zip / DEFLATE (`.gz`).
    Gzip,
    /// Burrows–Wheeler bzip2 (`.bz2`).
    Bzip2,
    /// LZMA2 XZ container (`.xz`).
    Xz,
    /// Zstandard (`.zst`).
    Zstd,
    /// LZ4 frame format (`.lz4`).
    Lz4,
    /// Brotli (`.br`).
    ///
    /// Note: Brotli has no magic bytes signature; detection is extension-only.
    Brotli,
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Codec::Gzip => "gzip",
            Codec::Bzip2 => "bzip2",
            Codec::Xz => "xz",
            Codec::Zstd => "zstd",
            Codec::Lz4 => "lz4",
            Codec::Brotli => "brotli",
        };
        f.write_str(name)
    }
}

/// Short file extension used for a codec when it appears as the trailing part
/// of a layered name (e.g. `tar.gz`, `tar.zst`).
impl Codec {
    /// Returns the canonical short extension for this codec (without the leading dot).
    pub(crate) fn short_ext(self) -> &'static str {
        match self {
            Codec::Gzip => "gz",
            Codec::Bzip2 => "bz2",
            Codec::Xz => "xz",
            Codec::Zstd => "zst",
            Codec::Lz4 => "lz4",
            Codec::Brotli => "br",
        }
    }
}

/// A multi-entry archive container format.
///
/// zip, 7z, and rar compress entries internally; tar is uncompressed on its
/// own and is typically paired with a [`Codec`] (see [`Format::layered`]).
/// Rar is supported for extraction only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// POSIX tape archive (`.tar`).
    Tar,
    /// Info-ZIP / PKZIP (`.zip`).
    Zip,
    /// 7-Zip archive (`.7z`).
    SevenZ,
    /// RAR archive — extract-only (`.rar`).
    Rar,
}

impl fmt::Display for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Container::Tar => "tar",
            Container::Zip => "zip",
            Container::SevenZ => "7z",
            Container::Rar => "rar",
        };
        f.write_str(name)
    }
}

/// A fully described archive/compression format.
///
/// At least one of `container` and `codec` is always `Some`.  Use the
/// provided constructors to build values; direct struct literal construction
/// could violate the invariant.
///
/// # Display
///
/// Standalone codecs and containers display their canonical name (`gzip`,
/// `tar`, `7z`, …).  Layered formats use the `<container>.<codec-ext>` style
/// that matches the conventional file extension — for example `tar.gz`,
/// `tar.bz2`, `tar.zst`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// The optional archive container layer.
    pub container: Option<Container>,
    /// The optional stream-compression codec layer.
    pub codec: Option<Codec>,
}

impl Format {
    /// A codec-only format (e.g. a plain `.gz` file).
    pub fn codec(codec: Codec) -> Self {
        Self {
            container: None,
            codec: Some(codec),
        }
    }

    /// A container-only format (e.g. a plain `.tar`, `.zip`, or `.7z` file).
    pub fn container(container: Container) -> Self {
        Self {
            container: Some(container),
            codec: None,
        }
    }

    /// A layered format: a container piped through a codec (e.g. `tar.gz`).
    pub fn layered(container: Container, codec: Codec) -> Self {
        Self {
            container: Some(container),
            codec: Some(codec),
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.container, self.codec) {
            (Some(c), Some(codec)) => write!(f, "{}.{}", c, codec.short_ext()),
            (Some(c), None) => write!(f, "{}", c),
            (None, Some(codec)) => write!(f, "{}", codec),
            // Invariant: at least one is Some — constructors enforce this.
            (None, None) => write!(f, "<invalid format>"),
        }
    }
}

/// Parse a [`Format`] from a string name.
///
/// Accepts:
///
/// - Codec canonical names and aliases: `gzip`/`gz`, `bzip2`/`bz2`, `xz`,
///   `zstd`/`zst`, `lz4`, `brotli`/`br`.
/// - Container canonical names and aliases: `tar`, `zip`, `7z`/`sevenz`, `rar`.
/// - Layered names via the extension table (e.g. `tar.gz`, `tar.bz2`, `.tgz`).
///
/// Matching is case-insensitive.
///
/// # Errors
///
/// Returns [`crate::Error::UnknownFormat`] when the string does not map to
/// any known format, with `path` set to the input string as a [`PathBuf`].
impl FromStr for Format {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use std::path::PathBuf;
        let lower = s.to_ascii_lowercase();

        // 1. Codec name/alias map.
        let codec_match = match lower.as_str() {
            "gzip" | "gz" => Some(Format::codec(Codec::Gzip)),
            "bzip2" | "bz2" => Some(Format::codec(Codec::Bzip2)),
            "xz" => Some(Format::codec(Codec::Xz)),
            "zstd" | "zst" => Some(Format::codec(Codec::Zstd)),
            "lz4" => Some(Format::codec(Codec::Lz4)),
            "brotli" | "br" => Some(Format::codec(Codec::Brotli)),
            _ => None,
        };
        if let Some(f) = codec_match {
            return Ok(f);
        }

        // 2. Container name/alias map.
        let container_match = match lower.as_str() {
            "tar" => Some(Format::container(Container::Tar)),
            "zip" => Some(Format::container(Container::Zip)),
            "7z" | "sevenz" => Some(Format::container(Container::SevenZ)),
            "rar" => Some(Format::container(Container::Rar)),
            _ => None,
        };
        if let Some(f) = container_match {
            return Ok(f);
        }

        // 3. Layered (and abbreviated) names via the extension table.
        //    Prepend "x." so the suffix-based table can match, e.g. "tar.gz" → "x.tar.gz".
        let probe = format!("x.{lower}");
        if let Some(f) = crate::detect::detect_from_extension(&probe) {
            return Ok(f);
        }

        Err(crate::Error::UnknownFormat {
            path: PathBuf::from(s),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Codec Display ---

    #[test]
    fn codec_display_gzip() {
        assert_eq!(Codec::Gzip.to_string(), "gzip");
    }

    #[test]
    fn codec_display_bzip2() {
        assert_eq!(Codec::Bzip2.to_string(), "bzip2");
    }

    #[test]
    fn codec_display_xz() {
        assert_eq!(Codec::Xz.to_string(), "xz");
    }

    #[test]
    fn codec_display_zstd() {
        assert_eq!(Codec::Zstd.to_string(), "zstd");
    }

    #[test]
    fn codec_display_lz4() {
        assert_eq!(Codec::Lz4.to_string(), "lz4");
    }

    #[test]
    fn codec_display_brotli() {
        assert_eq!(Codec::Brotli.to_string(), "brotli");
    }

    // --- Container Display ---

    #[test]
    fn container_display_tar() {
        assert_eq!(Container::Tar.to_string(), "tar");
    }

    #[test]
    fn container_display_zip() {
        assert_eq!(Container::Zip.to_string(), "zip");
    }

    #[test]
    fn container_display_sevenz() {
        assert_eq!(Container::SevenZ.to_string(), "7z");
    }

    #[test]
    fn container_display_rar() {
        assert_eq!(Container::Rar.to_string(), "rar");
    }

    // --- Format constructors ---

    #[test]
    fn format_codec_constructor() {
        let f = Format::codec(Codec::Gzip);
        assert_eq!(f.container, None);
        assert_eq!(f.codec, Some(Codec::Gzip));
    }

    #[test]
    fn format_container_constructor() {
        let f = Format::container(Container::Zip);
        assert_eq!(f.container, Some(Container::Zip));
        assert_eq!(f.codec, None);
    }

    #[test]
    fn format_layered_constructor() {
        let f = Format::layered(Container::Tar, Codec::Gzip);
        assert_eq!(f.container, Some(Container::Tar));
        assert_eq!(f.codec, Some(Codec::Gzip));
    }

    // --- Format Display ---

    #[test]
    fn format_display_codec_only() {
        assert_eq!(Format::codec(Codec::Gzip).to_string(), "gzip");
        assert_eq!(Format::codec(Codec::Brotli).to_string(), "brotli");
    }

    #[test]
    fn format_display_container_only() {
        assert_eq!(Format::container(Container::Tar).to_string(), "tar");
        assert_eq!(Format::container(Container::Zip).to_string(), "zip");
        assert_eq!(Format::container(Container::SevenZ).to_string(), "7z");
        assert_eq!(Format::container(Container::Rar).to_string(), "rar");
    }

    #[test]
    fn format_display_layered_tar_gz() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Gzip).to_string(),
            "tar.gz"
        );
    }

    #[test]
    fn format_display_layered_tar_bz2() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Bzip2).to_string(),
            "tar.bz2"
        );
    }

    #[test]
    fn format_display_layered_tar_xz() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Xz).to_string(),
            "tar.xz"
        );
    }

    #[test]
    fn format_display_layered_tar_zst() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Zstd).to_string(),
            "tar.zst"
        );
    }

    #[test]
    fn format_display_layered_tar_lz4() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Lz4).to_string(),
            "tar.lz4"
        );
    }

    #[test]
    fn format_display_layered_tar_br() {
        assert_eq!(
            Format::layered(Container::Tar, Codec::Brotli).to_string(),
            "tar.br"
        );
    }

    // --- FromStr — codec canonical names and aliases ---

    #[test]
    fn from_str_gzip_canonical() {
        assert_eq!("gzip".parse::<Format>().unwrap(), Format::codec(Codec::Gzip));
    }

    #[test]
    fn from_str_gz_alias() {
        assert_eq!("gz".parse::<Format>().unwrap(), Format::codec(Codec::Gzip));
    }

    #[test]
    fn from_str_bzip2_canonical() {
        assert_eq!("bzip2".parse::<Format>().unwrap(), Format::codec(Codec::Bzip2));
    }

    #[test]
    fn from_str_bz2_alias() {
        assert_eq!("bz2".parse::<Format>().unwrap(), Format::codec(Codec::Bzip2));
    }

    #[test]
    fn from_str_xz() {
        assert_eq!("xz".parse::<Format>().unwrap(), Format::codec(Codec::Xz));
    }

    #[test]
    fn from_str_zstd_canonical() {
        assert_eq!("zstd".parse::<Format>().unwrap(), Format::codec(Codec::Zstd));
    }

    #[test]
    fn from_str_zst_alias() {
        assert_eq!("zst".parse::<Format>().unwrap(), Format::codec(Codec::Zstd));
    }

    #[test]
    fn from_str_lz4() {
        assert_eq!("lz4".parse::<Format>().unwrap(), Format::codec(Codec::Lz4));
    }

    #[test]
    fn from_str_brotli_canonical() {
        assert_eq!("brotli".parse::<Format>().unwrap(), Format::codec(Codec::Brotli));
    }

    #[test]
    fn from_str_br_alias() {
        assert_eq!("br".parse::<Format>().unwrap(), Format::codec(Codec::Brotli));
    }

    // --- FromStr — container canonical names and aliases ---

    #[test]
    fn from_str_tar() {
        assert_eq!("tar".parse::<Format>().unwrap(), Format::container(Container::Tar));
    }

    #[test]
    fn from_str_zip() {
        assert_eq!("zip".parse::<Format>().unwrap(), Format::container(Container::Zip));
    }

    #[test]
    fn from_str_7z_canonical() {
        assert_eq!("7z".parse::<Format>().unwrap(), Format::container(Container::SevenZ));
    }

    #[test]
    fn from_str_sevenz_alias() {
        assert_eq!("sevenz".parse::<Format>().unwrap(), Format::container(Container::SevenZ));
    }

    #[test]
    fn from_str_rar() {
        assert_eq!("rar".parse::<Format>().unwrap(), Format::container(Container::Rar));
    }

    // --- FromStr — layered names via extension table ---

    #[test]
    fn from_str_tar_gz() {
        assert_eq!(
            "tar.gz".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Gzip)
        );
    }

    #[test]
    fn from_str_tar_bz2() {
        assert_eq!(
            "tar.bz2".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Bzip2)
        );
    }

    #[test]
    fn from_str_tar_xz() {
        assert_eq!(
            "tar.xz".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Xz)
        );
    }

    #[test]
    fn from_str_tar_zst() {
        assert_eq!(
            "tar.zst".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Zstd)
        );
    }

    #[test]
    fn from_str_tar_lz4() {
        assert_eq!(
            "tar.lz4".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Lz4)
        );
    }

    #[test]
    fn from_str_tar_br() {
        assert_eq!(
            "tar.br".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Brotli)
        );
    }

    #[test]
    fn from_str_tgz_abbreviated() {
        assert_eq!(
            "tgz".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Gzip)
        );
    }

    #[test]
    fn from_str_tbz2_abbreviated() {
        assert_eq!(
            "tbz2".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Bzip2)
        );
    }

    #[test]
    fn from_str_txz_abbreviated() {
        assert_eq!(
            "txz".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Xz)
        );
    }

    #[test]
    fn from_str_tzst_abbreviated() {
        assert_eq!(
            "tzst".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Zstd)
        );
    }

    // --- FromStr — case-insensitive ---

    #[test]
    fn from_str_case_insensitive_gzip_uppercase() {
        assert_eq!("GZIP".parse::<Format>().unwrap(), Format::codec(Codec::Gzip));
    }

    #[test]
    fn from_str_case_insensitive_tar_gz_mixed() {
        assert_eq!(
            "TAR.GZ".parse::<Format>().unwrap(),
            Format::layered(Container::Tar, Codec::Gzip)
        );
    }

    // --- FromStr — unknown name ---

    #[test]
    fn from_str_unknown_returns_err() {
        let err = "lzma".parse::<Format>().unwrap_err();
        assert!(matches!(err, crate::Error::UnknownFormat { .. }));
    }

    #[test]
    fn from_str_empty_string_returns_err() {
        let err = "".parse::<Format>().unwrap_err();
        assert!(matches!(err, crate::Error::UnknownFormat { .. }));
    }

    // --- Display ↔ FromStr roundtrip for all canonical names ---

    #[test]
    fn roundtrip_codec_only_formats() {
        let formats = [
            Format::codec(Codec::Gzip),
            Format::codec(Codec::Bzip2),
            Format::codec(Codec::Xz),
            Format::codec(Codec::Zstd),
            Format::codec(Codec::Lz4),
            Format::codec(Codec::Brotli),
        ];
        for fmt in formats {
            let displayed = fmt.to_string();
            let parsed: Format = displayed.parse().unwrap_or_else(|e| {
                panic!("roundtrip failed for {displayed:?}: {e}")
            });
            assert_eq!(parsed, fmt, "roundtrip mismatch for {displayed:?}");
        }
    }

    #[test]
    fn roundtrip_container_only_formats() {
        let formats = [
            Format::container(Container::Tar),
            Format::container(Container::Zip),
            Format::container(Container::SevenZ),
            Format::container(Container::Rar),
        ];
        for fmt in formats {
            let displayed = fmt.to_string();
            let parsed: Format = displayed.parse().unwrap_or_else(|e| {
                panic!("roundtrip failed for {displayed:?}: {e}")
            });
            assert_eq!(parsed, fmt, "roundtrip mismatch for {displayed:?}");
        }
    }

    #[test]
    fn roundtrip_layered_formats() {
        let formats = [
            Format::layered(Container::Tar, Codec::Gzip),
            Format::layered(Container::Tar, Codec::Bzip2),
            Format::layered(Container::Tar, Codec::Xz),
            Format::layered(Container::Tar, Codec::Zstd),
            Format::layered(Container::Tar, Codec::Lz4),
            Format::layered(Container::Tar, Codec::Brotli),
        ];
        for fmt in formats {
            let displayed = fmt.to_string();
            let parsed: Format = displayed.parse().unwrap_or_else(|e| {
                panic!("roundtrip failed for {displayed:?}: {e}")
            });
            assert_eq!(parsed, fmt, "roundtrip mismatch for {displayed:?}");
        }
    }
}
