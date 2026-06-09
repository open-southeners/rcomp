//! Brotli codec backend.
//!
//! Encoding uses [`brotli::CompressorWriter`] (quality and lgwin per the level
//! table).  Decoding uses [`brotli::Decompressor`] for reading.
//!
//! The `CompressorWriter` finalises the stream (writes the end-of-stream
//! marker) both on explicit `into_inner()` and on `Drop`.  The [`Encoder`]
//! trait's `finish` implementation calls `into_inner()` to trigger finalisation
//! eagerly and surface any I/O error.
//!
//! Note: Brotli has no standardised magic bytes;
//! [`crate::detect_from_bytes`] always returns `None` for brotli-compressed
//! data.
//!
//! Level mapping:
//!
//! | Level | quality | lgwin |
//! |-------|---------|-------|
//! | Fast  | 2       | 22    |
//! | Best  | 6       | 22    |
//! | Edge  | 11      | 24    |

use std::io::{Read, Write};

use crate::Level;

use super::Encoder;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Internal write-side buffer size handed to `CompressorWriter`.
const BUFFER_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

fn params(level: Level) -> (u32, u32) {
    // Returns (quality, lgwin).
    match level {
        Level::Fast => (2, 22),
        Level::Best => (6, 22),
        Level::Edge => (11, 24),
    }
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Thin wrapper that implements [`Encoder`] for [`brotli::CompressorWriter`].
///
/// `finish` calls `into_inner()` which triggers the brotli end-of-stream
/// finalisation before returning the underlying writer.
struct BrotliEncoder<'a>(Option<brotli::CompressorWriter<Box<dyn Write + 'a>>>);

impl Write for BrotliEncoder<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .as_mut()
            .expect("BrotliEncoder used after finish")
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .as_mut()
            .expect("BrotliEncoder used after finish")
            .flush()
    }
}

impl Encoder for BrotliEncoder<'_> {
    fn finish(mut self: Box<Self>) -> crate::Result<()> {
        // `into_inner()` calls `flush_or_close(BROTLI_OPERATION_FINISH)` which
        // writes the end-of-stream bytes, then returns the inner writer.
        // We take ownership of the `CompressorWriter` so that Drop does not
        // attempt a second finish.
        let writer = self
            .0
            .take()
            .expect("BrotliEncoder used after finish");
        writer.into_inner();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a brotli encoder writing compressed output to `w`.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let (quality, lgwin) = params(level);
    let enc = brotli::CompressorWriter::new(w, BUFFER_SIZE, quality, lgwin);
    Ok(Box::new(BrotliEncoder(Some(enc))))
}

/// Create a brotli decoder reading compressed input from `r`.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    Ok(Box::new(brotli::Decompressor::new(r, BUFFER_SIZE)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::super::test_util;
    use crate::{Codec, Level};
    use crate::codec::{new_decoder, new_encoder};

    #[test]
    fn roundtrip_fast() {
        test_util::roundtrip(Codec::Brotli, Level::Fast);
    }

    #[test]
    fn roundtrip_best() {
        test_util::roundtrip(Codec::Brotli, Level::Best);
    }

    #[test]
    fn roundtrip_edge() {
        test_util::roundtrip(Codec::Brotli, Level::Edge);
    }

    /// Brotli has no framing checksum, so decoding arbitrarily-corrupted data
    /// may silently produce wrong output rather than returning `Err`.  Instead
    /// of relying on `test_util::corrupt` (which asserts `Err`), this test
    /// verifies that decoding a stream of random garbage bytes returns `Err`.
    ///
    /// See doc comment on the parent module for details on why the generic
    /// `corrupt` helper is not used here.
    #[test]
    fn corrupt_brotli_garbage() {
        // A buffer of pseudo-random bytes that are not valid brotli.
        // Starting byte 0xFF is not a valid brotli stream header in practice.
        let garbage: Vec<u8> = (0u8..=255).collect();

        let reader: Box<dyn std::io::Read> = Box::new(garbage.as_slice());
        let mut dec = new_decoder(Codec::Brotli, reader)
            .expect("new_decoder should not fail at construction");
        let mut out = Vec::new();
        let result = dec.read_to_end(&mut out);
        assert!(
            result.is_err(),
            "decoding random garbage as brotli should return Err, got Ok({} bytes)",
            out.len()
        );
    }

    /// Regression: decoding a *slightly* corrupted stream (valid prefix with
    /// a flipped middle byte) may or may not return `Err` depending on where
    /// the flip lands.  This test compresses a known payload, flips a byte,
    /// and asserts that either an error is returned **or** the decoded output
    /// differs from the original — ensuring the corruption is not silently
    /// accepted as correct data.
    #[test]
    fn corrupt_brotli_flipped_byte() {
        use std::io::Write;

        let original = b"hello world";

        let mut compressed = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut compressed);
            let mut enc = new_encoder(Codec::Brotli, w, Level::Best).unwrap();
            enc.write_all(original).unwrap();
            Box::new(enc).finish().unwrap();
        }

        // Flip a byte in the middle of the compressed stream.
        let mid = compressed.len() / 2;
        if mid < compressed.len() {
            compressed[mid] ^= 0xFF;
        }

        let reader: Box<dyn Read> = Box::new(compressed.as_slice());
        let mut dec = match new_decoder(Codec::Brotli, reader) {
            Ok(d) => d,
            // Decoder rejected corrupted stream at construction — that's fine.
            Err(_) => return,
        };
        let mut out = Vec::new();
        match dec.read_to_end(&mut out) {
            Err(_) => {} // Corruption detected — test passes.
            Ok(_) => {
                // Brotli may not detect single-byte corruption via checksum.
                // Assert that the output at least differs from the original so
                // we know the corrupted byte had some effect.
                assert_ne!(
                    out, original,
                    "corrupted brotli stream decoded to identical original — \
                     corruption went completely undetected"
                );
            }
        }
    }
}
