//! LZ4 frame codec backend.
//!
//! # Encoding
//!
//! The encoder writes one standard LZ4 Frame whose blocks are compressed in
//! parallel.  Input is cut into 4 MiB chunks; each chunk becomes one
//! *independent* LZ4 block, compressed on a worker thread by the block API of
//! liblz4 ([`lz4::block::compress_to_buffer`]) and written in input order by
//! [`super::parallel::ParallelWriter`].  Frame layout:
//!
//! | Field              | Bytes | Value                                              |
//! |--------------------|-------|----------------------------------------------------|
//! | magic              | 4     | `0x184D2204`, little-endian                        |
//! | FLG                | 1     | `0x74`: version 01, independent blocks, block checksums, content checksum |
//! | BD                 | 1     | `0x70`: 4 MiB maximum block size                   |
//! | header checksum    | 1     | `(XXH32(FLG, BD) >> 8) & 0xFF`                     |
//! | data blocks        | …     | per block: 4-byte size, data, 4-byte XXH32 of data |
//! | end mark           | 4     | `0x00000000`                                       |
//! | content checksum   | 4     | XXH32 of the whole uncompressed input              |
//!
//! A block that compression does not shrink is stored uncompressed, with the
//! high bit of its size field set.  Every block except the last holds exactly
//! 4 MiB of input; empty input produces a frame with no data blocks.  Block
//! and content checksums are always written, so corruption is detected on
//! decompression.  Content size is not recorded in the header.
//!
//! Because blocks are independent, matches never reach back into the previous
//! block; with 4 MiB blocks this costs well under 0.1 % of output size against
//! linked blocks, and lets every block be compressed on its own core.  The
//! content checksum is sequential, so workers take turns feeding it in chunk
//! order (see [`ContentHash`]) while the rest of their work runs in parallel.
//! The
//! output is deterministic: it does not depend on the number of threads or on
//! how the input is split across [`Write::write`] calls.
//!
//! Level mapping (the same strength the LZ4 frame API uses at these levels):
//! Fast → level 1 (`LZ4_compress_fast`, acceleration 1), Best → HC level 6,
//! Edge → HC level 12.
//!
//! # Decoding
//!
//! Decoding uses [`lz4::Decoder`], restarted per frame so concatenated frames
//! decode in full.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Condvar, Mutex};

use lz4::block::CompressionMode;

use crate::Level;

use super::Encoder;
use super::lz4_accelerated::{Xxh32, xxhash32};
use super::parallel::{ParallelWriter, Sink};

// ---------------------------------------------------------------------------
// Frame constants
// ---------------------------------------------------------------------------

/// LZ4 Frame magic number.
const MAGIC: u32 = 0x184D_2204;

/// FLG byte: version `01`, block independence, block checksums and content
/// checksum on; no content size, no dictionary.
const FLG: u8 = 0b0111_0100;

/// BD byte: maximum block size 4 MiB (block maximum size id 7).
const BD: u8 = 7 << 4;

/// Bytes of input per block; the maximum BD declares.
const BLOCK_SIZE: usize = 4 * 1024 * 1024;

/// High bit of a block size field: the block is stored uncompressed.
const STORED_FLAG: u32 = 0x8000_0000;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

