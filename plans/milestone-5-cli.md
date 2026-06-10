# Milestone 5 — the `rcomp` CLI

Implements milestone 5 of `PLAN.md`: clap surface, inference rules, `ls`
subcommand, extraction wrapping + `--unwrap`, confirmations + `-y`, progress
bars, exit codes, shell completions + man page, assert_cmd integration tests.
Branch: `feat/m1-scaffold`.

Three sequential units. Verification for every unit (all must pass):

```
cargo build && cargo test && cargo test --workspace --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

## Binding decisions

- The CLI crate enables core's `rar` feature by default:
  `[features] default = ["rar"]`, `rar = ["rcomp-core/rar"]` in
  `crates/rcomp/Cargo.toml`; `rcomp-core` dep WITHOUT direct features so
  `--no-default-features` builds a rar-less binary.
- Exit codes: **0** success · **1** operation error (Io, AlreadyExists,
  PathTraversal, declined/non-interactive confirmation, list on bare codec) ·
  **2** usage/ambiguity (clap's default, inference rule 4, bad `--algo`).
- `-q/--quiet` suppresses ALL non-error output (bar and summary). Errors
  always go to stderr.
- New CLI deps: `clap` (derive), `anyhow`, `indicatif`, `clap_complete`,
  `clap_mangen`; dev: `assert_cmd`, `predicates`, `tempfile` (all current
  latest, workspace-deps pattern).

## Unit 1 — core helpers the CLI needs (sequential, first)

**Files:** `crates/rcomp-core/src/detect.rs`, `src/format.rs`, `src/ops.rs`,
`src/lib.rs` (re-export), `CURRENT_ISSUES.md`.

1. `detect.rs`: `pub fn split_format_suffix(file_name: &str) -> Option<(&str, Format)>`
   — longest recognized suffix, case-insensitive, returns stem + format
   (`"photos.TAR.GZ"` → `("photos", tar.gz)`). Reuses the existing extension
   table (single source of truth). Re-export from lib.rs. Tests: every table
   row family, multi-dot stems (`a.b.tar.gz` → `a.b`), no match → None.
2. `format.rs`: `impl FromStr for Format` (Err = `Error::UnknownFormat`).
   Accepts canonical names and aliases: codecs `gzip|gz`, `bzip2|bz2`, `xz`,
   `zstd|zst`, `lz4`, `brotli|br`; containers `tar`, `zip`, `7z|sevenz`,
   `rar`; layered via the extension table (delegate: try name map, then
   `detect_from_extension(&format!("x.{s}"))`). Case-insensitive. Tests incl.
   roundtrip with Display for all canonical names.
3. `ops.rs` `list()`: codec-only formats now decode + sniff the first 512
   decompressed bytes (same ustar@257 check as extract): tar found →
   `tar::list` through the decoder; otherwise UnsupportedOperation as today.
   Mirrors extract's behavior so the CLI's wrap logic can rely on `list()`
   for every input. Integration test: dir → `out.bz2` (silent-tar) →
   `list("out.bz2")` returns the tree; `list()` on a bare `.zst` of a single
   file → still UnsupportedOperation.
4. `CURRENT_ISSUES.md`: the `Codec::short_ext` visibility entry is now
   resolved by `split_format_suffix` (the CLI consumes the table through it)
   — move that entry to a `## Resolved` section with a one-line outcome.

## Unit 2 — CLI core: surface, inference, wrapping, confirmations (after 1)

**Files:** `crates/rcomp/Cargo.toml`, root `Cargo.toml` (dep versions),
`crates/rcomp/src/main.rs`, `src/cli.rs`, `src/infer.rs`, `src/run.rs`,
`crates/rcomp/tests/cli.rs` (new).

### clap surface (cli.rs, derive)

```
rcomp <INPUT> [OUTPUT] [OPTIONS]        # compress or extract, inferred
rcomp ls <ARCHIVE>                      # list entries

-a, --algo <ALGO>     force format (FromStr from unit 1; bad value → exit 2)
    --fast | --best | --edge            (ArgGroup, default --best)
-c, --compress / -x, --extract          (conflict with each other)
    --unwrap                            extract directly, skip wrap folder
-y, --yes                               auto-accept confirmations
-f, --force                             overwrite existing outputs
-q, --quiet
```

