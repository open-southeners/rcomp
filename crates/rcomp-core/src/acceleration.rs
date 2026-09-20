//! Hardware-acceleration selection shared by the core, CLI, and desktop app.
//!
//! This module deliberately separates a user's preference from the backend
//! that is actually selected. Native implementations are registered through
//! the safe provider boundary, keeping vendor APIs and unsafe code outside
//! `rcomp-core`. `Auto` falls back observably when no supported capability
//! matches, while `Required` returns a clear error.

use std::{fmt, str::FromStr, sync::Arc};

use crate::{CancelToken, Codec, Error, Format, Level, Result};

/// User preference for hardware acceleration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum AccelerationPreference {
    /// Always use the canonical CPU implementation.
    #[default]
    Cpu,
    /// Use a compatible accelerator when available, otherwise use the CPU and
    /// report the fallback to the caller.
    Auto,
    /// Require a compatible accelerator and fail rather than falling back.
    Required,
}

impl fmt::Display for AccelerationPreference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Auto => "auto",
            Self::Required => "required",
        })
    }
}

impl FromStr for AccelerationPreference {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "cpu" | "off" | "disabled" => Ok(Self::Cpu),
            "auto" | "on" | "enabled" => Ok(Self::Auto),
            "required" | "require" | "gpu" => Ok(Self::Required),
            _ => Err(format!(
                "invalid accelerator mode `{value}` (expected auto, cpu, or required)"
            )),
        }
    }
}

/// Direction of the codec operation for capability matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Compression/encoding.
    Encode,
    /// Decompression/decoding.
    Decode,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Encode => "encoding",
            Self::Decode => "decoding",
        })
    }
}

/// Backend selected for an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum ProcessingBackend {
    /// The existing CPU codec implementation.
    Cpu,
    /// A hardware provider/device identifier.
    Accelerator(String),
}

/// Stability level of an accelerator capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityMaturity {
    /// Correctness or performance is still being evaluated. Automatic
    /// selection must not choose this capability.
    Experimental,
    /// The capability has passed compatibility and performance gates and may
    /// participate in automatic selection.
    Supported,
}

/// Metadata identifying an acceleration provider implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// Stable machine-readable provider identifier, such as `metal`.
    pub id: String,
    /// Human-readable provider name.
    pub name: String,
}

/// A physical device exposed by an acceleration provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratorDevice {
    /// Identifier of the provider that owns this device.
    pub provider_id: String,
    /// Stable provider-specific device identifier.
    pub device_id: String,
    /// Human-readable device name.
    pub name: String,
}

/// One exact operation supported by an accelerator device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratorCapability {
    /// Identifier of the provider implementing the operation.
    pub provider_id: String,
    /// Provider-specific device identifier.
    pub device_id: String,
    /// Codec implemented by the provider.
    pub codec: Codec,
    /// External format produced or consumed by the operation.
    pub format: Format,
    /// Encode or decode direction.
    pub direction: Direction,
    /// rcomp compression levels whose semantics this implementation supports.
    pub levels: Vec<Level>,
    /// Independent block size used by the provider operation.
    pub block_size: usize,
    /// Whether the capability is eligible for automatic selection.
    pub maturity: CapabilityMaturity,
}

impl AcceleratorCapability {
    /// Return whether this capability can serve an exact operation request.
    #[must_use]
    pub fn supports(&self, request: &AccelerationRequest) -> bool {
        self.device_id == request.device_id
            && self.format == request.format
            && self.direction == request.direction
            && request
                .level
                .is_none_or(|level| self.levels.contains(&level))
    }
}

/// Request used to open a provider session for one device and operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelerationRequest {
    /// Provider-specific device to use.
    pub device_id: String,
    /// External file format being produced or consumed.
    pub format: Format,
    /// Encode or decode direction.
    pub direction: Direction,
    /// Requested compression level, if the operation has one.
    pub level: Option<Level>,
}

/// Compressed representation of one independent input block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedBlock {
    /// Number of uncompressed bytes represented by this block.
    pub input_size: usize,
    /// Raw codec block bytes. Standard file/container framing is not included.
    pub bytes: Vec<u8>,
}

/// A stateful provider session that compresses bounded batches of independent
/// blocks.
pub trait BlockEncoderSession: Send {
    /// Fixed maximum input size of each block accepted by this session.
    fn block_size(&self) -> usize;

    /// Preferred number of input bytes submitted in one provider call.
    ///
    /// The framing bridge rounds this down to a whole number of blocks and
    /// applies its own memory-safety ceiling. Providers should choose a value
    /// large enough to use their hardware efficiently without requiring
    /// unbounded host or device memory.
    fn preferred_batch_size(&self) -> usize {
        16 * 1024 * 1024
    }

