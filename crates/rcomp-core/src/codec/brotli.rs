//! Brotli codec backend.
//!
//! Decoding uses [`brotli::Decompressor`] for reading.
//!
//! # Encoding: one stream, compressed in parallel chunks
//!
//! Encoding cuts the input into fixed-size chunks and compresses them on a
//! pool of worker threads through [`ParallelWriter`], then stitches the
//! compressed chunks into **one** standard brotli stream on the calling
//! thread.  Any brotli decoder reads the result; nothing about it is specific
//! to rcomp.  The technique is the one the `brotli` crate's own
//! `compress_multi` uses, applied to a bounded stream instead of an in-memory
//! buffer:
//!
//! - Every chunk is compressed as a *catable* brotli stream: it starts with a
//!   tiny uncompressed meta-block, never refers to the static dictionary, and
//!   never reuses a distance from the distance cache it did not set itself, so
//!   its meaning does not depend on the bytes or state that precede it in the
//!   joined stream.  It also never marks a meta-block as the last one.
//! - A chunk may be *primed* with the previous chunk as a custom dictionary
//!   (`compress_multi` primes with all preceding input; a stream cannot keep
//!   that).  Its back-references can then reach into the previous chunk, and
//!   they stay valid in the joined stream because those bytes are exactly what
//!   the decoder has just produced there, within the stream's window.  This is
//!   what keeps the ratio close to a serial encoder at Best and Edge.
//! - [`brotli::concat::BroCatli`] joins the chunks: it writes one stream header
//!   for the level's window, strips each chunk's own header and end marker,
//!   and writes the single end-of-stream marker after the last chunk.
//!
//! An input that fits in one chunk (including an empty input) is compressed as
//! a single ordinary brotli stream instead.
//!
//! Memory stays bounded: at most `2 × threads` chunks (and their dictionaries)
//! are in flight, plus one encoder state per worker.  Each chunk is fed to its
//! encoder in pieces of at most [`FLUSH_INTERVAL`] with a flush after each,
//! which caps the encoder's per-meta-block working memory.  At Edge the encoder
//! state is still large (a 16 MiB window needs a 128 MiB match-finder tree),
//! so Edge uses at most [`EDGE_MAX_THREADS`] workers.
//!
//! Dropping the encoder without calling [`Encoder::finish`] abandons the
//! stream without blocking; workers stop after the chunk they are on.  A
//! `write` can block until the oldest chunk in flight is compressed, which at
//! Edge (an 8 MiB chunk at quality 11) takes several seconds.
//!
//! Note: Brotli has no standardised magic bytes;
//! [`crate::detect_from_bytes`] always returns `None` for brotli-compressed
//! data.
//!
//! Level mapping:
//!
//! | Level | quality | lgwin | chunk | dictionary (from previous chunk) | workers |
//! |-------|---------|-------|-------|----------------------------------|---------|
//! | Fast  | 2       | 22    | 2 MiB | none                             | all     |
//! | Best  | 6       | 22    | 2 MiB | 2 MiB                            | all     |
//! | Edge  | 11      | 24    | 8 MiB | 8 MiB                            | ≤ 4     |
//!
//! Fast takes no dictionary because the `brotli` crate's quality-2 encoder
//! panics on some inputs when one is set.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};

use brotli::concat::{BroCatli, BroCatliResult};
use brotli::enc::encode::{
    BrotliEncoderDestroyInstance, BrotliEncoderOperation, BrotliEncoderParameter,
    BrotliEncoderStateStruct,
};
use brotli::enc::{StandardAlloc, UnionHasher};

use crate::Level;

use super::Encoder;
use super::parallel::{ParallelWriter, Sink};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Internal read-side buffer size handed to `Decompressor`.
const BUFFER_SIZE: usize = 4096;

/// Size of the scratch buffers used to drain an encoder and to run the
/// stitcher.
const SCRATCH_SIZE: usize = 64 * 1024;

/// Most input fed to a chunk encoder between flushes; bounds its meta-block
/// size and with it the encoder's working memory.  Chunks smaller than four
/// intervals (only in tests) flush four times per chunk instead.
const FLUSH_INTERVAL: usize = 1 << 20;

