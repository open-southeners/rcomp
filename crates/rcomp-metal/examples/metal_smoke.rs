use rcomp_metal::{SMOKE_XOR_MASK, run_smoke_test};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = [
        0,
        1,
        0x1234_5678,
        u32::MAX,
        0xdead_beef,
        0xcafe_babe,
        42,
        4_294_000_000,
    ];
    let report = run_smoke_test(&input)?;
    let expected = input.map(|value| value ^ SMOKE_XOR_MASK);

    if report.output != expected {
        return Err("Metal output did not match the expected transformation".into());
    }

    println!("Metal compute smoke test passed");
    println!("  device: {}", report.device.name);
    println!("  architecture: {}", report.device.architecture);
    println!("  registry id: {}", report.device.registry_id);
    println!(
        "  unified memory: {}",
        if report.device.has_unified_memory {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "  maximum buffer length: {} bytes",
        report.device.max_buffer_length
    );
    println!(
        "  recommended working set: {} bytes",
        report.device.recommended_working_set_size
    );
    println!("  verified elements: {}", report.output.len());

    Ok(())
}
