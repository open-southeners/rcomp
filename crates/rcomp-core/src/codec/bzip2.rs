//! Bzip2 codec backend.
//!
//! # Encoding: one stream, compressed in parallel
//!
//! The encoder writes a single standard bzip2 stream (`BZh` header, blocks,
//! end-of-stream marker with the combined CRC), so every bzip2 reader decodes
//! it, including minimal ones that stop after the first stream.  The blocks
//! are compressed in parallel and spliced together, the way `lbzip2` does it:
//!
//! 1. The input is cut into chunks of [`BLOCKS_PER_CHUNK`] blocks' worth of
//!    bytes, and the chunks are compressed on worker threads by
//!    [`ParallelWriter`].
//! 2. A worker splits its chunk into pieces at exactly the points where
//!    libbzip2 itself would close a block (it replays libbzip2's first
//!    run-length stage and block-size check), so that each piece compressed
//!    on its own with [`BzEncoder`] yields a stream with **exactly one
//!    block**.  Within a chunk the blocks are therefore identical to the
//!    ones a serial encoder makes.
//! 3. In such a one-block stream every boundary sits at a known place: the
//!    block starts right after the 4-byte header, its CRC is the 4 bytes
//!    after the 6-byte block magic, and the stream ends with the 48-bit
//!    end-of-stream magic, a 32-bit combined CRC equal to that block CRC, and
//!    0–7 zero bits of padding.  The worker checks all of this (and that the
//!    block CRC matches the piece's own CRC) and reports an error instead of
//!    guessing if anything differs; nothing is found by scanning compressed
//!    data for magic numbers.
//! 4. On the calling thread the blocks are bit-shifted into place one after
//!    another, the combined CRC is folded from the block CRCs
//!    (`combined = rotl(combined, 1) ^ block_crc`), and one end-of-stream
//!    marker closes the stream.
//!
//! Output is deterministic and independent of the thread count.  Blocks only
//! differ from a serial encoder's where a chunk boundary does not coincide
//! with a serial block boundary, which only happens when the first
//! run-length stage shrank or grew the data (runs of 4 or more equal bytes).
//!
//! Decoding uses [`bzip2::read::MultiBzDecoder`], so concatenated streams (as
//! written by `pbzip2`) decode in full.
//!
//! Level mapping: Fast → 1, Best → 6, Edge → 9.

use std::io::{self, Read, Write};

use bzip2::Compression;
use bzip2::read::MultiBzDecoder;
use bzip2::write::BzEncoder;

use crate::Level;

use super::Encoder;
use super::parallel::{ParallelWriter, Sink};

// ---------------------------------------------------------------------------
// Format constants
// ---------------------------------------------------------------------------

/// The 48-bit magic that starts every compressed block.
const BLOCK_MAGIC: u64 = 0x3141_5926_5359;

/// The 48-bit magic that starts the end-of-stream marker.
const EOS_MAGIC: u64 = 0x1772_4538_5090;

/// Length of the `BZh<level>` stream header, in bytes.
const HEADER_LEN: usize = 4;

/// Length of the end-of-stream marker (magic plus combined CRC), in bits.
const EOS_BITS: usize = 80;

/// Number of full blocks of input per parallel chunk.
const BLOCKS_PER_CHUNK: usize = 1;

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

/// The bzip2 block-size level (1–9) used for `level`.
fn level_number(level: Level) -> u32 {
    match level {
        Level::Fast => 1,
        Level::Best => 6,
        Level::Edge => 9,
    }
}

/// libbzip2's `nblockMAX`: a block is closed once its run-length-encoded
/// contents reach this many bytes.
fn block_capacity(level: u32) -> usize {
    level as usize * 100_000 - 19
}

/// Default chunk size for `level`: [`BLOCKS_PER_CHUNK`] blocks of input.
///
/// For input without runs of 4 or more equal bytes the first run-length stage
/// is the identity, so a block holds exactly [`block_capacity`] input bytes
/// and these chunks line up with the serial encoder's blocks.
fn default_chunk_size(level: u32) -> usize {
    block_capacity(level) * BLOCKS_PER_CHUNK
}

