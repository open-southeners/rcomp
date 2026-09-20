//! Standards-compatible LZ4 Frame bridge for raw accelerator blocks.

use std::{
    io::{self, Write},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use crate::{BlockEncoderSession, Error, Result, codec::Encoder};

const MAX_BATCH_BYTES: usize = 1024 * 1024 * 1024;
const MAX_IN_FLIGHT_BATCHES: usize = 2;

pub(crate) fn encoder<'a>(
    session: Box<dyn BlockEncoderSession>,
    mut writer: Box<dyn Write + 'a>,
) -> Result<Box<dyn Encoder + 'a>> {
    let block_size = session.block_size();
    if block_size == 0 {
        return Err(Error::AccelerationUnavailable {
            reason: "accelerator LZ4 block size cannot be zero".to_owned(),
        });
    }
    let block_descriptor =
        block_descriptor(block_size).ok_or_else(|| Error::AccelerationUnavailable {
            reason: format!(
                "accelerator LZ4 block size {block_size} is not valid for an LZ4 Frame"
            ),
        })?;

    let descriptor = [0x60_u8, block_descriptor];
    writer.write_all(&0x184d_2204_u32.to_le_bytes())?;
    writer.write_all(&descriptor)?;
    writer.write_all(&[(xxhash32(&descriptor, 0) >> 8) as u8])?;

    let preferred_batch_size = session.preferred_batch_size();
    if preferred_batch_size == 0 {
        return Err(Error::AccelerationUnavailable {
            reason: "accelerator LZ4 preferred batch size cannot be zero".to_owned(),
        });
    }
    let batch_capacity = preferred_batch_size
        .min(MAX_BATCH_BYTES)
        .checked_div(block_size)
        .unwrap_or(0)
        .max(1)
        .checked_mul(block_size)
        .ok_or_else(|| Error::AccelerationUnavailable {
            reason: "accelerator LZ4 batch capacity overflowed".to_owned(),
        })?;

    let (input_sender, input_receiver) = mpsc::sync_channel::<Vec<u8>>(MAX_IN_FLIGHT_BATCHES - 1);
    let (result_sender, result_receiver) = mpsc::channel::<Result<EncodedBatch>>();
    let worker = thread::Builder::new()
        .name("rcomp-lz4-accelerator".to_owned())
        .spawn(move || {
            let mut session = session;
            while let Ok(input) = input_receiver.recv() {
                let result = session
                    .compress_blocks(&input)
                    .map(|blocks| EncodedBatch { input, blocks });
                let failed = result.is_err();
                if result_sender.send(result).is_err() || failed {
                    break;
                }
            }
        })?;

    Ok(Box::new(AcceleratedLz4Encoder {
        writer,
        block_size,
        batch_capacity,
        pending: Vec::with_capacity(batch_capacity),
        input_sender: Some(input_sender),
        result_receiver,
        worker: Some(worker),
        in_flight: 0,
    }))
}

struct EncodedBatch {
    input: Vec<u8>,
    blocks: Vec<crate::CompressedBlock>,
}

struct AcceleratedLz4Encoder<'a> {
    writer: Box<dyn Write + 'a>,
    block_size: usize,
    batch_capacity: usize,
    pending: Vec<u8>,
    input_sender: Option<SyncSender<Vec<u8>>>,
    result_receiver: Receiver<Result<EncodedBatch>>,
    worker: Option<JoinHandle<()>>,
    in_flight: usize,
}

