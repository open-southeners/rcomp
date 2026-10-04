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
Metal reference. Windows/DX12 validation, and Linux/Vulkan validation on
NVIDIA and AMD hardware, remain pending.

## Linux/Intel qualification (Vulkan)

Correctness-qualified on 27 September 2026 against commit
`0da8c5e85fea07f59bfbdb3f4c35956258570d49`. The capability stays
experimental and excluded from `Auto`: the portable path is slower than CPU
LZ4 on every measured input on this machine.

### Test machine

- Intel NUC8i7BEH, Core i7-8559U, Iris Plus Graphics 655 (CFL GT3),
  PCI `8086:3ea5`, kernel driver `i915`
- Ubuntu 24.04.5 LTS, kernel 6.8.0-139-generic
- Mesa `25.2.8-0ubuntu0.24.04.2` (Intel open-source Mesa driver, Vulkan 1.4.318)
- Rust 1.94.1, release profile
- `rcomp hardware` device `wgpu:vulkan:8086:3ea5:0000-00-02.0`, preferred
  batch 64 MiB, max storage-buffer binding 2 GiB
- Mains power, `powersave` CPU governor, NVMe SSD with ext4; CPU package
  temperature reached 97–98 °C during repeated runs, so sustained results
  include thermal throttling

### Correctness coverage

- Physical-GPU suite passes, including an input 17 bytes larger than the
  64 MiB batch.
- CLI `--accelerator required` output for the empty, one-byte, 64 KiB − 1/exact/+ 1,
  and 128 MiB + 17 incompressible inputs passes `lz4 -t` (lz4 1.9.4) and
  decodes byte-for-byte through both `lz4` and rcomp.
- `.tar.lz4` of a source tree passes `lz4 -t`, its checksum sidecar verifies,
  and it extracts to an identical tree.
- Three GPU compressions of the same 128 MiB input produce identical SHA-256.
- `auto` selects CPU with a warning; `required` with `--best`, an unknown
  device ID, or extraction fails with exit 1 and leaves existing destinations
  untouched. SIGINT at 0.5 s, 1.5 s, and 3 s leaves no output and no
  temporary file behind.
- No Vulkan validation errors, device loss, or i915 hang/reset messages were
  logged.

### Results

Timings come from `cargo run -p rcomp-wgpu --example wgpu-lz4 --release`.
The realistic corpus is a 1,923,317,760-byte tar of `/usr/lib/x86_64-linux-gnu`,
`/usr/bin`, and `/usr/share/doc`. The table reports medians over five samples.

| Corpus | GPU cold | GPU warm | CPU Fast | GPU output | CPU output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Synthetic 64 MiB (compressible) | 572.8 ms | 486.5 ms | 42.3 ms | 341,003 B | 312,847 B |
| Random 128 MiB + 17 B | 1.882 s | 1.852 s | 161.6 ms | 134,225,952 B | 134,234,152 B |
| Realistic 1.79 GiB (5 samples) | 17.433 s | 17.350 s | 6.437 s | 970,499,414 B | 933,062,462 B |

On the realistic corpus the warm GPU path runs at about 106 MiB/s against
about 285 MiB/s for CPU, roughly 2.7× slower. Its output is 4.0% larger.
Through the CLI, peak resident memory was 311 MiB for GPU and 77 MiB for CPU.
The example binary loads the whole input into memory, so its own peak
(about 3.7 GiB) is not representative.