/// Consecutive encoder calls without progress tolerated before a chunk fails
/// with an error instead of looping.
const MAX_IDLE_CALLS: u32 = 16;

/// Upper bound on Edge workers.  Each quality-11 / 16 MiB-window encoder holds
/// roughly 200–250 MiB, so this keeps Edge's peak memory near 1.2 GiB on a
/// many-core machine instead of several GiB.
const EDGE_MAX_THREADS: usize = 4;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

/// Encoder settings for one level.
#[derive(Clone, Copy, Debug)]
struct Plan {
    quality: u32,
    lgwin: u32,
    /// Bytes of input per chunk.
    chunk_size: usize,
    /// Bytes of preceding input each chunk is primed with (0 = none).
    /// Never more than `chunk_size`, so it always comes from the previous
    /// chunk alone.
    dict_size: usize,
    /// Upper bound on worker threads.
    max_threads: usize,
}

const MIB: usize = 1 << 20;

fn plan(level: Level) -> Plan {
    match level {
        Level::Fast => Plan {
            quality: 2,
            lgwin: 22,
            chunk_size: 2 * MIB,
            dict_size: 0,
            max_threads: usize::MAX,
        },
        Level::Best => Plan {
            quality: 6,
            lgwin: 22,
            chunk_size: 2 * MIB,
            // The whole previous chunk.
            dict_size: 2 * MIB,
            max_threads: usize::MAX,
        },
        Level::Edge => Plan {
            quality: 11,
            lgwin: 24,
            chunk_size: 8 * MIB,
            dict_size: 8 * MIB,
            max_threads: EDGE_MAX_THREADS,
        },
    }
}

// ---------------------------------------------------------------------------
// Chunk compression (worker threads)
// ---------------------------------------------------------------------------

/// A compressed chunk, ready for the sink.
struct Chunk {
    bytes: Vec<u8>,
    /// `true` when the chunk is a complete ordinary stream (the input fit in
    /// one chunk) rather than a catable piece for the stitcher.
    standalone: bool,
}

/// Owns a raw encoder state and releases it however the scope is left.
struct State(BrotliEncoderStateStruct<StandardAlloc>);

impl Drop for State {
    fn drop(&mut self) {
        BrotliEncoderDestroyInstance(&mut self.0);
    }
}

/// Compress `data` into one brotli stream.
///
/// With `standalone` the result is an ordinary stream.  Otherwise it is a
/// catable stream for [`BroCatli`], primed with `dict` (the input bytes that
/// immediately precede `data`) when `dict` is not empty.
fn compress_chunk(
    quality: u32,
    lgwin: u32,
    flush_interval: usize,
    data: &[u8],
    dict: &[u8],
    standalone: bool,
) -> io::Result<Vec<u8>> {
    let mut state = State(BrotliEncoderStateStruct::new(StandardAlloc::default()));
    let s = &mut state.0;
    s.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_QUALITY, quality);
    s.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_LGWIN, lgwin);
    // The whole chunk is known up front.  Its size picks the match finder (a
    // small or unknown size picks one that costs Best about 20% in ratio), so
    // it must be set before a dictionary, which creates the match finder.
    s.set_parameter(
        BrotliEncoderParameter::BROTLI_PARAM_SIZE_HINT,
        u32::try_from(data.len()).unwrap_or(u32::MAX),
    );
    if !standalone {
        // Catable implies appendable and no static-dictionary references.
        s.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_CATABLE, 1);
        s.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_MAGIC_NUMBER, 0);
        if !dict.is_empty() {
            // `true`: the dictionary is the stream's own preceding bytes, so
            // the literal context starts from its last two bytes.
            s.set_custom_dictionary_with_optional_precomputed_hasher(
                dict.len(),
                dict,
                UnionHasher::Uninit,
                true,
            );
        }
    }

    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let mut scratch = vec![0u8; SCRATCH_SIZE];
    let mut in_offset = 0usize;
    loop {
        // Feed the chunk in pieces and flush after each one.  A flush ends
        // the current meta-block, and the encoder's working memory grows with
        // meta-block size (about 24 bytes per input byte at quality 6), so
        // this caps it at the cost of a few bytes per piece.
        let piece_end = (in_offset + flush_interval).min(data.len());
        let last = piece_end == data.len();
        let op = if last {
            BrotliEncoderOperation::BROTLI_OPERATION_FINISH
        } else {
            BrotliEncoderOperation::BROTLI_OPERATION_FLUSH
        };
        let mut available_in = piece_end - in_offset;
        let mut idle_calls = 0u32;
        loop {
            let in_before = available_in;
            let mut available_out = scratch.len();
            let mut out_offset = 0usize;
            let ok = s.compress_stream(
                op,
                &mut available_in,
                data,
                &mut in_offset,
                &mut available_out,
                &mut scratch,
                &mut out_offset,
                &mut None,
                &mut |_, _, _, _| (),
            );
            if !ok {
                return Err(io::Error::other("brotli encoder rejected its input"));
            }
            out.extend_from_slice(&scratch[..out_offset]);
            let done = if last {
                s.is_finished()
            } else {
                available_in == 0 && !s.has_more_output()
            };
            if done {
                break;
            }
            // Every call should consume input or produce output.  Give up
            // rather than spin if the encoder stops making progress.
            if out_offset == 0 && available_in == in_before {
                idle_calls += 1;
                if idle_calls > MAX_IDLE_CALLS {
                    return Err(io::Error::other("brotli encoder stalled"));
                }
            } else {
                idle_calls = 0;
            }
        }
        if last {
            return Ok(out);
        }
    }
}