    /// Compress one or more contiguous blocks.
    ///
    /// All blocks except the final block have [`Self::block_size`] bytes. The
    /// returned blocks must preserve input order. File-format framing remains
    /// the responsibility of `rcomp-core`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] when cancellation is observed. A provider
    /// that cannot abort active device work must wait for that work to become
    /// safe to release, discard its result, and then return cancellation.
    /// Other acceleration errors cover rejected input, device loss, and
    /// invalid output metadata.
    fn compress_blocks(
        &mut self,
        input: &[u8],
        cancel: &CancelToken,
    ) -> Result<Vec<CompressedBlock>>;
}

/// Safe, vendor-neutral interface implemented by native accelerator crates.
///
/// Objective-C, CUDA, ROCm, driver handles, and unsafe device-memory code stay
/// behind this trait in their respective provider crates.
pub trait AcceleratorProvider: Send + Sync {
    /// Return stable provider metadata.
    fn descriptor(&self) -> ProviderDescriptor;

    /// Discover devices currently available to this provider.
    ///
    /// # Errors
    ///
    /// Returns an acceleration error when provider initialization or device
    /// discovery fails.
    fn devices(&self) -> Result<Vec<AcceleratorDevice>>;

    /// Return exact capabilities for a discovered device.
    ///
    /// # Errors
    ///
    /// Returns an acceleration error when the device does not belong to this
    /// provider or capability probing fails.
    fn capabilities(&self, device: &AcceleratorDevice) -> Result<Vec<AcceleratorCapability>>;

    /// Open a bounded raw-block compression session.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AccelerationUnavailable`] when the requested device,
    /// format, direction, or level is unsupported, and an acceleration error
    /// when native provider initialization fails.
    fn open_block_encoder(
        &self,
        request: &AccelerationRequest,
    ) -> Result<Box<dyn BlockEncoderSession>>;
}

/// Collection of optional accelerator providers supplied by an application.
///
/// `rcomp-core` never constructs native providers itself. CLI, desktop, and
/// third-party applications register the platform providers they chose to
/// compile, avoiding a dependency from core back to vendor crates.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn AcceleratorProvider>>,
}

impl ProviderRegistry {
    /// Create an empty CPU-only registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider, replacing an existing provider with the same stable ID.
    pub fn register(&mut self, provider: Arc<dyn AcceleratorProvider>) {
        let id = provider.descriptor().id;
        if let Some(existing) = self
            .providers
            .iter_mut()
            .find(|candidate| candidate.descriptor().id == id)
        {
            *existing = provider;
        } else {
            self.providers.push(provider);
        }
    }

    /// Return the registered providers in registration order.
    #[must_use]
    pub fn providers(&self) -> &[Arc<dyn AcceleratorProvider>] {
        &self.providers
    }

    /// Discover every capability currently exposed by registered providers.
    ///
    /// # Errors
    ///
    /// Returns the first provider discovery or capability-probing error.
    pub fn capabilities(&self) -> Result<Vec<AcceleratorCapability>> {
        let mut capabilities = Vec::new();
        for provider in &self.providers {
            for device in provider.devices()? {
                capabilities.extend(provider.capabilities(&device)?);
            }
        }
        Ok(capabilities)
    }
}

/// Result of resolving an acceleration preference for an operation.
pub(crate) struct Selection {
    pub(crate) backend: ProcessingBackend,
    pub(crate) notice: Option<String>,
    pub(crate) block_encoder: Option<Box<dyn BlockEncoderSession>>,
}

/// Resolve the requested preference against the registered providers.
pub(crate) fn select(
    registry: &ProviderRegistry,
    preference: AccelerationPreference,
    direction: Direction,
    format: Format,
    level: Option<Level>,
) -> Result<Selection> {
    if preference == AccelerationPreference::Cpu {
        return Ok(Selection {
            backend: ProcessingBackend::Cpu,
            notice: None,
            block_encoder: None,
        });
    }

    let mut experimental_match = false;
    for provider in registry.providers() {
        let devices = match provider.devices() {
            Ok(devices) => devices,
            Err(error) if preference == AccelerationPreference::Auto => {
                let _ = error;
                continue;
            }
            Err(error) => return Err(error),
        };
        for device in devices {
            let capabilities = match provider.capabilities(&device) {
                Ok(capabilities) => capabilities,
                Err(error) if preference == AccelerationPreference::Auto => {
                    let _ = error;
                    continue;
                }
                Err(error) => return Err(error),
            };
            for capability in capabilities {
                let request = AccelerationRequest {
                    device_id: device.device_id.clone(),
                    format,
                    direction,
                    level,
                };
                if !capability.supports(&request) {
                    continue;
                }
                if capability.maturity == CapabilityMaturity::Experimental
                    && preference == AccelerationPreference::Auto
                {
                    experimental_match = true;
                    continue;
                }

                let descriptor = provider.descriptor();
                let block_encoder = match provider.open_block_encoder(&request) {
                    Ok(encoder) => encoder,
                    Err(error) if preference == AccelerationPreference::Auto => {
                        return Ok(Selection {
                            backend: ProcessingBackend::Cpu,
                            notice: Some(format!(
                                "{} failed to initialize for {format} {direction}: {error}; using CPU",
                                descriptor.name
                            )),
                            block_encoder: None,
                        });
                    }
                    Err(error) => return Err(error),
                };
                return Ok(Selection {
                    backend: ProcessingBackend::Accelerator(format!(
                        "{}:{}",
                        descriptor.id, device.device_id
                    )),
                    notice: None,
                    block_encoder: Some(block_encoder),
                });
            }
        }
    }

    if preference == AccelerationPreference::Required {
        return Err(Error::AccelerationUnavailable {
            reason: unavailable_message(direction, format, level),
        });
    }

    let notice = if experimental_match {
        format!(
            "a compatible experimental accelerator exists for {format} {direction}, but automatic selection requires a supported performance profile; using CPU"
        )
    } else {
        unavailable_message(direction, format, level)
    };
    Ok(Selection {
        backend: ProcessingBackend::Cpu,
        notice: Some(notice),
        block_encoder: None,
    })
}

