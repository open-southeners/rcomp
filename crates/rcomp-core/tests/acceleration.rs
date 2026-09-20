//! Operation-level acceleration selection tests.

use std::{fs, io::Read, sync::Arc};

use rcomp_core::{
    AccelerationPreference, AccelerationRequest, AcceleratorCapability, AcceleratorDevice,
    AcceleratorProvider, BlockEncoderSession, CapabilityMaturity, Codec, CompressOptions,
    CompressedBlock, Container, Direction, Engine, Error, Format, Level, ProcessingBackend,
    ProviderDescriptor, ProviderRegistry, compress,
};

struct StoredBlockProvider;

struct StoredBlockSession;

impl BlockEncoderSession for StoredBlockSession {
    fn block_size(&self) -> usize {
        65_536
    }

    fn compress_blocks(&mut self, input: &[u8]) -> rcomp_core::Result<Vec<CompressedBlock>> {
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

    let mut decoder = lz4::Decoder::new(fs::File::open(output).unwrap()).unwrap();
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, contents);
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
}