Subcommands (`ls`, plus unit 3's `completions`/`man`) coexist with the
positional default via `args_conflicts_with_subcommands = true`. Version from
crate; helpful `--help` with the PLAN.md examples in after_help.

### Inference (infer.rs — pure function + unit tests)

Input facts: flags, OUTPUT presence + `split_format_suffix(OUTPUT)`,
INPUT readable-file? + `detect(INPUT)` ok?. Rules in order:
1. `--compress`/`--extract` → obey (compress with no `-a` and no recognized
   OUTPUT suffix → usage error 2).
2. OUTPUT given with recognized suffix → Compress.
3. INPUT is a readable file whose format detects — or `--algo` is given
   (PLAN.md mystery-file rule: undetectable input + stated codec → extract) —
   → Extract (OUTPUT = dest dir, default `.`).
4. Ambiguous/neither → exit 2, message stating both readings and the flags
   that disambiguate.

### run.rs — execution

- **Compress:** options from flags (`format` = `-a` parsed, else None →
  core's extension detection; level; overwrite = `--force`).
  **Silent-tar confirmation:** input is dir AND effective format is
  codec-only → warn on stderr that the output will contain a tar archive
  (other tools expect `.tar.<ext>`); prompt `[y/N]` reading stdin; `-y`
  skips; stdin not a TTY and no `-y` → error with "pass -y" hint (exit 1);
  declined → "aborted" (exit 1).
- **Extract:** wrap decision first: `list(input)` →
  - Ok(entries): roots = distinct first path components; >1 root and no
    `--unwrap` → effective dest = `dest/<stem>` (stem from
    split_format_suffix, fallback file_stem); else dest as given.
  - Err(UnsupportedOperation) (true bare codec) → dest as given.
  Then `extract(input, effective_dest, ...)`.
- **ls:** `list(archive)`, print one line per entry:
  `{size:>12}  {path}` with trailing `/` on dirs. Bare codec → friendly
  error, exit 1.
- Map errors: clap → 2 (automatic); inference-ambiguity → 2;
  everything from core → 1 with anyhow context (AlreadyExists gets a
  "use --force" hint). main.rs stays thin: parse → run → process::exit code.
- Progress callback: unit 2 passes a no-op (bars are unit 3); `-y`, `-q`
  accepted already.

### Tests (crates/rcomp/tests/cli.rs, assert_cmd)

Compress file→`.gz`; dir→`.tar.gz`; dir→`.bz2` without `-y` non-TTY → fails
with hint; with `-y` → succeeds AND extracting it restores the tree (silent
tar end-to-end through the binary); `-a bzip2` on extensionless output;
extract `.tar.gz` → wrap folder appears for multi-root archive; single-root
archive → no extra folder; `--unwrap` → no folder; extract to explicit dest
dir; `ls` output exact format (golden assert on a small zip); bare-codec `ls`
→ exit 1; ambiguity (plain file, no output) → exit 2 + both readings in
stderr; overwrite refused → exit 1 + `--force` hint; `--force` succeeds;
`--fast/--best/--edge` accepted; bad `--algo` → exit 2.

## Unit 3 — CLI UX: progress, summary, completions, man (after 2)

**Files:** `crates/rcomp/src/ui.rs` (new), `src/run.rs`, `src/cli.rs`,
Cargo.tomls (indicatif, clap_complete, clap_mangen), `tests/cli.rs`.

- **Progress** (ui.rs): indicatif bar from the core callback. bytes_total
  Some → bytes-style bar (HumanBytes, percentage, eta) · None → spinner with
  HumanBytes counter. current_entry → bar message (truncated). Draw to
  stderr (default), so stdout stays clean. Disabled by `-q` or when stderr
  is not a TTY (indicatif handles non-TTY by default — verify).
- **Summary line** (stdout, suppressed by `-q`):
  compress: `<output>  <in> → <out> (<ratio>%)  in <elapsed>`
  extract: `extracted <entries> entries to <dest>  in <elapsed>`
  using Report fields; HumanBytes; ratio = output/input × 100, 1 decimal.
- **`rcomp completions <shell>`** (bash|zsh|fish|elvish|powershell) →
  clap_complete::generate to stdout.
- **`rcomp man`** → clap_mangen::Man render to stdout (troff).
- Tests: summary line format (regex via predicates) on a compress and an
  extract; `-q` produces NO stdout; completions bash output contains
  `_rcomp`/`rcomp` markers for each shell; man output starts with `.TH` and
  mentions every long flag; bars don't corrupt stdout (assert stdout is
  exactly summary).

## Out of scope for all units

README/LICENSE (M6), CI (M7), Ctrl-C graceful cancellation wiring (log it),
interop fixtures, `rcomp verify`. Report — do not fix — anything beyond your
unit.
