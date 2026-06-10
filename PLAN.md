# rcomp — V1 Plan

Unify the most popular file compression/archive formats behind one user-friendly
tool: a reusable Rust library (`rcomp-core`) plus a CLI (`rcomp`). The library is
the product; the CLI is its first consumer, a future Tauri desktop app the second.

## Goal

```
rcomp /path/to/folder compressed.bz2     # picks bzip2 from the extension, tars the folder transparently
rcomp big.iso big.iso.zst --edge         # max zstd compression
rcomp archive.7z                         # detects 7z by magic bytes, extracts to cwd
rcomp archive.7z ~/restored              # ...or into an explicit destination folder
rcomp ls archive.zip                     # list entries without extracting
rcomp weird-file -a bzip2                # force an algorithm explicitly
```

## Non-goals (V1)

The Tauri app itself, async API, stdin/stdout piping, multi-input batching,
passwords/encryption, custom parallel codec implementations (pigz-style),
release distribution (prebuilt binaries, Homebrew). All tracked in
`PLAN_EXTRAS.md`.

## Locked decisions

| Decision | Choice |
|---|---|
| V1 formats | gzip, bzip2, xz, zstd, lz4, brotli (codecs) · tar + tar.\* combos, zip, 7z (archives) · rar **extract-only** |
| Folder → single-stream target | Silently tar first, keep the exact output name the user gave (`out.bz2` contains tar-then-bzip2 data) |
| Compress vs extract | Inferred from arguments; `--compress` / `--extract` flags resolve ambiguity |
| Execution model | Synchronous core + progress callback + cancellation token (Tauri will run it on a worker thread) |
| Crate names | `rcomp` (CLI) + `rcomp-core` — `rcomp` confirmed available on crates.io; project will be open source |
| License | Dual **MIT OR Apache-2.0** (Rust convention; MIT preferred, Apache-2.0 accepted). README must note the `rar` feature binds the freeware unrar library |
| Publishing | No placeholder reservation. First GitHub release drives crates.io publishing via CI; manual guided publish as fallback. Prerequisite: crates.io account + `CARGO_REGISTRY_TOKEN` repo secret |
| Silent-tar naming | Keep the user's name verbatim, but warn + ask confirmation before compressing; `-y` auto-accepts |
| `--edge` meaning | Highest output *ratio* the codec can produce — hardware cost is explicitly not part of the level contract |
| Extraction wrap | Auto-wrap loose multi-entry archives into `./<archive-stem>/`; `--unwrap` disables |
| Checksum | Opt-in `--checksum`: SHA-256 of the **compressed output** plus, where a single pre-compression stream exists, a **content digest** — both computed streaming, carried in `Report`; CLI writes a `sha256sum`-compatible sidecar `<output>.sha256` (content digest as a `#` comment line) — decided 2026-06-10 |
| Sidecar auto-verify | Extraction auto-verifies when `<input>.sha256` exists next to the archive: compressed digest checked **before** unpacking, content digest checked during; mismatch → error, exit 1. No flag — present sidecar means verify — decided 2026-06-10 |
| .gitignore follower | On by default for **folder** inputs: full git semantics (origin + nested `.gitignore` files, via the `ignore` crate) and the `.git` directory itself is excluded; `--all` disables all of it; CLI prints an excluded-count note (never silent omission) — decided 2026-06-10 |
| `--exclude` | Repeatable gitignore-style globs matched relative to the origin folder, applied independently of `--all` (so `--all --exclude target` works); counts into the same excluded-count note — decided 2026-06-10 |

## Workspace layout

```
rcomp/
├── Cargo.toml              # [workspace]
├── crates/
│   ├── rcomp-core/         # the library — zero CLI dependencies
│   └── rcomp/              # the CLI binary, depends on rcomp-core
└── tests/fixtures/         # known-good archives created by reference tools
```

New standalone git repo (`git init` inside `rcomp/`), like the other projects
under the shared workspace umbrella.

## Core library design (`rcomp-core`)

### Two-layer format model

- **Codec** — single-stream compressor exposing `Read`/`Write` adapters:
  gzip, bzip2, xz, zstd, lz4, brotli.
- **Archive** — multi-entry container: tar, zip, 7z, rar (read-only).
- **Composition** — `tar.gz`, `tar.bz2`, `tar.xz`, `tar.zst`, `tbz2`, `tgz`,
  `txz` etc. = tar archive piped through a codec. zip/7z/rar compress internally.

### Operation matrix

| Input | Target ext | Behavior |
|---|---|---|
| file | codec (`.bz2`) | straight stream compression |
| folder | codec (`.bz2`) | silent tar → codec, output name kept verbatim |
| file/folder | archive (`.zip`, `.7z`, `.tar`, `.tar.*`) | build archive |
| codec stream | (extract) | decompress to file with the codec extension stripped |
| archive | (extract) | unpack entries to destination |
| tar-inside-codec | (extract) | detected via magic bytes of the decompressed stream; decompress + unpack in one pass |