// ---------------------------------------------------------------------------
// Block planning
// ---------------------------------------------------------------------------

/// Bytes a run of `len` equal bytes takes after bzip2's first run-length
/// stage: runs of 1–3 are kept, runs of 4–255 become 4 bytes plus a count.
fn rle1_len(len: u32) -> usize {
    if len < 4 { len as usize } else { 5 }
}

/// Exclusive end offsets of the pieces `data` must be cut into so that each
/// piece, compressed on its own, yields exactly one block.
///
/// This replays libbzip2's `copy_input_until_stop`/`ADD_CHAR_TO_BLOCK`: runs
/// are capped at 255 bytes and only count towards the block once they end,
/// and before each byte is added the block is closed if it already holds
/// `capacity` bytes, leaving the unfinished run for the next block.  Cutting
/// at the start of that run therefore gives pieces whose blocks are the
/// serial encoder's blocks, and within a piece the "block full" check never
/// fires, whatever way the input is fed to the encoder.
fn block_ends(data: &[u8], capacity: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut run_byte: Option<u8> = None;
    let mut run_len = 0u32;
    let mut run_start = 0usize;
    let mut filled = 0usize;
    for (i, &byte) in data.iter().enumerate() {
        if filled >= capacity {
            ends.push(run_start);
            filled = 0;
        }
        if run_byte == Some(byte) && run_len < 255 {
            run_len += 1;
        } else {
            if run_byte.is_some() {
                filled += rle1_len(run_len);
            }
            run_byte = Some(byte);
            run_len = 1;
            run_start = i;
        }
    }
    // The encoder also checks for a full block once all input is consumed.
    if filled >= capacity {
        ends.push(run_start);
    }
    if !data.is_empty() {
        ends.push(data.len());
    }
    ends
}

// ---------------------------------------------------------------------------
// CRC
// ---------------------------------------------------------------------------

/// bzip2's CRC-32 table (polynomial 0x04C11DB7, most significant bit first).
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// The block CRC bzip2 stores for a block holding `data`.
fn block_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) ^ u32::from(byte)) as usize];
    }
    !crc
}

// ---------------------------------------------------------------------------
// Compressing one block
// ---------------------------------------------------------------------------

/// One compressed block, still inside the one-block stream it came from.
struct Block {
    /// The whole one-block stream; the block's bits start at byte
    /// [`HEADER_LEN`].
    stream: Vec<u8>,
    /// Number of bits in the block (from the block magic up to, not
    /// including, the end-of-stream magic).
    bits: usize,
    /// The block's CRC.
    crc: u32,
}

fn unexpected(what: &str) -> io::Error {
    io::Error::other(format!(
        "bzip2 encoder produced an unexpected stream ({what})"
    ))
}

/// Compress `piece`, which [`block_ends`] guarantees fits one block, and
/// locate that block inside the resulting stream.
fn compress_block(piece: &[u8], level: u32) -> io::Result<Block> {
    let mut encoder = BzEncoder::new(
        Vec::with_capacity(piece.len() / 2 + 64),
        Compression::new(level),
    );
    encoder.write_all(piece)?;
    let stream = encoder.finish()?;
    let crc = block_crc(piece);
    let bits = locate_block(&stream, level, crc)?;
    Ok(Block { stream, bits, crc })
}

