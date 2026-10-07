use std::{
    ffi::c_void,
    mem,
    ptr::NonNull,
    sync::{LazyLock, Mutex},
};

use dispatch2::DispatchData;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::ns_string;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLComputeCommandEncoder, MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLResourceOptions, MTLSize,
};
use rcomp_core::CompressedBlock;

use super::device_info;
use crate::{MetalError, MetalLz4Report, Result};

const KERNEL_NAME: &str = "rcomp_lz4_compress_blocks";
const BLOCK_SIZE: usize = 64 * 1024;
const HASH_LOG: usize = 12;
const HASH_SIZE: usize = 1 << HASH_LOG;
const OUTPUT_ERROR: u32 = u32::MAX;

#[derive(Clone, Copy)]
#[repr(C)]
struct Lz4Parameters {
    total_size: u32,
    block_size: u32,
    output_slot_size: u32,
    block_count: u32,
}

pub(crate) fn compress_lz4_frame(input: &[u8]) -> Result<MetalLz4Report> {
    if input.is_empty() {
        let device = MTLCreateSystemDefaultDevice().ok_or(MetalError::NoDevice)?;
        let info = device_info(&device);
        let frame = assemble_frame(input, BLOCK_SIZE, &[])?;
        return Ok(MetalLz4Report {
            device: info,
            output_size: frame.len(),
            frame,
            input_size: 0,
            block_count: 0,
            compressed_block_count: 0,
            stored_block_count: 0,
        });
    }

    let mut encoder = MetalLz4BlockEncoder::new()?;
    let info = encoder.device_info.clone();
    let blocks = encoder.compress_blocks(input)?;
    let compressed_block_count = blocks
        .iter()
        .filter(|block| block.bytes.len() < block.input_size)
        .count();
    let frame = assemble_frame(input, BLOCK_SIZE, &blocks)?;

    Ok(MetalLz4Report {
        device: info,
        output_size: frame.len(),
        frame,
        input_size: input.len(),
        block_count: blocks.len(),
        compressed_block_count,
        stored_block_count: blocks.len() - compressed_block_count,
    })
}

pub(crate) struct MetalLz4BlockEncoder {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    buffers: Option<MetalLz4Buffers>,
    pub(super) device_info: crate::MetalDeviceInfo,
}

#[derive(Clone)]
struct MetalLz4Context {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    device_info: crate::MetalDeviceInfo,
}

static SHARED_CONTEXT: LazyLock<Mutex<Option<MetalLz4Context>>> =
    LazyLock::new(|| Mutex::new(None));

type MetalBuffer = Retained<ProtocolObject<dyn MTLBuffer>>;

struct MetalLz4Buffers {
    input: MetalBuffer,
    input_capacity: usize,
    output: MetalBuffer,
    output_capacity: usize,
    sizes: MetalBuffer,
    sizes_capacity: usize,
    scratch: MetalBuffer,
    scratch_capacity: usize,
}

// SAFETY: The buffers are exclusively owned by one encoder session, every
// operation requires `&mut self`, and `compress_blocks` waits for its command
// buffer to complete before returning or allowing the session to move again.
// No CPU pointer into a buffer escapes the call. Moving the retained Metal
// resources between threads therefore cannot introduce concurrent host/GPU or
// host/host access.
unsafe impl Send for MetalLz4Buffers {}

impl MetalLz4BlockEncoder {
    pub(crate) fn new() -> Result<Self> {
        let context = shared_context()?;
        let queue = context
            .device
            .newCommandQueue()
            .ok_or(MetalError::Creation("an LZ4 command queue"))?;

        Ok(Self {
            device: context.device,
            pipeline: context.pipeline,
            queue,
            buffers: None,
            device_info: context.device_info,
        })
    }

    pub(crate) fn preferred_batch_size(&self) -> usize {
        preferred_batch_size_for(self.device_info.recommended_working_set_size)
    }

