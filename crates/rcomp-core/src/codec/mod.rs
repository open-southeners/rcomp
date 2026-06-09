//! Stream-codec dispatch layer.
//!
//! This module defines the [`Encoder`] trait that all codec backends implement,
//! and the [`new_encoder`] / [`new_decoder`] dispatcher functions that select
//! the right backend for a given [`crate::Codec`] variant.
//!
//! # Finishing a compressed stream
//!
//! Every compressor writes trailers (checksums, end-of-stream markers, etc.)
//! when the stream is finalised.  You **must** call [`Encoder::finish`] to
//! ensure those trailers are flushed.  Dropping an [`Encoder`] without calling
//! `finish` may truncate the output and produce a corrupt compressed file.

use std::io::{Read, Write};

use crate::{Codec, Level};

pub(crate) mod brotli;
pub(crate) mod bzip2;
pub(crate) mod gzip;
pub(crate) mod lz4;
pub(crate) mod xz;
pub(crate) mod zstd;

// ---------------------------------------------------------------------------
// Encoder trait
// ---------------------------------------------------------------------------

/// A streaming compressor that writes compressed bytes to an underlying writer.
///
/// Implementors also implement [`Write`]; callers feed uncompressed data through
/// the [`Write`] interface and then call [`finish`][Encoder::finish] to flush
/// all pending compressed output and write any required end-of-stream trailers.
///
/// # Important: always call `finish`
///
/// Dropping an [`Encoder`] without calling `finish` may leave the compressed
/// stream incomplete (missing trailers / checksums).  The resulting file will
/// typically be rejected by decompressors.  `finish` consumes the encoder by
/// `Box<Self>`, so it cannot be called twice.
pub trait Encoder: Write {
    /// Flush all buffered data, write end-of-stream trailers, and release the
    /// underlying writer.
    ///
    /// Returns `Err` if the final flush fails.  On success the encoder is
    /// consumed and cannot be used again.
    fn finish(self: Box<Self>) -> crate::Result<()>;
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Create a new streaming encoder for the given codec.
///
/// The returned encoder implements [`Write`]; feed uncompressed bytes through
/// it and then call [`Encoder::finish`] to finalise the compressed stream.
///
/// The `writer` receives compressed output.  The encoder borrows it for the
/// lifetime `'a`.
///
/// # Errors
///
/// Returns [`crate::Error::Io`] if the underlying codec library fails to
/// initialise (rare; usually indicates invalid parameters or OOM).
pub fn new_encoder<'a>(
    codec: Codec,
    writer: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    match codec {
        Codec::Gzip => gzip::encoder(writer, level),
        Codec::Bzip2 => bzip2::encoder(writer, level),
        Codec::Xz => xz::encoder(writer, level),
        Codec::Zstd => zstd::encoder(writer, level),
        Codec::Lz4 => lz4::encoder(writer, level),
        Codec::Brotli => brotli::encoder(writer, level),
    }
}