/// Check that `stream` is a bzip2 stream at `level` holding exactly one block
/// with CRC `crc`, and return the block's length in bits.
///
/// Everything is read from fixed positions: the header and block magic at the
/// start, and the end-of-stream marker in the last 11 bytes.  The marker is
/// followed by 0–7 zero padding bits; since its 80 bits (magic plus a combined
/// CRC that must equal `crc`) are fully known, each padding length is tried
/// and exactly one must match.  Two could only match if the 48-bit magic
/// repeated itself at a shift of 1–7 bits, which it does not (see the
/// `eos_magic_is_not_periodic` test); the check is kept anyway.
fn locate_block(stream: &[u8], level: u32, crc: u32) -> io::Result<usize> {
    // Header (4) + block magic and CRC (10) + at least one byte of block
    // body + end-of-stream marker (10).
    if stream.len() < HEADER_LEN + 10 + 1 + 10 {
        return Err(unexpected("too short"));
    }
    let digit = b'0' + level as u8;
    if stream[..HEADER_LEN] != [b'B', b'Z', b'h', digit] {
        return Err(unexpected("bad header"));
    }
    let magic = BLOCK_MAGIC.to_be_bytes();
    if stream[HEADER_LEN..HEADER_LEN + 6] != magic[2..] {
        return Err(unexpected("bad block magic"));
    }
    let stored_crc = u32::from_be_bytes(
        stream[HEADER_LEN + 6..HEADER_LEN + 10]
            .try_into()
            .map_err(|_| unexpected("short block header"))?,
    );
    if stored_crc != crc {
        return Err(unexpected("block CRC mismatch, not a single block"));
    }

    // The last 11 bytes (88 bits) hold the 80-bit marker plus 0–7 padding bits.
    let tail = stream[stream.len() - 11..]
        .iter()
        .fold(0u128, |acc, &b| (acc << 8) | u128::from(b));
    // For a single block the combined CRC is rotl(0, 1) ^ crc = crc.
    let marker = (u128::from(EOS_MAGIC) << 32) | u128::from(crc);
    let marker_mask = (1u128 << EOS_BITS) - 1;
    let mut padding = None;
    for pad in 0..8 {
        let pad_is_zero = tail & ((1u128 << pad) - 1) == 0;
        if pad_is_zero && (tail >> pad) & marker_mask == marker {
            if padding.is_some() {
                return Err(unexpected("ambiguous end-of-stream marker"));
            }
            padding = Some(pad);
        }
    }
    let padding = padding.ok_or_else(|| unexpected("end-of-stream marker not found"))?;
    let end = stream.len() * 8 - padding - EOS_BITS;
    let start = HEADER_LEN * 8;
    // At least the block magic, CRC and some body must remain.
    if end <= start + 80 {
        return Err(unexpected("empty block"));
    }
    Ok(end - start)
}

/// Compress one chunk into its blocks (none for the empty input).
fn compress_chunk(chunk: &[u8], level: u32) -> io::Result<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut start = 0;
    for end in block_ends(chunk, block_capacity(level)) {
        blocks.push(compress_block(&chunk[start..end], level)?);
        start = end;
    }
    Ok(blocks)
}

// ---------------------------------------------------------------------------
// Splicing blocks into one stream
// ---------------------------------------------------------------------------

/// Appends blocks to the output stream at arbitrary bit alignment and keeps
/// the combined CRC.  Runs on the thread that owns the output writer.
struct Splicer {
    combined_crc: u32,
    /// Bits not yet written, in the top `pending_bits` bits.
    pending: u8,
    /// 0–7.
    pending_bits: usize,
    /// Whole bytes ready for the output writer.
    scratch: Vec<u8>,
}

impl Splicer {
    fn new() -> Self {
        Self {
            combined_crc: 0,
            pending: 0,
            pending_bits: 0,
            scratch: Vec::new(),
        }
    }

    /// Append the first `bits` bits of `src` (most significant bit first).
    fn push_bits(&mut self, src: &[u8], bits: usize) {
        let whole = bits / 8;
        let rest = bits % 8;
        let shift = self.pending_bits;
        if shift == 0 {
            self.scratch.extend_from_slice(&src[..whole]);
        } else {
            self.scratch.reserve(whole);
            let mut carry = self.pending;
            for &byte in &src[..whole] {
                self.scratch.push(carry | (byte >> shift));
                carry = byte << (8 - shift);
            }
            self.pending = carry;
        }
        if rest > 0 {
            let byte = src[whole] & !(0xFFu8 >> rest);
            let total = shift + rest;
            let joined = self.pending | (byte >> shift);
            if total >= 8 {
                self.scratch.push(joined);
                // `shift` > 0 here, since `rest` < 8.
                self.pending = byte << (8 - shift);
                self.pending_bits = total - 8;
            } else {
                self.pending = joined;
                self.pending_bits = total;
            }
        }
    }

