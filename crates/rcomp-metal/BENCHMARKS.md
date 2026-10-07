# Metal LZ4 benchmark log

This log records end-to-end evidence for each experimental optimization. Metal
remains ineligible for `auto` until repeatable results justify a device- and
workload-aware selection policy.

## Test machine and corpus

- Apple M3 Max, 40-core GPU
- Release profile
- LZ4 Fast, independent 64 KiB blocks
- Source: `apps/rcomp-desktop/node_modules` (71 MiB, 1,580 files)
- Large fixed corpus: a tar containing that directory eight times (573 MiB)
- Warm results use filesystem-cached input and report `/usr/bin/time -p`

The fixed-corpus commands compare the same prebuilt tar so directory walking
and tar creation do not obscure codec/provider changes:

```sh
target/release/rcomp corpus.tar cpu.lz4 --fast --accelerator cpu --force --quiet
target/release/rcomp corpus.tar metal.lz4 --fast --accelerator required --force --quiet
```

## Evolution on the 573 MiB fixed corpus

| Phase | CPU wall | Metal wall | Metal/CPU | Observation |
| --- | ---: | ---: | ---: | --- |
| Initial 16 MiB synchronous batches | 1.05 s | 6.36 s | 6.06× | Correct, heavily dispatch/batch bound |
| Reusable Metal buffers | 0.93 s | 6.06 s | 6.52× | Allocation reuse helped Metal by about 5% |
| Provider-directed 128 MiB batches | 0.97 s | 2.07 s | 2.13× | GPU occupancy and dispatch amortization dominate |
| Provider-directed 256 MiB batches | 1.01 s | 1.64 s | 1.62× | Larger bounded batches continue to help |
| Release path without exhaustive CPU decode | 0.92 s | 0.95 s | 1.03× | Near wall-time parity; Metal host CPU time is much lower |
| Parallel hash-table clear experiment | 0.93 s | 0.94–0.98 s | ~1.03× | No material improvement; reverted to avoid extra pipeline complexity |
| Embedded offline-compiled `.metallib` | 0.94 s | 1.00 s | 1.06× | No speedup; retained for deterministic packaging |
| Shared context + device-aware 512 MiB batch ceiling | 0.95 s | 0.85 s | 0.89× | First repeatable end-to-end Metal win: about 10.5% faster |
| Bounded two-batch CPU/Metal overlap | 0.96 s | 0.77 s | 0.80× | About 20% faster; input fill and framing overlap accelerator work |
| Copy validated Metal payload ranges only | 0.95 s | 0.72 s | 0.76× | About 24% faster; avoids a full fixed-slot output-buffer copy |
| Fine-grained working-set-derived batch (599 MiB) | 0.93 s | 0.59 s | 0.63× | About 37% faster; the corpus fits in one bounded dispatch |

The release-validation row is the median of three warm runs. CPU timings were
0.97, 0.92, and 0.92 seconds. Metal timings were 0.94, 0.95, and 0.95 seconds. Host CPU
time was approximately 0.84 seconds for CPU LZ4 and 0.06 seconds for Metal.
The Metal artifact was 254 MiB versus 237 MiB for CPU LZ4, about 7% larger.

The device-aware row is also the median of three warm runs on the same fixed
corpus. CPU timings were 0.93, 0.96, and 0.95 seconds; Metal timings were 0.95,
0.85, and 0.82 seconds. Host CPU time was approximately 0.85 seconds for CPU
LZ4 and 0.07 seconds for Metal. On this M3 Max the provider selected a 512 MiB
batch ceiling; devices with smaller recommended working sets select 64, 128,
or 256 MiB instead. Both artifacts passed the external Homebrew `lz4 -t`
validator and decoded to all 601,154,560 source bytes.

Within one long-running process, caching the device, embedded library, and
pipeline reduced a synthetic 16 MiB Metal pass from 55.3 ms cold to 18.1 ms
warm. On the complete 573 MiB in-memory input, a warm single Metal dispatch
took 453 ms versus 853 ms for CPU LZ4. This establishes that the kernel itself
can win at sufficient batch size; remaining CLI time is primarily the bounded
bridge, file I/O, and frame assembly.

The bounded-overlap row used warm CPU timings of 0.93, 0.96, and 1.02 seconds
(median 0.96) and Metal timings of 0.80, 0.76, and 0.77 seconds (median 0.77).
Metal host CPU time remained about 0.08 seconds. The bridge permits at most two
submitted batches, writes results strictly in source order, and recycles large
input allocations. Its output again passed `lz4 -t` and decoded to all
601,154,560 bytes.

The payload-range row used warm CPU timings of 0.95, 0.97, and 0.95 seconds
(median 0.95) and Metal timings of 0.74, 0.71, and 0.72 seconds (median 0.72).
Metal host CPU time fell to about 0.06 seconds. Instead of allocating and
copying every fixed output slot, the provider now validates GPU-reported sizes
and copies only ranges that the frame will use. The resulting frame passed
`lz4 -t` and decoded to all source bytes.

The fine-grained batch row used CPU timings of 0.93, 0.93, and 0.93 seconds
and Metal timings of 0.67, 0.59, and 0.57 seconds (median 0.59); an additional
Metal run completed in 0.56 seconds. Metal host CPU time was about 0.05 seconds.
The M3 Max reports a 40,200,896,512-byte recommended working set, so the
provider's conservative 1/64 budget rounds to 599 MiB in whole 64 KiB blocks.
The bridge retains a hard 1 GiB ceiling. This lets the 573 MiB corpus use one
dispatch without making the policy corpus-specific or unbounded. The artifact
again passed external validation and decoded to every source byte.

Release builds still validate command completion, reported sizes, buffer
ranges, block counts, and frame metadata. Debug and test builds additionally
decode and compare every GPU block. The release artifact passed the external
Homebrew `lz4 -t` validator and decoded to all 601,154,560 source bytes.

## Direct directory result

The provider now advertises `tar.lz4`, allowing the actual directory to pass
through rcomp's walker and tar writer without a prebuilt archive. For the real
71 MiB directory, three warm runs produced:

| Backend | Wall times | Median | Host CPU time |
| --- | --- | ---: | ---: |
| CPU | 0.17, 0.15, 0.15 s | 0.15 s | about 0.10 s |
| Metal | 0.32, 0.32, 0.33 s | 0.32 s | about 0.02 s |

The Metal archive was 31 MiB versus 29 MiB for CPU. The Metal output passed
`lz4 -t` and decoded to the complete 71,421,440-byte tar stream. At this size,
device/pipeline setup and initial shared-buffer allocation outweigh the
compression offload. Repeating this test with an embedded `.metallib` produced
the same 0.32–0.33-second Metal range, disproving runtime source compilation as
the main startup cost. The bounded-overlap and payload-copy phases remained at
0.32 seconds versus a warm 0.16–0.18-second CPU run, as expected for a directory
that fits in one batch and is dominated by fixed Metal startup/allocation cost.

## Next measurement gates

1. Prototype intra-block parallel match finding, then compare ratio as well as
   wall time; faster output is not sufficient if compatibility or ratio
   regresses materially.
2. Repeat on several Apple Silicon generations and memory sizes before defining
   any `auto` threshold.