    fn ensure_buffers(
        &mut self,
        input_capacity: usize,
        output_capacity: usize,
        sizes_capacity: usize,
        scratch_capacity: usize,
    ) -> Result<()> {
        let reusable = self.buffers.as_ref().is_some_and(|buffers| {
            buffers.input_capacity >= input_capacity
                && buffers.output_capacity >= output_capacity
                && buffers.sizes_capacity >= sizes_capacity
                && buffers.scratch_capacity >= scratch_capacity
        });
        if reusable {
            return Ok(());
        }

        self.buffers = Some(MetalLz4Buffers {
            input: shared_buffer(&self.device, input_capacity, "an LZ4 input buffer")?,
            input_capacity,
            output: shared_buffer(&self.device, output_capacity, "an LZ4 output buffer")?,
            output_capacity,
            sizes: shared_buffer(&self.device, sizes_capacity, "an LZ4 size buffer")?,
            sizes_capacity,
            scratch: shared_buffer(&self.device, scratch_capacity, "an LZ4 scratch buffer")?,
            scratch_capacity,
        });
        Ok(())
    }

    pub(crate) fn compress_blocks(&mut self, input: &[u8]) -> Result<Vec<CompressedBlock>> {
        if input.is_empty() {
            return Ok(Vec::new());
        }

        let total_size = u32::try_from(input.len()).map_err(|_| MetalError::InputTooLarge)?;
        let block_count = input.len().div_ceil(BLOCK_SIZE);
        let output_slot_size = lz4_compress_bound(BLOCK_SIZE)?;
        let output_capacity = checked_mul(block_count, output_slot_size)?;
        let sizes_capacity = checked_mul(block_count, mem::size_of::<u32>())?;
        let scratch_capacity =
            checked_mul(checked_mul(block_count, HASH_SIZE)?, mem::size_of::<u32>())?;

        for length in [
            input.len(),
            output_capacity,
            sizes_capacity,
            scratch_capacity,
        ] {
            if length > self.device.maxBufferLength() {
                return Err(MetalError::InputTooLarge);
            }
        }

        let parameters = Lz4Parameters {
            total_size,
            block_size: BLOCK_SIZE as u32,
            output_slot_size: u32::try_from(output_slot_size)
                .map_err(|_| MetalError::InputTooLarge)?,
            block_count: u32::try_from(block_count).map_err(|_| MetalError::InputTooLarge)?,
        };

        self.ensure_buffers(
            input.len(),
            output_capacity,
            sizes_capacity,
            scratch_capacity,
        )?;
        let buffers = self
            .buffers
            .as_ref()
            .expect("Metal buffers were initialized above");

        // SAFETY: The Metal input buffer owns `input.len()` writable bytes and no
        // GPU work references it until after the copy. The source and destination
        // allocations are distinct and exactly the requested length is copied.
        unsafe {
            std::ptr::copy_nonoverlapping(
                input.as_ptr(),
                buffers.input.contents().as_ptr().cast::<u8>(),
                input.len(),
            );
        }

        let command_buffer = self
            .queue
            .commandBuffer()
            .ok_or(MetalError::Creation("an LZ4 command buffer"))?;
        let encoder = command_buffer
            .computeCommandEncoder()
            .ok_or(MetalError::Creation("an LZ4 compute command encoder"))?;
        encoder.setComputePipelineState(&self.pipeline);

        // SAFETY: Buffer indices and element types exactly match `lz4.metal`.
        // Every capacity is checked above, offsets are zero, and all retained
        // buffers outlive synchronous command completion. The CPU does not access
        // the buffers between commit and `waitUntilCompleted`.
        unsafe {
            encoder.setBuffer_offset_atIndex(Some(&buffers.input), 0, 0);
            encoder.setBuffer_offset_atIndex(Some(&buffers.output), 0, 1);
            encoder.setBuffer_offset_atIndex(Some(&buffers.sizes), 0, 2);
            encoder.setBuffer_offset_atIndex(Some(&buffers.scratch), 0, 3);
            encoder.setBytes_length_atIndex(
                NonNull::from(&parameters).cast::<c_void>(),
                mem::size_of::<Lz4Parameters>(),
                4,
            );
        }

        let execution_width = self.pipeline.threadExecutionWidth().max(1);
        let threads_per_group = execution_width
            .min(self.pipeline.maxTotalThreadsPerThreadgroup())
            .min(block_count);
        encoder.dispatchThreads_threadsPerThreadgroup(
            MTLSize {
                width: block_count,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: threads_per_group,
                height: 1,
                depth: 1,
            },
        );
        encoder.endEncoding();
        command_buffer.commit();
        command_buffer.waitUntilCompleted();

        if command_buffer.status() != MTLCommandBufferStatus::Completed {
            let detail = command_buffer.error().map_or_else(
                || "unknown Metal command-buffer error".to_owned(),
                |error| error.to_string(),
            );
            return Err(MetalError::CommandExecution(detail));
        }

        let mut output_sizes = vec![0_u32; block_count];
        // SAFETY: Successful synchronous completion establishes that the GPU no
        // longer accesses these shared buffers. The size destination has the
        // exact initialized capacity copied from its Metal allocation, and the
        // source and destination allocations do not overlap. Payload bytes are
        // copied below only after their reported ranges have been validated.
        unsafe {
            std::ptr::copy_nonoverlapping(
                buffers.sizes.contents().as_ptr().cast::<u32>(),
                output_sizes.as_mut_ptr(),
                block_count,
            );
        }

        let mut blocks = Vec::with_capacity(block_count);
        for (block, &compressed_size) in output_sizes.iter().enumerate() {
            let input_length = block_input_length(input.len(), BLOCK_SIZE, block);

            if compressed_size == OUTPUT_ERROR {
                return Err(MetalError::InvalidOutput {
                    block,
                    detail: "the kernel reported an output-capacity overflow".to_owned(),
                });
            }
            let compressed_size = compressed_size as usize;
            if compressed_size > output_slot_size {
                return Err(MetalError::InvalidOutput {
                    block,
                    detail: format!(
                        "reported {compressed_size} bytes for a {output_slot_size}-byte slot"
                    ),
                });
            }
            let slot_start = checked_mul(block, output_slot_size)?;
            let slot_end = slot_start
                .checked_add(compressed_size)
                .ok_or(MetalError::InputTooLarge)?;
            if slot_end > output_capacity {
                return Err(MetalError::InvalidOutput {
                    block,
                    detail: "the compressed payload lies outside the output buffer".to_owned(),
                });
            }
            // SAFETY: Command completion makes the shared output buffer readable
            // by the CPU. `slot_start..slot_end` was checked against the buffer's
            // allocation above, and the retained buffer outlives this slice.
            let payload = unsafe {
                std::slice::from_raw_parts(
                    buffers
                        .output
                        .contents()
                        .as_ptr()
                        .cast::<u8>()
                        .add(slot_start),
                    compressed_size,
                )
            };
            // Exhaustively decode every GPU block in development and tests so
            // kernel regressions fail at the provider boundary. Release builds
            // retain all command-status, size, range, and frame checks but avoid
            // duplicating the entire codec workload on the CPU.
            #[cfg(debug_assertions)]
            {
                let input_start = checked_mul(block, BLOCK_SIZE)?;
                let input_end = input_start
                    .checked_add(input_length)
                    .ok_or(MetalError::InputTooLarge)?;
                validate_lz4_block(payload, &input[input_start..input_end])
                    .map_err(|detail| MetalError::InvalidOutput { block, detail })?;
            }
            blocks.push(CompressedBlock {
                input_size: input_length,
                // The frame bridge stores the original block whenever the
                // provider payload is not smaller. Avoid copying those unused
                // bytes out of Metal while preserving that size signal.
                bytes: if compressed_size < input_length {
                    payload.to_vec()
                } else {
                    vec![0; input_length]
                },
            });
        }

        Ok(blocks)
    }
}

