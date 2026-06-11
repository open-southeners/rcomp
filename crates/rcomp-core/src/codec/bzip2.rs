//! Bzip2 codec backend.
//!
//! Encoding uses [`bzip2::write::BzEncoder`]; decoding uses
//! [`bzip2::read::BzDecoder`].
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 9.

use std::io::{Read, Write};

use bzip2::Compression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;

use crate::Level;

use super::Encoder;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

fn compression(level: Level) -> Compression {
    let n = match level {
        Level::Fast => 1,
        Level::Best => 6,
        Level::Edge => 9,
    };
    Compression::new(n)
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`Encoder`] for [`BzEncoder`].
struct Bzip2Encoder<'a>(BzEncoder<Box<dyn Write + 'a>>);

impl Write for Bzip2Encoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for Bzip2Encoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        self.0.finish()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a bzip2 encoder writing compressed output to `w`.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    Ok(Box::new(Bzip2Encoder(BzEncoder::new(
        w,
        compression(level),
    ))))
}

/// Create a bzip2 decoder reading compressed input from `r`.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    Ok(Box::new(BzDecoder::new(r)))
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
        test_util::roundtrip(Codec::Bzip2, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Bzip2, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Bzip2, Level::Edge);
    }

    #[test]
    fn corrupt_bzip2() {
        test_util::corrupt(Codec::Bzip2);
    }
}