### Format detection

- **Compressing:** by output extension (longest-suffix match so `.tar.gz` wins
  over `.gz`); `-a/--algo` overrides.
- **Extracting:** by magic bytes first (own small magic table), extension as
  fallback/tiebreaker. This is what makes `rcomp mystery-download` work.
- **Extensionless / unrecognized inputs:** codec via magic bytes; if still
  undetectable the user must pass `--algo`. Output name: gzip's embedded
  original-filename header when present, otherwise `<input>.out`.

### Public API sketch

```rust
pub enum Codec { Gzip, Bzip2, Xz, Zstd, Lz4, Brotli }
pub enum Container { Tar, Zip, SevenZ, Rar }
pub struct Format { /* codec and/or container, e.g. TarGz = Tar + Gzip */ }

pub enum Level { Fast, Best, Edge }   // Best is the default

pub struct Progress { pub bytes_done: u64, pub bytes_total: Option<u64>, pub current_entry: Option<String> }
pub struct CancelToken(Arc<AtomicBool>);  // checked between chunks
pub struct Entry { pub path: PathBuf, pub size: u64, pub is_dir: bool }

pub fn detect(path: &Path) -> Result<Format>;
pub fn list(archive: &Path) -> Result<Vec<Entry>>;
pub fn compress(input: &Path, output: &Path, opts: &CompressOptions,
                on_progress: impl FnMut(&Progress)) -> Result<Report>;
pub fn extract(input: &Path, dest: &Path, opts: &ExtractOptions,
               on_progress: impl FnMut(&Progress)) -> Result<Report>;
```

Errors: one `thiserror` enum (`UnknownFormat`, `UnsupportedOperation` — e.g.
rar compression, `Io`, `Cancelled`, `PathTraversal`, …). `Report` carries input
size, output size, ratio, duration, entry count.

### Output checksum (opt-in) + sidecar auto-verify

- `CompressOptions` gains `checksum: bool` (default `false`). When set, the
  output stream is teed through a SHA-256 hasher **while being written** — no
  second read pass — and `Report` gains `sha256: Option<String>` (lowercase
  hex digest of the compressed artifact).
- **Content digest:** where the operation has a single pre-compression byte
  stream — file → codec, folder → silent-tar → codec, and `tar.*` targets
  (the tar stream) — that stream is teed through a second hasher;
  `Report.content_sha256: Option<String>`. zip/7z compress entries
  individually, so they have no canonical content stream and emit no content
  digest (`None`).
- The core only computes and returns digests. The sidecar is the consumer's
  job: the CLI writes `<output>.sha256` next to the output —

  ```
  # content-sha256: <hex>          ← only when a content digest exists
  <hex>  <output-file-name>
  ```

  `sha256sum -c` verifies the artifact line and ignores the `#` comment;
  rcomp parses both. The summary line shows the artifact digest.
- The artifact digest covers the compressed bytes (transport verification);
  the content digest proves a later extraction reproduces the original
  stream — and is the only integrity check brotli can ever have (no internal
  checksum).
- **Auto-verify on extraction:** when extracting `<input>` and a sibling
  `<input>.sha256` exists, the CLI parses it and passes the expected digests
  via `ExtractOptions` (`verify_sha256` / `verify_content_sha256`,
  `Option<String>`). Core checks the artifact digest **before** unpacking
  (streaming pre-pass over the input) and the content digest **during**
  extraction (tee on the decompressed stream, compared at the end). Mismatch
  → `Error::ChecksumMismatch` (new variant: which digest, expected, actual),
  CLI exit 1. An existing but unparseable sidecar is an error, not a skip.
  No sidecar → no verification, exactly as before.
- Sidecar collisions on compress follow the existing overwrite policy:
  refuse without `--force`.
- Crate: `sha2` (pure Rust, RustCrypto).

### .gitignore-aware folder compression

- Applies only when the **input is a directory** (any target: archive build
  or the silent-tar path — both walk the tree through one shared walker).