// ---------------------------------------------------------------------------
// Stitching (calling thread)
// ---------------------------------------------------------------------------

/// Joins catable chunks into one stream with [`BroCatli`].
struct Stitcher {
    cat: BroCatli,
    scratch: Vec<u8>,
    /// Whether any catable chunk went through `cat` (an input that fits in one
    /// chunk is written as-is and needs no end marker from `cat`).
    used: bool,
}

impl Stitcher {
    fn new(lgwin: u32) -> io::Result<Self> {
        let lgwin =
            u8::try_from(lgwin).map_err(|_| cat_error(BroCatliResult::InvalidWindowSize))?;
        Ok(Self {
            cat: BroCatli::try_new_with_window_size(lgwin).map_err(cat_error)?,
            scratch: vec![0u8; SCRATCH_SIZE],
            used: false,
        })
    }

    /// Write the end-of-stream marker.
    fn finish(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if !self.used {
            return Ok(());
        }
        loop {
            let mut written = 0usize;
            let result = self.cat.finish(&mut self.scratch, &mut written);
            out.write_all(&self.scratch[..written])?;
            match result {
                BroCatliResult::Success => return Ok(()),
                BroCatliResult::NeedsMoreOutput if written > 0 => {}
                other => return Err(cat_error(other)),
            }
        }
    }
}

impl Sink<Chunk> for Stitcher {
    fn emit(&mut self, out: &mut dyn Write, chunk: Chunk) -> io::Result<()> {
        if chunk.standalone {
            return out.write_all(&chunk.bytes);
        }
        self.used = true;
        self.cat.new_brotli_file();
        let input = chunk.bytes.as_slice();
        let mut in_offset = 0usize;
        loop {
            let before = in_offset;
            let mut written = 0usize;
            let result = self
                .cat
                .stream(input, &mut in_offset, &mut self.scratch, &mut written);
            out.write_all(&self.scratch[..written])?;
            match result {
                BroCatliResult::NeedsMoreInput | BroCatliResult::Success
                    if in_offset == input.len() =>
                {
                    return Ok(());
                }
                BroCatliResult::NeedsMoreInput
                | BroCatliResult::Success
                | BroCatliResult::NeedsMoreOutput
                    if written > 0 || in_offset > before => {}
                other => return Err(cat_error(other)),
            }
        }
    }
}

fn cat_error(result: BroCatliResult) -> io::Error {
    io::Error::other(format!("brotli chunk stitching failed: {result:?}"))
}

// ---------------------------------------------------------------------------
// Encoder wrapper
// ---------------------------------------------------------------------------

/// Dictionaries waiting for their chunk, keyed by chunk index.
type Dicts = Arc<Mutex<HashMap<u64, Vec<u8>>>>;

