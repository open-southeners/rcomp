//! Gzip codec backend.
//!
//! Encoding uses [`flate2::write::GzEncoder`]; decoding uses
//! [`flate2::read::MultiGzDecoder`] to support multi-member gzip files.
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 9.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;

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

/// Thin wrapper that implements [`Encoder`] for [`GzEncoder`].
struct GzipEncoder<'a>(GzEncoder<Box<dyn Write + 'a>>);

impl Write for GzipEncoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for GzipEncoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        self.0.finish()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a gzip encoder writing compressed output to `w`.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    Ok(Box::new(GzipEncoder(GzEncoder::new(w, compression(level)))))
}

/// Create a gzip decoder reading compressed input from `r`.
///
/// Uses [`MultiGzDecoder`] so that multi-member gzip streams (e.g. files
/// produced by concatenating two `.gz` files) are decoded transparently.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    Ok(Box::new(MultiGzDecoder::new(r)))
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
        test_util::roundtrip(Codec::Gzip, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Gzip, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Gzip, Level::Edge);
    }

    #[test]
    fn corrupt_gzip() {
        test_util::corrupt(Codec::Gzip);
    }

    /// Verify that a stream built by concatenating two independent gzip members
    /// decompresses correctly end-to-end (multi-member support via
    /// `MultiGzDecoder`).
    #[test]
    fn multi_member_decode() {
        use std::io::{Read, Write};

        use crate::codec::{new_decoder, new_encoder};

        let part1 = b"hello ";
        let part2 = b"world";

        // Build two separate gzip members and concatenate their bytes.
        let mut member1 = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut member1);
            let mut enc = new_encoder(Codec::Gzip, w, Level::Best).unwrap();
            enc.write_all(part1).unwrap();
            Box::new(enc).finish().unwrap();
        }

        let mut member2 = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut member2);
            let mut enc = new_encoder(Codec::Gzip, w, Level::Best).unwrap();
            enc.write_all(part2).unwrap();
            Box::new(enc).finish().unwrap();
        }

        // Concatenate the two members into one stream.
        let mut combined = member1;
        combined.extend_from_slice(&member2);

        // Decode — MultiGzDecoder must yield both members.
        let r: Box<dyn Read> = Box::new(combined.as_slice());
        let mut dec = new_decoder(Codec::Gzip, r).unwrap();
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();

        assert_eq!(out, b"hello world");
    }
}
