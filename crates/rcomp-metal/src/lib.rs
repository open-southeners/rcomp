//! Experimental native Metal compute support for rcomp.
//!
//! This crate deliberately contains the Objective-C and GPU `unsafe` boundary
//! so that `rcomp-core` can continue to forbid unsafe Rust. The current API is
//! an experimental provider: it discovers the default Metal device, exposes a
//! reusable LZ4 Fast raw-block session, and keeps standards-compatible LZ4
//! Frame construction in `rcomp-core`. The kernel is not eligible for
//! automatic selection until it passes the project's performance gates.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use thiserror::Error;

#[cfg(target_os = "macos")]
mod macos;
mod provider;

pub use provider::MetalProvider;

/// The XOR mask applied by the feasibility shader.
///
/// This is public only so callers can independently verify smoke-test output.
pub const SMOKE_XOR_MASK: u32 = 0xa5a5_a5a5;

/// Information about the Metal device used for a compute operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalDeviceInfo {
    /// The user-facing Metal device name.
    pub name: String,
    /// The device architecture reported by Metal.
    pub architecture: String,
    /// The stable I/O Registry identifier for this device.
    pub registry_id: u64,
    /// Whether the CPU and GPU share the same memory pool.
    pub has_unified_memory: bool,
    /// Whether Metal classifies this as a low-power device.
    pub is_low_power: bool,
    /// Whether the device is not connected to a display.
    pub is_headless: bool,
    /// Whether the device is removable.
    pub is_removable: bool,
    /// Maximum Metal buffer length in bytes.
    pub max_buffer_length: u64,
    /// Approximate working-set size Metal recommends for good performance.
    pub recommended_working_set_size: u64,
}

/// Successful result of the native compute feasibility probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeTestReport {
    /// Device that executed the shader.
    pub device: MetalDeviceInfo,
    /// Values read back after the GPU applied [`SMOKE_XOR_MASK`].
    pub output: Vec<u32>,
}

/// Result of the experimental Metal LZ4 Frame encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalLz4Report {
    /// Device that executed the raw-block compression kernel.
    pub device: MetalDeviceInfo,
    /// Complete standards-compatible LZ4 Frame bytes.
    pub frame: Vec<u8>,
    /// Number of uncompressed input bytes.
    pub input_size: usize,
    /// Number of bytes in [`Self::frame`].
    pub output_size: usize,
    /// Number of independent blocks in the frame.
    pub block_count: usize,
    /// Blocks emitted using their GPU-compressed representation.
    pub compressed_block_count: usize,
    /// Blocks stored verbatim because compression was not smaller.
    pub stored_block_count: usize,
}

/// Failure from Metal discovery, setup, dispatch, or validation.
#[derive(Debug, Error)]
pub enum MetalError {
    /// This build target cannot use the native Metal implementation.
    #[error("native Metal acceleration is only available on macOS")]
    UnsupportedPlatform,
    /// Metal did not return a system-default device.
    #[error("no system-default Metal device is available")]
    NoDevice,
    /// The input cannot be represented by the feasibility shader's ABI.
    #[error("the smoke-test input is too large")]
    InputTooLarge,
    /// Metal failed to allocate or construct a required object.
    #[error("Metal could not create {0}")]
    Creation(&'static str),
    /// Metal could not compile the embedded shader source.
    #[error("Metal shader compilation failed: {0}")]
    ShaderCompilation(String),
    /// Metal could not load the embedded offline-compiled shader library.
    #[error("Metal shader library loading failed: {0}")]
    LibraryLoading(String),
    /// The compiled library did not contain the expected kernel.
    #[error("the Metal library does not contain kernel `{0}`")]
    MissingKernel(&'static str),
    /// Metal could not create a compute pipeline for the kernel.
    #[error("Metal compute-pipeline creation failed: {0}")]
    PipelineCreation(String),
    /// The submitted command buffer failed on the GPU.
    #[error("Metal command execution failed: {0}")]
    CommandExecution(String),
    /// The GPU returned metadata outside the buffers and limits supplied by
    /// the host.
    #[error("Metal returned invalid output for block {block}: {detail}")]
    InvalidOutput {
        /// Zero-based block index.
        block: usize,
        /// Validation failure detail.
        detail: String,
    },
}

/// Result type used by this experimental provider.
pub type Result<T> = std::result::Result<T, MetalError>;

/// Return information about the system-default Metal device.
///
/// # Errors
///
/// Returns [`MetalError::UnsupportedPlatform`] outside macOS and
/// [`MetalError::NoDevice`] when macOS cannot provide a Metal device.
pub fn default_device_info() -> Result<MetalDeviceInfo> {
    #[cfg(target_os = "macos")]
    {
        macos::default_device_info()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(MetalError::UnsupportedPlatform)
    }
}

/// Compile and execute the embedded Metal feasibility shader.
///
/// The shader applies [`SMOKE_XOR_MASK`] to each input element. This operation
/// is intentionally simple: its purpose is to validate device discovery,
/// shader compilation, shared-buffer access, command submission,
/// synchronization, and CPU readback before rcomp introduces a compression
/// kernel.
///
/// # Errors
///
/// Returns a [`MetalError`] when the platform has no usable Metal device or
/// any stage of shader compilation and execution fails.
pub fn run_smoke_test(input: &[u32]) -> Result<SmokeTestReport> {
    #[cfg(target_os = "macos")]
    {
        macos::run_smoke_test(input)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = input;
        Err(MetalError::UnsupportedPlatform)
    }
}

/// Compress bytes into an experimental standards-compatible LZ4 Frame using
/// the system-default Metal device.
///
/// The current feasibility kernel assigns one GPU thread to each independent
/// 64 KiB block and uses a small per-block hash table. Frame construction and
/// validation remain on the CPU. Blocks whose compressed form is not smaller
/// are stored verbatim as allowed by the LZ4 Frame specification.
///
/// This standalone API makes no performance guarantee. The provider-facing
/// integration uses the same raw-block implementation while delegating frame
/// construction to `rcomp-core`.
///
/// # Errors
///
/// Returns a [`MetalError`] when the input exceeds the prototype ABI, the
/// platform has no usable Metal device, resource allocation fails, the shader
/// cannot run, or GPU-produced metadata fails host validation.
pub fn compress_lz4_frame(input: &[u8]) -> Result<MetalLz4Report> {
    #[cfg(target_os = "macos")]
    {
        macos::compress_lz4_frame(input)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = input;
        Err(MetalError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_mask_is_self_inverse() {
        let value = 0x1234_5678;
        assert_eq!((value ^ SMOKE_XOR_MASK) ^ SMOKE_XOR_MASK, value);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_is_reported() {
        assert!(matches!(
            default_device_info(),
            Err(MetalError::UnsupportedPlatform)
        ));
        assert!(matches!(
            run_smoke_test(&[1, 2, 3]),
            Err(MetalError::UnsupportedPlatform)
        ));
        assert!(matches!(
            compress_lz4_frame(b"not available"),
            Err(MetalError::UnsupportedPlatform)
        ));
    }
}