/// The block-API mode matching what the LZ4 frame API does at our levels.
///
/// The frame API compresses levels below 3 with `LZ4_compress_fast` at
/// acceleration 1 (level 1 included) and levels 3 and above with
/// `LZ4_compress_HC` at that level.
fn compression_mode(level: Level) -> CompressionMode {
    match level {
        Level::Fast => CompressionMode::FAST(1),
        Level::Best => CompressionMode::HIGHCOMPRESSION(6),
        Level::Edge => CompressionMode::HIGHCOMPRESSION(12),
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Writes framed blocks, already complete with size field and checksum, in
/// order.
struct BlockSink;

impl Sink<Vec<u8>> for BlockSink {
    fn emit(&mut self, out: &mut dyn Write, block: Vec<u8>) -> io::Result<()> {
        out.write_all(&block)
    }
}

/// The frame's content checksum, fed by the workers one chunk at a time in
/// chunk order.
///
/// XXH32 is sequential and cannot be combined across chunks, but hashing on
/// the calling thread would cost about as much as compressing at the Fast
/// level on every core.  Instead, each worker hashes its chunk before
/// compressing it, waiting until the previous chunk has been hashed.  The
/// hashing is still serial, but it overlaps with compression on the other
/// workers and with reading input on the calling thread.
///
/// This cannot deadlock because [`ParallelWriter`] hands chunks to workers
/// from a single FIFO queue in chunk order, and a worker always hashes the
/// chunk it took before doing anything else: whenever chunk `i` waits, every
/// earlier chunk is already held by a running worker that is either hashing
/// or waiting on a chunk earlier still.
struct ContentHash {
    state: Mutex<OrderedHash>,
    turn: Condvar,
}

struct OrderedHash {
    /// Index of the chunk whose turn it is to be hashed.
    next: u64,
    hasher: Xxh32,
}

impl ContentHash {
    fn new() -> Self {
        Self {
            state: Mutex::new(OrderedHash {
                next: 0,
                hasher: Xxh32::new(0),
            }),
            turn: Condvar::new(),
        }
    }

    /// Hash chunk `index`, after waiting for every earlier chunk.
    fn update(&self, index: u64, data: &[u8]) -> io::Result<()> {
        let mut state = self.state.lock().map_err(|_| hash_failed())?;
        while state.next != index {
            // A worker that panicked while hashing poisons the lock, so the
            // workers waiting behind it fail instead of waiting forever.
            state = self.turn.wait(state).map_err(|_| hash_failed())?;
        }
        state.hasher.update(data);
        state.next += 1;
        drop(state);
        self.turn.notify_all();
        Ok(())
    }

    /// The digest of every chunk hashed so far.
    fn digest(&self) -> io::Result<u32> {
        let state = self.state.lock().map_err(|_| hash_failed())?;
        Ok(state.hasher.digest())
    }
}

fn hash_failed() -> io::Error {
    io::Error::other("LZ4 content checksum worker failed")
}

/// Parallel LZ4 Frame encoder.
struct Lz4Encoder<'a> {
    blocks: ParallelWriter<'a, Vec<u8>, BlockSink>,
    content: Arc<ContentHash>,
}

impl Write for Lz4Encoder<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.blocks.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.blocks.flush()
    }
}

impl Encoder for Lz4Encoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        let Self { blocks, content } = *self;
        // Every chunk has been through a worker, and so hashed, once
        // `finish` returns.
        let (mut out, BlockSink) = blocks.finish()?;
        out.write_all(&0_u32.to_le_bytes())?;
        out.write_all(&content.digest()?.to_le_bytes())?;
        out.flush()?;
        Ok(())
    }
}

/// Compress one chunk into a complete frame data block: size field, block
/// data (compressed, or the input itself when compression does not shrink
/// it), and the XXH32 of the block data.  An empty chunk (empty input)
/// produces no block at all, since a zero size field is the end mark.
fn frame_block(data: &[u8], mode: CompressionMode) -> io::Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let bound = lz4::block::compress_bound(data.len())?;
    // Room for the size field, the worst-case compressed data (which is
    // larger than the stored form) and the checksum, so nothing reallocates.
    let mut block = vec![0_u8; 4 + bound + 4];
    let compressed =
        lz4::block::compress_to_buffer(data, Some(mode), false, &mut block[4..4 + bound])?;
    let size = if compressed < data.len() {
        block.truncate(4 + compressed);
        compressed as u32
    } else {
        block.truncate(4);
        block.extend_from_slice(data);
        data.len() as u32 | STORED_FLAG
    };
    block[..4].copy_from_slice(&size.to_le_bytes());
    let checksum = xxhash32(&block[4..], 0);
    block.extend_from_slice(&checksum.to_le_bytes());
    Ok(block)
}

/// Create an lz4 encoder writing compressed output to `w`.
///
/// Compresses 4 MiB blocks on every available core and writes them as one
/// LZ4 Frame.  Content checksums and block checksums are enabled so that
/// corruption is detected on decompression.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    encoder_with(w, level, BLOCK_SIZE, super::workers() as usize)
}

