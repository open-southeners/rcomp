//! Operation-level acceleration selection tests.

use std::{fs, io::Read, sync::Arc};

use rcomp_core::{
    AccelerationFallbackReason, AccelerationPreference, AccelerationRequest, AcceleratorCapability,
    AcceleratorDevice, AcceleratorProvider, BlockEncoderSession, CapabilityMaturity, Codec,
    CompressOptions, CompressedBlock, Container, Direction, Engine, Error, Format, Level,
    ProcessingBackend, ProviderDescriptor, ProviderRegistry, compress,
};

struct StoredBlockProvider;

struct StoredBlockSession;

struct FailingBlockProvider;

struct FailingBlockSession;

impl BlockEncoderSession for StoredBlockSession {
    fn block_size(&self) -> usize {
        65_536
    }

    fn compress_blocks(
        &mut self,
        input: &[u8],
        _cancel: &rcomp_core::CancelToken,
    ) -> rcomp_core::Result<Vec<CompressedBlock>> {
        Ok(input
            .chunks(self.block_size())
            .map(|chunk| CompressedBlock {
                input_size: chunk.len(),
                bytes: chunk.to_vec(),
            })
            .collect())
    }
}

impl AcceleratorProvider for StoredBlockProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "test".to_owned(),
            name: "Test accelerator".to_owned(),
        }
    }

    fn devices(&self) -> rcomp_core::Result<Vec<AcceleratorDevice>> {
        Ok(vec![AcceleratorDevice {
            provider_id: "test".to_owned(),
            device_id: "test-device".to_owned(),
            name: "Test GPU".to_owned(),
        }])
    }

    fn capabilities(
        &self,
        device: &AcceleratorDevice,
    ) -> rcomp_core::Result<Vec<AcceleratorCapability>> {
        Ok([
            Format::codec(Codec::Lz4),
            Format::layered(Container::Tar, Codec::Lz4),
        ]
        .into_iter()
        .map(|format| AcceleratorCapability {
            provider_id: "test".to_owned(),
            device_id: device.device_id.clone(),
            codec: Codec::Lz4,
            format,
            direction: Direction::Encode,
            levels: vec![Level::Fast],
            block_size: 65_536,
            maturity: CapabilityMaturity::Experimental,
        })
        .collect())
    }

    fn open_block_encoder(
        &self,
        _request: &AccelerationRequest,
    ) -> rcomp_core::Result<Box<dyn BlockEncoderSession>> {
        Ok(Box::new(StoredBlockSession))
    }
}

impl BlockEncoderSession for FailingBlockSession {
    fn block_size(&self) -> usize {
        65_536
    }

    fn compress_blocks(
        &mut self,
        _input: &[u8],
        _cancel: &rcomp_core::CancelToken,
    ) -> rcomp_core::Result<Vec<CompressedBlock>> {
        Err(Error::AccelerationFailed {
            provider: "failing-test".to_owned(),
            reason: "simulated asynchronous device failure".to_owned(),
        })
    }
}

impl AcceleratorProvider for FailingBlockProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "failing-test".to_owned(),
            name: "Failing test accelerator".to_owned(),
        }
    }

    fn devices(&self) -> rcomp_core::Result<Vec<AcceleratorDevice>> {
        Ok(vec![AcceleratorDevice {
            provider_id: "failing-test".to_owned(),
            device_id: "failing-device".to_owned(),
            name: "Failing GPU".to_owned(),
        }])
    }

    fn capabilities(
        &self,
        device: &AcceleratorDevice,
    ) -> rcomp_core::Result<Vec<AcceleratorCapability>> {
        Ok(vec![AcceleratorCapability {
            provider_id: "failing-test".to_owned(),
            device_id: device.device_id.clone(),
            codec: Codec::Lz4,
            format: Format::codec(Codec::Lz4),
            direction: Direction::Encode,
            levels: vec![Level::Fast],
            block_size: 65_536,
            maturity: CapabilityMaturity::Supported,
        }])
    }

    fn open_block_encoder(
        &self,
        _request: &AccelerationRequest,
    ) -> rcomp_core::Result<Box<dyn BlockEncoderSession>> {
        Ok(Box::new(FailingBlockSession))
    }
}

#[test]
fn auto_compression_reports_cpu_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.zst");
    fs::write(&input, b"automatic acceleration fallback").unwrap();

    let opts = CompressOptions {
        acceleration: AccelerationPreference::Auto,
        ..Default::default()
    };
    let report = compress(&input, &output, &opts, |_| {}).unwrap();

    assert_eq!(report.backend, ProcessingBackend::Cpu);
    assert!(
        report
            .acceleration_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("zstd"))
    );
    assert_eq!(
        report.acceleration_fallback,
        Some(AccelerationFallbackReason::NoProviderInstalled)
    );
    assert!(output.is_file());
}

#[test]
fn required_compression_fails_without_creating_output() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.gz");
    fs::write(&input, b"required acceleration").unwrap();

    let opts = CompressOptions {
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };
    let error = compress(&input, &output, &opts, |_| {}).unwrap_err();

    assert!(matches!(error, Error::AccelerationUnavailable { .. }));
    assert!(!output.exists());
}