/// Parallel brotli encoder: a [`ParallelWriter`] plus the bookkeeping that
/// hands each chunk the tail of the chunk before it.
struct BrotliEncoder<'a> {
    inner: ParallelWriter<'a, Chunk, Stitcher>,
    chunk_size: usize,
    dict_size: usize,
    /// `None` when the level uses no dictionary.
    dicts: Option<Dicts>,
    /// Index of the chunk being filled.
    index: u64,
    /// Bytes written into the chunk being filled.
    filled: usize,
    /// The last `dict_size` bytes of the chunk being filled, collected as
    /// they pass.
    tail: Vec<u8>,
}

impl<'a> BrotliEncoder<'a> {
    fn new(
        w: Box<dyn Write + 'a>,
        plan: Plan,
        chunk_size: usize,
        threads: usize,
    ) -> io::Result<Self> {
        let dict_size = plan.dict_size.min(chunk_size);
        let dicts: Option<Dicts> = (dict_size > 0).then(Dicts::default);
        let worker_dicts = dicts.clone();
        let (quality, lgwin) = (plan.quality, plan.lgwin);
        let flush_interval = FLUSH_INTERVAL.min(chunk_size.div_ceil(4));
        let work = move |index: u64, data: &[u8], last: bool| -> io::Result<Chunk> {
            let standalone = index == 0 && last;
            // The writer stores chunk `index`'s dictionary before it submits
            // the chunk, so it is always here when wanted.  A missing one only
            // costs ratio, never correctness.
            let dict = match &worker_dicts {
                Some(map) if index > 0 => map
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&index)
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            let bytes = compress_chunk(quality, lgwin, flush_interval, data, &dict, standalone)?;
            Ok(Chunk { bytes, standalone })
        };
        let inner = ParallelWriter::new(w, chunk_size, threads, work, Stitcher::new(lgwin)?)?;
        Ok(Self {
            inner,
            chunk_size,
            dict_size,
            dicts,
            index: 0,
            filled: 0,
            tail: Vec::new(),
        })
    }
}

impl Write for BrotliEncoder<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut rest = data;
        while !rest.is_empty() {
            if self.filled == self.chunk_size {
                // More input follows a full chunk: the next chunk starts here.
                // Its dictionary must be stored before any of its bytes reach
                // the inner writer, which submits the full chunk on the next
                // byte and the next chunk only after it is full.
                if let Some(dicts) = &self.dicts {
                    let tail = std::mem::take(&mut self.tail);
                    dicts
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(self.index + 1, tail);
                }
                self.index += 1;
                self.filled = 0;
            }
            let n = (self.chunk_size - self.filled).min(rest.len());
            let (piece, after) = rest.split_at(n);
            self.inner.write_all(piece)?;
            if self.dicts.is_some() {
                // Keep the bytes of this piece that land in the chunk's last
                // `dict_size` bytes.
                let tail_start = self.chunk_size - self.dict_size;
                let skip = tail_start.saturating_sub(self.filled).min(n);
                self.tail.extend_from_slice(&piece[skip..]);
            }
            self.filled += n;
            rest = after;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Encoder for BrotliEncoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        let (mut out, mut stitcher) = self.inner.finish()?;
        stitcher.finish(&mut *out)?;
        out.flush()?;
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
    let plan = plan(level);
    encoder_with(w, level, plan.chunk_size)
}

