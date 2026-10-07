//! LZ4 frame codec backend.
//!
//! Encoding uses [`lz4::EncoderBuilder`] with the `.level()` setting from the
//! level table.  Levels 3 and above engage the HC (high-compression) encoder
//! inside liblz4.  Decoding uses [`lz4::Decoder`], restarted per frame so
//! concatenated frames decode in full.
//!
//! Note: [`lz4::Encoder::finish`] returns `(W, std::io::Result<()>)` — the
//! inner `Result` error is surfaced through [`super::Encoder::finish`].
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 12.

use std::io::{self, BufRead, BufReader, Read, Write};

use crate::Level;

use super::Encoder;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

fn compression_level(level: Level) -> u32 {
    match level {
        Level::Fast => 1,
        Level::Best => 6,
        Level::Edge => 12,
    }
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`Encoder`] for [`lz4::Encoder`].
///
/// The lz4 crate's `Encoder::finish` returns `(W, Result<()>)`, so we extract
/// and surface the inner `Result` through our trait.
struct Lz4Encoder<'a>(lz4::Encoder<Box<dyn Write + 'a>>);

impl Write for Lz4Encoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for Lz4Encoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        let (_writer, result) = self.0.finish();
        result.map_err(crate::Error::Io)
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create an lz4 encoder writing compressed output to `w`.
///
/// Content checksums and block checksums are enabled so that corruption is
/// detected on decompression.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let enc = lz4::EncoderBuilder::new()
        .level(compression_level(level))
        .build(w)
        .map_err(crate::Error::Io)?;
    Ok(Box::new(Lz4Encoder(enc)))
}

/// Create an lz4 decoder reading compressed input from `r`.
///
/// Decodes every concatenated frame in the input, matching the reference
/// `lz4 -d` behaviour.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    let dec = lz4::Decoder::new(BufReader::new(r)).map_err(crate::Error::Io)?;
    Ok(Box::new(MultiFrameDecoder { inner: Some(dec) }))
}

/// [`lz4::Decoder`] stops at the end of the first frame; this wrapper starts
/// a fresh decoder on the remaining input until it is exhausted.
///
/// `lz4::Decoder` only reads as many bytes as the current frame needs, so the
/// next frame's header is still unread in the [`BufReader`] when a frame ends.
struct MultiFrameDecoder<'a> {
    inner: Option<lz4::Decoder<BufReader<Box<dyn Read + 'a>>>>,
}

impl Read for MultiFrameDecoder<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let Some(dec) = self.inner.as_mut() else {
                return Ok(0);
            };
            let n = dec.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            // Current frame is done: move on to the next one, if any.
            let (mut reader, result) = self.inner.take().expect("decoder present").finish();
            result?;
            if reader.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.inner = Some(lz4::Decoder::new(reader)?);
        }
    }
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
        test_util::roundtrip(Codec::Lz4, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Lz4, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Lz4, Level::Edge);
    }

    #[test]
    fn corrupt_lz4() {
        test_util::corrupt(Codec::Lz4);
    }

    #[test]
    fn concatenated_streams() {
        test_util::concatenated(Codec::Lz4);
    }
}