    fn push_block(&mut self, block: &Block) {
        self.combined_crc = self.combined_crc.rotate_left(1) ^ block.crc;
        self.push_bits(&block.stream[HEADER_LEN..], block.bits);
    }

    fn drain(&mut self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(&self.scratch)?;
        self.scratch.clear();
        Ok(())
    }

    /// Write the end-of-stream marker and the padding.
    fn finish(mut self, out: &mut dyn Write) -> io::Result<()> {
        let mut marker = [0u8; 10];
        marker[..6].copy_from_slice(&EOS_MAGIC.to_be_bytes()[2..]);
        marker[6..].copy_from_slice(&self.combined_crc.to_be_bytes());
        self.push_bits(&marker, EOS_BITS);
        if self.pending_bits > 0 {
            self.scratch.push(self.pending);
        }
        self.drain(out)
    }
}

impl Sink<Vec<Block>> for Splicer {
    fn emit(&mut self, out: &mut dyn Write, blocks: Vec<Block>) -> io::Result<()> {
        for block in &blocks {
            self.push_block(block);
        }
        self.drain(out)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Parallel bzip2 encoder writing one stream; see the module docs.
struct Bzip2Encoder<'a>(ParallelWriter<'a, Vec<Block>, Splicer>);

impl Write for Bzip2Encoder<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Encoder for Bzip2Encoder<'_> {
    fn finish(self: Box<Self>) -> crate::Result<()> {
        let (mut out, splicer) = self.0.finish()?;
        splicer.finish(&mut *out)?;
        out.flush()?;
        Ok(())
    }
}

/// Build the encoder with an explicit chunk size and thread count.
fn encoder_with<'a>(
    mut w: Box<dyn Write + 'a>,
    level: Level,
    chunk_size: usize,
    threads: usize,
) -> crate::Result<Bzip2Encoder<'a>> {
    let n = level_number(level);
    w.write_all(&[b'B', b'Z', b'h', b'0' + n as u8])?;
    let writer = ParallelWriter::new(
        w,
        chunk_size,
        threads,
        move |_index, chunk: &[u8], _last| compress_chunk(chunk, n),
        Splicer::new(),
    )?;
    Ok(Bzip2Encoder(writer))
}

// ---------------------------------------------------------------------------
// Public(crate) entry points
// ---------------------------------------------------------------------------

/// Create a bzip2 encoder writing compressed output to `w`.
pub(crate) fn encoder<'a>(
    w: Box<dyn Write + 'a>,
    level: Level,
) -> crate::Result<Box<dyn Encoder + 'a>> {
    let chunk_size = default_chunk_size(level_number(level));
    Ok(Box::new(encoder_with(
        w,
        level,
        chunk_size,
        super::workers() as usize,
    )?))
}

