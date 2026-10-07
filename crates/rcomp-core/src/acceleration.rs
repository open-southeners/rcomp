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

/// Explicit accelerator provider and device requested by an application.
///
/// The textual form is `PROVIDER:DEVICE-ID`. Device identifiers may contain
/// additional colons, so only the first colon separates the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratorTarget {
    /// Stable provider identifier, such as `wgpu` or `metal`.
    pub provider_id: String,
    /// Provider-specific device identifier.
    pub device_id: String,
}

impl fmt::Display for AcceleratorTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider_id, self.device_id)
    }
}

impl FromStr for AcceleratorTarget {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let (provider_id, device_id) = value.split_once(':').ok_or_else(|| {
            format!("invalid accelerator device `{value}` (expected PROVIDER:DEVICE-ID)")
        })?;
        if provider_id.is_empty() || device_id.is_empty() {
            return Err(format!(
                "invalid accelerator device `{value}` (provider and device ID must be non-empty)"
            ));
        }
        Ok(Self {
            provider_id: provider_id.to_owned(),
            device_id: device_id.to_owned(),
        })
    }
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

/// Stable reason that an automatic acceleration request used the CPU.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "code", rename_all = "snake_case")
)]
pub enum AccelerationFallbackReason {
    /// The application did not register any hardware provider.
    NoProviderInstalled,
    /// Providers were present but exposed no matching device.
    NoCompatibleDevice,
    /// Devices were present but exposed no capabilities.
    UnsupportedOperation,
    /// No capability implemented the requested codec.
    UnsupportedCodec,
    /// The codec was present but not with the requested external format.
    UnsupportedFormat,
    /// The format was present but not for encode/decode as requested.
    UnsupportedDirection,
    /// The operation was present but not at the requested compression level.
    UnsupportedLevel,
    /// A matching capability exists but is not yet eligible for `Auto`.
    ExperimentalCapability,
    /// Device discovery or capability probing failed for a provider.
    ProviderDiscoveryFailed {
        /// Stable provider identifier.
        provider_id: String,
    },
    /// A matching provider failed while opening an execution session.
    ProviderInitializationFailed {
        /// Stable provider identifier.
        provider_id: String,
    },
    /// A selected provider failed and the file operation succeeded on CPU.
    ProviderExecutionFailedCpuRetry {
        /// Stable provider identifier reported by the failed session.
        provider_id: String,
    },
    /// The explicitly selected provider/device was not usable.
    ExplicitTargetUnavailable {
        /// `PROVIDER:DEVICE-ID` selector supplied by the caller.
        target: String,
    },
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

/// One provider-reported diagnostic property for an accelerator device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceleratorDeviceProperty {
    /// Stable machine-readable property name.
    pub key: String,
    /// Human-readable property value.
    pub value: String,
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

    /// Return provider-specific diagnostic properties for one device.
    ///
    /// Properties are informational and must not be used as the capability
    /// contract. Providers may return an empty list.
    fn device_properties(
        &self,
        _device: &AcceleratorDevice,
    ) -> Result<Vec<AcceleratorDeviceProperty>> {
        Ok(Vec::new())
    }

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
    pub(crate) fallback_reason: Option<AccelerationFallbackReason>,
    pub(crate) block_encoder: Option<Box<dyn BlockEncoderSession>>,
}

