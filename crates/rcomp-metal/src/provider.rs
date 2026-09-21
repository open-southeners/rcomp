use rcomp_core::{
    AccelerationRequest, AcceleratorCapability, AcceleratorDevice, AcceleratorProvider,
    BlockEncoderSession, CapabilityMaturity, Codec, CompressedBlock, Container, Direction,
    Error as CoreError, Format, Level, ProviderDescriptor,
};

#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;

use crate::{MetalDeviceInfo, MetalError, Result, default_device_info};

const PROVIDER_ID: &str = "metal";
const LZ4_BLOCK_SIZE: usize = 64 * 1024;

/// Native Apple Metal acceleration provider.
///
/// The provider currently advertises one experimental capability: LZ4 Fast
/// encoding with independent 64 KiB blocks. Because it is experimental,
/// automatic backend selection must ignore it until its kernel passes the
/// project's performance gates.
#[derive(Debug, Clone)]
pub struct MetalProvider {
    device: MetalDeviceInfo,
}

impl MetalProvider {
    /// Discover the system-default Metal device and construct a provider.
    ///
    /// # Errors
    ///
    /// Returns a [`MetalError`] outside macOS or when no default Metal device
    /// is available.
    pub fn new() -> Result<Self> {
        Ok(Self {
            device: default_device_info()?,
        })
    }

    /// Return metadata for the device selected by this provider.
    #[must_use]
    pub fn device_info(&self) -> &MetalDeviceInfo {
        &self.device
    }

    fn device_id(&self) -> String {
        self.device.registry_id.to_string()
    }

    fn unavailable(request: &AccelerationRequest) -> CoreError {
        CoreError::AccelerationUnavailable {
            reason: format!(
                "Metal does not support {} {} at {:?} level on device {}",
                request.format, request.direction, request.level, request.device_id
            ),
        }
    }
}

impl AcceleratorProvider for MetalProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID.to_owned(),
            name: "Apple Metal".to_owned(),
        }
    }

    fn devices(&self) -> rcomp_core::Result<Vec<AcceleratorDevice>> {
        Ok(vec![AcceleratorDevice {
            provider_id: PROVIDER_ID.to_owned(),
            device_id: self.device_id(),
            name: self.device.name.clone(),
        }])
    }

    fn capabilities(
        &self,
        device: &AcceleratorDevice,
    ) -> rcomp_core::Result<Vec<AcceleratorCapability>> {
        if device.provider_id != PROVIDER_ID || device.device_id != self.device_id() {
            return Err(CoreError::AccelerationUnavailable {
                reason: format!(
                    "device {} does not belong to the Metal provider",
                    device.device_id
                ),
            });
        }

        Ok([
            Format::codec(Codec::Lz4),
            Format::layered(Container::Tar, Codec::Lz4),
        ]
        .into_iter()
        .map(|format| AcceleratorCapability {
            provider_id: PROVIDER_ID.to_owned(),
            device_id: self.device_id(),
            codec: Codec::Lz4,
            format,
            direction: Direction::Encode,
            levels: vec![Level::Fast],
            block_size: LZ4_BLOCK_SIZE,
            maturity: CapabilityMaturity::Experimental,
        })
        .collect())
    }

    fn open_block_encoder(
        &self,
        request: &AccelerationRequest,
    ) -> rcomp_core::Result<Box<dyn BlockEncoderSession>> {
        let supported_format = request.format == Format::codec(Codec::Lz4)
            || request.format == Format::layered(Container::Tar, Codec::Lz4);
        let supported = request.device_id == self.device_id()
            && supported_format
            && request.direction == Direction::Encode
            && request.level == Some(Level::Fast);
        if !supported {
            return Err(Self::unavailable(request));
        }

        #[cfg(target_os = "macos")]
        {
            // Core creates sessions on its accelerator worker. Give that
            // background thread an explicit pool while Objective-C/Metal
            // factories create temporary autoreleased objects; retained
            // session resources safely outlive the pool.
            let encoder = autoreleasepool(|_| crate::macos::MetalLz4BlockEncoder::new())
                .map_err(core_failure)?;
            Ok(Box::new(MetalBlockEncoder(encoder)))
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err(Self::unavailable(request))
        }
    }
}

#[cfg(target_os = "macos")]
struct MetalBlockEncoder(crate::macos::MetalLz4BlockEncoder);

#[cfg(target_os = "macos")]
impl BlockEncoderSession for MetalBlockEncoder {
    fn block_size(&self) -> usize {
        LZ4_BLOCK_SIZE
    }

    fn preferred_batch_size(&self) -> usize {
        self.0.preferred_batch_size()
    }

    fn compress_blocks(
        &mut self,
        input: &[u8],
        cancel: &rcomp_core::CancelToken,
    ) -> rcomp_core::Result<Vec<CompressedBlock>> {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        // The bridge reuses this worker for many batches, so scope temporary
        // Objective-C objects to one dispatch instead of relying on a
        // process/main-thread autorelease pool.
        let blocks = autoreleasepool(|_| self.0.compress_blocks(input)).map_err(core_failure)?;
        // Metal command buffers cannot be cancelled after commit. The native
        // call waits synchronously so all retained resources remain alive,
        // then this check discards completed output when cancellation arrived
        // during execution.
        if cancel.is_cancelled() {
            Err(CoreError::Cancelled)
        } else {
            Ok(blocks)
        }
    }
}

#[cfg(target_os = "macos")]
fn core_failure(error: MetalError) -> CoreError {
    CoreError::AccelerationFailed {
        provider: PROVIDER_ID.to_owned(),
        reason: error.to_string(),
    }
}