/// Create a bzip2 decoder reading compressed input from `r`.
pub(crate) fn decoder<'a>(r: Box<dyn Read + 'a>) -> crate::Result<Box<dyn Read + 'a>> {
    Ok(Box::new(MultiBzDecoder::new(r)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use bzip2::Compression;
    use bzip2::write::BzEncoder;

    use super::super::test_util;
    use super::{
        EOS_MAGIC, Encoder, block_capacity, block_crc, block_ends, default_chunk_size,
        encoder_with, level_number,
    };
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

    #[test]
    fn concatenated_streams() {
        test_util::concatenated(Codec::Bzip2);
    }

    // --- helpers ----------------------------------------------------------

    /// Compress with an explicit chunk size and thread count, feeding the
    /// input in uneven pieces.
    fn compress(data: &[u8], level: Level, chunk: usize, threads: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = encoder_with(Box::new(&mut out), level, chunk, threads).unwrap();
            for piece in data.chunks(77_777) {
                enc.write_all(piece).unwrap();
            }
            Box::new(enc).finish().unwrap();
        }
        out
    }

    /// What the serial `bzip2` encoder makes of `data`.
    fn serial(data: &[u8], level: Level) -> Vec<u8> {
        let mut enc = BzEncoder::new(Vec::new(), Compression::new(level_number(level)));
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    /// Decode with the single-stream decoder and require that it consumes
    /// every byte: the output must be exactly one stream.
    fn decode_single_stream(compressed: &[u8]) -> Vec<u8> {
        let mut decoder = bzip2::bufread::BzDecoder::new(compressed);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap();
        assert!(
            decoder.into_inner().is_empty(),
            "bytes left after the first stream"
        );
        out
    }

    fn check(data: &[u8], level: Level, chunk: usize, threads: usize) -> Vec<u8> {
        let compressed = compress(data, level, chunk, threads);
        assert_eq!(decode_single_stream(&compressed), data);
        let mut multi = Vec::new();
        bzip2::read::MultiBzDecoder::new(&compressed[..])
            .read_to_end(&mut multi)
            .unwrap();
        assert_eq!(multi, data);
        compressed
    }

    /// Deterministic pseudo-random bytes.
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// Text-like data without long runs.
    fn text(len: usize) -> Vec<u8> {
        let words = [
            "alpha ", "beta ", "gamma ", "delta\n", "epsilon ", "zeta ", "eta, ",
        ];
        let rand = noise(len, 7);
        let mut out = Vec::with_capacity(len + 8);
        let mut i = 0;
        while out.len() < len {
            out.extend_from_slice(words[rand[i] as usize % words.len()].as_bytes());
            i += 1;
        }
        out.truncate(len);
        out
    }

    // --- format facts the splicing relies on ------------------------------

    #[test]
    fn eos_magic_is_not_periodic() {
        // Two padding lengths can only both match the end-of-stream marker
        // if its 48-bit magic equals itself shifted by 1–7 bits.
        for shift in 1..8 {
            let overlap = 48 - shift;
            let mask = (1u64 << overlap) - 1;
            assert_ne!(EOS_MAGIC >> shift & mask, EOS_MAGIC & mask, "shift {shift}");
        }
    }

    #[test]
    fn crc_matches_bzip2() {
        // A one-block stream stores the block CRC at bytes 10..14.
        for data in [&b"hello world"[..], &noise(5000, 3)] {
            let stream = serial(data, Level::Fast);
            let stored = u32::from_be_bytes(stream[10..14].try_into().unwrap());
            assert_eq!(block_crc(data), stored);
        }
    }

    // --- required cases -----------------------------------------------------

    #[test]
    fn empty_input_matches_serial() {
        for level in [Level::Fast, Level::Best, Level::Edge] {
            let compressed = check(&[], level, 1000, 4);
            assert_eq!(compressed, serial(&[], level));
        }
    }

    #[test]
    fn smaller_than_one_chunk() {
        let data = text(500);
        let compressed = check(&data, Level::Best, 1000, 4);
        // A single chunk holding a single block is exactly the serial output.
        assert_eq!(compressed, serial(&data, Level::Best));
    }

    #[test]
    fn exactly_one_chunk() {
        let data = text(1000);
        check(&data, Level::Fast, 1000, 4);
    }

    #[test]
    fn one_chunk_plus_one_byte() {
        let data = text(1001);
        check(&data, Level::Fast, 1000, 4);
    }

    #[test]
    fn several_chunks() {
        let data = text(50_000);
        check(&data, Level::Edge, 1234, 4);
        check(&data, Level::Fast, 1234, 1);
    }

    #[test]
    fn highly_repetitive_data() {
        // Long runs (shrink in the run-length stage), runs of exactly four
        // (grow by a quarter, so a chunk spills into a second block) and runs
        // longer than 255.
        let long_runs = vec![0u8; 1_000_000];
        let mut fours = Vec::new();
        while fours.len() < 400_000 {
            fours.extend_from_slice(b"aaaabbbbccccd");
        }
        let mut mixed = Vec::new();
        for i in 0..2000u32 {
            mixed.extend(std::iter::repeat_n((i % 7) as u8, (i % 600) as usize));
        }
        for data in [&long_runs, &fours, &mixed] {
            for level in [Level::Fast, Level::Best] {
                let chunk = default_chunk_size(level_number(level));
                check(data, level, chunk, 4);
                check(data, level, 4096, 4);
            }
        }
        // Runs of four expand: a capacity-sized chunk needs two blocks.
        let capacity = block_capacity(1);
        assert_eq!(block_ends(&fours[..capacity], capacity).len(), 2);
    }

    #[test]
    fn incompressible_data() {
        let data = noise(300_000, 11);
        check(&data, Level::Fast, default_chunk_size(1), 4);
        check(&data, Level::Edge, 10_000, 4);
    }

    #[test]
    fn corrupted_byte_is_detected() {
        let data = text(300_000);
        let mut compressed = compress(&data, Level::Fast, 50_000, 4);
        let mid = compressed.len() / 2;
        compressed[mid] ^= 0x55;
        let mut out = Vec::new();
        let result = bzip2::read::MultiBzDecoder::new(&compressed[..]).read_to_end(&mut out);
        assert!(result.is_err() || out != data, "corruption went unnoticed");
    }

    #[test]
    fn output_is_deterministic_and_thread_independent() {
        let mut data = text(400_000);
        data.extend(vec![9u8; 50_000]);
        data.extend(noise(100_000, 5));
        let first = compress(&data, Level::Fast, 30_000, 4);
        for threads in [1, 2, 4, 8] {
            assert_eq!(compress(&data, Level::Fast, 30_000, threads), first);
        }
    }

    #[test]
    fn one_chunk_reproduces_serial_blocks_exactly() {
        // With a chunk holding all the input, the planned block boundaries
        // must be libbzip2's own, so the output equals the serial stream.
        let mut data = Vec::new();
        for i in 0..40_000u32 {
            let len = [1usize, 2, 3, 4, 5, 255, 256, 300][(i % 8) as usize];
            data.extend(std::iter::repeat_n((i % 5) as u8, len));
            data.extend_from_slice(&i.to_le_bytes());
        }
        data.extend(noise(250_000, 9));
        for level in [Level::Fast, Level::Best] {
            let compressed = check(&data, level, data.len(), 4);
            assert_eq!(compressed, serial(&data, level), "{level:?}");
        }
    }

    #[test]
    fn default_chunks_reproduce_serial_output_without_runs() {
        // Without runs of four or more, chunks of one block line up with the
        // serial encoder's blocks, so the whole stream is identical.
        let data = text(350_000);
        assert!(data.windows(4).all(|w| w.iter().any(|&b| b != w[0])));
        let compressed = check(&data, Level::Fast, default_chunk_size(1), 4);
        assert_eq!(compressed, serial(&data, Level::Fast));
    }

    #[test]
    fn randomized_contents_and_alignments() {
        // Varying chunk contents and sizes puts every block at many different
        // bit alignments in the output.
        let mut seed = 0x1234_5678_9ABC_DEF1u64;
        for round in 0..60 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let len = (seed >> 40) as usize % 60_000;
            let chunk = 1 + (seed >> 20) as usize % 7_000;
            let mut data = noise(len, seed);
            // Make some rounds compressible and run-heavy.
            match round % 3 {
                0 => data.iter_mut().for_each(|b| *b %= 3),
                1 => data
                    .iter_mut()
                    .for_each(|b| *b = if *b < 200 { 0 } else { *b }),
                _ => {}
            }
            let level = [Level::Fast, Level::Best, Level::Edge][round % 3];
            check(&data, level, chunk, 3);
        }
    }

    #[test]
    fn dropping_without_finish_does_not_block() {
        let data = text(200_000);
        let mut out = Vec::new();
        let mut enc = encoder_with(Box::new(&mut out), Level::Fast, 10_000, 4).unwrap();
        enc.write_all(&data).unwrap();
        drop(enc);
    }
}