- `CompressOptions` gains `follow_gitignore: bool` (default **true**). When
  true, the walk uses the `ignore` crate (ripgrep's) configured for:
  - the origin folder's `.gitignore` plus **nested** `.gitignore` files in
    subdirectories (full git semantics);
  - the `.git` directory itself excluded — the archive contains what a fresh
    checkout's working tree would;
  - hidden files **included** (dotfiles like `.editorconfig` belong in an
    archive; only ignore rules and `.git` exclude things);
  - **no** parent-directory `.gitignore` traversal, no global gitignore, no
    `.git/info/exclude` — archives must be reproducible from the folder
    alone, not depend on machine-local git config.
- A folder with no `.gitignore` anywhere behaves exactly as before.
- **`--exclude <GLOB>`** (repeatable): gitignore-style globs matched relative
  to the origin folder, fed to the same walker via the `ignore` crate's
  override builder. `CompressOptions.exclude: Vec<String>`. Applied
  **independently** of `follow_gitignore`, so `--all --exclude target/`
  means "everything except target/". An invalid glob is a usage error
  (exit 2).
- `Report` gains `entries_excluded: u64` — one combined count across both
  exclusion sources.
- CLI: `--all` sets `follow_gitignore = false`. When anything was excluded,
  the summary adds one note line naming whichever sources applied, e.g.
  `excluded N paths via .gitignore (use --all to include)` /
  `excluded N paths via .gitignore and --exclude` — exclusion is never
  silent (except under `-q`, which the user explicitly asked for; the note
  follows the same rule as all non-error output).
- Crate: `ignore` (core dependency — the library is the product; the future
  Tauri app gets the same behavior).

### Unified level mapping

| Codec | `--fast` | `--best` (default) | `--edge` |
|---|---|---|---|
| gzip | 1 | 6 | 9 |
| bzip2 | 1 | 6 | 9 |
| xz | 1 | 6 | 9 + extreme |
| zstd | 1 | 3 | 22 + long-distance matching |
| brotli | 2 | 6 | 11 + large window |
| lz4 | 1 | 6 | 12 (HC) |
| zip (deflate) | 1 | 6 | 9 |
| 7z (LZMA2) | 1 | 5 | 9 |

The unified levels describe the **output ratio**, never the hardware cost:
`--edge` turns on every ratio-improving feature a codec offers, and
codec-native multithreading (zstd workers, xz threads) is used whenever
available at every level since it doesn't change the result.

### Crates

| Purpose | Crate | Note |
|---|---|---|
| gzip/deflate | `flate2` | |
| bzip2 | `bzip2` | |
| xz | `liblzma` | maintained successor of `xz2` |
| zstd | `zstd` | |
| lz4 | `lz4` | C bindings — required for real LZ4HC levels |
| brotli | `brotli` | |
| tar | `tar` | |
| zip | `zip` | |
| 7z | `sevenz-rust2` | read + write, maintained fork |
| rar | `unrar` | extract-only; **feature-gated** (`rar`, on by default in the CLI) because its license is freeware, not OSI |
| sha256 | `sha2` | core only, for `--checksum` |
| gitignore walk | `ignore` | core only, ripgrep's gitignore engine |
| errors | `thiserror` | core only |
| completions / man page | `clap_complete`, `clap_mangen` | CLI only, generated at build/release time |

Verify each crate's current name/version on crates.io at scaffold time.

## CLI (`rcomp`)

```
rcomp <INPUT> [OUTPUT] [OPTIONS]    # compress or extract, inferred
rcomp ls <ARCHIVE>                  # list entries without extracting

OPTIONS:
  -a, --algo <ALGO>      Force algorithm/format (bzip2, zstd, 7z, tar.xz, ...)
      --fast             Fastest compression
      --best             Balanced (default)
      --edge             Maximum compression ratio, hardware expensive
  -c, --compress         Force compress mode (for re-compressing a .gz, etc.)
  -x, --extract          Force extract mode
      --unwrap           Extract entries directly into the destination
                         (skip the auto-wrap folder)
      --checksum         Write a sha256sum-format sidecar <OUTPUT>.sha256
                         and show the digest in the summary (compress only;
                         on extraction a sidecar is auto-verified when found)
      --all              Ignore .gitignore rules: include every file in the
                         input folder, .git included (compress only)
      --exclude <GLOB>   Exclude paths matching a gitignore-style glob,
                         relative to the input folder; repeatable; works
                         with or without --all (compress only)
  -y, --yes              Auto-accept all confirmation prompts
  -f, --force            Overwrite existing output
  -q, --quiet            No progress output
```

The `ls` subcommand coexists with the flag-style default invocation via clap's
`args_conflicts_with_subcommands`.

### Inference rules (in order)

1. `--compress`/`--extract` given → obey.
2. `OUTPUT` given with a recognizable compression/archive extension → compress.
3. `INPUT` is a **readable file** recognized as compressed/archive (magic
   bytes) → extract; `OUTPUT`, when given, is the destination *directory*
   (created if missing), otherwise the current working directory.
4. Otherwise → error listing both interpretations and the flag to pick one.

### Extraction wrapping

- If the archive's contents are **not** already self-contained — i.e. multiple
  loose top-level entries — wrap them in a new folder named after the archive
  file minus its extensions (`photos.tar.gz` → `./photos/`).
- A single top-level folder, a single file, or a bare codec stream extracts
  directly into the destination — no extra nesting.
- `--unwrap` forces direct extraction into the destination regardless.
- Never overwrite existing files without `--force`.

### Confirmations

- Silent-tar case (`rcomp ./folder out.bz2`): proceed with the user's exact
  name, but warn that the content is tar-wrapped (other tools expect
  `.tar.bz2`) and ask for confirmation first.
- `-y/--yes` auto-accepts every prompt (Linux convention). In non-interactive
  contexts (no TTY), prompts fail with a hint to pass `-y`.

### UX details

- Progress bar via `indicatif`, fed by the core's callback; final summary line
  with sizes, ratio, and elapsed time.
- Exit codes: 0 ok, 1 operation error, 2 usage/ambiguity error.
- `clap` derive API; `anyhow` at the binary boundary only.
- Shell completions (bash/zsh/fish) via `clap_complete` and a man page via
  `clap_mangen`.

## Safety behaviors (extraction)

- Zip-slip / path-traversal rejection: every entry path is normalized and must
  resolve inside the destination.
- Absolute paths and `..` components in entries → error, not silent skip.
- Symlink entries: created only if the target stays inside the destination.
- These rules live in one shared "sanitize entry path" function used by all
  archive backends, with dedicated tests.

## Testing

- **Roundtrip unit tests** per codec and archive type (compress → extract →
  byte-identical), including folder→`.bz2`→folder via the silent-tar path.
- **Interop fixtures**: small archives created by reference tools (gzip, 7z,
  WinRAR sample, Info-ZIP) committed under `tests/fixtures/`; rcomp must extract
  them correctly. Proves we read real-world files, not just our own output.
- **Detection tests**: magic-byte table against every fixture, including
  extensionless files.
- **Security tests**: crafted traversal/symlink archives must be rejected.
- **CLI integration** via `assert_cmd`: inference rules, ambiguity errors,
  overwrite refusal, exit codes.
- **Checksum tests**: sidecar verifies with `sha256sum -c` (comment line
  ignored by coreutils); digests in `Report` match independently computed
  hashes; content digest present for codec/tar paths and `None` for zip/7z;
  sidecar respects the overwrite policy.
- **Auto-verify tests**: extraction with a valid sidecar succeeds; corrupted
  archive + sidecar → `ChecksumMismatch` before any file is written;
  tampered content digest → mismatch error during extraction; unparseable
  sidecar → error; absent sidecar → behavior unchanged; roundtrip
  compress `--checksum` → extract verifies both digests end-to-end.
- **Gitignore/exclude tests**: nested `.gitignore` honored; `.git` excluded;
  hidden non-ignored dotfiles included; `--all` includes everything;
  `--exclude` filters with and without `--all`; invalid glob → exit 2;
  folder without `.gitignore` unchanged; excluded-count note appears with
  the right source names (and never appears when nothing was excluded);
  silent-tar path filters identically to archive paths.

## Build order

1. **Scaffold** — workspace, two crates, `git init`, error/progress/cancel
   types, `Format`/`Level` model, detection (extension + magic table) with tests.
2. **Codecs** — all six stream codecs behind one trait, level mapping, roundtrip
   tests.
3. **Archives** — tar + tar.codec composition (incl. silent-tar-folder rule),
   zip; entry-path sanitizer with security tests.
4. **7z + rar** — sevenz-rust2 read/write; unrar extract behind the `rar` feature.
5. **CLI** — clap surface, inference rules, `ls` subcommand, wrapping +
   `--unwrap`, confirmation prompts + `-y`, progress bars, exit codes, shell
   completions + man page, integration tests.
6. **Hardening & docs** — interop fixtures, overwrite policy edge cases,
   README with usage examples (incl. the unrar license note), rustdoc on the
   public API, `LICENSE-MIT` + `LICENSE-APACHE` files.
7. **Checksum & gitignore** — `--checksum` SHA-256 sidecar with artifact +
   content digests (core digests in `Report`, sidecar + summary in the CLI)
   and sidecar auto-verify on extraction (`ChecksumMismatch` error path);
   `.gitignore`-aware folder walks through one shared walker (`ignore`
   crate, nested gitignores, `.git` excluded, hidden files kept) with
   `--all` opt-out, repeatable `--exclude` globs, and the excluded-count
   note; README + rustdoc updates for all of it.
8. **CI & release** — GitHub Actions: rustfmt + clippy + test matrix on
   Linux/macOS/Windows (Windows coverage matters before the Tauri app, which
   is cross-desktop), plus a release workflow that publishes `rcomp-core` then
   `rcomp` to crates.io when a GitHub release is tagged (needs the
   `CARGO_REGISTRY_TOKEN` secret; manual guided `cargo publish` as fallback).

Each step compiles and has green tests before the next begins.