fn preferred_batch_size_for(recommended_working_set_size: u64) -> usize {
    const MIB: usize = 1024 * 1024;
    let budget = usize::try_from(recommended_working_set_size / 64).unwrap_or(usize::MAX);
    budget
        .clamp(64 * MIB, 1024 * MIB)
        .checked_div(BLOCK_SIZE)
        .expect("the LZ4 block size is nonzero")
        * BLOCK_SIZE
}

fn shared_context() -> Result<MetalLz4Context> {
    let mut cached = SHARED_CONTEXT
        .lock()
        .map_err(|_| MetalError::Creation("the shared LZ4 Metal context lock"))?;
    if let Some(context) = cached.as_ref() {
        return Ok(context.clone());
    }

    let device = MTLCreateSystemDefaultDevice().ok_or(MetalError::NoDevice)?;
    let device_info = device_info(&device);
    let library_data = DispatchData::from_static_bytes(include_bytes!("../shaders/lz4.metallib"));
    let library = device
        .newLibraryWithData_error(&library_data)
        .map_err(|error| MetalError::LibraryLoading(error.to_string()))?;
    let function = library
        .newFunctionWithName(ns_string!("rcomp_lz4_compress_blocks"))
        .ok_or(MetalError::MissingKernel(KERNEL_NAME))?;
    let pipeline = device
        .newComputePipelineStateWithFunction_error(&function)
        .map_err(|error| MetalError::PipelineCreation(error.to_string()))?;
    let context = MetalLz4Context {
        device,
        pipeline,
        device_info,
    };
    *cached = Some(context.clone());
    Ok(context)
}

