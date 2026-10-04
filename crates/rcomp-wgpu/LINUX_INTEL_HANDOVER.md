# Linux/Intel portable GPU qualification handover

**Prepared:** 28 September 2026  
**Prepared against:** `feat/gpu-acceleration` at `0da8c5e85fea07f59bfbdb3f4c35956258570d49`  
**Target:** Ubuntu on a physical Intel NUC with an Intel integrated GPU  
**Provider under test:** `rcomp-wgpu` through Vulkan and Mesa's Intel driver  

This is an execution handover for qualifying the already implemented portable
GPU path. It is not a request to design a second Intel-specific provider.
Record the actual commit tested because the branch may advance after this
document was prepared.

## 1. What exists now

The portable provider uses the same baseline WGSL LZ4 kernel on all supported
vendors. On Linux, `wgpu` is compiled with Vulkan only. OpenGL, OpenCL, Level
Zero, and oneAPI are not fallback paths.

The implemented capability is deliberately narrow:

- LZ4 Frame **compression** only;
- `Fast` level only;
- `.lz4` and `.tar.lz4` output;
- independent 64 KiB blocks framed by `rcomp-core`;
- a conservative, adapter-limit-bounded 64 MiB maximum Linux batch;
- explicit device selection, cancellation, atomic output publication, and an
  observable CPU retry for execution failures in `Auto` mode.

The capability remains `Experimental`, so `--accelerator auto` must not select
it merely because the NUC has a compatible GPU. Qualification must use
`--accelerator required` and the exact identifier printed by `rcomp hardware`.

Already qualified elsewhere:

- deterministic shader, framing, selection, fallback, and failure tests;
- Apple M3 Max correctness through WGPU/Metal, including multi-batch input;
- standard LZ4 Frame decoding and external `lz4` interoperability on macOS.

Not yet qualified:

- Linux/Vulkan on physical Intel hardware;
- the Intel/Mesa driver combination on this NUC;
- Linux performance, memory behavior, cancellation, and long-run stability;
- automatic-selection crossover rules.

## 2. Exit criteria

Linux/Intel qualification is complete only when the evidence bundle shows all
of the following:

1. Vulkan enumerates the physical Intel GPU without using `llvmpipe` or
   `lavapipe`.
2. `rcomp hardware` reports a `wgpu:vulkan:8086:...` device, its Intel name,
   driver information, buffer limits, and both experimental LZ4 encode
   capabilities.
3. The deterministic library tests and ignored physical-GPU suite pass in a
   release build, including an input larger than one selected batch.
4. Explicit-device CLI compression produces `.lz4` and `.tar.lz4` files that
   decode byte-for-byte through rcomp and the external `lz4` tool.
5. Empty, tiny, 64 KiB boundaries, compressible, incompressible, and
   multi-batch inputs pass without Vulkan validation errors, GPU resets,
   hangs, or staging-file leaks.
6. Unsupported levels/directions fail clearly and `Auto` remains on CPU.
7. Cold GPU, warm GPU, and CPU results are recorded with output size—not just
   throughput—on synthetic and realistic data.
8. The exact NUC model, Intel PCI ID, Ubuntu release, kernel, Mesa/Vulkan
   driver, Rust version, and tested commit are recorded.

A performance win is **not** required for correctness qualification. Do not
enable `Auto` from one NUC result.

## 3. Host preparation

Run directly on the Ubuntu host rather than in a container for the first pass.
The provider creates a headless Vulkan instance, so a graphical desktop or
display server should not be required; access to the physical render node is.

Install the normal Rust build tools plus the Ubuntu Vulkan and interoperability
utilities:

```sh
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pciutils \
  pkg-config \
  mesa-vulkan-drivers \
  vulkan-tools \
  lz4
```

`mesa-vulkan-drivers` contains the Intel Vulkan ICDs on supported Ubuntu
releases. `vulkan-tools` provides `vulkaninfo`. The optional
`intel-gpu-tools` package is useful for observing utilization, but is not part
of the correctness gate.

If the host should not gain extra packages, run `vulkaninfo` and `lz4` from
the tool image in `qualification/` instead. rcomp and cargo still run on the
host; only the reference tools are containerized. From the repository root,
define these once per shell and the commands in the rest of this document work
unchanged:

```sh
vulkaninfo() { crates/rcomp-wgpu/qualification/tools.sh vulkaninfo "$@"; }
lz4() { crates/rcomp-wgpu/qualification/tools.sh lz4 "$@"; }
```

