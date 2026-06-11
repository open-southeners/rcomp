//! Integration tests for the codec public API.
//!
//! All tests use only the re-exported symbols from `rcomp_core` — no access
//! to internal implementation details.  This file is Unit 4's integration
//! test harness and covers:
//!
//! 1. All 6 codecs × 3 levels roundtrip of a ~100 KiB deterministic buffer.
//! 2. `detect_from_bytes` returns `Some(Format::codec(c))` for every codec
//!    except Brotli, which must return `None` (no magic bytes).
//! 3. Calling `finish()` is required: dropping a gzip encoder without it
//!    produces an incomplete/corrupt stream that the decoder rejects.

use std::io::{Read, Write};

use rcomp_core::{Codec, Format, Level, detect_from_bytes, new_decoder, new_encoder};

// ---------------------------------------------------------------------------
// Deterministic ~100 KiB payload (LCG, same parameters as test_util)
// ---------------------------------------------------------------------------

fn deterministic_100kib() -> Vec<u8> {
    let mut state: u64 = 0xCAFE_BABE_DEAD_BEEFu64;
    let n = 100 * 1024;
    let mut buf = Vec::with_capacity(n);
    for _ in 0..n {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        buf.push((state >> 56) as u8);
    }
    buf
}

// ---------------------------------------------------------------------------
// Helper: compress `data` with the given codec+level, return bytes.
// ---------------------------------------------------------------------------

fn compress(codec: Codec, level: Level, data: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::new();
    {
        let w: Box<dyn Write> = Box::new(&mut compressed);
        let mut enc = new_encoder(codec, w, level)
            .unwrap_or_else(|e| panic!("new_encoder({codec}, {level:?}) failed: {e}"));
        enc.write_all(data)
            .unwrap_or_else(|e| panic!("write_all failed: {e}"));
        Box::new(enc)
            .finish()
            .unwrap_or_else(|e| panic!("finish() failed: {e}"));
    }
    compressed
}

// ---------------------------------------------------------------------------
// Helper: decompress `data` with the given codec, return bytes.
// ---------------------------------------------------------------------------

