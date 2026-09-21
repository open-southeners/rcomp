use std::{fs, io::Read, sync::Arc};

use rcomp_core::{
    AccelerationPreference, AccelerationRequest, AcceleratorProvider, Codec, CompressOptions,
    Direction, Engine, Format, Level, ProcessingBackend, ProviderRegistry,
};
use rcomp_wgpu::WgpuProvider;

#[test]
#[ignore = "requires a compatible native GPU adapter and driver"]
fn portable_provider_round_trips_lz4_through_core_framing() {
    let provider = Arc::new(WgpuProvider::new().expect("a compatible GPU adapter should exist"));
    let devices = provider
        .devices()
        .expect("portable adapter discovery should succeed");
    let selected = devices.first().expect("at least one compatible adapter");
    let capabilities = provider
        .capabilities(selected)
        .expect("capability discovery should succeed");
    assert_eq!(capabilities.len(), 2);

    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    let engine = Engine::with_registry(registry);
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
    let selected = provider.devices().unwrap().remove(0);
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
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    let engine = Engine::with_registry(registry);
    let temporary = tempfile::tempdir().unwrap();
    let input = patterned(input_size);
    round_trip(&engine, temporary.path(), 0, &selected.device_id, &input);
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