impl AcceleratedLz4Encoder<'_> {
    fn submit_batch(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let input = std::mem::take(&mut self.pending);
        self.input_sender
            .as_ref()
            .ok_or_else(|| bridge_failure("accelerator worker is no longer available"))?
            .send(input)
            .map_err(|_| bridge_failure("accelerator worker stopped before accepting a batch"))?;
        self.in_flight += 1;

        if self.in_flight >= MAX_IN_FLIGHT_BATCHES {
            self.write_next_batch()?;
        }
        Ok(())
    }

    fn write_next_batch(&mut self) -> Result<()> {
        let mut batch = self
            .result_receiver
            .recv()
            .map_err(|_| bridge_failure("accelerator worker stopped before returning a batch"))??;
        self.in_flight = self.in_flight.saturating_sub(1);

        let expected_count = batch.input.len().div_ceil(self.block_size);
        let blocks = &batch.blocks;
        if blocks.len() != expected_count {
            return Err(Error::AccelerationFailed {
                provider: "registered provider".to_owned(),
                reason: format!(
                    "returned {} LZ4 blocks for {expected_count} input blocks",
                    blocks.len()
                ),
            });
        }

        for (index, block) in blocks.iter().enumerate() {
            let input_start = index * self.block_size;
            let input_end = (input_start + self.block_size).min(batch.input.len());
            let input = &batch.input[input_start..input_end];
            if block.input_size != input.len() {
                return Err(Error::AccelerationFailed {
                    provider: "registered provider".to_owned(),
                    reason: format!(
                        "LZ4 block {index} represents {} bytes, expected {}",
                        block.input_size,
                        input.len()
                    ),
                });
            }

            if block.bytes.len() < input.len() {
                let size =
                    u32::try_from(block.bytes.len()).map_err(|_| Error::AccelerationFailed {
                        provider: "registered provider".to_owned(),
                        reason: format!("LZ4 block {index} is too large to frame"),
                    })?;
                self.writer.write_all(&size.to_le_bytes())?;
                self.writer.write_all(&block.bytes)?;
            } else {
                let size = u32::try_from(input.len()).map_err(|_| Error::AccelerationFailed {
                    provider: "registered provider".to_owned(),
                    reason: format!("stored LZ4 block {index} is too large to frame"),
                })? | 0x8000_0000;
                self.writer.write_all(&size.to_le_bytes())?;
                self.writer.write_all(input)?;
            }
        }

        // Recycle the large host allocation once it is no longer needed for
        // stored blocks. At most two submitted inputs plus one actively filled
        // input are retained by the bounded pipeline.
        if self.pending.is_empty() {
            batch.input.clear();
            self.pending = batch.input;
        }
        Ok(())
    }

    fn drain_pipeline(&mut self) -> Result<()> {
        while self.in_flight > 0 {
            self.write_next_batch()?;
        }
        Ok(())
    }

    fn close_worker(&mut self) -> Result<()> {
        self.input_sender.take();
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            return Err(bridge_failure("accelerator worker panicked"));
        }
        Ok(())
    }
}

impl Write for AcceleratedLz4Encoder<'_> {
    fn write(&mut self, mut input: &[u8]) -> io::Result<usize> {
        let original_length = input.len();
        while !input.is_empty() {
            let available = self.batch_capacity - self.pending.len();
            let consumed = available.min(input.len());
            self.pending.extend_from_slice(&input[..consumed]);
            input = &input[consumed..];
            if self.pending.len() == self.batch_capacity {
                self.submit_batch().map_err(io::Error::other)?;
            }
        }
        Ok(original_length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.submit_batch().map_err(io::Error::other)?;
        self.drain_pipeline().map_err(io::Error::other)?;
        self.writer.flush()
    }
}

impl Encoder for AcceleratedLz4Encoder<'_> {
    fn finish(mut self: Box<Self>) -> Result<()> {
        self.submit_batch()?;
        self.input_sender.take();
        self.drain_pipeline()?;
        self.close_worker()?;
        self.writer.write_all(&0_u32.to_le_bytes())?;
        self.writer.flush()?;
        Ok(())
    }
}

fn bridge_failure(reason: &str) -> Error {
    Error::AccelerationFailed {
        provider: "registered provider".to_owned(),
        reason: reason.to_owned(),
    }
}

fn block_descriptor(block_size: usize) -> Option<u8> {
    match block_size {
        65_536 => Some(0x40),
        262_144 => Some(0x50),
        1_048_576 => Some(0x60),
        4_194_304 => Some(0x70),
        _ => None,
    }
}

