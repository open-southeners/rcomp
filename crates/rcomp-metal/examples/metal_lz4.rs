use std::{
    env, fs,
    io::{Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use rcomp_metal::compress_lz4_frame;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input_path = arguments.next().map(PathBuf::from);
    let output_path = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err("usage: metal-lz4 [input [output.lz4]]".into());
    }

    let input = match input_path.as_deref() {
        Some(path) => fs::read(path)?,
        None => sample_input(),
    };

    let cold_started = Instant::now();
    let cold_report = compress_lz4_frame(&input)?;
    let cold_metal_elapsed = cold_started.elapsed();
    let cold_output_size = cold_report.output_size;
    drop(cold_report);

    let warm_started = Instant::now();
    let report = compress_lz4_frame(&input)?;
    let warm_metal_elapsed = warm_started.elapsed();

    let cpu_started = Instant::now();
    let mut cpu_encoder = lz4::EncoderBuilder::new().level(1).build(Vec::new())?;
    cpu_encoder.write_all(&input)?;
    let (cpu_frame, cpu_result) = cpu_encoder.finish();
    cpu_result?;
    let cpu_elapsed = cpu_started.elapsed();

    let mut decoder = lz4::Decoder::new(report.frame.as_slice())?;
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded)?;
    if decoded != input {
        return Err("the generated LZ4 Frame did not round-trip".into());
    }

    if let Some(path) = output_path {
        fs::write(&path, &report.frame)?;
        println!("wrote {}", path.display());
    }

    println!("Metal LZ4 feasibility run passed");
    println!("  device: {}", report.device.name);
    println!("  input: {} bytes", report.input_size);
    println!("  output: {} bytes", report.output_size);
    println!(
        "  ratio: {:.3}",
        ratio(report.output_size, report.input_size)
    );
    println!(
        "  blocks: {} compressed, {} stored",
        report.compressed_block_count, report.stored_block_count
    );
    println!("  cold Metal output: {cold_output_size} bytes");
    println!("  cold Metal elapsed: {:.3?}", cold_metal_elapsed);
    println!(
        "  cold Metal end-to-end throughput: {:.2} MiB/s",
        throughput_mib(report.input_size, cold_metal_elapsed)
    );
    println!("  warm Metal elapsed: {:.3?}", warm_metal_elapsed);
    println!(
        "  warm Metal end-to-end throughput: {:.2} MiB/s",
        throughput_mib(report.input_size, warm_metal_elapsed)
    );
    println!("  CPU Fast output: {} bytes", cpu_frame.len());
    println!("  CPU Fast elapsed: {:.3?}", cpu_elapsed);
    println!(
        "  CPU Fast throughput: {:.2} MiB/s",
        throughput_mib(input.len(), cpu_elapsed)
    );
    println!("  decoder verification: passed");

    Ok(())
}

fn sample_input() -> Vec<u8> {
    const MIB: usize = 1024 * 1024;
    let pattern = b"rcomp Metal LZ4 feasibility data: abcdefghijklmnopqrstuvwxyz\n";
    let mut input = Vec::with_capacity(16 * MIB);
    while input.len() < 16 * MIB {
        input.extend_from_slice(pattern);
    }
    input.truncate(16 * MIB);
    input
}

fn ratio(output: usize, input: usize) -> f64 {
    if input == 0 {
        0.0
    } else {
        output as f64 / input as f64
    }
}

fn throughput_mib(bytes: usize, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        bytes as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
    }
}
