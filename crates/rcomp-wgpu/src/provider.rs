use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use rcomp_core::{
    AccelerationRequest, AcceleratorCapability, AcceleratorDevice, AcceleratorDeviceProperty,
    AcceleratorProvider, BlockEncoderSession, CapabilityMaturity, Codec, CompressedBlock,
    Container, Direction, Error as CoreError, Format, Level, ProviderDescriptor,
};

use crate::{Result, WgpuDeviceInfo, WgpuError};

const PROVIDER_ID: &str = "wgpu";
const LZ4_BLOCK_SIZE: usize = 64 * 1024;
const HASH_SIZE: usize = 1 << 12;
const OUTPUT_ERROR: u32 = u32::MAX;
const WORKGROUP_SIZE: u32 = 64;
const PORTABLE_BATCH_SIZE: usize = 64 * 1024 * 1024;
const APPLE_SILICON_BATCH_SIZE: usize = 256 * 1024 * 1024;
const PARAMETER_BYTES: usize = 32;
/// Target GPU time per submission: a quarter of the 2 s Windows TDR timeout.
const SUBMISSION_TIME_BUDGET: Duration = Duration::from_millis(500);
/// One workgroup: smaller submissions only add overhead.
const MIN_BLOCKS_PER_SUBMISSION: usize = WORKGROUP_SIZE as usize;
/// 8 MiB. Measured at about 150 ms on a 2-CU Radeon iGPU with incompressible
/// input, leaving room for much slower adapters before the cap adapts.
const INITIAL_BLOCKS_PER_SUBMISSION: usize = 128;

/// Portable `wgpu` acceleration provider.
///
/// Adapters are ranked deterministically by device class, with discrete GPUs
/// ahead of integrated GPUs. Every advertised capability remains experimental
/// until the project has correctness and crossover evidence for the relevant
/// backend, device, and driver.
pub struct WgpuProvider {
    _instance: wgpu::Instance,
    adapters: Vec<AdapterRecord>,
    contexts: Mutex<HashMap<String, Arc<GpuContext>>>,
}

struct AdapterRecord {
    adapter: wgpu::Adapter,
    info: WgpuDeviceInfo,
    limits: wgpu::Limits,
}

struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    preferred_batch_size: usize,
    /// Learned per device and shared by every session on it.
    submission_budget: Mutex<SubmissionBudget>,
}

struct WgpuBlockEncoder {
    context: Arc<GpuContext>,
    buffers: Option<GpuBuffers>,
}

struct GpuBuffers {
    input: wgpu::Buffer,
    input_capacity: usize,
    output: wgpu::Buffer,
    output_capacity: usize,
    sizes: wgpu::Buffer,
    sizes_capacity: usize,
    scratch: wgpu::Buffer,
    scratch_capacity: usize,
    parameters: wgpu::Buffer,
    output_readback: wgpu::Buffer,
    sizes_readback: wgpu::Buffer,
}

impl WgpuProvider {
    /// Discover compatible adapters for the platform's primary native API.
    ///
    /// # Errors
    ///
    /// Returns [`WgpuError::UnsupportedPlatform`] outside Windows, Linux, and
    /// Apple Silicon, or [`WgpuError::NoAdapter`] when no adapter satisfies the
    /// portable LZ4 kernel's baseline limits.
    pub fn new() -> Result<Self> {
        let backends = platform_backends()?;
        let mut instance_descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_descriptor.backends = backends;
        let instance = wgpu::Instance::new(instance_descriptor);
        let discovered = pollster::block_on(instance.enumerate_adapters(backends));
        let mut records = discovered
            .into_iter()
            .filter_map(|adapter| {
                let native = adapter.get_info();
                let limits = adapter.limits();
                (native.device_type != wgpu::DeviceType::Cpu && adapter_supports_kernel(&limits))
                    .then_some((adapter, native, limits))
            })
            .collect::<Vec<_>>();
        retain_first_per_pci_location(&mut records, |(_, native, _)| native);
        records.sort_by(|(_, left, _), (_, right, _)| {
            adapter_rank(left.device_type)
                .cmp(&adapter_rank(right.device_type))
                .then_with(|| left.vendor.cmp(&right.vendor))
                .then_with(|| left.device.cmp(&right.device))
                .then_with(|| left.name.cmp(&right.name))
        });

        let adapters = records
            .into_iter()
            .enumerate()
            .map(|(ordinal, (adapter, native, limits))| AdapterRecord {
                info: device_info(&native, ordinal),
                adapter,
                limits,
            })
            .collect::<Vec<_>>();
        if adapters.is_empty() {
            return Err(WgpuError::NoAdapter);
        }

        Ok(Self {
            _instance: instance,
            adapters,
            contexts: Mutex::new(HashMap::new()),
        })
    }

    /// Return information about every compatible adapter in selection order.
    #[must_use]
    pub fn device_infos(&self) -> Vec<WgpuDeviceInfo> {
        self.adapters
            .iter()
            .map(|record| record.info.clone())
            .collect()
    }