fn xxhash32(input: &[u8], seed: u32) -> u32 {
    const PRIME1: u32 = 2_654_435_761;
    const PRIME2: u32 = 2_246_822_519;
    const PRIME3: u32 = 3_266_489_917;
    const PRIME4: u32 = 668_265_263;
    const PRIME5: u32 = 374_761_393;

    fn round(accumulator: u32, value: u32) -> u32 {
        const PRIME1: u32 = 2_654_435_761;
        const PRIME2: u32 = 2_246_822_519;
        accumulator
            .wrapping_add(value.wrapping_mul(PRIME2))
            .rotate_left(13)
            .wrapping_mul(PRIME1)
    }

    let mut offset = 0;
    let mut hash = if input.len() >= 16 {
        let mut lane1 = seed.wrapping_add(PRIME1).wrapping_add(PRIME2);
        let mut lane2 = seed.wrapping_add(PRIME2);
        let mut lane3 = seed;
        let mut lane4 = seed.wrapping_sub(PRIME1);
        while offset <= input.len() - 16 {
            lane1 = round(lane1, read_u32(input, offset));
            lane2 = round(lane2, read_u32(input, offset + 4));
            lane3 = round(lane3, read_u32(input, offset + 8));
            lane4 = round(lane4, read_u32(input, offset + 12));
            offset += 16;
        }
        lane1
            .rotate_left(1)
            .wrapping_add(lane2.rotate_left(7))
            .wrapping_add(lane3.rotate_left(12))
            .wrapping_add(lane4.rotate_left(18))
    } else {
        seed.wrapping_add(PRIME5)
    };

    hash = hash.wrapping_add(input.len() as u32);
    while offset + 4 <= input.len() {
        hash = hash
            .wrapping_add(read_u32(input, offset).wrapping_mul(PRIME3))
            .rotate_left(17)
            .wrapping_mul(PRIME4);
        offset += 4;
    }
    while offset < input.len() {
        hash = hash
            .wrapping_add((input[offset] as u32).wrapping_mul(PRIME5))
            .rotate_left(11)
            .wrapping_mul(PRIME1);
        offset += 1;
    }

    hash ^= hash >> 15;
    hash = hash.wrapping_mul(PRIME2);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(PRIME3);
    hash ^ (hash >> 16)
}

fn read_u32(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(input[offset..offset + 4].try_into().expect("four bytes"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::CompressedBlock;

    struct StoredSession;

    impl BlockEncoderSession for StoredSession {
        fn block_size(&self) -> usize {
            65_536
        }

        fn compress_blocks(&mut self, input: &[u8]) -> Result<Vec<CompressedBlock>> {
            Ok(input
                .chunks(self.block_size())
                .map(|chunk| CompressedBlock {
                    input_size: chunk.len(),
                    bytes: vec![0; chunk.len()],
                })
                .collect())
        }
    }

    struct RecordingSession {
        calls: Arc<Mutex<Vec<usize>>>,
    }

    impl BlockEncoderSession for RecordingSession {
        fn block_size(&self) -> usize {
            65_536
        }

        fn preferred_batch_size(&self) -> usize {
            2 * self.block_size()
        }

        fn compress_blocks(&mut self, input: &[u8]) -> Result<Vec<CompressedBlock>> {
            self.calls.lock().unwrap().push(input.len());
            Ok(input
                .chunks(self.block_size())
                .map(|chunk| CompressedBlock {
                    input_size: chunk.len(),
                    bytes: vec![0; chunk.len()],
                })
                .collect())
        }
    }

    #[test]
    fn valid_lz4_block_sizes_have_descriptors() {
        assert_eq!(block_descriptor(65_536), Some(0x40));
        assert_eq!(block_descriptor(262_144), Some(0x50));
        assert_eq!(block_descriptor(1_048_576), Some(0x60));
        assert_eq!(block_descriptor(4_194_304), Some(0x70));
        assert_eq!(block_descriptor(1_000), None);
    }

    #[test]
    fn bridge_writes_a_decodable_stored_block_frame() {
        let mut output = Vec::new();
        {
            let mut encoder = encoder(Box::new(StoredSession), Box::new(&mut output)).unwrap();
            encoder.write_all(b"accelerated bridge").unwrap();
            encoder.finish().unwrap();
        }

        let mut decoder = lz4::Decoder::new(output.as_slice()).unwrap();
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
        assert_eq!(decoded, b"accelerated bridge");
    }

    #[test]
    fn bridge_honors_provider_batch_size_and_preserves_the_tail() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let session = RecordingSession {
            calls: Arc::clone(&calls),
        };
        let input = vec![b'x'; 5 * 65_536 + 17];
        let mut output = Vec::new();
        {
            let mut encoder = encoder(Box::new(session), Box::new(&mut output)).unwrap();
            encoder.write_all(&input).unwrap();
            encoder.finish().unwrap();
        }

        assert_eq!(*calls.lock().unwrap(), [131_072, 131_072, 65_553]);
        let mut decoder = lz4::Decoder::new(output.as_slice()).unwrap();
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
        assert_eq!(decoded, input);
    }
}