/// [`encoder`] with an explicit chunk size, so tests can cross many chunk
/// boundaries with little data.
fn encoder_with<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
    chunk_size: usize,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let plan = plan(level);
    let threads = (super::workers() as usize).min(plan.max_threads);
    Ok(Box::new(BrotliEncoder::new(w, plan, chunk_size, threads)?))
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
    use std::io::{Read, Write};

    use super::super::test_util;
    use super::encoder_with;
    use crate::codec::{new_decoder, new_encoder};
    use crate::{Codec, Level};

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

    // --- Chunked encoding --------------------------------------------------

    const LEVELS: [Level; 3] = [Level::Fast, Level::Best, Level::Edge];

    /// Small chunks so a few KiB of input crosses many chunk boundaries (and
    /// quality 11 stays quick in a debug build).
    const CHUNK: usize = 4096;

    /// Text-like data with repeats both inside a chunk and across chunks, so
    /// back-references into the previous chunk's dictionary actually occur.
    fn text(len: usize) -> Vec<u8> {
        let words: [&[u8]; 8] = [
            b"brotli ",
            b"chunk ",
            b"window ",
            b"stream ",
            b"rcomp ",
            b"parallel ",
            b"\n",
            b"dictionary ",
        ];
        let mut state: u32 = 0x1234_5678;
        let mut out = Vec::with_capacity(len + 16);
        while out.len() < len {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            out.extend_from_slice(words[(state >> 16) as usize % words.len()]);
        }
        out.truncate(len);
        out
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect()
    }

    fn compress(data: &[u8], level: Level, chunk: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut out);
            let mut enc = encoder_with(w, level, chunk).unwrap();
            // Uneven writes so chunk boundaries fall inside writes.
            for piece in data.chunks(1000) {
                enc.write_all(piece).unwrap();
            }
            enc.finish().unwrap();
        }
        out
    }

    fn decompress(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        brotli::Decompressor::new(data, 4096)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn assert_roundtrip(data: &[u8], chunk: usize) {
        for level in LEVELS {
            let compressed = compress(data, level, chunk);
            assert_eq!(
                decompress(&compressed),
                data,
                "{level:?}, {} bytes, chunk {chunk}",
                data.len()
            );
        }
    }

    #[test]
    fn empty_input() {
        assert_roundtrip(&[], CHUNK);
    }

    #[test]
    fn smaller_than_one_chunk() {
        assert_roundtrip(&text(CHUNK / 3), CHUNK);
        assert_roundtrip(b"x", CHUNK);
    }

    #[test]
    fn exactly_one_chunk() {
        assert_roundtrip(&text(CHUNK), CHUNK);
    }

    #[test]
    fn one_chunk_plus_one_byte() {
        assert_roundtrip(&text(CHUNK + 1), CHUNK);
    }

    #[test]
    fn one_chunk_plus_two_bytes() {
        assert_roundtrip(&text(CHUNK + 2), CHUNK);
    }

    #[test]
    fn several_chunks() {
        assert_roundtrip(&text(CHUNK * 7 + 123), CHUNK);
        assert_roundtrip(&text(CHUNK * 5), CHUNK);
    }

    #[test]
    fn many_sizes_around_chunk_boundaries() {
        let data = text(3 * 256 + 3);
        for len in [255, 256, 257, 511, 512, 513, 767, 768, 769, 770, 771] {
            assert_roundtrip(&data[..len], 256);
        }
    }

    #[test]
    fn incompressible_data() {
        assert_roundtrip(&noise(CHUNK * 3 + 17), CHUNK);
    }

    #[test]
    fn mixed_compressible_and_incompressible() {
        let mut data = text(CHUNK * 2);
        data.extend(noise(CHUNK + 5));
        data.extend(text(CHUNK * 2));
        assert_roundtrip(&data, CHUNK);
    }

    #[test]
    fn output_is_identical_across_runs() {
        let data = text(CHUNK * 6 + 99);
        for level in LEVELS {
            let first = compress(&data, level, CHUNK);
            for _ in 0..3 {
                assert_eq!(compress(&data, level, CHUNK), first, "{level:?}");
            }
        }
    }

    #[test]
    fn dictionary_carries_matches_across_chunks() {
        // Two identical incompressible chunks: the second compresses to almost
        // nothing only if it can see the first through the dictionary.
        let half = noise(CHUNK);
        let mut data = half.clone();
        data.extend_from_slice(&half);
        for level in [Level::Best, Level::Edge] {
            let compressed = compress(&data, level, CHUNK);
            assert_eq!(decompress(&compressed), data);
            assert!(
                compressed.len() < CHUNK + CHUNK / 8,
                "{level:?}: {} bytes",
                compressed.len()
            );
        }
    }

    #[test]
    fn dropping_without_finish_does_not_block() {
        let mut sink = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut sink);
            let mut enc = encoder_with(w, Level::Edge, CHUNK).unwrap();
            enc.write_all(&text(CHUNK * 9)).unwrap();
            drop(enc);
        }
    }
}
