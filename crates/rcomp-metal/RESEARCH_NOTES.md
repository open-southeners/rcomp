# Research notes: evolving native Metal acceleration in rcomp

These are concise source notes for a future research article. Detailed raw
measurements remain in [`BENCHMARKS.md`](./BENCHMARKS.md).

## Central story

The useful narrative is not simply “a GPU made compression faster.” The first
correct Metal implementation was about six times slower than CPU LZ4. Most of
the loss came from orchestration—small synchronous batches, repeated resource
work, validation, and unnecessary memory copies—not from an absolute inability
of the GPU kernel to compete. Iteratively removing those costs turned a 6.36 s
Metal result into 0.59 s on the same 573 MiB corpus, versus a final 0.93 s CPU
baseline: approximately 37% lower wall time and about 94% less host CPU time.

The counterpoint is equally important: on a real 71 MiB directory, Metal still
takes about 0.32 s versus 0.15–0.18 s on CPU. GPU acceleration therefore needs
a device- and workload-aware crossover policy, not a universal “GPU on” switch.

## Research question

Can a native Apple Metal backend accelerate a standards-compatible compression
utility without making the core library Apple-specific, changing file-format
semantics, requiring Xcode on user machines, or silently harming small-input
performance?

## Design decisions

- Native Metal through `objc2-metal`, isolated in a dedicated `rcomp-metal`
  crate. This gave direct control over Apple GPU resources while containing
  Objective-C and unsafe code.
- A safe, vendor-neutral provider/session API lives in `rcomp-core`. Future
  CUDA, HIP, or portable providers can implement the same boundary.
- The first codec is LZ4 Frame compression at `Fast` only, using independent
  64 KiB blocks. CPU code owns standards-compatible frame construction; Metal
  produces raw compressed blocks.
- Supported outputs are bare `.lz4` and tar-over-LZ4 `.tar.lz4`. Metal
  decompression and other codecs or compression levels are out of scope.
- Applications register providers explicitly. The CLI, environment variable,
  and Tauri preference expose `cpu`, `auto`, and `required` modes.
- Metal remains `Experimental`. `required` deliberately opts in; `auto` stays
  on CPU until cross-device crossover evidence exists.
- The production shader is embedded as an offline-compiled `.metallib`, so end
  users do not need Xcode or the optional Metal Toolchain.

## Experimental setup

- Machine: Apple M3 Max with a 40-core GPU.
- Build: optimized Rust release profile.
- Codec: LZ4 Fast, independent 64 KiB blocks.
- Small workload: `apps/rcomp-desktop/node_modules`, 71 MiB and 1,580 files.
- Fixed large workload: a prebuilt tar containing that directory eight times,
  601,154,560 bytes (573 MiB).
- The prebuilt tar isolates codec/provider behavior from directory traversal
  and tar creation.
- Timings use `/usr/bin/time -p` with filesystem-cached input. Each important
  phase compares CPU and Metal side by side on the unchanged corpus.
- Every promoted result round-trips through Rust's LZ4 decoder and passes the
  external `lz4 -t` interoperability check.

## Optimization chronology

| Phase | CPU | Metal | Interpretation |
| --- | ---: | ---: | --- |
| Initial 16 MiB synchronous batches | 1.05 s | 6.36 s | Correct but dispatch/batch bound |
| Reuse Metal buffers | 0.93 s | 6.06 s | About 5% Metal improvement; allocation was not the main loss |
| Provider-selected 128 MiB batches | 0.97 s | 2.07 s | Occupancy and dispatch amortization mattered greatly |
| Increase batches to 256 MiB | 1.01 s | 1.64 s | Larger bounded work continued to help |
| Keep exhaustive decode in debug/tests | 0.92 s | 0.95 s | Near parity; release still validates status, sizes, and ranges |
| Parallel hash-table clear experiment | 0.93 s | 0.94–0.98 s | No useful gain; reverted |
| Embed offline `.metallib` | 0.94 s | 1.00 s | No speed gain; retained for reliable packaging |
| Cache context; device-aware 512 MiB ceiling | 0.95 s | 0.85 s | First repeatable end-to-end win, about 10.5% |
| Bounded two-batch overlap | 0.96 s | 0.77 s | Input/framing overlap raised the win to about 20% |
| Copy only validated payload ranges | 0.95 s | 0.72 s | Avoiding a full slot-buffer copy raised it to about 24% |
| Fine-grained 599 MiB device-derived batch | 0.93 s | 0.59 s | One bounded dispatch; about 37% faster |