/// Create a new streaming decoder for the given codec.
///
/// The returned reader implements [`Read`]; read decompressed bytes from it.
/// The `reader` provides compressed input.  The decoder borrows it for the
/// lifetime `'a`.
///
/// # Errors
///
/// Returns [`crate::Error::Io`] if the underlying codec library fails to
/// initialise.
pub fn new_decoder<'a>(
    codec: Codec,
    reader: Box<dyn Read + 'a>,
) -> crate::Result<Box<dyn Read + 'a>> {
    match codec {
        Codec::Gzip => gzip::decoder(reader),
        Codec::Bzip2 => bzip2::decoder(reader),
        Codec::Xz => xz::decoder(reader),
        Codec::Zstd => zstd::decoder(reader),
        Codec::Lz4 => lz4::decoder(reader),
        Codec::Brotli => brotli::decoder(reader),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the number of logical CPU threads available, falling back to `1`
/// on error (e.g. unsupported platform or permission restriction).
///
/// Used by codec backends that support multi-threaded encoding (xz, zstd).
pub(crate) fn workers() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

// ---------------------------------------------------------------------------
// Test utilities (compiled only for tests, shared across codec submodules)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_util {
    use std::io::{Read, Write};

    use crate::{Codec, Format, Level, detect_from_bytes};

    use super::{new_decoder, new_encoder};

    /// A set of representative payloads for roundtrip / corruption tests.
    ///
    /// Each entry is `(label, data)`:
    ///
    /// - `"empty"` — zero bytes (edge case for every codec).
    /// - `"hello"` — the ASCII string `b"hello world"`.
    /// - `"repetitive-1mib"` — ~1 MiB of highly compressible repeated bytes.
    /// - `"lcg-256kib"` — ~256 KiB of deterministic pseudo-random bytes
    ///   generated by a simple LCG; tests codecs against incompressible input.
    pub(crate) fn corpus() -> Vec<(&'static str, Vec<u8>)> {
        // ~1 MiB repetitive pattern (1 048 576 bytes of 0x42).
        let repetitive: Vec<u8> = vec![0x42u8; 1 << 20];

        // ~256 KiB deterministic pseudo-random bytes via a linear congruential
        // generator.  Parameters: multiplier = 6364136223846793005,
        // increment = 1442695040888963407 (Knuth / Newlib LCG constants scaled
        // to u64).  No `rand` dependency — just bit mixing.
        let lcg_bytes: Vec<u8> = {
            let mut state: u64 = 0xDEAD_BEEF_CAFE_BABEu64;
            let n = 256 * 1024;
            let mut buf = Vec::with_capacity(n);
            for _ in 0..n {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                buf.push((state >> 56) as u8);
            }
            buf
        };

        vec![
            ("empty", vec![]),
            ("hello", b"hello world".to_vec()),
            ("repetitive-1mib", repetitive),
            ("lcg-256kib", lcg_bytes),
        ]
    }

    /// Roundtrip test: for every corpus entry, compress with `new_encoder` at
    /// the given level, decompress with `new_decoder`, and assert byte identity.
    ///
    /// Also asserts that [`detect_from_bytes`] on the compressed output returns:
    ///
    /// - `Some(Format::codec(codec))` for all codecs **except** Brotli.
    /// - `None` for Brotli, which has no standardised magic bytes.
    pub(crate) fn roundtrip(codec: Codec, level: Level) {
        for (label, original) in corpus() {
            // --- Compress ---
            let mut compressed = Vec::new();
            {
                let writer: Box<dyn Write> = Box::new(&mut compressed);
                let mut enc = new_encoder(codec, writer, level)
                    .unwrap_or_else(|e| panic!("new_encoder({codec}, {level:?}) failed: {e}"));
                enc.write_all(&original)
                    .unwrap_or_else(|e| panic!("[{label}] write_all failed: {e}"));
                Box::new(enc)
                    .finish()
                    .unwrap_or_else(|e| panic!("[{label}] finish() failed: {e}"));
            }

            // --- Magic-byte detection ---
            if codec == Codec::Brotli {
                assert_eq!(
                    detect_from_bytes(&compressed),
                    None,
                    "[{label}] brotli compressed output should not be detectable from bytes"
                );
            } else {
                assert_eq!(
                    detect_from_bytes(&compressed),
                    Some(Format::codec(codec)),
                    "[{label}] detect_from_bytes returned wrong format for {codec}"
                );
            }

            // --- Decompress ---
            let reader: Box<dyn Read> = Box::new(compressed.as_slice());
            let mut dec = new_decoder(codec, reader)
                .unwrap_or_else(|e| panic!("new_decoder({codec}) failed: {e}"));
            let mut decompressed = Vec::new();
            dec.read_to_end(&mut decompressed)
                .unwrap_or_else(|e| panic!("[{label}] read_to_end failed: {e}"));

            // --- Byte identity ---
            assert_eq!(
                original, decompressed,
                "[{label}] roundtrip produced different bytes for codec {codec}"
            );
        }
    }

    /// Corruption test: compress `b"hello world"` at [`Level::Best`], flip a
    /// byte in the middle of the compressed stream, then attempt decompression
    /// and assert it returns `Err` rather than panicking or silently succeeding.
    ///
    /// This guards against codecs that swallow corruption silently.
    pub(crate) fn corrupt(codec: Codec) {
        // Compress.
        let original = b"hello world";
        let mut compressed = Vec::new();
        {
            let writer: Box<dyn Write> = Box::new(&mut compressed);
            let mut enc = new_encoder(codec, writer, Level::Best)
                .unwrap_or_else(|e| panic!("new_encoder({codec}, Best) failed: {e}"));
            enc.write_all(original)
                .unwrap_or_else(|e| panic!("write_all failed: {e}"));
            Box::new(enc)
                .finish()
                .unwrap_or_else(|e| panic!("finish() failed: {e}"));
        }

        // Flip a byte near the middle of the stream, skipping the first few
        // bytes (which are often the magic header and must not be changed, or
        // the decoder would refuse to open the stream at all rather than
        // returning a checksum/framing error).
        let mid = compressed.len() / 2;
        if mid < compressed.len() {
            compressed[mid] ^= 0xFF;
        }

        // Attempt decompression — must return Err, not panic.
        let reader: Box<dyn Read> = Box::new(compressed.as_slice());
        let mut dec = match new_decoder(codec, reader) {
            Ok(d) => d,
            // If the decoder itself rejects the (now-corrupt) stream header,
            // that still counts as detecting corruption — the test passes.
            Err(_) => return,
        };
        let mut out = Vec::new();
        let result = dec.read_to_end(&mut out);
        assert!(
            result.is_err(),
            "decompressing corrupt {codec} data should return Err, got Ok({} bytes)",
            out.len()
        );
    }
}
