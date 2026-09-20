# Metal shader assets

`lz4.metal` is the editable source. `lz4.metallib` is the embedded production
library loaded by `rcomp-metal`; it is intentionally checked in so users do not
need Xcode or the optional Metal Toolchain to build or run rcomp.

Regenerate the library on macOS with Apple's Metal Toolchain:

```sh
xcrun -sdk macosx metal -mmacosx-version-min=11.0 \
  -c crates/rcomp-metal/src/shaders/lz4.metal \
  -o /tmp/rcomp-lz4.air
xcrun -sdk macosx metallib /tmp/rcomp-lz4.air \
  -o crates/rcomp-metal/src/shaders/lz4.metallib
```

After regeneration, run the ignored `rcomp-metal` hardware tests and both
benchmark corpora documented in `../../BENCHMARKS.md`. The `.metallib` must not
be updated without its corresponding source change and compatibility tests.