fn shared_buffer(
    device: &objc2::runtime::ProtocolObject<dyn MTLDevice>,
    length: usize,
    description: &'static str,
) -> Result<objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn MTLBuffer>>> {
    device
        .newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared)
        .ok_or(MetalError::Creation(description))
}

fn assemble_frame(input: &[u8], block_size: usize, blocks: &[CompressedBlock]) -> Result<Vec<u8>> {
    let mut frame = Vec::with_capacity(input.len().saturating_add(32));
    frame.extend_from_slice(&0x184d_2204_u32.to_le_bytes());

    // Version 01 + independent blocks; no optional content size/checksums.
    let descriptor = [0x60_u8, 0x40_u8];
    frame.extend_from_slice(&descriptor);
    frame.push((xxhash32(&descriptor, 0) >> 8) as u8);

    for (block, compressed) in blocks.iter().enumerate() {
        let input_start = checked_mul(block, block_size)?;
        let input_length = block_input_length(input.len(), block_size, block);
        let input_end = input_start
            .checked_add(input_length)
            .ok_or(MetalError::InputTooLarge)?;

        if compressed.input_size != input_length {
            return Err(MetalError::InvalidOutput {
                block,
                detail: format!(
                    "provider block represents {} bytes, expected {input_length}",
                    compressed.input_size
                ),
            });
        }

        if compressed.bytes.len() < input_length {
            let compressed_size =
                u32::try_from(compressed.bytes.len()).map_err(|_| MetalError::InputTooLarge)?;
            frame.extend_from_slice(&compressed_size.to_le_bytes());
            frame.extend_from_slice(&compressed.bytes);
        } else {
            let stored_size =
                u32::try_from(input_length).map_err(|_| MetalError::InputTooLarge)? | 0x8000_0000;
            frame.extend_from_slice(&stored_size.to_le_bytes());
            frame.extend_from_slice(&input[input_start..input_end]);
        }
    }

    frame.extend_from_slice(&0_u32.to_le_bytes());
    Ok(frame)
}

fn block_input_length(total_size: usize, block_size: usize, block: usize) -> usize {
    total_size
        .saturating_sub(block.saturating_mul(block_size))
        .min(block_size)
}

fn checked_mul(left: usize, right: usize) -> Result<usize> {
    left.checked_mul(right).ok_or(MetalError::InputTooLarge)
}

fn lz4_compress_bound(input_size: usize) -> Result<usize> {
    input_size
        .checked_add(input_size / 255)
        .and_then(|value| value.checked_add(16))
        .ok_or(MetalError::InputTooLarge)
}