#[test]
fn required_uses_registered_experimental_provider_and_core_framing() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.lz4");
    let contents = vec![b'x'; 70_000];
    fs::write(&input, &contents).unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StoredBlockProvider));
    let engine = Engine::with_registry(registry);
    let opts = CompressOptions {
        level: Level::Fast,
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };
    let report = engine.compress(&input, &output, &opts, |_| {}).unwrap();

    assert_eq!(
        report.backend,
        ProcessingBackend::Accelerator("test:test-device".to_owned())
    );
    assert!(report.acceleration_notice.is_none());
    assert!(report.acceleration_fallback.is_none());

    let mut decoder = lz4::Decoder::new(fs::File::open(output).unwrap()).unwrap();
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, contents);
}

#[test]
fn accelerated_success_atomically_replaces_an_existing_output() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.lz4");
    let contents = vec![b'x'; 70_000];
    fs::write(&input, &contents).unwrap();
    fs::write(&output, b"existing output must survive until publication").unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StoredBlockProvider));
    let engine = Engine::with_registry(registry);
    let opts = CompressOptions {
        level: Level::Fast,
        overwrite: true,
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };
    engine.compress(&input, &output, &opts, |_| {}).unwrap();

    let mut decoder = lz4::Decoder::new(fs::File::open(&output).unwrap()).unwrap();
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, contents);
    assert_no_staged_outputs(temp.path());
}

#[test]
fn accelerated_failure_preserves_existing_output_and_removes_staging_file() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.lz4");
    let sentinel = b"existing output must remain byte-identical";
    fs::write(&input, vec![b'x'; 70_000]).unwrap();
    fs::write(&output, sentinel).unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FailingBlockProvider));
    let engine = Engine::with_registry(registry);
    let opts = CompressOptions {
        level: Level::Fast,
        overwrite: true,
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };
    let error = engine.compress(&input, &output, &opts, |_| {}).unwrap_err();

    assert!(matches!(error, Error::AccelerationFailed { .. }));
    assert_eq!(fs::read(&output).unwrap(), sentinel);
    assert_no_staged_outputs(temp.path());
}

#[test]
fn auto_retries_a_failed_accelerator_on_cpu_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.lz4");
    let contents = vec![b'x'; 70_000];
    fs::write(&input, &contents).unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FailingBlockProvider));
    let engine = Engine::with_registry(registry);
    let report = engine
        .compress(
            &input,
            &output,
            &CompressOptions {
                level: Level::Fast,
                acceleration: AccelerationPreference::Auto,
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();

    assert_eq!(report.backend, ProcessingBackend::Cpu);
    assert_eq!(
        report.acceleration_fallback,
        Some(
            AccelerationFallbackReason::ProviderExecutionFailedCpuRetry {
                provider_id: "failing-test".to_owned(),
            }
        )
    );
    assert!(
        report
            .acceleration_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("retried successfully on CPU"))
    );
    let mut decoder = lz4::Decoder::new(fs::File::open(&output).unwrap()).unwrap();
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, contents);
    assert_no_staged_outputs(temp.path());
}

fn assert_no_staged_outputs(directory: &std::path::Path) {
    let staged = fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".rcomp-"))
        .collect::<Vec<_>>();
    assert!(staged.is_empty(), "staged outputs remained: {staged:?}");
}

#[test]
fn required_provider_can_compress_a_directory_as_tar_lz4() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    let output = temp.path().join("output.tar.lz4");
    let extracted = temp.path().join("extracted");
    fs::create_dir(&input).unwrap();
    fs::write(input.join("first.txt"), b"first accelerated tar entry").unwrap();
    fs::write(input.join("second.txt"), b"second accelerated tar entry").unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StoredBlockProvider));
    let engine = Engine::with_registry(registry);
    let opts = CompressOptions {
        level: Level::Fast,
        acceleration: AccelerationPreference::Required,
        ..Default::default()
    };
    let report = engine.compress(&input, &output, &opts, |_| {}).unwrap();

    assert_eq!(
        report.backend,
        ProcessingBackend::Accelerator("test:test-device".to_owned())
    );
    rcomp_core::extract(
        &output,
        &extracted,
        &rcomp_core::ExtractOptions::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(extracted.join("first.txt")).unwrap(),
        b"first accelerated tar entry"
    );
    assert_eq!(
        fs::read(extracted.join("second.txt")).unwrap(),
        b"second accelerated tar entry"
    );
}

#[test]
fn auto_ignores_registered_experimental_provider() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.txt");
    let output = temp.path().join("output.lz4");
    fs::write(&input, b"experimental providers need explicit selection").unwrap();

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StoredBlockProvider));
    let engine = Engine::with_registry(registry);
    let opts = CompressOptions {
        level: Level::Fast,
        acceleration: AccelerationPreference::Auto,
        ..Default::default()
    };
    let report = engine.compress(&input, &output, &opts, |_| {}).unwrap();

    assert_eq!(report.backend, ProcessingBackend::Cpu);
    assert!(
        report
            .acceleration_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("experimental"))
    );
    assert_eq!(
        report.acceleration_fallback,
        Some(AccelerationFallbackReason::ExperimentalCapability)
    );
}
