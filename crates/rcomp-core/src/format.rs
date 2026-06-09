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

use std::fmt;

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
}
