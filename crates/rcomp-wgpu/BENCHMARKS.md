# Portable GPU LZ4 benchmark log

This log records the first Apple Silicon qualification of the shared WGSL
provider. The capability remains experimental; these measurements are evidence
for tuning and do not define an `Auto` policy.

## Test machine and corpus

- Apple M3 Max, 40-core GPU
- macOS Metal backend through `wgpu` 30.0.1
- Release profile
- LZ4 Fast with independent 64 KiB blocks
- Fixed 601,154,560-byte tar corpus used by the native Metal benchmark
- Output decoded and compared byte-for-byte after every reported run

The comparison command is:

```sh
cargo run -p rcomp-wgpu --example wgpu-lz4 --release -- \
  /private/tmp/rcomp-node-modules-8x.tar
```

## Results

| Portable phase | Cold | Warm | CPU Fast | Portable output | CPU output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Conservative 64 MiB batches | 2.929 s | 2.880 s | 0.998 s | 265,880,019 B | 248,301,321 B |
| Apple 256 MiB bounded batches | 1.318 s | 1.243 s | 0.982 s | 265,880,019 B | 248,301,321 B |
| Payload-only compact readback | 1.283 s | 1.177 s | 0.956 s | 265,880,019 B | 248,301,321 B |

The adapter-limit-derived 256 MiB policy reduced the warm portable time by
about 57% relative to the 64 MiB baseline. Copying only compressed payloads to
the mapped readback buffer improved it further. The final portable result is
still about 23% slower than CPU for this corpus and roughly 2x the native Metal
provider's recorded 0.59 s median. Its output is about 7% larger than CPU LZ4,
matching the native kernel's ratio tradeoff.

A separate synthetic 64 MiB highly compressible input produced 39.9 ms warm
portable time versus 23.6 ms CPU time. This confirms that the portable path
does not yet have a measured winning region on this machine.

## Correctness coverage

The M3 Max hardware suite passes:

- empty input;
- one-byte input;
- 64 KiB minus one, exact 64 KiB, and 64 KiB plus one boundaries;
- mixed pseudo-random/incompressible blocks;
- an input 17 bytes larger than the selected 256 MiB batch;
- standard LZ4 Frame decoding and byte-for-byte comparison.

`rcomp hardware` reports both the portable `wgpu`/Metal adapter and the native
Metal reference. Windows/Direct3D 12 results for NVIDIA and AMD follow below.
Linux/Vulkan validation and Intel hardware remain pending.

## Windows / Direct3D 12

The first Windows qualification ran on an NVIDIA discrete GPU and an AMD
integrated GPU in the same machine. The capability stays experimental, and
`--accelerator auto` still selects the CPU.

### Machine

- Windows 11 Pro 25H2, build 26200.9457, High performance power plan
- AMD Ryzen 7 7800X3D (8 cores / 16 threads), 32 GB RAM, ASUS board, BIOS 2202
- NVIDIA GeForce RTX 3090 24 GB (`10de:2204`), driver 616.92
  (`32.0.16.1692`), WDDM 3.2, hardware-accelerated GPU scheduling on
- AMD Radeon Graphics, the Ryzen iGPU (`1002:164e`), driver
  `32.0.21043.5001`
- Samsung 970 EVO Plus NVMe (system drive, where all inputs and outputs lived)
- `wgpu` 30.0.1 with the Direct3D 12 backend; Rust 1.96.1 (MSVC); release
  profile
- The session ran over Remote Desktop. Another application held about 9 GiB of
  the RTX 3090's memory, and Windows Defender real-time scanning was on.

`rcomp hardware` reports the two adapters as:

```text
wgpu:dx12:10de:2204:0000-01-00.0   NVIDIA GeForce RTX 3090   discretegpu
wgpu:dx12:1002:164e:0000-73-00.0   AMD Radeon(TM) Graphics   integratedgpu
```

Both report a 2 GiB storage-binding limit and a 64 MiB preferred batch. The
discrete GPU ranks first.

### Commits

Testing started at `93fa3c1`. Results below are for `ec9d569`, which adds the
fixes found during the run:

- `Cargo.lock` resolved `gpu-allocator` to `windows` 0.61 while `wgpu-hal`
  needs 0.62, so the Direct3D 12 backend did not compile on Windows.
- DXGI listed the RTX 3090 twice (same PCI location, two LUIDs), so
  `rcomp hardware` showed two devices with one ID. The provider now keeps one
  adapter per PCI location.
- Cancelling while a large file was being added to a tar spun forever. This
  was a core bug that affected CPU and GPU alike.
- Windows-only `clippy -D warnings` failures in core and the CLI tests were
  fixed.

### Results

