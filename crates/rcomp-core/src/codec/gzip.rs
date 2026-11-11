//! Gzip codec backend.
//!
//! # Encoding
//!
//! The encoder deflates the input in parallel and writes **one** standard
//! single-member gzip stream (RFC 1952), the same approach as `pigz`, so
//! every gzip reader handles the output:
//!
//! ```text
//! header (10 bytes)  1f 8b 08 00 | mtime 00 00 00 00 | XFL | OS ff
//! deflate segment 0  raw deflate of chunk 0, ended by a sync flush
//! deflate segment 1  raw deflate of chunk 1, ended by a sync flush
//! …
//! deflate segment n  raw deflate of the final chunk, ended by finish
//! trailer (8 bytes)  CRC-32 of the whole input | input length mod 2^32
//! ```
//!
//! The input is cut into [`CHUNK_SIZE`] chunks that are compressed
//! independently on [`super::parallel::ParallelWriter`]'s worker threads.  A
//! sync flush ends a segment with an empty stored block, so it stops on a byte
//! boundary without setting the final-block bit; the final chunk alone ends
//! with the final block.  Concatenated in order the segments form one valid
//! deflate stream.  No dictionary is carried between chunks, so a match
//! cannot reach back into the previous chunk; at 1 MiB per chunk that costs
//! roughly 0.2% of output size.
//!
//! Each worker also computes its chunk's CRC-32; the sink combines them in
//! input order on the calling thread for the trailer.
//!
//! The header matches what [`flate2::write::GzEncoder`] writes: no flags, a
//! zero modification time, XFL `4` for level 1 and `2` for level 9 (`0`
//! otherwise), and OS `255` (unknown), so output is reproducible.  Since the
//! chunking is fixed and the segments are emitted in order, the output does
//! not depend on the number of threads or on scheduling.
//!
//! # Decoding
//!
//! Decoding uses [`flate2::read::MultiGzDecoder`] to support multi-member
//! gzip files.
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 9.

use std::io::{self, Read, Write};

use flate2::read::MultiGzDecoder;
use flate2::{Compress, Compression, Crc, FlushCompress, Status};

use crate::Level;

use super::Encoder;
use super::parallel::{ParallelWriter, Sink};

/// Uncompressed bytes per independently deflated chunk.
///
/// Larger chunks lose less ratio to the missing cross-chunk dictionary;
/// smaller ones spread better over threads and use less memory.  1 MiB keeps
/// the ratio cost near 0.2% at level 6.
const CHUNK_SIZE: usize = 1 << 20;

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

/// The 10-byte gzip member header, byte for byte what `flate2`'s
/// `GzEncoder` writes for the same level.
fn header(level: Compression) -> [u8; 10] {
    let xfl = if level.level() >= Compression::best().level() {
        2
    } else if level.level() <= Compression::fast().level() {
        4
    } else {
        0
    };
    [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, xfl, 255]
}

// ---------------------------------------------------------------------------
// Per-chunk compression (worker threads)
// ---------------------------------------------------------------------------

/// One chunk's raw deflate segment and the CRC-32 of its input.
struct Segment {
    deflate: Vec<u8>,
    crc: Crc,
}

/// Raw-deflate `data` as one segment of a larger stream.
///
/// A non-final segment ends with a sync flush (byte-aligned, final-block bit
/// clear); the final one ends with finish.
fn deflate_chunk(level: Compression, data: &[u8], last: bool) -> io::Result<Segment> {
    let mut crc = Crc::new();
    crc.update(data);

    let mut z = Compress::new(level, false);
    let flush = if last {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };
    // Roomy enough for incompressible input in one call; the loop below
    // grows it if the backend still asks for more.
    let mut out = Vec::with_capacity(data.len() + data.len() / 8 + 1024);
    loop {
        if out.len() == out.capacity() {
            out.reserve(out.capacity() / 2 + 1024);
        }
        let before = (z.total_in(), z.total_out());
        let consumed = usize::try_from(z.total_in()).unwrap_or(usize::MAX);
        let input = data.get(consumed..).unwrap_or_default();
        let status = z
            .compress_vec(input, &mut out, flush)
            .map_err(io::Error::other)?;
        let all_in = usize::try_from(z.total_in()).is_ok_and(|n| n == data.len());
        let room_left = out.len() < out.capacity();
        match status {
            Status::StreamEnd => break,
            // A flush is complete once all input is in and the backend left
            // output space unused.
            Status::Ok | Status::BufError if !last && all_in && room_left => break,
            Status::Ok | Status::BufError => {
                if (z.total_in(), z.total_out()) == before && room_left {
                    return Err(io::Error::other("deflate made no progress"));
                }
            }
        }
    }
    Ok(Segment { deflate: out, crc })
}