    fn adapter(&self, device_id: &str) -> Option<&AdapterRecord> {
        self.adapters
            .iter()
            .find(|record| record.info.device_id == device_id)
    }

    fn context(&self, device_id: &str) -> Result<Arc<GpuContext>> {
        if let Some(context) = self
            .contexts
            .lock()
            .map_err(|_| WgpuError::CommandExecution("GPU context cache is poisoned".to_owned()))?
            .get(device_id)
            .cloned()
        {
            return Ok(context);
        }

        let adapter = self.adapter(device_id).ok_or(WgpuError::NoAdapter)?;
        let context = Arc::new(GpuContext::new(adapter)?);
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| WgpuError::CommandExecution("GPU context cache is poisoned".to_owned()))?;
        Ok(contexts
            .entry(device_id.to_owned())
            .or_insert_with(|| Arc::clone(&context))
            .clone())
    }
}

impl AcceleratorProvider for WgpuProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID.to_owned(),
            name: "Portable GPU (wgpu)".to_owned(),
        }
    }

    fn devices(&self) -> rcomp_core::Result<Vec<AcceleratorDevice>> {
        Ok(self
            .adapters
            .iter()
            .map(|record| AcceleratorDevice {
                provider_id: PROVIDER_ID.to_owned(),
                device_id: record.info.device_id.clone(),
                name: record.info.name.clone(),
            })
            .collect())
    }

    fn device_properties(
        &self,
        device: &AcceleratorDevice,
    ) -> rcomp_core::Result<Vec<AcceleratorDeviceProperty>> {
        let record =
            self.adapter(&device.device_id)
                .ok_or_else(|| CoreError::AccelerationUnavailable {
                    reason: format!(
                        "device {} does not belong to the portable GPU provider",
                        device.device_id
                    ),
                })?;
        let info = &record.info;
        let limits = &record.limits;
        Ok([
            ("backend", info.backend.clone()),
            ("device_type", info.device_type.clone()),
            ("vendor_id", format!("0x{:04x}", info.vendor_id)),
            (
                "numeric_device_id",
                format!("0x{:04x}", info.device_id_numeric),
            ),
            ("driver", info.driver.clone()),
            ("driver_info", info.driver_info.clone()),
            (
                "max_storage_buffer_binding_size",
                limits.max_storage_buffer_binding_size.to_string(),
            ),
            ("max_buffer_size", limits.max_buffer_size.to_string()),
            (
                "preferred_batch_size",
                preferred_batch_size(info, limits)
                    .map_err(core_failure)?
                    .to_string(),
            ),
        ]
        .into_iter()
        .map(|(key, value)| AcceleratorDeviceProperty {
            key: key.to_owned(),
            value,
        })
        .collect())
    }

    fn capabilities(
        &self,
        device: &AcceleratorDevice,
    ) -> rcomp_core::Result<Vec<AcceleratorCapability>> {
        if device.provider_id != PROVIDER_ID || self.adapter(&device.device_id).is_none() {
            return Err(CoreError::AccelerationUnavailable {
                reason: format!(
                    "device {} does not belong to the portable GPU provider",
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
            device_id: device.device_id.clone(),
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
        let supported = self.adapter(&request.device_id).is_some()
            && supported_format
            && request.direction == Direction::Encode
            && request.level == Some(Level::Fast);
        if !supported {
            return Err(CoreError::AccelerationUnavailable {
                reason: format!(
                    "portable GPU does not support {} {} at {:?} level on device {}",
                    request.format, request.direction, request.level, request.device_id
                ),
            });
        }

        let context = self.context(&request.device_id).map_err(core_failure)?;
        Ok(Box::new(WgpuBlockEncoder {
            context,
            buffers: None,
        }))
    }
}

impl GpuContext {
    fn new(record: &AdapterRecord) -> Result<Self> {
        let preferred_batch_size = preferred_batch_size(&record.info, &record.limits)?;
        let required_limits = required_limits(preferred_batch_size)?;
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("rcomp portable LZ4 device"),
            required_features: wgpu::Features::empty(),
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        };
        let (device, queue) = pollster::block_on(record.adapter.request_device(&descriptor))
            .map_err(|error| WgpuError::DeviceRequest {
                adapter: record.info.name.clone(),
                reason: error.to_string(),
            })?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rcomp portable LZ4 WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lz4.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("rcomp portable LZ4 pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("rcomp_lz4_compress_blocks"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            preferred_batch_size,
            submission_budget: Mutex::new(SubmissionBudget::new(
                preferred_batch_size / LZ4_BLOCK_SIZE,
            )),
        })
    }
}

impl BlockEncoderSession for WgpuBlockEncoder {
    fn block_size(&self) -> usize {
        LZ4_BLOCK_SIZE
    }

    fn preferred_batch_size(&self) -> usize {
        self.context.preferred_batch_size
    }

    fn compress_blocks(
        &mut self,
        input: &[u8],
        cancel: &rcomp_core::CancelToken,
    ) -> rcomp_core::Result<Vec<CompressedBlock>> {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let blocks = self
            .compress_blocks_inner(input, cancel)
            .map_err(core_failure)?;
        // Native GPU work cannot be revoked safely after submission. We wait
        // for the active submission and discard the result if cancellation
        // arrived meanwhile; later submissions of the batch are never sent.
        match blocks {
            Some(blocks) if !cancel.is_cancelled() => Ok(blocks),
            _ => Err(CoreError::Cancelled),
        }
    }
}

impl WgpuBlockEncoder {
    /// Returns `None` when cancellation stopped the batch between submissions.
    fn compress_blocks_inner(
        &mut self,
        input: &[u8],
        cancel: &rcomp_core::CancelToken,
    ) -> Result<Option<Vec<CompressedBlock>>> {
        if input.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if input.len() > self.context.preferred_batch_size {
            return Err(WgpuError::InputTooLarge);
        }

        let total_size = u32::try_from(input.len()).map_err(|_| WgpuError::InputTooLarge)?;
        let block_count = input.len().div_ceil(LZ4_BLOCK_SIZE);
        let output_slot_size = lz4_compress_bound(LZ4_BLOCK_SIZE)?;
        let output_slot_stride = align_up(output_slot_size, 4)?;
        let input_capacity = align_up(input.len(), 4)?;
        let output_capacity = checked_mul(block_count, output_slot_stride)?;
        let sizes_capacity = checked_mul(block_count, size_of::<u32>())?;
        let scratch_capacity = checked_mul(checked_mul(block_count, HASH_SIZE)?, size_of::<u32>())?;
        self.ensure_buffers(
            input_capacity,
            output_capacity,
            sizes_capacity,
            scratch_capacity,
        )?;
        let buffers = self
            .buffers
            .as_ref()
            .expect("portable GPU buffers were initialized above");

        let mut padded_input = vec![0_u8; input_capacity];
        padded_input[..input.len()].copy_from_slice(input);
        self.context
            .queue
            .write_buffer(&buffers.input, 0, &padded_input);
        let layout = self.context.pipeline.get_bind_group_layout(0);
        let bind_group = self
            .context
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rcomp portable LZ4 buffers"),
                layout: &layout,
                entries: &[
                    binding(0, &buffers.input),
                    binding(1, &buffers.output),
                    binding(2, &buffers.sizes),
                    binding(3, &buffers.scratch),
                    binding(4, &buffers.parameters),
                ],
            });

        // A batch is compressed in several submissions, each a single
        // dispatch sized by the device's time budget, so no GPU packet runs
        // long enough to trip a driver watchdog (Windows TDR is 2 s). Each
        // submission is awaited before the next: the queue runs them in order
        // anyway, and the measured time steers the next submission's size.
        // The queued parameter write is applied before the submission that
        // follows it.
        let output_slot_size_u32 =
            u32::try_from(output_slot_size).map_err(|_| WgpuError::InputTooLarge)?;
        let output_slot_stride_u32 =
            u32::try_from(output_slot_stride).map_err(|_| WgpuError::InputTooLarge)?;
        let mut next_block = 0;
        while next_block < block_count {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let blocks = self
                .context
                .submission_budget
                .lock()
                .map_err(|_| {
                    WgpuError::CommandExecution("GPU submission budget is poisoned".to_owned())
                })?
                .blocks()
                .min(block_count - next_block);
            let start = u32::try_from(next_block).map_err(|_| WgpuError::InputTooLarge)?;
            let end = u32::try_from(next_block + blocks).map_err(|_| WgpuError::InputTooLarge)?;
            self.context.queue.write_buffer(
                &buffers.parameters,
                0,
                &parameter_bytes(
                    total_size,
                    output_slot_size_u32,
                    output_slot_stride_u32,
                    start,
                    end,
                ),
            );
            let mut encoder =
                self.context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("rcomp portable LZ4 command encoder"),
                    });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("rcomp portable LZ4 compute pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.context.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups((end - start).div_ceil(WORKGROUP_SIZE), 1, 1);
            }
            next_block += blocks;
            if next_block == block_count {
                encoder.copy_buffer_to_buffer(
                    &buffers.sizes,
                    0,
                    &buffers.sizes_readback,
                    0,
                    sizes_capacity as u64,
                );
            }
            let submitted = Instant::now();
            let submission = self.context.queue.submit([encoder.finish()]);
            self.context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
                .map_err(|error| WgpuError::CommandExecution(error.to_string()))?;
            let elapsed = submitted.elapsed();
            self.context
                .submission_budget
                .lock()
                .map_err(|_| {
                    WgpuError::CommandExecution("GPU submission budget is poisoned".to_owned())
                })?
                .record(blocks, elapsed);
        }

        let sizes_slice = buffers.sizes_readback.slice(..sizes_capacity as u64);
        let (sizes_sender, sizes_receiver) = mpsc::sync_channel(1);
        sizes_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sizes_sender.send(result);
        });
        let plan_result = (|| {
            self.context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .map_err(|error| WgpuError::CommandExecution(error.to_string()))?;
            sizes_receiver
                .recv()
                .map_err(|_| WgpuError::Readback("size mapping callback disconnected".to_owned()))?
                .map_err(|error| WgpuError::Readback(error.to_string()))?;
            let sizes = sizes_slice
                .get_mapped_range()
                .map_err(|error| WgpuError::Readback(error.to_string()))?;
            plan_packed_readback(input, &sizes, block_count, output_slot_size)
        })();
        buffers.sizes_readback.unmap();
        let (plans, packed_size) = plan_result?;
        if packed_size == 0 {
            return collect_packed_blocks(input, &[], &plans).map(Some);
        }

        let mut encoder =
            self.context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rcomp portable LZ4 compact readback encoder"),
                });
        for (block, plan) in plans.iter().enumerate() {
            let Some(packed_offset) = plan.packed_offset else {
                continue;
            };
            let source_offset = checked_mul(block, output_slot_stride)?;
            let copy_size = align_up(plan.compressed_size, 4)?;
            encoder.copy_buffer_to_buffer(
                &buffers.output,
                u64::try_from(source_offset).map_err(|_| WgpuError::InputTooLarge)?,
                &buffers.output_readback,
                u64::try_from(packed_offset).map_err(|_| WgpuError::InputTooLarge)?,
                u64::try_from(copy_size).map_err(|_| WgpuError::InputTooLarge)?,
            );
        }
        self.context.queue.submit([encoder.finish()]);

        let output_slice = buffers
            .output_readback
            .slice(..u64::try_from(packed_size).map_err(|_| WgpuError::InputTooLarge)?);
        let (output_sender, output_receiver) = mpsc::sync_channel(1);
        output_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = output_sender.send(result);
        });
        let result = (|| {
            self.context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .map_err(|error| WgpuError::CommandExecution(error.to_string()))?;
            output_receiver
                .recv()
                .map_err(|_| {
                    WgpuError::Readback("output mapping callback disconnected".to_owned())
                })?
                .map_err(|error| WgpuError::Readback(error.to_string()))?;
            let output = output_slice
                .get_mapped_range()
                .map_err(|error| WgpuError::Readback(error.to_string()))?;
            collect_packed_blocks(input, &output, &plans)
        })();
        buffers.output_readback.unmap();
        result.map(Some)
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

        let device = &self.context.device;
        self.buffers = Some(GpuBuffers {
            input: buffer(
                device,
                "rcomp LZ4 input",
                input_capacity,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            )?,
            input_capacity,
            output: buffer(
                device,
                "rcomp LZ4 output",
                output_capacity,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            )?,
            output_capacity,
            sizes: buffer(
                device,
                "rcomp LZ4 output sizes",
                sizes_capacity,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            )?,
            sizes_capacity,
            scratch: buffer(
                device,
                "rcomp LZ4 hash tables",
                scratch_capacity,
                wgpu::BufferUsages::STORAGE,
            )?,
            scratch_capacity,
            parameters: buffer(
                device,
                "rcomp LZ4 parameters",
                PARAMETER_BYTES,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            )?,
            output_readback: buffer(
                device,
                "rcomp LZ4 output readback",
                output_capacity,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )?,
            sizes_readback: buffer(
                device,
                "rcomp LZ4 size readback",
                sizes_capacity,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )?,
        });
        Ok(())
    }
}