The final Metal comparison runs were 0.67, 0.59, and 0.57 s, with a subsequent
0.56 s confirmation. CPU produced three stable 0.93 s runs. Metal used about
0.05 s of host CPU time versus about 0.85 s for CPU LZ4.

## What the experiments revealed

1. **End-to-end measurement changed the engineering priorities.** Kernel
   correctness alone said little about CLI performance. Batch boundaries,
   host copies, frame assembly, file I/O, and initialization determined whether
   Metal won.
2. **Batch size was the dominant early variable.** Moving from 16 MiB to large,
   bounded device-aware batches removed most dispatch overhead.
3. **The kernel was already competitive at sufficient scale.** On the full
   573 MiB input in memory, a warm single Metal pass took about 453 ms versus
   853 ms for CPU LZ4. The remaining gap was host-side pipeline work.
4. **Caching helps long-running applications more than one-shot CLI use.** A
   synthetic 16 MiB pass fell from 55.3 ms cold to 18.1 ms warm after caching
   the device, library, and pipeline, but CPU still won that small workload.
5. **Unified memory does not make copies free.** Removing the temporary copy of
   the entire fixed-slot output buffer materially improved wall time.
6. **Not every plausible GPU optimization matters.** A separate parallel
   hash-table clearing kernel added complexity without measurable benefit.
7. **Runtime shader compilation was not the startup bottleneck.** Embedding a
   `.metallib` barely changed timings, although it improved distribution and
   reproducibility.
8. **Performance and compression ratio are separate outcomes.** The Metal
   archive was about 254 MiB versus 237 MiB for CPU LZ4—roughly 7% larger.
   Faster execution therefore carries a modest storage/transfer tradeoff.

## Safety and correctness strategy

- Unsafe Objective-C and shared-buffer operations remain inside `rcomp-metal`.
- Provider sessions own their queues and buffers exclusively and wait for
  command completion before exposing CPU-readable results.
- The core bridge bounds batch memory, applies backpressure, preserves block
  order, and emits standard LZ4 Frame metadata.
- Release builds validate command completion, block counts, output sizes,
  ranges, and frame metadata.
- Debug and test builds additionally decompress and byte-compare every GPU
  block.
- Hardware tests cover boundary-sized and incompressible data, provider reuse,
  bare LZ4, and tar-over-LZ4 directory round trips.

## Claims the article can support

- On this M3 Max and this fixed 573 MiB dependency corpus, the final Metal path
  was approximately 37% faster end to end than CPU LZ4 Fast.
- It reduced host CPU time from approximately 0.85 s to 0.05 s.
- The same backend was about twice as slow on the 71 MiB directory workload.
- The output was interoperable but approximately 7% larger than the CPU output.
- The largest improvements came from batching and data movement, not from
  changing the compression algorithm itself.

Avoid claiming that Metal is faster on all Apple Silicon, that 71 MiB is a
universal crossover threshold, or that GPU LZ4 has reached production maturity.
So far, measurements cover one high-end device and one main corpus.

## Current conclusion and next research

The evidence supports continuing the native implementation rather than parking
it in anticipation of future Apple improvements. It already wins substantially
on sufficiently large work on the test machine. It should nevertheless remain
experimental until the following work is complete:

1. Repeat the fixed protocol on several Apple Silicon generations and memory
   configurations.
2. Measure more data types, compressibility levels, cold starts, storage media,
   and corpus sizes to locate per-device crossover regions.
3. Prototype intra-block parallel match finding and evaluate wall time and
   compression ratio together.
4. Use that dataset to define conservative `auto` selection; retain explicit
   CPU and required-GPU modes.

## Possible article framing

Working title: **From 6× Slower to 37% Faster: What It Took to Accelerate LZ4
with Metal**.

Suggested structure: initial hypothesis → safe cross-platform architecture →
first disappointing result → measurement-led optimization sequence → rejected
experiments → small-versus-large crossover → correctness and packaging → why
automatic GPU selection still requires more evidence.