fn unavailable_message(direction: Direction, format: Format, level: Option<Level>) -> String {
    let level = level
        .map(|value| format!(" at {value:?} level"))
        .unwrap_or_default();
    format!(
        "no compatible hardware accelerator is available for {format} {direction}{level}; using the CPU is the only supported path on this system"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;

    struct FakeProvider {
        name: &'static str,
    }

    impl AcceleratorProvider for FakeProvider {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: "fake".to_owned(),
                name: self.name.to_owned(),
            }
        }

        fn devices(&self) -> Result<Vec<AcceleratorDevice>> {
            Ok(vec![AcceleratorDevice {
                provider_id: "fake".to_owned(),
                device_id: "device-1".to_owned(),
                name: "Fake GPU".to_owned(),
            }])
        }

        fn capabilities(&self, device: &AcceleratorDevice) -> Result<Vec<AcceleratorCapability>> {
            Ok(vec![AcceleratorCapability {
                provider_id: device.provider_id.clone(),
                device_id: device.device_id.clone(),
                codec: Codec::Lz4,
                format: Format::codec(Codec::Lz4),
                direction: Direction::Encode,
                levels: vec![Level::Fast],
                block_size: 64 * 1024,
                maturity: CapabilityMaturity::Experimental,
            }])
        }

        fn open_block_encoder(
            &self,
            _request: &AccelerationRequest,
        ) -> Result<Box<dyn BlockEncoderSession>> {
            Err(Error::AccelerationUnavailable {
                reason: "fake provider has no executor".to_owned(),
            })
        }
    }

    #[test]
    fn preference_aliases_parse() {
        assert_eq!("auto".parse(), Ok(AccelerationPreference::Auto));
        assert_eq!("off".parse(), Ok(AccelerationPreference::Cpu));
        assert_eq!("gpu".parse(), Ok(AccelerationPreference::Required));
        assert!("sometimes".parse::<AccelerationPreference>().is_err());
    }

    #[test]
    fn auto_falls_back_observably() {
        let selection = select(
            &ProviderRegistry::new(),
            AccelerationPreference::Auto,
            Direction::Encode,
            Format::codec(Codec::Gzip),
            Some(Level::Best),
        )
        .unwrap();
        assert_eq!(selection.backend, ProcessingBackend::Cpu);
        assert!(selection.notice.unwrap().contains("gzip"));
    }

    #[test]
    fn required_is_an_error_without_a_provider() {
        let result = select(
            &ProviderRegistry::new(),
            AccelerationPreference::Required,
            Direction::Decode,
            Format::codec(Codec::Zstd),
            None,
        );
        assert!(matches!(result, Err(Error::AccelerationUnavailable { .. })));
    }

    #[test]
    fn registry_replaces_duplicate_provider_ids() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider { name: "first" }));
        registry.register(Arc::new(FakeProvider {
            name: "replacement",
        }));

        assert_eq!(registry.providers().len(), 1);
        assert_eq!(registry.providers()[0].descriptor().name, "replacement");
    }

    #[test]
    fn registry_discovers_exact_capabilities() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider { name: "fake" }));

        let capabilities = registry.capabilities().unwrap();
        assert_eq!(capabilities.len(), 1);
        let capability = &capabilities[0];
        assert_eq!(capability.codec, Codec::Lz4);
        assert_eq!(capability.maturity, CapabilityMaturity::Experimental);
        assert!(capability.supports(&AccelerationRequest {
            device_id: "device-1".to_owned(),
            format: Format::codec(Codec::Lz4),
            direction: Direction::Encode,
            level: Some(Level::Fast),
        }));
        assert!(!capability.supports(&AccelerationRequest {
            device_id: "device-1".to_owned(),
            format: Format::codec(Codec::Lz4),
            direction: Direction::Encode,
            level: Some(Level::Best),
        }));
    }
}
