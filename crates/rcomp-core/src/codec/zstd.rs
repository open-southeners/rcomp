//! Zstandard codec backend.
//!
//! Encoding uses [`zstd::stream::write::Encoder`] with
//! [`.multithread(workers())`][zstd::stream::write::Encoder::multithread] at
//! every level (requires the `zstdmt` cargo feature, which is enabled in this
//! workspace).  [`Level::Edge`] uses level 22 and additionally enables
//! [long-distance matching][zstd::stream::write::Encoder::long_distance_matching].
//!
//! Decoding uses [`zstd::stream::read::Decoder`] with
//! [`window_log_max`][zstd::stream::read::Decoder::window_log_max] raised to
//! 31 so that LDM-encoded streams (which require a large back-reference window)
//! decompress without a memory-limit error.
//!
//! Level mapping: Fast → 1, Best → 3, Edge → 22 + LDM.

use std::io::{Read, Write};

use zstd::stream::read::Decoder as ZstdReadDecoder;
use zstd::stream::write::Encoder as ZstdWriteEncoder;

use crate::Level;

use super::{Encoder, workers};

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

fn compression_level(level: Level) -> i32 {
    match level {
        Level::Fast => 1,
        Level::Best => 3,
        Level::Edge => 22,
    }
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`Encoder`] for [`ZstdWriteEncoder`].
struct ZstdEncoder<'a>(ZstdWriteEncoder<'a, Box<dyn Write + 'a>>);

impl Write for ZstdEncoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for ZstdEncoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        self.0.finish().map(|_| ()).map_err(crate::Error::Io)
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a zstd encoder writing compressed output to `w`.
///
/// Multi-threading is enabled at every level via
/// [`.multithread(workers())`][zstd::stream::write::Encoder::multithread].
/// At [`Level::Edge`] long-distance matching is also enabled.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let mut enc = ZstdWriteEncoder::new(w, compression_level(level))
        .map_err(crate::Error::Io)?;

    enc.multithread(workers()).map_err(crate::Error::Io)?;
    // Always include a content checksum so that the decoder can detect
    // corruption in the compressed stream.
    enc.include_checksum(true).map_err(crate::Error::Io)?;

    if level == Level::Edge {
        enc.long_distance_matching(true)
            .map_err(crate::Error::Io)?;
    }

    Ok(Box::new(ZstdEncoder(enc)))
}

/// Create a zstd decoder reading compressed input from `r`.
///
/// [`window_log_max`][zstd::stream::read::Decoder::window_log_max] is set to
/// 31 (2 GiB back-reference distance) so that streams produced with
/// long-distance matching can be decoded without hitting the default memory
/// limit.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    let mut dec = ZstdReadDecoder::new(r).map_err(crate::Error::Io)?;
    // Raise the window-log ceiling so LDM-encoded streams (Edge level) can
    // be decoded.  The default cap is 27 (128 MiB); zstd level-22 + LDM may
    // produce back-references up to 2^31 bytes away.
    dec.window_log_max(31).map_err(crate::Error::Io)?;
    Ok(Box::new(dec))
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
        test_util::roundtrip(Codec::Zstd, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Zstd, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Zstd, Level::Edge);
    }

    #[test]
    fn corrupt_zstd() {
        test_util::corrupt(Codec::Zstd);
    }
}