// ---------------------------------------------------------------------------
// In-order sink (calling thread)
// ---------------------------------------------------------------------------

/// Writes segments in order and keeps the running CRC and length for the
/// trailer.
struct GzipSink {
    crc: Crc,
}

impl Sink<Segment> for GzipSink {
    fn emit(&mut self, out: &mut dyn Write, segment: Segment) -> io::Result<()> {
        out.write_all(&segment.deflate)?;
        self.crc.combine(&segment.crc);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Parallel gzip encoder implementing [`Encoder`].
struct GzipEncoder<'a>(ParallelWriter<'a, Segment, GzipSink>);

impl Write for GzipEncoder<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for GzipEncoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        let (mut out, sink) = self.0.finish()?;
        let mut trailer = [0u8; 8];
        trailer[..4].copy_from_slice(&sink.crc.sum().to_le_bytes());
        trailer[4..].copy_from_slice(&sink.crc.amount().to_le_bytes());
        out.write_all(&trailer)?;
        out.flush()?;
        Ok(())
    }
}

/// Build a gzip encoder with an explicit chunk size and thread count.
fn encoder_with<'a>(
    mut w: Box<dyn Write + 'a>,
    level: Level,
    chunk_size: usize,
    threads: usize,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let level = compression(level);
    w.write_all(&header(level))?;
    let writer = ParallelWriter::new(
        w,
        chunk_size,
        threads,
        move |_index, data: &[u8], last| deflate_chunk(level, data, last),
        GzipSink { crc: Crc::new() },
    )?;
    Ok(Box::new(GzipEncoder(writer)))
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a gzip encoder writing compressed output to `w`.
///
/// Compresses on one worker thread per available CPU; see the module docs
/// for the stream layout.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    encoder_with(w, level, CHUNK_SIZE, super::workers() as usize)
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

    #[test]
    fn concatenated_streams() {
        test_util::concatenated(Codec::Gzip);
    }

    // -----------------------------------------------------------------------
    // Parallel layout
    // -----------------------------------------------------------------------

    use std::io::{Read, Write};

    use flate2::read::GzDecoder;

    const LEVELS: [Level; 3] = [Level::Fast, Level::Best, Level::Edge];
    /// Small chunks so tests cover many segments cheaply.
    const CHUNK: usize = 4096;

    fn compress(data: &[u8], level: Level, chunk: usize, threads: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut out);
            let mut enc = super::encoder_with(w, level, chunk, threads).unwrap();
            // Uneven writes so chunk boundaries fall inside a write.
            for piece in data.chunks(1000) {
                enc.write_all(piece).unwrap();
            }
            enc.finish().unwrap();
        }
        out
    }

    /// Decode with the single-member decoder only: it stops after the first
    /// member, so getting all the input back proves there is exactly one, and
    /// nothing may follow it.
    fn decode_single_member(gz: &[u8]) -> Vec<u8> {
        let mut dec = GzDecoder::new(gz);
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();
        let mut rest = Vec::new();
        dec.into_inner().read_to_end(&mut rest).unwrap();
        assert!(
            rest.is_empty(),
            "{} bytes after the first member",
            rest.len()
        );
        out
    }

    /// Mildly compressible text-like bytes.
    fn text(len: usize) -> Vec<u8> {
        let words: [&[u8]; 6] = [
            b"alpha ",
            b"bravo ",
            b"charlie\n",
            b"delta ",
            b"echo ",
            b"fox ",
        ];
        let mut state: u32 = 12345;
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            out.extend_from_slice(words[(state >> 16) as usize % words.len()]);
        }
        out.truncate(len);
        out
    }

    /// Pseudo-random, incompressible bytes.
    fn noise(len: usize) -> Vec<u8> {
        let mut state: u64 = 0x1234_5678_9abc_def0;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect()
    }

    fn check(data: &[u8]) {
        for level in LEVELS {
            let gz = compress(data, level, CHUNK, 4);
            assert_eq!(decode_single_member(&gz), data, "level {level:?}");
            let len = gz.len();
            let crc = crc32(data);
            assert_eq!(&gz[len - 8..len - 4], &crc.to_le_bytes());
            assert_eq!(&gz[len - 4..], &(data.len() as u32).to_le_bytes());
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        crc.sum()
    }

    #[test]
    fn empty_input() {
        check(&[]);
    }

    #[test]
    fn smaller_than_one_chunk() {
        check(&text(CHUNK / 3));
    }

    #[test]
    fn exactly_one_chunk() {
        check(&text(CHUNK));
    }

    #[test]
    fn one_chunk_plus_one_byte() {
        check(&text(CHUNK + 1));
    }

    #[test]
    fn several_chunks() {
        check(&text(CHUNK * 7 + 123));
        check(&text(CHUNK * 8));
    }

    #[test]
    fn incompressible_chunks() {
        check(&noise(CHUNK * 5 + 17));
    }

    #[test]
    fn default_chunk_size_multi_chunk() {
        let data = text(super::CHUNK_SIZE * 2 + 5);
        let mut out = Vec::new();
        {
            let w: Box<dyn Write> = Box::new(&mut out);
            let mut enc = super::encoder(w, Level::Fast).unwrap();
            enc.write_all(&data).unwrap();
            enc.finish().unwrap();
        }
        assert_eq!(decode_single_member(&out), data);
    }

    #[test]
    fn header_matches_flate2() {
        for level in LEVELS {
            let mut reference =
                flate2::write::GzEncoder::new(Vec::new(), super::compression(level));
            reference.write_all(b"x").unwrap();
            let reference = reference.finish().unwrap();
            let ours = compress(b"x", level, CHUNK, 2);
            assert_eq!(ours[..10], reference[..10], "level {level:?}");
        }
    }

    #[test]
    fn output_is_deterministic() {
        let data = text(CHUNK * 9 + 99);
        for level in LEVELS {
            let first = compress(&data, level, CHUNK, 4);
            assert_eq!(compress(&data, level, CHUNK, 4), first);
            // Independent of the thread count, too.
            assert_eq!(compress(&data, level, CHUNK, 1), first);
            assert_eq!(compress(&data, level, CHUNK, 7), first);
        }
    }

    #[test]
    fn corrupted_byte_is_detected() {
        let data = text(CHUNK * 3 + 50);
        let clean = compress(&data, Level::Best, CHUNK, 4);
        // A deflate byte in the middle, and a CRC byte in the trailer.
        for at in [clean.len() / 2, clean.len() - 6] {
            let mut gz = clean.clone();
            gz[at] ^= 0x55;
            let mut out = Vec::new();
            let result = GzDecoder::new(gz.as_slice()).read_to_end(&mut out);
            assert!(result.is_err(), "corruption at byte {at} went unnoticed");
        }
    }

    /// A sync-flushed segment ends on a byte boundary with an empty stored
    /// block and no final-block bit, so inflating it alone never reaches the
    /// end of the stream; a finished segment does.
    #[test]
    fn segment_endings() {
        use flate2::{Decompress, FlushDecompress, Status};

        for level in LEVELS {
            for data in [text(CHUNK), noise(CHUNK), Vec::new()] {
                let level = super::compression(level);
                let mid = super::deflate_chunk(level, &data, false).unwrap().deflate;
                assert!(mid.ends_with(&[0, 0, 0xff, 0xff]), "{level:?}");
                let mut z = Decompress::new(false);
                let mut out = Vec::with_capacity(data.len() + 1);
                let status = z
                    .decompress_vec(&mid, &mut out, FlushDecompress::Sync)
                    .unwrap();
                assert_ne!(status, Status::StreamEnd);
                assert_eq!(out, data);

                let end = super::deflate_chunk(level, &data, true).unwrap().deflate;
                let mut z = Decompress::new(false);
                let mut out = Vec::with_capacity(data.len() + 1);
                let status = z
                    .decompress_vec(&end, &mut out, FlushDecompress::Finish)
                    .unwrap();
                assert_eq!(status, Status::StreamEnd);
                assert_eq!(out, data);
            }
        }
    }

    #[test]
    fn dropping_mid_stream_does_not_block() {
        let mut out = Vec::new();
        let w: Box<dyn Write> = Box::new(&mut out);
        let mut enc = super::encoder_with(w, Level::Edge, CHUNK, 2).unwrap();
        enc.write_all(&noise(CHUNK * 20)).unwrap();
        drop(enc);
    }
}