#[cfg(any(debug_assertions, test))]
fn validate_lz4_block(compressed: &[u8], expected: &[u8]) -> std::result::Result<(), String> {
    let mut input_position = 0;
    let mut decoded = Vec::with_capacity(expected.len());

    while input_position < compressed.len() {
        let token = compressed[input_position];
        input_position += 1;

        let literal_length = read_lz4_length(compressed, &mut input_position, token >> 4)?;
        let literal_end = input_position
            .checked_add(literal_length)
            .ok_or_else(|| "literal length overflowed".to_owned())?;
        let literals = compressed
            .get(input_position..literal_end)
            .ok_or_else(|| "literal bytes exceed the compressed block".to_owned())?;
        decoded.extend_from_slice(literals);
        input_position = literal_end;

        if decoded.len() > expected.len() {
            return Err("literal bytes exceed the original block length".to_owned());
        }
        if input_position == compressed.len() {
            break;
        }

        let offset_end = input_position
            .checked_add(2)
            .ok_or_else(|| "match offset overflowed".to_owned())?;
        let offset_bytes = compressed
            .get(input_position..offset_end)
            .ok_or_else(|| "match offset exceeds the compressed block".to_owned())?;
        let offset = u16::from_le_bytes([offset_bytes[0], offset_bytes[1]]) as usize;
        input_position = offset_end;
        if offset == 0 || offset > decoded.len() {
            return Err(format!("invalid match offset {offset}"));
        }

        let match_length = read_lz4_length(compressed, &mut input_position, token & 0x0f)?
            .checked_add(4)
            .ok_or_else(|| "match length overflowed".to_owned())?;
        if decoded.len().saturating_add(match_length) > expected.len() {
            return Err("match bytes exceed the original block length".to_owned());
        }
        for _ in 0..match_length {
            let value = decoded[decoded.len() - offset];
            decoded.push(value);
        }
    }

    if decoded != expected {
        return Err(format!(
            "decoded block differs from the original ({} bytes decoded, {} expected)",
            decoded.len(),
            expected.len()
        ));
    }

    Ok(())
}

#[cfg(any(debug_assertions, test))]
fn read_lz4_length(
    input: &[u8],
    position: &mut usize,
    nibble: u8,
) -> std::result::Result<usize, String> {
    let mut length = nibble as usize;
    if nibble != 15 {
        return Ok(length);
    }

    loop {
        let extension = *input
            .get(*position)
            .ok_or_else(|| "length extension exceeds the compressed block".to_owned())?;
        *position += 1;
        length = length
            .checked_add(extension as usize)
            .ok_or_else(|| "length extension overflowed".to_owned())?;
        if extension != 255 {
            return Ok(length);
        }
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
    use super::*;

    #[test]
    fn xxhash32_matches_empty_reference_vector() {
        assert_eq!(xxhash32(&[], 0), 0x02cc_5d05);
    }

    #[test]
    fn empty_frame_has_valid_descriptor_and_end_mark() {
        let frame = assemble_frame(&[], BLOCK_SIZE, &[]).unwrap();
        assert_eq!(&frame[..4], &[0x04, 0x22, 0x4d, 0x18]);
        assert_eq!(&frame[frame.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn raw_block_validator_accepts_overlapping_matches() {
        // One literal `a`, then an offset-one match of eleven bytes, followed
        // by five terminal literals. The match intentionally copies from its
        // own growing output, which is valid LZ4 behavior.
        let compressed = [0x17, b'a', 1, 0, 0x50, b'b', b'c', b'd', b'e', b'f'];
        let expected = b"aaaaaaaaaaaabcdef";
        validate_lz4_block(&compressed, expected).unwrap();
    }

    #[test]
    fn raw_block_validator_rejects_zero_offset() {
        let error = validate_lz4_block(&[0x00, 0, 0], b"test").unwrap_err();
        assert!(error.contains("offset"));
    }

    #[test]
    fn batch_size_scales_conservatively_with_working_set() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(preferred_batch_size_for(4 * GIB), 64 * 1024 * 1024);
        assert_eq!(preferred_batch_size_for(8 * GIB), 128 * 1024 * 1024);
        assert_eq!(preferred_batch_size_for(16 * GIB), 256 * 1024 * 1024);
        assert_eq!(preferred_batch_size_for(32 * GIB), 512 * 1024 * 1024);
        assert_eq!(preferred_batch_size_for(40_200_896_512), 599 * 1024 * 1024);
        assert_eq!(preferred_batch_size_for(128 * GIB), 1024 * 1024 * 1024);
    }
}
