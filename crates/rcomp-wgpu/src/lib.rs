//! Experimental cross-platform GPU compute provider for rcomp.
//!
//! The provider uses one WGSL LZ4 Fast kernel through `wgpu`: Direct3D 12 on
//! Windows, Vulkan on Linux, and Metal on Apple Silicon. Standard LZ4 Frame
//! construction remains in `rcomp-core`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod provider;

use thiserror::Error;

pub use provider::WgpuProvider;

/// Information reported by `wgpu` for one compatible adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuDeviceInfo {
    /// Provider-specific identifier used in acceleration requests.
    pub device_id: String,
    /// Human-readable adapter name.
    pub name: String,
    /// Native API selected by `wgpu` (`dx12`, `vulkan`, or `metal`).
    pub backend: String,
    /// Backend-specific vendor identifier, normally a PCI vendor ID.
    pub vendor_id: u32,
    /// Backend-specific device identifier, normally a PCI device ID.
    pub device_id_numeric: u32,
    /// Adapter class reported by `wgpu`.
    pub device_type: String,
    /// Driver name.
    pub driver: String,
    /// Driver version or additional driver information.
    pub driver_info: String,
}

/// Failure from portable adapter discovery, setup, dispatch, or validation.
#[derive(Debug, Error)]
pub enum WgpuError {
    /// The build target has no supported native `wgpu` backend.
    #[error("portable GPU acceleration is supported only on Windows, Linux, and Apple Silicon")]
    UnsupportedPlatform,
    /// No adapter exposed the baseline compute and buffer limits required by
    /// the LZ4 kernel.
    #[error("no compatible GPU adapter is available")]
    NoAdapter,
    /// A logical input or allocation size exceeded the portable shader ABI or
    /// the selected adapter's reported limits.
    #[error("input exceeds the selected GPU adapter's portable limits")]
    InputTooLarge,
    /// Device creation failed.
    #[error("could not open GPU adapter `{adapter}`: {reason}")]
    DeviceRequest {
        /// Human-readable adapter name.
        adapter: String,
        /// Native/backend error detail.
        reason: String,
    },
    /// GPU execution or polling failed.
    #[error("GPU command execution failed: {0}")]
    CommandExecution(String),
    /// A readback buffer could not be mapped.
    #[error("GPU readback failed: {0}")]
    Readback(String),
    /// The GPU returned metadata outside the buffers and limits supplied by
    /// the host.
    #[error("GPU returned invalid LZ4 output for block {block}: {detail}")]
    InvalidOutput {
        /// Zero-based block index.
        block: usize,
        /// Validation failure detail.
        detail: String,
    },
}

/// Result type used by the portable provider.
pub type Result<T> = std::result::Result<T, WgpuError>;
