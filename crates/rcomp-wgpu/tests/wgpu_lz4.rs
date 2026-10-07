use std::{env, fs, io::Read, sync::Arc};

use rcomp_core::{
    AccelerationPreference, AccelerationRequest, AcceleratorDevice, AcceleratorProvider,
    AcceleratorTarget, Codec, CompressOptions, Direction, Engine, Format, Level, ProcessingBackend,
    ProviderRegistry,
};
use rcomp_wgpu::WgpuProvider;

/// Set to an exact device ID from `rcomp hardware` to qualify an adapter other
/// than the first-ranked one.
const DEVICE_VARIABLE: &str = "RCOMP_WGPU_DEVICE";

#[test]
#[ignore = "requires a compatible native GPU adapter and driver"]
fn portable_provider_round_trips_lz4_through_core_framing() {
    let provider = Arc::new(WgpuProvider::new().expect("a compatible GPU adapter should exist"));
    let selected = select_device(&provider);
    let capabilities = provider
        .capabilities(&selected)
        .expect("capability discovery should succeed");
    assert_eq!(capabilities.len(), 2);

    let engine = targeted_engine(provider, &selected);
    let temporary = tempfile::tempdir().unwrap();
    let cases = [
        Vec::new(),
        vec![0x41],
        patterned(64 * 1024 - 1),
        patterned(64 * 1024),
        patterned(64 * 1024 + 1),
        pseudo_random(3 * 64 * 1024 + 17),
    ];
    for (index, input) in cases.into_iter().enumerate() {
        round_trip(
            &engine,
            temporary.path(),
            index,
            &selected.device_id,
            &input,
        );
    }
}

#[test]
#[ignore = "requires a compatible native GPU adapter and substantial GPU memory"]
fn portable_provider_round_trips_across_multiple_batches() {
    let provider = Arc::new(WgpuProvider::new().expect("a compatible GPU adapter should exist"));
    let selected = select_device(&provider);
    let session = provider
        .open_block_encoder(&AccelerationRequest {
            device_id: selected.device_id.clone(),
            format: Format::codec(Codec::Lz4),
            direction: Direction::Encode,
            level: Some(Level::Fast),
        })
        .unwrap();
    let input_size = session.preferred_batch_size() + 17;
    drop(session);
    let engine = targeted_engine(provider, &selected);
    let temporary = tempfile::tempdir().unwrap();
    let input = patterned(input_size);
    round_trip(&engine, temporary.path(), 0, &selected.device_id, &input);
}

/// Pick the adapter named by [`DEVICE_VARIABLE`], or the first-ranked one.
/// A named adapter that is absent fails the test instead of falling back.
fn select_device(provider: &WgpuProvider) -> AcceleratorDevice {
    let devices = provider
        .devices()
        .expect("portable adapter discovery should succeed");
    let selected = match env::var(DEVICE_VARIABLE) {
        // Accept the ID exactly as `rcomp hardware` prints it, with or without
        // the `wgpu:` provider prefix.
        Ok(device_id) => devices
            .iter()
            .find(|device| {
                device.device_id == device_id.strip_prefix("wgpu:").unwrap_or(&device_id)
            })
            .unwrap_or_else(|| {
                let available = devices
                    .iter()
                    .map(|device| device.device_id.as_str())
                    .collect::<Vec<_>>();
                panic!("{DEVICE_VARIABLE}={device_id} is not one of {available:?}")
            })
            .clone(),
        Err(_) => devices
            .first()
            .expect("at least one compatible adapter")
            .clone(),
    };
    println!("qualifying {} ({})", selected.device_id, selected.name);
    selected
}

fn targeted_engine(provider: Arc<WgpuProvider>, device: &AcceleratorDevice) -> Engine {
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    Engine::with_registry(registry).with_accelerator_target(AcceleratorTarget {
        provider_id: device.provider_id.clone(),
        device_id: device.device_id.clone(),
    })
}

fn round_trip(
    engine: &Engine,
    directory: &std::path::Path,
    index: usize,
    device_id: &str,
    input: &[u8],
) {
    let input_path = directory.join(format!("input-{index}.bin"));
    let output_path = directory.join(format!("output-{index}.lz4"));
    fs::write(&input_path, input).unwrap();
    let report = engine
        .compress(
            &input_path,
            &output_path,
            &CompressOptions {
                level: Level::Fast,
                acceleration: AccelerationPreference::Required,
                ..Default::default()
            },
            |_| {},
        )
        .expect("portable GPU compression should succeed");
    assert_eq!(
        report.backend,
        ProcessingBackend::Accelerator(format!("wgpu:{device_id}"))
    );

    let mut decoder = lz4::Decoder::new(fs::File::open(output_path).unwrap()).unwrap();
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

fn patterned(size: usize) -> Vec<u8> {
    let pattern = b"portable-wgsl-lz4-pattern-";
    pattern.iter().copied().cycle().take(size).collect()
}

fn pseudo_random(size: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    std::iter::repeat_with(|| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as u8
    })
    .take(size)
    .collect()
}