fn platform_backends() -> Result<wgpu::Backends> {
    #[cfg(target_os = "windows")]
    {
        return Ok(wgpu::Backends::DX12);
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(wgpu::Backends::VULKAN);
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Ok(wgpu::Backends::METAL);
    }
    #[allow(unreachable_code)]
    Err(WgpuError::UnsupportedPlatform)
}

fn adapter_supports_kernel(limits: &wgpu::Limits) -> bool {
    limits.max_storage_buffers_per_shader_stage >= 4
        && limits.max_compute_invocations_per_workgroup >= WORKGROUP_SIZE
        && limits.max_compute_workgroup_size_x >= WORKGROUP_SIZE
        && limits.max_compute_workgroups_per_dimension >= 1
        && limits.max_storage_buffer_binding_size as usize
            >= lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap_or(usize::MAX)
        && limits.max_buffer_size as usize
            >= lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap_or(usize::MAX)
}

/// Drop adapters that repeat an earlier adapter's PCI location.
///
/// DXGI can expose one physical GPU as several adapters with distinct LUIDs
/// (observed with an NVIDIA RTX 3090 on Windows 11 over Remote Desktop). Device
/// IDs are derived from the PCI location, so the copies would share an ID and
/// only the first could ever be selected. The first enumerated adapter is
/// kept. Adapters without a PCI location get ordinal IDs and are never merged.
fn retain_first_per_pci_location<T>(
    records: &mut Vec<T>,
    native: impl Fn(&T) -> &wgpu::AdapterInfo,
) {
    let mut seen = HashSet::new();
    records.retain(|record| {
        let info = native(record);
        info.device_pci_bus_id.is_empty()
            || seen.insert((
                info.backend,
                info.vendor,
                info.device,
                info.device_pci_bus_id.clone(),
            ))
    });
}