The benchmark command was
`RCOMP_WGPU_DEVICE=<id> RCOMP_WGPU_SAMPLES=5 cargo run -p rcomp-wgpu --example wgpu-lz4 --release -- <input>`.
Warm and CPU times are medians of 5 samples. Every output was decoded and
compared byte for byte.

| Device | Input | Cold GPU | Warm GPU | CPU Fast | GPU output | CPU output |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| RTX 3090 | random, 134,217,745 B | 0.908 s | 0.664 s (193 MiB/s) | 0.213 s (601 MiB/s) | 134,225,952 B | 134,234,152 B |
| RTX 3090 | realistic tar, 1,075,339,776 B | 3.746 s | 3.488 s (294 MiB/s) | 1.760 s (583 MiB/s) | 430,588,682 B | 400,271,413 B |
| Radeon iGPU | random, 134,217,745 B | 3.167 s | 2.899 s (44 MiB/s) | 0.215 s (597 MiB/s) | 134,225,952 B | 134,234,152 B |
| Radeon iGPU | realistic tar, 1,075,339,776 B | 10.778 s | 10.898 s (94 MiB/s) | 1.760 s (583 MiB/s) | 430,588,682 B | 400,271,413 B |

The iGPU rows used 3 warm samples and are for reference, not a performance
target. The realistic input is a tar of 600 build artifacts (`.rlib`, `.rmeta`,
`.pdb`, `.exe`, `.dll`, `.d`) from this repository's `target/debug/deps`.

The portable path is slower than CPU LZ4 on both adapters: about 2x on the
realistic tar and 3x on incompressible input on the RTX 3090. As on Apple
Silicon, its output is about 7.6% larger than CPU LZ4. On the 3090,
`nvidia-smi dmon` shows 80–100% SM time while batches run, but only about
150 W of the 390 W limit and 1–2% memory-controller use. With one invocation
per 64 KiB block, a 64 MiB batch is 1,024 invocations (16 workgroups), far too
few to fill 82 SMs. The kernel is serial per block, not bandwidth-bound.

Peak working set for the CLI on the 1 GiB tar was 697 MiB with the RTX 3090,
970 MiB with the iGPU, and 139 MiB on CPU.

TDR margin: a single incompressible 64 MiB batch, the kernel's worst case,
takes about 0.37 s on the RTX 3090 and about 1.5 s on the iGPU. The iGPU
figure is roughly 75% of the 2 s Windows TDR timeout. No TDR, device loss, or
display-driver event was logged during any run (System log: `nvlddmkm`,
`Display` 4101, `dxgkrnl`, `amdkmdag`, LiveKernelEvent). One `nvlddmkm`
event 153 was already in the log from about two hours before testing began.
`CURRENT_ISSUES.md` tracks the iGPU margin.

### Correctness coverage

Every item below passed on both adapters:

- the ignored `wgpu_lz4` hardware suite, in debug (which re-verifies every
  GPU block on the host) and in release: empty, 1 byte, 64 KiB − 1 / 64 KiB /
  64 KiB + 1, mixed pseudo-random blocks, and 64 MiB + 17 bytes across a batch
  boundary;
- CLI `--fast --accelerator required --accelerator-device <id>` on empty,
  1-byte, 65,535 / 65,536 / 65,537-byte, 48 MiB highly compressible and
  128 MiB + 17 random inputs. Each output passed `lz4 -t`, was decoded by the
  reference `lz4` 1.10.0 CLI and by `rcomp`, and matched the input's SHA-256;
- a source tree (`crates/`, 83 files) as `.tar.lz4 --checksum`: reference
  decode, listing, extraction with sidecar verification, and a per-file hash
  diff of the tree;
- determinism: three compressions of the 128 MiB random input had identical
  SHA-256 on each adapter. The RTX 3090 and the Radeon iGPU also produced
  byte-identical output, with no vendor-specific code path;
- policy: `auto` and `cpu` stay on CPU (`auto` says why), `--best` and the
  default level fail under `required`, made-up device and provider IDs fail
  without picking another GPU, `required` extraction fails as unsupported, and
  a failed `required` run with `--force` leaves the existing destination
  byte-identical;
- a real `CTRL_C_EVENT`, sent once 17–23 MiB of GPU output had been staged,
  for `.lz4` and `.tar.lz4`, with fresh and existing destinations. `rcomp`
  exits 1 within 0.25 s (RTX 3090) or 0.67 s (iGPU) of the event, publishes no
  output, removes its `.rcomp-*.tmp` file, and leaves an existing destination
  byte-identical.

Both qualification knobs are documented in the README: `RCOMP_WGPU_DEVICE`
selects the adapter for the hardware suite and example, and
`RCOMP_WGPU_SAMPLES` sets the warm sample count.
