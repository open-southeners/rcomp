use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use rcomp_core::{
    AccelerationPreference, AcceleratorTarget, CompressOptions, Engine, Level, ProviderRegistry,
};
use rcomp_wgpu::WgpuProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input_argument = arguments.next().map(PathBuf::from);
    let output_argument = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err("usage: wgpu-lz4 [input [output.lz4]]".into());
    }

    let temporary = tempfile::tempdir()?;
    let input_path = match input_argument {
        Some(path) => path,
        None => {
            let path = temporary.path().join("sample.bin");
            fs::write(&path, sample_input())?;
            path
        }
    };
    let gpu_output =
        output_argument.unwrap_or_else(|| temporary.path().join("portable-gpu-output.lz4"));
    let cpu_output = temporary.path().join("cpu-output.lz4");
    let input_size = fs::metadata(&input_path)?.len() as usize;

    // Optional qualification knobs: an exact device ID from `rcomp hardware`
    // and the number of warm GPU and CPU samples to take the median of.
    let requested_device = env::var("RCOMP_WGPU_DEVICE").ok();
    let samples = match env::var("RCOMP_WGPU_SAMPLES") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|samples| *samples > 0)
            .ok_or("RCOMP_WGPU_SAMPLES must be a positive integer")?,
        Err(_) => 1,
    };

    let provider = Arc::new(WgpuProvider::new()?);
    let infos = provider.device_infos();
    let info = match &requested_device {
        Some(device_id) => infos
            .iter()
            .find(|info| info.device_id == device_id.strip_prefix("wgpu:").unwrap_or(device_id))
            .cloned()
            .ok_or_else(|| format!("RCOMP_WGPU_DEVICE={device_id} is not a compatible adapter"))?,
        None => infos
            .into_iter()
            .next()
            .ok_or("no compatible portable GPU")?,
    };
    let target = AcceleratorTarget {
        provider_id: "wgpu".to_owned(),
        device_id: info.device_id.clone(),
    };
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    let gpu_engine = Engine::with_registry(registry).with_accelerator_target(target);
    let gpu_options = CompressOptions {
        level: Level::Fast,
        overwrite: true,
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };

    let cold_started = Instant::now();
    let cold_report = gpu_engine.compress(&input_path, &gpu_output, &gpu_options, |_| {})?;
    let cold_elapsed = cold_started.elapsed();
    verify(&gpu_output, &input_path)?;
    let mut warm_samples = Vec::with_capacity(samples);
    let mut warm_report = None;
    for _ in 0..samples {
        let warm_started = Instant::now();
        warm_report = Some(gpu_engine.compress(&input_path, &gpu_output, &gpu_options, |_| {})?);
        warm_samples.push(warm_started.elapsed());
        verify(&gpu_output, &input_path)?;
    }

    let cpu_engine = Engine::new();
    let cpu_options = CompressOptions {
        level: Level::Fast,
        overwrite: true,
        ..Default::default()
    };
    let mut cpu_samples = Vec::with_capacity(samples);
    let mut cpu_report = None;
    for _ in 0..samples {
        let cpu_started = Instant::now();
        cpu_report = Some(cpu_engine.compress(&input_path, &cpu_output, &cpu_options, |_| {})?);
        cpu_samples.push(cpu_started.elapsed());
        verify(&cpu_output, &input_path)?;
    }
    let warm_report = warm_report.expect("at least one warm GPU sample ran");
    let cpu_report = cpu_report.expect("at least one CPU sample ran");
    let warm_elapsed = median(&warm_samples);
    let cpu_elapsed = median(&cpu_samples);

    println!("Portable GPU LZ4 comparison passed");
    println!("  device: {}", info.name);
    println!("  backend: {}", info.backend);
    println!("  device id: {}", info.device_id);
    println!("  driver: {} {}", info.driver, info.driver_info);
    println!("  input: {input_size} bytes");
    println!("  warm samples (median reported): {samples}");
    if samples > 1 {
        println!("  warm portable GPU samples: {warm_samples:.3?}");
        println!("  CPU Fast samples: {cpu_samples:.3?}");
    }
    print_result(
        "cold portable GPU",
        input_size,
        cold_report.output_bytes as usize,
        cold_elapsed,
    );
    print_result(
        "warm portable GPU",
        input_size,
        warm_report.output_bytes as usize,
        warm_elapsed,
    );
    print_result(
        "CPU Fast",
        input_size,
        cpu_report.output_bytes as usize,
        cpu_elapsed,
    );
    println!("  decoder verification: passed");
    if let Some(parent) = gpu_output.parent() {
        println!(
            "  GPU output: {}",
            parent.join(gpu_output.file_name().unwrap()).display()
        );
    }

    Ok(())
}

fn verify(frame: &Path, original: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = lz4::Decoder::new(fs::File::open(frame)?)?;
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded)?;
    if decoded != fs::read(original)? {
        return Err(format!("{} did not round-trip", frame.display()).into());
    }
    Ok(())
}

/// Returns the middle sample (upper middle for an even count).
fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

fn sample_input() -> Vec<u8> {
    const MIB: usize = 1024 * 1024;
    let pattern = b"rcomp portable WGSL LZ4 benchmark: abcdefghijklmnopqrstuvwxyz\n";
    pattern.iter().copied().cycle().take(64 * MIB).collect()
}

fn print_result(label: &str, input: usize, output: usize, elapsed: Duration) {
    let ratio = if input == 0 {
        0.0
    } else {
        output as f64 / input as f64
    };
    let throughput = if elapsed.is_zero() {
        0.0
    } else {
        input as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
    };
    println!("  {label}: {elapsed:.3?}, {throughput:.2} MiB/s, {output} bytes, ratio {ratio:.3}");
}