/// [`encoder`] with an explicit chunk size (at most 4 MiB, the declared
/// maximum) and thread count, so tests can produce many blocks cheaply.
fn encoder_with<'a>(
    mut w: Box<dyn Write + 'a>,
    level: Level,
    block_size: usize,
    threads: usize,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    assert!(
        block_size > 0 && block_size <= BLOCK_SIZE,
        "LZ4 block size must be between 1 byte and the declared 4 MiB maximum"
    );
    let descriptor = [FLG, BD];
    w.write_all(&MAGIC.to_le_bytes())?;
    w.write_all(&descriptor)?;
    w.write_all(&[(xxhash32(&descriptor, 0) >> 8) as u8])?;

    let mode = compression_mode(level);
    let content = Arc::new(ContentHash::new());
    let worker_content = Arc::clone(&content);
    let blocks = ParallelWriter::new(
        w,
        block_size,
        threads,
        move |index, data: &[u8], _last| {
            worker_content.update(index, data)?;
            frame_block(data, mode)
        },
        BlockSink,
    )?;
    Ok(Box::new(Lz4Encoder { blocks, content }))
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

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
    use std::io::{Read, Write};

    use super::super::test_util;
    use super::*;
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

    // --- Parallel frame encoder -------------------------------------------

    const CHUNK: usize = 1024;

    /// Compress `input` with `CHUNK`-byte blocks, feeding it in uneven pieces.
    fn compress(input: &[u8], level: Level, threads: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = encoder_with(Box::new(&mut out), level, CHUNK, threads).unwrap();
            for piece in input.chunks(333) {
                enc.write_all(piece).unwrap();
            }
            enc.finish().unwrap();
        }
        out
    }

    fn decompress(frame: &[u8]) -> io::Result<Vec<u8>> {
        let mut dec = decoder(Box::new(frame)).map_err(io::Error::other)?;
        let mut out = Vec::new();
        dec.read_to_end(&mut out)?;
        Ok(out)
    }

    /// A data block as found in a frame: offset of its size field, payload
    /// length, and whether it is stored uncompressed.
    #[derive(Debug)]
    struct BlockInfo {
        offset: usize,
        len: usize,
        stored: bool,
    }

    /// Walk a frame independently of liblz4, checking the header, every
    /// block checksum and the content checksum against `input`.
    fn parse_frame(frame: &[u8], input: &[u8]) -> Vec<BlockInfo> {
        let u32_at = |at: usize| u32::from_le_bytes(frame[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(0), MAGIC);
        assert_eq!(frame[4], 0x74, "FLG");
        assert_eq!(frame[5], 0x70, "BD");
        assert_eq!(frame[6], (xxhash32(&frame[4..6], 0) >> 8) as u8);
        let mut at = 7;
        let mut blocks = Vec::new();
        loop {
            let field = u32_at(at);
            if field == 0 {
                at += 4;
                break;
            }
            let len = (field & !STORED_FLAG) as usize;
            assert!(len <= CHUNK + 16, "block payload too large");
            let data = &frame[at + 4..at + 4 + len];
            assert_eq!(u32_at(at + 4 + len), xxhash32(data, 0), "block checksum");
            blocks.push(BlockInfo {
                offset: at,
                len,
                stored: field & STORED_FLAG != 0,
            });
            at += 4 + len + 4;
        }
        assert_eq!(u32_at(at), xxhash32(input, 0), "content checksum");
        assert_eq!(at + 4, frame.len(), "trailing bytes after the frame");
        blocks
    }

    fn text(len: usize) -> Vec<u8> {
        b"the quick brown fox jumps over the lazy dog; "
            .iter()
            .copied()
            .cycle()
            .enumerate()
            .map(|(i, b)| {
                if i % 97 == 0 {
                    b'0' + (i / 97 % 10) as u8
                } else {
                    b
                }
            })
            .take(len)
            .collect()
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678_9ABC_DEF0_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    fn check(input: &[u8], expected_blocks: usize) -> Vec<BlockInfo> {
        let mut blocks = Vec::new();
        for level in [Level::Fast, Level::Best, Level::Edge] {
            let frame = compress(input, level, 4);
            blocks = parse_frame(&frame, input);
            assert_eq!(blocks.len(), expected_blocks, "{level:?}");
            assert_eq!(decompress(&frame).unwrap(), input, "{level:?}");
        }
        blocks
    }

    #[test]
    fn empty_input_is_a_frame_without_blocks() {
        let frame = compress(&[], Level::Fast, 2);
        // Header (7) + end mark (4) + content checksum (4).
        assert_eq!(frame.len(), 15);
        check(&[], 0);
    }

    #[test]
    fn input_smaller_than_one_block() {
        let blocks = check(&text(CHUNK / 2), 1);
        assert!(!blocks[0].stored);
    }

    #[test]
    fn input_of_exactly_one_block() {
        check(&text(CHUNK), 1);
    }

    #[test]
    fn one_block_plus_one_byte() {
        let blocks = check(&text(CHUNK + 1), 2);
        // A single byte cannot shrink, so the tail block is stored.
        assert!(blocks[1].stored);
        assert_eq!(blocks[1].len, 1);
    }

    #[test]
    fn several_blocks() {
        let input = text(CHUNK * 37 + 123);
        let blocks = check(&input, 38);
        assert!(blocks.iter().all(|b| !b.stored));
    }

    #[test]
    fn incompressible_input_is_stored() {
        let input = noise(CHUNK * 5 + 17);
        let blocks = check(&input, 6);
        assert!(blocks.iter().all(|b| b.stored));
        assert_eq!(blocks[0].len, CHUNK);
        assert_eq!(blocks[5].len, 17);
    }

    #[test]
    fn corruption_in_a_later_block_is_detected() {
        for input in [text(CHUNK * 6), noise(CHUNK * 6)] {
            let frame = compress(&input, Level::Fast, 4);
            let blocks = parse_frame(&frame, &input);
            let target = &blocks[4];
            let mut corrupted = frame.clone();
            corrupted[target.offset + 4 + target.len / 2] ^= 0x01;
            assert!(
                decompress(&corrupted).is_err(),
                "corruption in block 4 was not detected"
            );
        }
    }

    #[test]
    fn output_is_deterministic_across_runs_and_thread_counts() {
        let mut input = text(CHUNK * 20);
        input.extend(noise(CHUNK * 3 + 5));
        for level in [Level::Fast, Level::Best, Level::Edge] {
            let reference = compress(&input, level, 1);
            for threads in [1, 2, 8, 8] {
                assert_eq!(compress(&input, level, threads), reference, "{level:?}");
            }
        }
    }

    #[test]
    fn higher_levels_compress_harder() {
        let input = text(CHUNK * 16);
        let fast = compress(&input, Level::Fast, 4).len();
        let best = compress(&input, Level::Best, 4).len();
        let edge = compress(&input, Level::Edge, 4).len();
        assert!(best < fast, "best {best} >= fast {fast}");
        assert!(edge <= best, "edge {edge} > best {best}");
    }

    #[test]
    fn level_mapping_matches_the_frame_api() {
        assert!(matches!(
            compression_mode(Level::Fast),
            CompressionMode::FAST(1)
        ));
        assert!(matches!(
            compression_mode(Level::Best),
            CompressionMode::HIGHCOMPRESSION(6)
        ));
        assert!(matches!(
            compression_mode(Level::Edge),
            CompressionMode::HIGHCOMPRESSION(12)
        ));
    }

    #[test]
    fn real_block_size_frame_roundtrips() {
        // Spans two 4 MiB blocks through the public constructor.
        let input = text(BLOCK_SIZE + 4096);
        let mut frame = Vec::new();
        {
            let mut enc = encoder(Box::new(&mut frame), Level::Fast).unwrap();
            enc.write_all(&input).unwrap();
            enc.finish().unwrap();
        }
        assert_eq!(decompress(&frame).unwrap(), input);
    }

    #[test]
    fn dropping_without_finish_does_not_hang() {
        let input = text(CHUNK * 64);
        let mut out = Vec::new();
        let mut enc = encoder_with(Box::new(&mut out), Level::Edge, CHUNK, 4).unwrap();
        enc.write_all(&input).unwrap();
        drop(enc);
    }
}
