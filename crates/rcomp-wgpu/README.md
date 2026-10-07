# rcomp-wgpu

Experimental portable GPU acceleration for rcomp.

The provider runs one baseline WGSL LZ4 Fast kernel through:

- Direct3D 12 on Windows;
- Vulkan on Linux;
- Metal on Apple Silicon.

It targets compatible Intel, NVIDIA, and AMD adapters on Windows/Linux and
Apple GPUs on macOS. Intel macOS is intentionally unsupported. The provider
currently exposes `.lz4` and `.tar.lz4` encoding at rcomp's `Fast` level using
independent 64 KiB blocks. `rcomp-core` owns standard LZ4 Frame construction,
cancellation, bounded overlap, atomic output publication, and CPU fallback.

The capability is experimental, so `auto` does not select it. Build the CLI
with the feature and opt in explicitly:

```sh
cargo run -p rcomp --features wgpu -- \
  input.bin output.lz4 --fast --accelerator required
```

Discover adapters and select one exactly on multi-GPU systems:

```sh
cargo run -p rcomp --features wgpu -- hardware
cargo run -p rcomp --features wgpu -- \
  input.bin output.lz4 --fast --accelerator required \
  --accelerator-device wgpu:BACKEND:VENDOR:DEVICE:ORDINAL
```

Deterministic validation does not need a GPU:

```sh
cargo test -p rcomp-wgpu --lib
```

The end-to-end test needs a compatible adapter and driver:

```sh
cargo test -p rcomp-wgpu --test wgpu_lz4 -- --ignored --nocapture
```

The ignored suite covers empty and boundary-sized input, compressible and
incompressible blocks, and an input spanning more than one provider batch. A
release comparison against CPU LZ4 can be run with:

```sh
cargo run -p rcomp-wgpu --example wgpu-lz4 --release -- path/to/input
```

Both the ignored suite and the example use the first-ranked adapter. To
qualify another one, set `RCOMP_WGPU_DEVICE` to an exact device ID from
`rcomp hardware`; an ID that is not present fails instead of falling back.
The example also accepts `RCOMP_WGPU_SAMPLES=N` to report the median of `N`
warm GPU and CPU runs.

Apple M3 Max measurements, the portable/native comparison, and the Windows
Direct3D 12 qualification on NVIDIA and AMD are recorded in
[`BENCHMARKS.md`](./BENCHMARKS.md).

Do not promote the provider to automatic selection until the Windows/DX12,
Linux/Vulkan, and Apple Silicon/Metal correctness and crossover matrices are
complete. Native `rcomp-metal` remains the Apple performance reference during
that qualification.