fn adapter_rank(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

fn device_info(native: &wgpu::AdapterInfo, ordinal: usize) -> WgpuDeviceInfo {
    let backend = backend_name(native.backend).to_owned();
    let suffix = if native.device_pci_bus_id.is_empty() {
        ordinal.to_string()
    } else {
        native.device_pci_bus_id.replace(':', "-")
    };
    WgpuDeviceInfo {
        device_id: format!(
            "{backend}:{:04x}:{:04x}:{suffix}",
            native.vendor, native.device
        ),
        name: native.name.clone(),
        backend,
        vendor_id: native.vendor,
        device_id_numeric: native.device,
        device_type: format!("{:?}", native.device_type).to_ascii_lowercase(),
        driver: native.driver.clone(),
        driver_info: native.driver_info.clone(),
    }
}

fn backend_name(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "webgpu",
        wgpu::Backend::Noop => "noop",
    }
}

fn preferred_batch_size(info: &WgpuDeviceInfo, limits: &wgpu::Limits) -> Result<usize> {
    let binding_limit =
        usize::try_from(limits.max_storage_buffer_binding_size).unwrap_or(usize::MAX);
    let buffer_limit = usize::try_from(limits.max_buffer_size).unwrap_or(usize::MAX);
    let per_buffer_limit = binding_limit.min(buffer_limit);
    let output_slot_stride = align_up(lz4_compress_bound(LZ4_BLOCK_SIZE)?, 4)?;
    let max_blocks_by_output = per_buffer_limit / output_slot_stride;
    let max_blocks_by_input = per_buffer_limit / LZ4_BLOCK_SIZE;
    let max_blocks_by_scratch = per_buffer_limit / (HASH_SIZE * size_of::<u32>());
    let max_blocks_by_dispatch = usize::try_from(limits.max_compute_workgroups_per_dimension)
        .unwrap_or(usize::MAX)
        .saturating_mul(WORKGROUP_SIZE as usize);
    let max_blocks = max_blocks_by_output
        .min(max_blocks_by_input)
        .min(max_blocks_by_scratch)
        .min(max_blocks_by_dispatch)
        .min(batch_ceiling(info) / LZ4_BLOCK_SIZE);
    if max_blocks == 0 {
        Err(WgpuError::InputTooLarge)
    } else {
        Ok(max_blocks * LZ4_BLOCK_SIZE)
    }
}