The wrapper builds the image on first use, from the same Ubuntu release as the
host, and passes `/dev/dri` through. It mounts only the current directory, so
keep the evidence directory under the repository as shown below. The host
still needs `mesa-vulkan-drivers`, because rcomp uses the host driver.
Container package versions are not visible to the host's `dpkg-query`, so
record them separately:

```sh
crates/rcomp-wgpu/qualification/tools.sh \
  dpkg-query -W mesa-vulkan-drivers vulkan-tools lz4
```

Confirm that the container's `mesa-vulkan-drivers` version matches the
host's. Otherwise `vulkaninfo` describes a different driver than the one rcomp
runs on.

Use Rust 1.87 or newer because that is the crate's declared MSRV. Preserve
`Cargo.lock`:

```sh
rustup show
rustc --version --verbose
cargo --version
git rev-parse HEAD
git status --short
```

Do not continue with unexplained local source changes. If testing a deliberate
patch, record its diff with the results.

Useful upstream references:

- [Ubuntu `mesa-vulkan-drivers` package](https://packages.ubuntu.com/search?keywords=mesa-vulkan-drivers)
- [Ubuntu package file list showing Intel Vulkan ICDs](https://packages.ubuntu.com/noble/amd64/mesa-vulkan-drivers/filelist)
- [WebGPU Linux/Vulkan baseline requirements](https://github.com/gpuweb/gpuweb/wiki/System-Requirements)

## 4. Capture the machine before testing

Create one durable evidence directory. The following commands are read-only:

```sh
RCOMP_INTEL_EVIDENCE="$PWD/target/linux-intel-qualification"
mkdir -p "$RCOMP_INTEL_EVIDENCE"

date --iso-8601=seconds | tee "$RCOMP_INTEL_EVIDENCE/date.txt"
git rev-parse HEAD | tee "$RCOMP_INTEL_EVIDENCE/commit.txt"
git status --short | tee "$RCOMP_INTEL_EVIDENCE/git-status.txt"
uname -a | tee "$RCOMP_INTEL_EVIDENCE/uname.txt"
cat /etc/os-release | tee "$RCOMP_INTEL_EVIDENCE/os-release.txt"
lscpu | tee "$RCOMP_INTEL_EVIDENCE/lscpu.txt"
lspci -nnk | tee "$RCOMP_INTEL_EVIDENCE/lspci-nnk.txt"
rustc --version --verbose | tee "$RCOMP_INTEL_EVIDENCE/rustc.txt"
vulkaninfo --summary 2>&1 | tee "$RCOMP_INTEL_EVIDENCE/vulkan-summary.txt"
```

Also capture the installed driver packages:

```sh
dpkg-query -W \
  'mesa-vulkan-drivers' \
  'libvulkan1' \
  'vulkan-tools' 2>&1 | tee "$RCOMP_INTEL_EVIDENCE/vulkan-packages.txt"
```

The Vulkan summary must identify the Intel device. A CPU implementation named
`llvmpipe` or `lavapipe` does not qualify the NUC. Check render-node access if
the Intel device is absent:

```sh
ls -l /dev/dri 2>&1 | tee "$RCOMP_INTEL_EVIDENCE/dev-dri.txt"
id | tee "$RCOMP_INTEL_EVIDENCE/identity.txt"
```

Do not permanently force an ICD or change group membership without first
recording the default failure. If a local administrator corrects device
permissions or driver installation, rerun the entire capture section and note
the change.

## 5. Build and non-hardware gates

Start with the deterministic gates. They distinguish a general branch problem
from an Intel/Vulkan problem:

```sh
cargo fmt --all --check
cargo test -p rcomp-wgpu --lib --locked
cargo test -p rcomp-core --locked
cargo test -p rcomp --features wgpu --locked
cargo clippy -p rcomp-wgpu --all-targets --locked -- -D warnings
cargo clippy -p rcomp --all-targets --features wgpu --locked -- -D warnings
cargo build -p rcomp --features wgpu --release --locked
```

For full branch qualification, also run:

```sh
cargo test --workspace --all-features --exclude rcomp-desktop --locked
```

Save failures in full rather than pasting only the last line. `script` is a
convenient option when available:

```sh
script -q -e -c \
  'cargo test -p rcomp-wgpu --lib --locked' \
  "$RCOMP_INTEL_EVIDENCE/wgpu-lib-tests.log"
```

## 6. Discover and lock the Intel adapter

Use the release CLI built above:

```sh
target/release/rcomp hardware 2>&1 | \
  tee "$RCOMP_INTEL_EVIDENCE/rcomp-hardware.txt"
```

Expected shape:

```text
provider wgpu — Portable GPU (wgpu)
  device wgpu:vulkan:8086:DEVICE:ORDINAL-OR-PCI-ID
    name: Intel(...)
    backend: vulkan
    device_type: integratedgpu
    vendor_id: 0x8086
    driver: ...
    driver_info: ...
    preferred_batch_size: 67108864
    capability: lz4 encode levels=fast block=65536 maturity=Experimental
    capability: tar.lz4 encode levels=fast block=65536 maturity=Experimental
```

Copy the complete string after `device`, including the provider prefix:

```sh
RCOMP_INTEL_TARGET='wgpu:vulkan:8086:REPLACE_WITH_THE_REPORTED_ID'
```

Do not infer the numeric device ID or ordinal. On a multi-adapter system, run
the qualification once per intended Intel adapter and keep separate evidence
directories.

## 7. Physical-GPU correctness suite

The ignored suite opens the actual adapter, dispatches WGSL through Vulkan,
and decodes every result through the standard Rust LZ4 Frame decoder. Its
second test allocates an input 17 bytes larger than the adapter-selected batch
size, proving the core bridge crosses a batch boundary.

```sh
script -q -e -c \
  'cargo test -p rcomp-wgpu --test wgpu_lz4 --release --locked -- --ignored --nocapture' \
  "$RCOMP_INTEL_EVIDENCE/wgpu-hardware-tests.log"
```

Expected result: two passed tests. Treat Vulkan validation errors, device-loss
messages, kernel GPU-reset messages, or a process hang as failures even if a
retry later succeeds.

The current ignored test chooses the provider's first ranked adapter. That is
the Intel GPU on an iGPU-only NUC. If another discrete GPU is installed, the
explicit-device CLI cases below are authoritative for Intel; also record that
the generic suite ran on a different first-ranked adapter and open a follow-up
to make the test accept a target device.

Immediately inspect the current boot's kernel log for GPU faults. Some Ubuntu
configurations restrict this command; record that fact instead of weakening
system policy:

```sh
journalctl -k -b 2>&1 | grep -Ei \
  'drm|i915|xe|gpu|hang|reset|fault' | \
  tee "$RCOMP_INTEL_EVIDENCE/kernel-gpu-messages.txt"
```

## 8. CLI and interoperability qualification

Prepare small, boundary, realistic, and incompressible inputs:

```sh
RCOMP_INTEL_CORPUS="$RCOMP_INTEL_EVIDENCE/corpus"
mkdir -p "$RCOMP_INTEL_CORPUS/source-tree"
: > "$RCOMP_INTEL_CORPUS/empty.bin"
printf 'x' > "$RCOMP_INTEL_CORPUS/one-byte.bin"
head -c 65535 /dev/zero > "$RCOMP_INTEL_CORPUS/65535.bin"
head -c 65536 /dev/zero > "$RCOMP_INTEL_CORPUS/65536.bin"
head -c 65537 /dev/zero > "$RCOMP_INTEL_CORPUS/65537.bin"
head -c 134217745 /dev/urandom > "$RCOMP_INTEL_CORPUS/random-128m-plus.bin"
cp -a crates/rcomp-wgpu "$RCOMP_INTEL_CORPUS/source-tree/"
cp Cargo.toml Cargo.lock README.md "$RCOMP_INTEL_CORPUS/source-tree/"
```

For each `.bin` input, run explicit GPU compression, external validation, and
byte comparison. Repeat this block with the five filenames above:

```sh
RCOMP_INTEL_INPUT="$RCOMP_INTEL_CORPUS/65536.bin"
RCOMP_INTEL_OUTPUT="$RCOMP_INTEL_INPUT.lz4"

target/release/rcomp "$RCOMP_INTEL_INPUT" "$RCOMP_INTEL_OUTPUT" \
  --fast --force --accelerator required \
  --accelerator-device "$RCOMP_INTEL_TARGET"
lz4 -t "$RCOMP_INTEL_OUTPUT"
lz4 -dc "$RCOMP_INTEL_OUTPUT" | cmp - "$RCOMP_INTEL_INPUT"
```

Qualify the layered tar path as a separate capability:

```sh
target/release/rcomp "$RCOMP_INTEL_CORPUS/source-tree" \
  "$RCOMP_INTEL_EVIDENCE/source-tree.tar.lz4" \
  --fast --force --checksum --accelerator required \
  --accelerator-device "$RCOMP_INTEL_TARGET"

lz4 -t "$RCOMP_INTEL_EVIDENCE/source-tree.tar.lz4"
lz4 -dc "$RCOMP_INTEL_EVIDENCE/source-tree.tar.lz4" | tar -tf - > \
  "$RCOMP_INTEL_EVIDENCE/source-tree-members.txt"
sha256sum -c "$RCOMP_INTEL_EVIDENCE/source-tree.tar.lz4.sha256"

target/release/rcomp "$RCOMP_INTEL_EVIDENCE/source-tree.tar.lz4" \
  "$RCOMP_INTEL_EVIDENCE/restored" --unwrap --force
diff -ru "$RCOMP_INTEL_CORPUS/source-tree" \
  "$RCOMP_INTEL_EVIDENCE/restored"
```

If archive extraction creates one additional root directory because of the
current archive layout, compare against that directory and record the exact
tree. Do not call that a GPU codec failure when the archive bytes and contents
are correct.

Run the same GPU compression three times to different outputs and compare
their SHA-256 values. The WGSL encoder should be deterministic for fixed input,
device, driver, and commit:

```sh
for RCOMP_INTEL_RUN in 1 2 3; do
  target/release/rcomp "$RCOMP_INTEL_CORPUS/random-128m-plus.bin" \
    "$RCOMP_INTEL_EVIDENCE/determinism-$RCOMP_INTEL_RUN.lz4" \
    --fast --force --accelerator required \
    --accelerator-device "$RCOMP_INTEL_TARGET"
done
sha256sum "$RCOMP_INTEL_EVIDENCE"/determinism-*.lz4 | \
  tee "$RCOMP_INTEL_EVIDENCE/determinism-sha256.txt"
```

## 9. Selection and failure semantics

Confirm the user-visible policy, not only successful dispatch:

- `--accelerator auto` reports CPU because this capability is experimental;
- `--accelerator cpu` always reports CPU;
- `--accelerator required --best` fails because GPU LZ4 HC is not implemented;
- an invented Intel device ID fails clearly rather than selecting another GPU;
- extraction with `--accelerator required` fails as unsupported because GPU
  LZ4 decompression is not implemented;
- existing destination files remain intact after a failed required operation;
- no `.rcomp-*.tmp` sibling remains after failure or cancellation.

Example negative gate:

```sh
if target/release/rcomp "$RCOMP_INTEL_CORPUS/65536.bin" \
  "$RCOMP_INTEL_EVIDENCE/unsupported-best.lz4" \
  --best --force --accelerator required \
  --accelerator-device "$RCOMP_INTEL_TARGET"; then
  echo 'ERROR: unsupported GPU LZ4 Best unexpectedly succeeded'
  exit 1
fi
```

For cancellation, use a large local input, interrupt one required GPU run with
Ctrl-C while it is active, and verify that neither the requested output nor a
`.rcomp-*.tmp` sibling was published. Do not use a network filesystem for this
test.

## 10. Performance and resource evidence

Correctness comes first. The bundled comparison keeps one provider/context
alive for a cold GPU run and a warm GPU run, then runs CPU LZ4 and verifies both
outputs byte-for-byte:

```sh
script -q -e -c \
  'cargo run -p rcomp-wgpu --example wgpu-lz4 --release --locked' \
  "$RCOMP_INTEL_EVIDENCE/benchmark-synthetic-64m.log"

cargo run -p rcomp-wgpu --example wgpu-lz4 --release --locked -- \
  "$RCOMP_INTEL_CORPUS/random-128m-plus.bin" \
  "$RCOMP_INTEL_EVIDENCE/benchmark-random.lz4" 2>&1 | \
  tee "$RCOMP_INTEL_EVIDENCE/benchmark-random.log"
```

Also benchmark at least one realistic file of 512 MiB or larger from the NUC's
intended workload. Prefer local SSD storage and run at least five warm samples.
Record:

- input and output bytes;
- cold GPU, warm GPU, and CPU wall time and MiB/s;
- output ratio for both implementations;
- peak host memory;
- Intel GPU utilization when `intel_gpu_top` is available;
- whether the machine was plugged in and its power/performance mode;
- storage device/filesystem and whether data was already cached;
- any thermal throttling observed during repeated runs.

The current shader favors portability and produces output roughly 7% larger
than CPU LZ4 on the Apple corpus. A similar Intel ratio is not automatically a
bug, but it must be measured. A result slower than CPU is valid qualification
evidence and means Intel remains excluded from `Auto`.

## 11. Optional desktop build gate

The CLI/provider qualification above is the hardware gate. After it passes,
verify that the Tauri composition root still builds with the same provider:

```sh
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  librsvg2-dev \
  libsoup-3.0-dev \
  libxdo-dev \
  libssl-dev

cargo check -p rcomp-desktop --features wgpu --locked
```

A GUI smoke test is useful but must not replace the explicit-device CLI and
hardware test suite, which produce more precise diagnostics.

## 12. Troubleshooting order

### `vulkaninfo` cannot see Intel

1. Confirm `lspci -nnk` identifies the Intel display controller and kernel
   driver (`i915` or `xe`, depending on hardware/kernel).
2. Confirm `/dev/dri/renderD*` exists and the testing user can access it.
3. Confirm the Intel ICD files exist under `/usr/share/vulkan/icd.d/`.
4. Update Ubuntu/Mesa through the normal supported channel and reboot if the
   kernel or graphics stack changed.
5. Preserve the failed environment capture before making changes.

### `vulkaninfo` works but `rcomp hardware` reports no adapter

Capture the complete `vulkaninfo --summary`, `rcomp hardware`, PCI IDs, and
driver versions. The provider filters CPU adapters and requires at least four
storage buffers, 64 compute invocations/workgroup, and adequate storage-buffer
and allocation limits. Do not weaken these checks without proving the shader's
actual requirements and adding a regression test.

### Discovery succeeds but device creation fails

`rcomp hardware` enumerates adapter limits before opening a logical device.
Preserve the full `DeviceRequest` error and reported limits. Check whether the
64 MiB batch's input, output, and scratch buffers fit the driver limits. Prefer
a narrowly justified Intel/Vulkan batch adjustment over a vendor-specific
shader fork.

### Device loss, hang, reset, or wrong bytes

Stop stress testing. Preserve:

- the exact input and bad output;
- SHA-256 values and input size;
- complete command output;
- `journalctl -k -b` GPU messages;
- adapter/driver inventory;
- whether failure occurs at a 64 KiB or 64 MiB boundary;
- whether it reproduces after a cold reboot.

Do not promote a CPU retry as a successful GPU test. A required-mode failure
is the authoritative signal for qualification.

## 13. Result handback template

Return a concise summary plus the evidence directory (or an archive of it):

```text
Commit:
NUC model:
CPU / Intel GPU:
PCI vendor:device:
Ubuntu / kernel:
Mesa package / Vulkan driver:
Rust version:
Reported rcomp device ID:
Reported preferred batch size:

Deterministic tests: PASS/FAIL
Physical WGPU tests: PASS/FAIL
.lz4 external interoperability: PASS/FAIL
.tar.lz4 interoperability: PASS/FAIL
Boundary/incompressible/multi-batch: PASS/FAIL
Determinism: PASS/FAIL
Cancellation/cleanup: PASS/FAIL
Selection/negative cases: PASS/FAIL
Desktop feature build: PASS/FAIL/NOT RUN

Benchmark corpus and size:
GPU cold:
GPU warm (median, sample count):
CPU (median, sample count):
GPU output bytes:
CPU output bytes:
Peak RAM / observed GPU utilization:

Kernel/Vulkan warnings:
Patches required:
Open issues:
Recommendation: correctness-qualified / blocked / needs driver-specific fix
```

## 14. Change policy after the NUC run

Keep Linux/Intel fixes inside the portable provider unless evidence proves a
shared-core defect. Preserve these invariants:

- one canonical WGSL kernel across Intel, NVIDIA, AMD, and Apple;
- no Vulkan or WGPU dependency in `rcomp-core`;
- no automatic selection without measured crossover data;
- CPU builds and CPU selection remain available;
- standard LZ4 Frame output remains externally readable;
- driver workarounds are keyed as narrowly as the evidence permits and carry a
  regression test plus the affected PCI/driver matrix.

After qualification, add the exact results to `BENCHMARKS.md`, update the
Linux/Intel checkbox in `.claude/plans/ACCELERATION_ROADMAP.md`, and keep the
raw evidence outside the repository unless it is small and useful as a stable
fixture.
