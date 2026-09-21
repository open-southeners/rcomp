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
Metal reference. Windows/DX12 and Linux/Vulkan validation remains pending on
physical Intel, NVIDIA, and AMD hardware.
