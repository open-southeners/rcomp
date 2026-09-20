use std::{io::Read, sync::Arc};

use rcomp_core::{
    AccelerationRequest, AcceleratorProvider, CapabilityMaturity, Codec, Container, Direction,
    Format, Level, ProviderRegistry,
};
use rcomp_metal::{
    MetalProvider, SMOKE_XOR_MASK, compress_lz4_frame, default_device_info, run_smoke_test,
};

#[test]
#[ignore = "requires a Metal-capable macOS runner"]
fn native_compute_dispatch_round_trips_verified_data() {
    let input = [0, 1, 2, 3, 0x1234_5678, u32::MAX];
    let info = default_device_info().expect("a Metal device should be available");
    let report = run_smoke_test(&input).expect("the Metal compute dispatch should succeed");

    assert_eq!(report.device.registry_id, info.registry_id);
    assert_eq!(report.output, input.map(|value| value ^ SMOKE_XOR_MASK));
}

#[test]
#[ignore = "requires a Metal-capable macOS runner"]
fn metal_lz4_frame_round_trips_boundary_and_incompressible_data() {
    let mut input = Vec::new();
    let mut random_state = 0x1234_5678_u32;
    for index in 0..(3 * 64 * 1024 + 17) {
        let value = if index < 2 * 64 * 1024 {
            b"metal-lz4-pattern"[index % b"metal-lz4-pattern".len()]
        } else {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 17;
            random_state ^= random_state << 5;
            random_state as u8
        };
        input.push(value);
    }

    let report = compress_lz4_frame(&input).expect("Metal LZ4 compression should succeed");
    let mut decoder = lz4::Decoder::new(report.frame.as_slice()).expect("valid LZ4 Frame header");
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .expect("the LZ4 Frame should decode");

    assert_eq!(decoded, input);
    assert_eq!(report.block_count, 4);
    assert_eq!(
        report.compressed_block_count + report.stored_block_count,
        report.block_count
    );
    assert!(report.compressed_block_count >= 2);
    assert!(report.stored_block_count >= 1);
}

#[test]
#[ignore = "requires a Metal-capable macOS runner"]
fn metal_provider_registers_and_opens_a_persistent_block_session() {
    let provider = Arc::new(MetalProvider::new().expect("Metal provider should initialize"));
    let device_id = provider.device_info().registry_id.to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(provider.clone());

    let capabilities = registry
        .capabilities()
        .expect("Metal capability discovery should succeed");
    assert_eq!(capabilities.len(), 2);
    assert!(capabilities.iter().all(|capability| {
        capability.maturity == CapabilityMaturity::Experimental && capability.codec == Codec::Lz4
    }));
    assert!(
        capabilities
            .iter()
            .any(|capability| { capability.format == Format::layered(Container::Tar, Codec::Lz4) })
    );

    let request = AccelerationRequest {
        device_id,
        format: Format::codec(Codec::Lz4),
        direction: Direction::Encode,
        level: Some(Level::Fast),
    };
    let mut session = provider
        .open_block_encoder(&request)
        .expect("the exact experimental capability should open");
    let input = vec![b'a'; 2 * session.block_size() + 17];
    let first = session
        .compress_blocks(&input)
        .expect("first batch should compress");
    let second = session
        .compress_blocks(&input[..session.block_size()])
        .expect("the compiled session should be reusable");
    drop(session);
    let mut second_session = provider
        .open_block_encoder(&request)
        .expect("a second session should reuse the immutable Metal context");
    let from_second_session = second_session
        .compress_blocks(&input[..second_session.block_size()])
        .expect("the second session should execute independently");

    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 1);
    assert_eq!(from_second_session.len(), 1);
    assert_eq!(
        first.iter().map(|block| block.input_size).sum::<usize>(),
        input.len()
    );
}