fn decompress(codec: Codec, data: &[u8]) -> std::io::Result<Vec<u8>> {
    let r: Box<dyn Read> = Box::new(data);
    let mut dec = new_decoder(codec, r).map_err(|e| match e {
        rcomp_core::Error::Io(io) => io,
        other => std::io::Error::other(other.to_string()),
    })?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// 1. All 6 codecs × 3 levels: roundtrip of ~100 KiB buffer
// ---------------------------------------------------------------------------

macro_rules! roundtrip_tests {
    ( $( ($name:ident, $codec:expr, $level:expr) ),* $(,)? ) => {
        $(
            #[test]
            fn $name() {
                let original = deterministic_100kib();
                let compressed = compress($codec, $level, &original);
                let decompressed = decompress($codec, &compressed)
                    .unwrap_or_else(|e| panic!("decompress failed: {e}"));
                assert_eq!(
                    original, decompressed,
                    "roundtrip mismatch for {:?} at {:?}",
                    $codec, $level
                );
            }
        )*
    };
}

roundtrip_tests! {
    (roundtrip_gzip_fast,   Codec::Gzip,   Level::Fast),
    (roundtrip_gzip_best,   Codec::Gzip,   Level::Best),
    (roundtrip_gzip_edge,   Codec::Gzip,   Level::Edge),
    (roundtrip_bzip2_fast,  Codec::Bzip2,  Level::Fast),
    (roundtrip_bzip2_best,  Codec::Bzip2,  Level::Best),
    (roundtrip_bzip2_edge,  Codec::Bzip2,  Level::Edge),
    (roundtrip_xz_fast,     Codec::Xz,     Level::Fast),
    (roundtrip_xz_best,     Codec::Xz,     Level::Best),
    (roundtrip_xz_edge,     Codec::Xz,     Level::Edge),
    (roundtrip_zstd_fast,   Codec::Zstd,   Level::Fast),
    (roundtrip_zstd_best,   Codec::Zstd,   Level::Best),
    (roundtrip_zstd_edge,   Codec::Zstd,   Level::Edge),
    (roundtrip_lz4_fast,    Codec::Lz4,    Level::Fast),
    (roundtrip_lz4_best,    Codec::Lz4,    Level::Best),
    (roundtrip_lz4_edge,    Codec::Lz4,    Level::Edge),
    (roundtrip_brotli_fast, Codec::Brotli, Level::Fast),
    (roundtrip_brotli_best, Codec::Brotli, Level::Best),
    (roundtrip_brotli_edge, Codec::Brotli, Level::Edge),
}

// ---------------------------------------------------------------------------
// 2. detect_from_bytes: Some for all codecs except Brotli
// ---------------------------------------------------------------------------

macro_rules! detection_tests {
    ( $( ($name:ident, $codec:expr, $expect_some:expr) ),* $(,)? ) => {
        $(
            #[test]
            fn $name() {
                let data = b"hello world";
                let compressed = compress($codec, Level::Best, data);
                if $expect_some {
                    assert_eq!(
                        detect_from_bytes(&compressed),
                        Some(Format::codec($codec)),
                        "detect_from_bytes should return Some(Format::codec({:?}))",
                        $codec
                    );
                } else {
                    assert_eq!(
                        detect_from_bytes(&compressed),
                        None,
                        "detect_from_bytes should return None for {:?} (no magic bytes)",
                        $codec
                    );
                }
            }
        )*
    };
}

detection_tests! {
    (detect_gzip,   Codec::Gzip,   true),
    (detect_bzip2,  Codec::Bzip2,  true),
    (detect_xz,     Codec::Xz,     true),
    (detect_zstd,   Codec::Zstd,   true),
    (detect_lz4,    Codec::Lz4,    true),
    (detect_brotli, Codec::Brotli, false),
}

// ---------------------------------------------------------------------------
// 3. finish() is required: gzip encoder dropped without finish yields a
//    corrupt or truncated stream that the decoder rejects or truncates.
// ---------------------------------------------------------------------------

/// Demonstrate that dropping a gzip encoder without calling `finish()` leaves
/// the compressed stream incomplete (missing the gzip end-of-stream trailer).
///
/// The test writes data into a `Vec<u8>` via a boxed encoder, then leaks the
/// encoder with `std::mem::forget` so that `drop` is never called.  Because
/// `new_encoder` is generic over the writer lifetime, a plain mutable borrow
/// suffices — the borrow held by the encoder ends when the encoder is moved
/// into `std::mem::forget`, after which `compressed` is freely readable.
///
/// The resulting buffer lacks the gzip IEND/checksum trailer, so decoding it
/// should either return an `Err` or yield fewer bytes than the original.
#[test]
fn finish_required_gzip() {
    let original: Vec<u8> = vec![0x42u8; 4096];

    let mut compressed = Vec::new();

    // The gzip frame header is written on construction, so `compressed` will
    // contain a partial (header-only or partially-flushed) gzip stream.
    {
        let w: Box<dyn Write + '_> = Box::new(&mut compressed);
        let mut enc = new_encoder(Codec::Gzip, w, Level::Best).expect("new_encoder should succeed");
        enc.write_all(&original).expect("write_all should succeed");

        // Forget the encoder — finish() is NOT called, no Drop either.
        std::mem::forget(enc);
    }

    // At this point `compressed` holds a partial gzip stream (header + some
    // compressed blocks, but no gzip end-of-stream trailer / CRC32 / ISIZE).
    // Decoding must either return Err or produce fewer bytes than `original`.
    let r: Box<dyn Read> = Box::new(compressed.as_slice());
    let dec_result = new_decoder(Codec::Gzip, r);
    match dec_result {
        Err(_) => {
            // Decoder rejected the partial stream at construction — correct.
        }
        Ok(mut dec) => {
            let mut out = Vec::new();
            match dec.read_to_end(&mut out) {
                Err(_) => {
                    // Decoder returned an error — expected for a truncated stream.
                }
                Ok(_) => {
                    // Some decoders may return what they managed to read without
                    // returning an error (e.g. if auto-flush was on for some of
                    // the blocks).  In that case assert the output is at most the
                    // original size and is NOT equal to the full original — the
                    // stream is incomplete.
                    assert!(
                        out.len() < original.len() || out != original,
                        "decoding a gzip stream that had no finish() call should \
                         not yield the full correct original data (got {} bytes, \
                         original was {})",
                        out.len(),
                        original.len()
                    );
                }
            }
        }
    }
}
