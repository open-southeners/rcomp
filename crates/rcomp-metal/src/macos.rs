use std::{ffi::c_void, mem, ptr::NonNull};

use objc2_foundation::ns_string;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLComputeCommandEncoder, MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLResourceOptions, MTLSize,
};

use crate::{MetalDeviceInfo, MetalError, Result, SmokeTestReport};

mod lz4;

const KERNEL_NAME: &str = "rcomp_compute_smoke";

// MTLCreateSystemDefaultDevice lives in Metal but its implementation needs
// CoreGraphics to be linked on macOS. Keeping this declaration here avoids a
// broad CoreGraphics Rust dependency for one linker requirement.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

pub(crate) fn default_device_info() -> Result<MetalDeviceInfo> {
    let device = MTLCreateSystemDefaultDevice().ok_or(MetalError::NoDevice)?;
    Ok(device_info(&device))
}

pub(crate) fn run_smoke_test(input: &[u32]) -> Result<SmokeTestReport> {
    let count = u32::try_from(input.len()).map_err(|_| MetalError::InputTooLarge)?;
    let device = MTLCreateSystemDefaultDevice().ok_or(MetalError::NoDevice)?;
    let info = device_info(&device);

    if input.is_empty() {
        return Ok(SmokeTestReport {
            device: info,
            output: Vec::new(),
        });
    }

    let byte_len = input
        .len()
        .checked_mul(mem::size_of::<u32>())
        .ok_or(MetalError::InputTooLarge)?;
    if byte_len > device.maxBufferLength() {
        return Err(MetalError::InputTooLarge);
    }

    let library = device
        .newLibraryWithSource_options_error(
            ns_string!(include_str!("shaders/compute_smoke.metal")),
            None,
        )
        .map_err(|error| MetalError::ShaderCompilation(error.to_string()))?;
    let function = library
        .newFunctionWithName(ns_string!("rcomp_compute_smoke"))
        .ok_or(MetalError::MissingKernel(KERNEL_NAME))?;
    let pipeline = device
        .newComputePipelineStateWithFunction_error(&function)
        .map_err(|error| MetalError::PipelineCreation(error.to_string()))?;
    let queue = device
        .newCommandQueue()
        .ok_or(MetalError::Creation("a command queue"))?;

    let input_buffer = device
        .newBufferWithLength_options(byte_len, MTLResourceOptions::StorageModeShared)
        .ok_or(MetalError::Creation("an input buffer"))?;
    let output_buffer = device
        .newBufferWithLength_options(byte_len, MTLResourceOptions::StorageModeShared)
        .ok_or(MetalError::Creation("an output buffer"))?;

    // SAFETY: `input_buffer` owns at least `byte_len` writable bytes, the source
    // slice contains exactly `byte_len` initialized bytes, and the ranges cannot
    // overlap. No GPU command references the buffer until after this copy.
    unsafe {
        std::ptr::copy_nonoverlapping(
            input.as_ptr().cast::<u8>(),
            input_buffer.contents().as_ptr().cast::<u8>(),
            byte_len,
        );
    }

    let command_buffer = queue
        .commandBuffer()
        .ok_or(MetalError::Creation("a command buffer"))?;
    let encoder = command_buffer
        .computeCommandEncoder()
        .ok_or(MetalError::Creation("a compute command encoder"))?;

    encoder.setComputePipelineState(&pipeline);

    // SAFETY: The embedded kernel ABI binds two arrays of `u32` at indices 0
    // and 1 and one copied `u32` count at index 2. Both buffers are `byte_len`
    // bytes, use offset zero, remain retained until completion, and the CPU does
    // not access them while the command buffer is executing.
    unsafe {
        encoder.setBuffer_offset_atIndex(Some(&input_buffer), 0, 0);
        encoder.setBuffer_offset_atIndex(Some(&output_buffer), 0, 1);
        encoder.setBytes_length_atIndex(
            NonNull::from(&count).cast::<c_void>(),
            mem::size_of::<u32>(),
            2,
        );
    }

    let execution_width = pipeline.threadExecutionWidth().max(1);
    let threads_per_group = execution_width
        .min(pipeline.maxTotalThreadsPerThreadgroup())
        .min(input.len());
    encoder.dispatchThreads_threadsPerThreadgroup(
        MTLSize {
            width: input.len(),
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

    let mut output = vec![0_u32; input.len()];
    // SAFETY: Metal reports successful command completion, so the GPU no longer
    // accesses either buffer. `output_buffer` contains `byte_len` bytes written
    // by the kernel, and `output` owns the same number of writable bytes. The
    // allocations are distinct and therefore do not overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            output_buffer.contents().as_ptr().cast::<u8>(),
            output.as_mut_ptr().cast::<u8>(),
            byte_len,
        );
    }

    Ok(SmokeTestReport {
        device: info,
        output,
    })
}

pub(crate) use lz4::MetalLz4BlockEncoder;
pub(crate) use lz4::compress_lz4_frame;

pub(super) fn device_info(
    device: &objc2::runtime::ProtocolObject<dyn MTLDevice>,
) -> MetalDeviceInfo {
    MetalDeviceInfo {
        name: device.name().to_string(),
        architecture: device.architecture().name().to_string(),
        registry_id: device.registryID(),
        has_unified_memory: device.hasUnifiedMemory(),
        is_low_power: device.isLowPower(),
        is_headless: device.isHeadless(),
        is_removable: device.isRemovable(),
        max_buffer_length: device.maxBufferLength() as u64,
        recommended_working_set_size: device.recommendedMaxWorkingSetSize(),
    }
}