/// Resolve the requested preference against the registered providers.
pub(crate) fn select(
    registry: &ProviderRegistry,
    preference: AccelerationPreference,
    target: Option<&AcceleratorTarget>,
    direction: Direction,
    format: Format,
    level: Option<Level>,
) -> Result<Selection> {
    if preference == AccelerationPreference::Cpu {
        return Ok(Selection {
            backend: ProcessingBackend::Cpu,
            notice: None,
            fallback_reason: None,
            block_encoder: None,
        });
    }

    let mut experimental_match = false;
    let mut matching_provider = false;
    let mut matching_device = false;
    let mut saw_capability = false;
    let mut matching_codec = false;
    let mut matching_format = false;
    let mut matching_direction = false;
    let mut matching_level = false;
    let mut discovery_failure: Option<(String, String)> = None;
    for provider in registry.providers() {
        let descriptor = provider.descriptor();
        if target.is_some_and(|target| target.provider_id != descriptor.id) {
            continue;
        }
        matching_provider = true;
        let devices = match provider.devices() {
            Ok(devices) => devices,
            Err(error) if preference == AccelerationPreference::Auto => {
                discovery_failure.get_or_insert_with(|| (descriptor.id.clone(), error.to_string()));
                continue;
            }
            Err(error) => return Err(error),
        };
        for device in devices {
            if target.is_some_and(|target| target.device_id != device.device_id) {
                continue;
            }
            matching_device = true;
            let capabilities = match provider.capabilities(&device) {
                Ok(capabilities) => capabilities,
                Err(error) if preference == AccelerationPreference::Auto => {
                    discovery_failure
                        .get_or_insert_with(|| (descriptor.id.clone(), error.to_string()));
                    continue;
                }
                Err(error) => return Err(error),
            };
            for capability in capabilities {
                saw_capability = true;
                if format.codec != Some(capability.codec) {
                    continue;
                }
                matching_codec = true;
                if capability.format != format {
                    continue;
                }
                matching_format = true;
                if capability.direction != direction {
                    continue;
                }
                matching_direction = true;
                if level.is_some_and(|level| !capability.levels.contains(&level)) {
                    continue;
                }
                matching_level = true;
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

                let block_encoder = match provider.open_block_encoder(&request) {
                    Ok(encoder) => encoder,
                    Err(error) if preference == AccelerationPreference::Auto => {
                        return Ok(Selection {
                            backend: ProcessingBackend::Cpu,
                            notice: Some(format!(
                                "{} failed to initialize for {format} {direction}: {error}; using CPU",
                                descriptor.name
                            )),
                            fallback_reason: Some(
                                AccelerationFallbackReason::ProviderInitializationFailed {
                                    provider_id: descriptor.id,
                                },
                            ),
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
                    fallback_reason: None,
                    block_encoder: Some(block_encoder),
                });
            }
        }
    }

    if preference == AccelerationPreference::Required {
        return Err(Error::AccelerationUnavailable {
            reason: target.map_or_else(
                || unavailable_message(direction, format, level),
                |target| {
                    format!(
                        "requested accelerator `{target}` is unavailable or incompatible with {format} {direction}"
                    )
                },
            ),
        });
    }

    let (notice, fallback_reason) = if experimental_match {
        (
            format!(
                "a compatible experimental accelerator exists for {format} {direction}, but automatic selection requires a supported performance profile; using CPU"
            ),
            AccelerationFallbackReason::ExperimentalCapability,
        )
    } else if let Some((provider_id, error)) = discovery_failure {
        (
            format!(
                "hardware provider `{provider_id}` could not discover or probe a compatible device: {error}; using CPU"
            ),
            AccelerationFallbackReason::ProviderDiscoveryFailed { provider_id },
        )
    } else {
        let reason = if let Some(target) = target {
            AccelerationFallbackReason::ExplicitTargetUnavailable {
                target: target.to_string(),
            }
        } else if registry.providers().is_empty() {
            AccelerationFallbackReason::NoProviderInstalled
        } else if !matching_provider || !matching_device {
            AccelerationFallbackReason::NoCompatibleDevice
        } else if !saw_capability {
            AccelerationFallbackReason::UnsupportedOperation
        } else if !matching_codec {
            AccelerationFallbackReason::UnsupportedCodec
        } else if !matching_format {
            AccelerationFallbackReason::UnsupportedFormat
        } else if !matching_direction {
            AccelerationFallbackReason::UnsupportedDirection
        } else if !matching_level {
            AccelerationFallbackReason::UnsupportedLevel
        } else {
            AccelerationFallbackReason::UnsupportedOperation
        };
        (unavailable_message(direction, format, level), reason)
    };
    Ok(Selection {
        backend: ProcessingBackend::Cpu,
        notice: Some(notice),
        fallback_reason: Some(fallback_reason),
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

    struct DiscoveryFailureProvider;

    struct InitializationFailureProvider;

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

    impl AcceleratorProvider for DiscoveryFailureProvider {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: "discovery-failure".to_owned(),
                name: "Discovery failure".to_owned(),
            }
        }

        fn devices(&self) -> Result<Vec<AcceleratorDevice>> {
            Err(Error::AccelerationUnavailable {
                reason: "simulated driver discovery failure".to_owned(),
            })
        }

        fn capabilities(&self, _device: &AcceleratorDevice) -> Result<Vec<AcceleratorCapability>> {
            unreachable!("device discovery failed")
        }

        fn open_block_encoder(
            &self,
            _request: &AccelerationRequest,
        ) -> Result<Box<dyn BlockEncoderSession>> {
            unreachable!("device discovery failed")
        }
    }

    impl AcceleratorProvider for InitializationFailureProvider {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: "initialization-failure".to_owned(),
                name: "Initialization failure".to_owned(),
            }
        }

        fn devices(&self) -> Result<Vec<AcceleratorDevice>> {
            Ok(vec![AcceleratorDevice {
                provider_id: "initialization-failure".to_owned(),
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
                maturity: CapabilityMaturity::Supported,
            }])
        }

        fn open_block_encoder(
            &self,
            _request: &AccelerationRequest,
        ) -> Result<Box<dyn BlockEncoderSession>> {
            Err(Error::AccelerationUnavailable {
                reason: "simulated device initialization failure".to_owned(),
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
    fn accelerator_target_parses_provider_and_colon_rich_device_id() {
        let target: AcceleratorTarget = "wgpu:metal:106b:0000:0".parse().unwrap();
        assert_eq!(target.provider_id, "wgpu");
        assert_eq!(target.device_id, "metal:106b:0000:0");
        assert_eq!(target.to_string(), "wgpu:metal:106b:0000:0");
        assert!("wgpu".parse::<AcceleratorTarget>().is_err());
        assert!(":device".parse::<AcceleratorTarget>().is_err());
        assert!("wgpu:".parse::<AcceleratorTarget>().is_err());
    }

    #[test]
    fn auto_falls_back_observably() {
        let selection = select(
            &ProviderRegistry::new(),
            AccelerationPreference::Auto,
            None,
            Direction::Encode,
            Format::codec(Codec::Gzip),
            Some(Level::Best),
        )
        .unwrap();
        assert_eq!(selection.backend, ProcessingBackend::Cpu);
        assert!(selection.notice.unwrap().contains("gzip"));
        assert_eq!(
            selection.fallback_reason,
            Some(AccelerationFallbackReason::NoProviderInstalled)
        );
    }

    #[test]
    fn auto_reports_an_experimental_capability_structurally() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider { name: "fake" }));
        let selection = select(
            &registry,
            AccelerationPreference::Auto,
            None,
            Direction::Encode,
            Format::codec(Codec::Lz4),
            Some(Level::Fast),
        )
        .unwrap();
        assert_eq!(selection.backend, ProcessingBackend::Cpu);
        assert_eq!(
            selection.fallback_reason,
            Some(AccelerationFallbackReason::ExperimentalCapability)
        );
    }

    #[test]
    fn auto_distinguishes_unsupported_operation_dimensions() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider { name: "fake" }));
        let cases = [
            (
                Direction::Encode,
                Format::codec(Codec::Gzip),
                Some(Level::Fast),
                AccelerationFallbackReason::UnsupportedCodec,
            ),
            (
                Direction::Encode,
                Format::layered(crate::Container::Tar, Codec::Lz4),
                Some(Level::Fast),
                AccelerationFallbackReason::UnsupportedFormat,
            ),
            (
                Direction::Decode,
                Format::codec(Codec::Lz4),
                None,
                AccelerationFallbackReason::UnsupportedDirection,
            ),
            (
                Direction::Encode,
                Format::codec(Codec::Lz4),
                Some(Level::Best),
                AccelerationFallbackReason::UnsupportedLevel,
            ),
        ];

        for (direction, format, level, expected) in cases {
            let selection = select(
                &registry,
                AccelerationPreference::Auto,
                None,
                direction,
                format,
                level,
            )
            .unwrap();
            assert_eq!(selection.fallback_reason, Some(expected));
        }
    }

    #[test]
    fn auto_reports_provider_discovery_failure_structurally() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(DiscoveryFailureProvider));
        let selection = select(
            &registry,
            AccelerationPreference::Auto,
            None,
            Direction::Encode,
            Format::codec(Codec::Lz4),
            Some(Level::Fast),
        )
        .unwrap();
        assert_eq!(selection.backend, ProcessingBackend::Cpu);
        assert_eq!(
            selection.fallback_reason,
            Some(AccelerationFallbackReason::ProviderDiscoveryFailed {
                provider_id: "discovery-failure".to_owned(),
            })
        );
        assert!(selection.notice.unwrap().contains("simulated driver"));
    }

    #[test]
    fn auto_reports_provider_initialization_failure_structurally() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(InitializationFailureProvider));
        let selection = select(
            &registry,
            AccelerationPreference::Auto,
            None,
            Direction::Encode,
            Format::codec(Codec::Lz4),
            Some(Level::Fast),
        )
        .unwrap();
        assert_eq!(selection.backend, ProcessingBackend::Cpu);
        assert_eq!(
            selection.fallback_reason,
            Some(AccelerationFallbackReason::ProviderInitializationFailed {
                provider_id: "initialization-failure".to_owned(),
            })
        );
    }

    #[test]
    fn required_is_an_error_without_a_provider() {
        let result = select(
            &ProviderRegistry::new(),
            AccelerationPreference::Required,
            None,
            Direction::Decode,
            Format::codec(Codec::Zstd),
            None,
        );
        assert!(matches!(result, Err(Error::AccelerationUnavailable { .. })));
    }

    #[test]
    fn required_reports_an_explicit_target_that_does_not_exist() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(FakeProvider { name: "fake" }));
        let target = AcceleratorTarget {
            provider_id: "fake".to_owned(),
            device_id: "missing-device".to_owned(),
        };
        let error = select(
            &registry,
            AccelerationPreference::Required,
            Some(&target),
            Direction::Encode,
            Format::codec(Codec::Lz4),
            Some(Level::Fast),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("fake:missing-device"));
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
