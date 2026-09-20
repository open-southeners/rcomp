# rcomp-metal

Experimental native Metal compute support for rcomp.

This crate contains the experimental Apple Metal provider behind rcomp's safe,
vendor-neutral core interface. It implements a persistent LZ4 Fast raw-block
session; `rcomp-core` owns LZ4 Frame construction, selection policy, and the
CPU fallback. Native Objective-C objects and unsafe buffer operations stay in
this crate.

The LZ4 shader is shipped as a small offline-compiled `.metallib`, so building
or running rcomp does not require Xcode or Apple's optional Metal Toolchain.
The editable Metal source and regeneration instructions remain beside the
embedded asset.

On a Metal-capable Mac, run:

```sh
cargo run -p rcomp-metal --example metal-smoke
cargo test -p rcomp-metal -- --ignored
```

The smoke test compiles an embedded Metal shader at runtime, submits it to the
default device, waits for completion, and verifies the data copied back from a
shared Metal buffer.

After the smoke test passes, the experimental LZ4 path can be exercised with:

```sh
cargo run -p rcomp-metal --example metal-lz4 --release
cargo run -p rcomp-metal --example metal-lz4 --release -- path/to/input.bin output.lz4
```

The second example batches independent 64 KiB blocks on Metal, constructs a
standard LZ4 Frame on the CPU, and verifies it with the normal Rust LZ4 decoder.

The opt-in end-to-end CLI path is:

```sh
cargo run -p rcomp --features metal -- input.bin output.lz4 --fast --accelerator required
```

The Tauri backend can likewise be built with its `metal` feature, after which
the desktop **Require GPU** setting can invoke this exact capability.

The provider supports bare `.lz4` streams and tar-over-LZ4 directory archives
at rcomp's `Fast` level. Its capability maturity is `Experimental`, so `auto`
ignores it and prints a CPU-fallback notice. Current M3 Max results show a
medium-input startup penalty, but the final working-set-derived 599 MiB batch
made Metal about 37% faster than CPU LZ4 on the fixed 573 MiB source corpus,
with substantially less host CPU time and a modestly larger output. See
[`BENCHMARKS.md`](./BENCHMARKS.md) for the evolving evidence. Do not promote it
to automatic selection until measurements across representative Apple Silicon
devices establish a safe crossover policy.
