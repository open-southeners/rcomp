//! XZ (LZMA2) codec backend.
//!
//! Encoding uses [`liblzma::write::XzEncoder`]; decoding uses
//! [`liblzma::read::XzDecoder`].
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 9 + [`PRESET_EXTREME`].
//!
//! When more than one logical CPU is available, the encoder is initialised via
//! [`liblzma::stream::MtStreamBuilder`] (enabled by the `parallel` feature of
//! the `liblzma` workspace dependency) and uses [`super::workers()`] threads.
//! On single-core hosts it falls back to the standard single-threaded
//! [`XzEncoder::new`].
//!
//! [`PRESET_EXTREME`]: liblzma::stream::PRESET_EXTREME

use std::io::{Read, Write};

use liblzma::read::XzDecoder;
use liblzma::stream::{Check, MtStreamBuilder, PRESET_EXTREME};
use liblzma::write::XzEncoder;

use crate::Level;

use super::Encoder;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

/// Returns the xz preset value for the given level, with `PRESET_EXTREME`
/// OR-ed in at [`Level::Edge`].
fn preset(level: Level) -> u32 {
    match level {
        Level::Fast => 1,
        Level::Best => 6,
        Level::Edge => 9 | PRESET_EXTREME,
    }
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`Encoder`] for [`XzEncoder`].
struct XzWriteEncoder<'a>(XzEncoder<Box<dyn Write + 'a>>);

impl Write for XzWriteEncoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for XzWriteEncoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        self.0.finish()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create an xz encoder writing compressed output to `w`.
///
/// When [`super::workers()`] returns a value greater than `1`, encoding is
/// performed in parallel using [`MtStreamBuilder`] with that many threads and
/// a [`Check::Crc64`] integrity check.  On single-core hosts the standard
/// single-threaded [`XzEncoder::new`] is used instead.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let p = preset(level);
    let inner = if super::workers() > 1 {
        let stream = MtStreamBuilder::new()
            .preset(p)
            .threads(super::workers())
            .check(Check::Crc64)
            .encoder()
            .map_err(std::io::Error::from)?;
        XzEncoder::new_stream(w, stream)
    } else {
        XzEncoder::new(w, p)
    };
    Ok(Box::new(XzWriteEncoder(inner)))
}

/// Create an xz decoder reading compressed input from `r`.
///
/// Uses [`XzDecoder`] which auto-detects the stream format (xz or lzma).
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    Ok(Box::new(XzDecoder::new(r)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::test_util;
    use crate::{Codec, Level};

    #[test]
    fn roundtrip_fast() {
        test_util::roundtrip(Codec::Xz, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Xz, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Xz, Level::Edge);
    }

    #[test]
    fn corrupt_xz() {
        test_util::corrupt(Codec::Xz);
    }
}