fn batch_ceiling(info: &WgpuDeviceInfo) -> usize {
    // The Metal backend is built only for Apple Silicon. Local M3 Max
    // qualification shows that 64 MiB is dispatch-bound, while 256 MiB keeps
    // the total live buffer set bounded and materially reduces submissions.
    // Unqualified DX12/Vulkan devices retain the conservative portable cap.
    if info.backend == "metal" {
        APPLE_SILICON_BATCH_SIZE
    } else {
        PORTABLE_BATCH_SIZE
    }
}

fn required_limits(batch_size: usize) -> Result<wgpu::Limits> {
    let block_count = batch_size.div_ceil(LZ4_BLOCK_SIZE);
    let output_slot_stride = align_up(lz4_compress_bound(LZ4_BLOCK_SIZE)?, 4)?;
    let output_size = checked_mul(block_count, output_slot_stride)?;
    let scratch_size = checked_mul(checked_mul(block_count, HASH_SIZE)?, size_of::<u32>())?;
    let largest_buffer = batch_size.max(output_size).max(scratch_size);
    let largest_buffer = u64::try_from(largest_buffer).map_err(|_| WgpuError::InputTooLarge)?;
    let defaults = wgpu::Limits::default();
    Ok(wgpu::Limits {
        max_storage_buffer_binding_size: defaults
            .max_storage_buffer_binding_size
            .max(largest_buffer),
        max_buffer_size: defaults.max_buffer_size.max(largest_buffer),
        ..defaults
    })
}

fn buffer(
    device: &wgpu::Device,
    label: &'static str,
    size: usize,
    usage: wgpu::BufferUsages,
) -> Result<wgpu::Buffer> {
    let size = u64::try_from(size).map_err(|_| WgpuError::InputTooLarge)?;
    Ok(device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    }))
}

fn binding(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn parameter_bytes(
    total_size: u32,
    output_slot_size: u32,
    output_slot_stride: u32,
    block_offset: u32,
    block_end: u32,
) -> [u8; PARAMETER_BYTES] {
    let values = [
        total_size,
        LZ4_BLOCK_SIZE as u32,
        output_slot_size,
        output_slot_stride,
        block_end,
        block_offset,
        0,
        0,
    ];
    let mut bytes = [0_u8; PARAMETER_BYTES];
    for (index, value) in values.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Adaptive cap on the blocks compressed by one GPU submission.
///
/// A dispatch takes as long as its slowest block when the GPU runs every
/// block at once (large discrete GPUs), and grows with the block count when it
/// cannot (small integrated GPUs). A fixed split therefore either costs wide
/// GPUs a multiple of their batch time or leaves narrow GPUs near the 2 s
/// Windows TDR limit: on a Ryzen 7800X3D iGPU one 1,024-block submission of
/// incompressible input took 1.36 s, while splitting an RTX 3090's realistic
/// batches into 256-block submissions made them 3x slower.
///
/// The cap starts small, doubles after a full-size submission that finished
/// within half the budget (so the next one should still fit even if time grows
/// linearly with blocks), and halves after any submission over budget.
#[derive(Debug)]
struct SubmissionBudget {
    blocks: usize,
    max_blocks: usize,
}

impl SubmissionBudget {
    fn new(max_blocks: usize) -> Self {
        let max_blocks = max_blocks.max(MIN_BLOCKS_PER_SUBMISSION);
        Self {
            blocks: INITIAL_BLOCKS_PER_SUBMISSION.clamp(MIN_BLOCKS_PER_SUBMISSION, max_blocks),
            max_blocks,
        }
    }

    fn blocks(&self) -> usize {
        self.blocks
    }

    fn record(&mut self, blocks: usize, elapsed: Duration) {
        if elapsed > SUBMISSION_TIME_BUDGET {
            self.blocks = (blocks / 2).max(MIN_BLOCKS_PER_SUBMISSION);
        } else if blocks >= self.blocks && elapsed * 2 <= SUBMISSION_TIME_BUDGET {
            // Only a full-size submission says anything about a larger one; a
            // batch's short tail does not.
            self.blocks = (self.blocks * 2).min(self.max_blocks);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockReadbackPlan {
    compressed_size: usize,
    packed_offset: Option<usize>,
}

fn plan_packed_readback(
    input: &[u8],
    sizes: &[u8],
    block_count: usize,
    output_slot_size: usize,
) -> Result<(Vec<BlockReadbackPlan>, usize)> {
    let mut plans = Vec::with_capacity(block_count);
    let mut packed_size = 0_usize;
    for block in 0..block_count {
        let size_start = block * size_of::<u32>();
        let size_end = size_start + size_of::<u32>();
        let compressed_size = u32::from_le_bytes(
            sizes
                .get(size_start..size_end)
                .ok_or_else(|| invalid_output(block, "size metadata is truncated"))?
                .try_into()
                .expect("the checked metadata range is four bytes"),
        );
        if compressed_size == OUTPUT_ERROR {
            return Err(invalid_output(
                block,
                "the kernel reported an output-capacity overflow",
            ));
        }
        let compressed_size = compressed_size as usize;
        if compressed_size > output_slot_size {
            return Err(invalid_output(
                block,
                format!("reported {compressed_size} bytes for a {output_slot_size}-byte slot"),
            ));
        }
        let input_start = checked_mul(block, LZ4_BLOCK_SIZE)?;
        let input_end = (input_start + LZ4_BLOCK_SIZE).min(input.len());
        let input_size = input_end - input_start;
        if compressed_size == 0 && input_size != 0 {
            return Err(invalid_output(block, "reported an empty compressed block"));
        }
        let packed_offset = if compressed_size < input_size {
            let offset = packed_size;
            packed_size = packed_size
                .checked_add(align_up(compressed_size, 4)?)
                .ok_or(WgpuError::InputTooLarge)?;
            Some(offset)
        } else {
            None
        };
        plans.push(BlockReadbackPlan {
            compressed_size,
            packed_offset,
        });
    }
    Ok((plans, packed_size))
}

fn collect_packed_blocks(
    input: &[u8],
    output: &[u8],
    plans: &[BlockReadbackPlan],
) -> Result<Vec<CompressedBlock>> {
    let mut blocks = Vec::with_capacity(plans.len());
    for (block, plan) in plans.iter().enumerate() {
        let input_start = checked_mul(block, LZ4_BLOCK_SIZE)?;
        let input_end = (input_start + LZ4_BLOCK_SIZE).min(input.len());
        let original = &input[input_start..input_end];
        let bytes = if let Some(packed_offset) = plan.packed_offset {
            let payload_end = packed_offset
                .checked_add(plan.compressed_size)
                .ok_or(WgpuError::InputTooLarge)?;
            let payload = output.get(packed_offset..payload_end).ok_or_else(|| {
                invalid_output(
                    block,
                    "the compressed payload lies outside the packed readback buffer",
                )
            })?;

            #[cfg(debug_assertions)]
            validate_lz4_block(payload, original)
                .map_err(|detail| WgpuError::InvalidOutput { block, detail })?;
            payload.to_vec()
        } else {
            vec![0; original.len()]
        };

        blocks.push(CompressedBlock {
            input_size: original.len(),
            bytes,
        });
    }
    Ok(blocks)
}

fn invalid_output(block: usize, detail: impl Into<String>) -> WgpuError {
    WgpuError::InvalidOutput {
        block,
        detail: detail.into(),
    }
}

fn align_up(value: usize, alignment: usize) -> Result<usize> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or(WgpuError::InputTooLarge)
}

fn checked_mul(left: usize, right: usize) -> Result<usize> {
    left.checked_mul(right).ok_or(WgpuError::InputTooLarge)
}

fn lz4_compress_bound(input_size: usize) -> Result<usize> {
    input_size
        .checked_add(input_size / 255)
        .and_then(|value| value.checked_add(16))
        .ok_or(WgpuError::InputTooLarge)
}

fn core_failure(error: WgpuError) -> CoreError {
    CoreError::AccelerationFailed {
        provider: PROVIDER_ID.to_owned(),
        reason: error.to_string(),
    }
}

#[cfg(debug_assertions)]
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
            decoded.push(decoded[decoded.len() - offset]);
        }
    }
    if decoded == expected {
        Ok(())
    } else {
        Err(format!(
            "decoded block differs from the original ({} bytes decoded, {} expected)",
            decoded.len(),
            expected.len()
        ))
    }
}

#[cfg(debug_assertions)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_device(backend: &str) -> WgpuDeviceInfo {
        WgpuDeviceInfo {
            device_id: format!("{backend}:0000:0000:0"),
            name: "Test GPU".to_owned(),
            backend: backend.to_owned(),
            vendor_id: 0,
            device_id_numeric: 0,
            device_type: "integratedgpu".to_owned(),
            driver: String::new(),
            driver_info: String::new(),
        }
    }

    fn dx12_adapter(name: &str, device: u32, pci_bus_id: &str) -> wgpu::AdapterInfo {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, wgpu::Backend::Dx12);
        info.name = name.to_owned();
        info.vendor = 0x10de;
        info.device = device;
        info.device_pci_bus_id = pci_bus_id.to_owned();
        info
    }

    #[test]
    fn one_physical_adapter_enumerated_twice_gets_one_device_id() {
        // DXGI on Windows 11 listed an RTX 3090 twice (distinct LUIDs, same
        // PCI location); both copies mapped to the same device ID.
        let mut adapters = vec![
            dx12_adapter("first", 0x2204, "0000:01:00.0"),
            dx12_adapter("integrated", 0x164e, "0000:73:00.0"),
            dx12_adapter("second", 0x2204, "0000:01:00.0"),
        ];
        retain_first_per_pci_location(&mut adapters, |info| info);
        let names = adapters
            .iter()
            .map(|info| info.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["first", "integrated"]);

        let ids = adapters
            .iter()
            .enumerate()
            .map(|(ordinal, info)| device_info(info, ordinal).device_id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), adapters.len());
    }

    #[test]
    fn identical_models_at_distinct_pci_locations_stay_separate() {
        let mut adapters = vec![
            dx12_adapter("slot 1", 0x2204, "0000:01:00.0"),
            dx12_adapter("slot 2", 0x2204, "0000:02:00.0"),
        ];
        retain_first_per_pci_location(&mut adapters, |info| info);
        assert_eq!(adapters.len(), 2);
    }

    #[test]
    fn adapters_without_a_pci_location_are_never_merged() {
        let mut adapters = vec![dx12_adapter("a", 0x2204, ""), dx12_adapter("b", 0x2204, "")];
        retain_first_per_pci_location(&mut adapters, |info| info);
        assert_eq!(adapters.len(), 2);
    }

    /// Compress `batches` full 1,024-block batches with a simulated device and
    /// return the cap afterwards plus every submission's (blocks, time).
    fn simulate(
        batches: usize,
        device_time: impl Fn(usize) -> Duration,
    ) -> (usize, Vec<(usize, Duration)>) {
        let block_count = 1024;
        let mut budget = SubmissionBudget::new(block_count);
        let mut submissions = Vec::new();
        for _ in 0..batches {
            let mut next = 0;
            while next < block_count {
                let blocks = budget.blocks().min(block_count - next);
                let elapsed = device_time(blocks);
                budget.record(blocks, elapsed);
                submissions.push((blocks, elapsed));
                next += blocks;
            }
        }
        (budget.blocks(), submissions)
    }

    #[test]
    fn narrow_gpu_submissions_stay_within_the_tdr_budget() {
        // Measured on a Ryzen 7800X3D iGPU with incompressible input: one
        // 1,024-block dispatch took 1.36 s, about 1.33 ms per block.
        let (blocks, submissions) =
            simulate(3, |blocks| Duration::from_micros(1_330 * blocks as u64));
        assert_eq!(blocks, 256);
        assert!(
            submissions
                .iter()
                .all(|(_, elapsed)| *elapsed <= SUBMISSION_TIME_BUDGET),
            "{submissions:?}"
        );
    }

    #[test]
    fn wide_gpu_converges_to_whole_batches() {
        // On an RTX 3090 every block of a batch runs at once, so a dispatch
        // takes about as long as its slowest block whatever its size.
        let (blocks, submissions) = simulate(2, |_| Duration::from_millis(230));
        assert_eq!(blocks, 1024);
        assert_eq!(
            submissions.last(),
            Some(&(1024, Duration::from_millis(230)))
        );
    }

    #[test]
    fn submission_budget_halves_after_an_overrun_but_not_below_one_workgroup() {
        let mut budget = SubmissionBudget::new(1024);
        budget.record(128, SUBMISSION_TIME_BUDGET + Duration::from_millis(1));
        assert_eq!(budget.blocks(), MIN_BLOCKS_PER_SUBMISSION);
        budget.record(64, Duration::from_secs(5));
        assert_eq!(budget.blocks(), MIN_BLOCKS_PER_SUBMISSION);
    }

    #[test]
    fn a_short_tail_submission_does_not_grow_the_budget() {
        let mut budget = SubmissionBudget::new(1024);
        budget.record(3, Duration::from_millis(1));
        assert_eq!(budget.blocks(), INITIAL_BLOCKS_PER_SUBMISSION);
    }

    #[test]
    fn submission_budget_never_exceeds_the_batch() {
        let mut budget = SubmissionBudget::new(100);
        assert_eq!(budget.blocks(), 100);
        budget.record(100, Duration::from_millis(1));
        assert_eq!(budget.blocks(), 100);
    }

    #[test]
    fn output_slots_are_word_aligned() {
        let bound = lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap();
        let stride = align_up(bound, 4).unwrap();
        assert!(stride >= bound);
        assert_eq!(stride % 4, 0);
    }

    #[test]
    fn apple_batch_policy_is_larger_than_the_unqualified_portable_baseline() {
        assert_eq!(batch_ceiling(&test_device("metal")), 256 * 1024 * 1024);
        assert_eq!(batch_ceiling(&test_device("vulkan")), 64 * 1024 * 1024);
        assert_eq!(batch_ceiling(&test_device("dx12")), 64 * 1024 * 1024);
    }

    #[test]
    fn requested_limits_cover_every_buffer_for_the_selected_batch() {
        let batch_size = APPLE_SILICON_BATCH_SIZE;
        let limits = required_limits(batch_size).unwrap();
        let blocks = batch_size / LZ4_BLOCK_SIZE;
        let output_size =
            blocks * align_up(lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap(), 4).unwrap();
        assert!(limits.max_storage_buffer_binding_size >= output_size as u64);
        assert!(limits.max_buffer_size >= output_size as u64);
    }

    #[test]
    fn parameter_layout_matches_wgsl_uniform() {
        let bytes = parameter_bytes(7, 11, 12, 3, 5);
        assert_eq!(bytes.len(), 32);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 7);
        // `block_end`, then `block_offset`, as declared in the WGSL struct.
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 3);
    }

    #[test]
    fn canonical_wgsl_parses_and_validates_without_optional_capabilities() {
        let module = naga::front::wgsl::parse_str(include_str!("shaders/lz4.wgsl"))
            .expect("the canonical LZ4 WGSL should parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("the canonical LZ4 WGSL should use only baseline capabilities");
    }

    #[test]
    fn host_collection_validates_and_preserves_compressed_blocks() {
        let expected = vec![b'a'; 20];
        // One literal `a`, then an offset-1 match for the remaining 19 bytes.
        let compressed = [0x1f, b'a', 0x01, 0x00, 0x00];
        let slot_size = lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap();
        let sizes = (compressed.len() as u32).to_le_bytes();

        let (plans, packed_size) = plan_packed_readback(&expected, &sizes, 1, slot_size).unwrap();
        assert_eq!(packed_size, align_up(compressed.len(), 4).unwrap());
        let blocks = collect_packed_blocks(&expected, &compressed, &plans).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].input_size, expected.len());
        assert_eq!(blocks[0].bytes, compressed);
    }

    #[test]
    fn host_collection_rejects_out_of_range_gpu_sizes() {
        let slot_size = lz4_compress_bound(LZ4_BLOCK_SIZE).unwrap();
        let sizes = u32::try_from(slot_size + 1).unwrap().to_le_bytes();

        let error = plan_packed_readback(b"input", &sizes, 1, slot_size).unwrap_err();
        assert!(matches!(error, WgpuError::InvalidOutput { block: 0, .. }));
    }

    #[test]
    fn stored_blocks_do_not_consume_readback_space() {
        let input = b"incompressible";
        let sizes = (input.len() as u32).to_le_bytes();
        let (plans, packed_size) = plan_packed_readback(input, &sizes, 1, LZ4_BLOCK_SIZE).unwrap();
        assert_eq!(packed_size, 0);
        let blocks = collect_packed_blocks(input, &[], &plans).unwrap();
        assert_eq!(blocks[0].input_size, input.len());
        assert_eq!(blocks[0].bytes.len(), input.len());
    }
}
