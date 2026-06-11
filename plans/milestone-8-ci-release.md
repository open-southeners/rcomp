# Milestone 8 — CI & release workflows

Implements build-order step 8 of `PLAN.md`: GitHub Actions CI (rustfmt +
clippy + test matrix on Linux/macOS/Windows) and a release workflow that
publishes `rcomp-core` then `rcomp` to crates.io when a GitHub release is
published. Written **ahead of the remote** — no GitHub repo exists yet, so
workflows are validated for syntax/shape locally and the first real run
happens once the remote is created. Branch: `feat/m1-scaffold`.

Unit A runs first (alone — it reformats the whole tree). Units B and C are
**independent** of each other (disjoint files, run in parallel) after A.

Verification for every unit (all must pass):

```
cargo build && cargo test --workspace --all-features
cargo clippy --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p rcomp-core
```

Units B/C additionally: parse every workflow file with
`python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" <file>`
(actionlint is not installed).

## Facts established by local probing (don't re-derive)

- `cargo fmt --check` currently has ~201 diffs — the tree was never
  holistically formatted. CI will gate on fmt, so Unit A formats everything.
- `cargo package -p rcomp-core --allow-dirty` succeeds (46 files).
- `cargo package -p rcomp` **cannot succeed at all before `rcomp-core` is on
  crates.io** (confirmed empirically in Unit A): cargo strips the path from
  the dependency and must resolve `rcomp-core` from the index — even
  `--no-verify --offline` fails. There is NO local pre-publish validation
  possible for `rcomp` packaging. The release workflow publishes
  `rcomp-core` first (cargo ≥1.66 blocks until it is in the index), then
  runs `cargo publish -p rcomp --no-verify` — that publish is the first
  moment `rcomp` packaging can be exercised.
- Tests are already cross-platform-gated: `sha256sum` cross-checks are
  runtime-guarded, signal/`/dev/urandom` tests are `#[cfg(unix)]`, interop
  fixtures are committed (no external tools needed). Windows behavior is
  nevertheless unproven until the first real CI run — log that, don't chase it.

## Unit A — publish readiness + workspace format (first, alone)

**Files:** `crates/rcomp/Cargo.toml`; every `.rs` file (mechanical
`cargo fmt` only — zero semantic changes).

1. `crates/rcomp/Cargo.toml`: `rcomp-core = { path = "../rcomp-core",
   version = "0.1.0" }`.
2. `cargo fmt` across the workspace. No manual edits to the formatted
   output; if rustfmt produces something clippy then rejects, fix minimally
   and note it.
3. Validate packaging: `cargo package -p rcomp-core --allow-dirty`
   succeeds; `cargo package -p rcomp --allow-dirty --no-verify` is
   **expected to fail** (index resolution — see Facts); report its error
   verbatim plus anything cargo warns about (e.g. missing fields).
   *(Outcome: done — core packages with a "no documentation, homepage or
   repository" warning; rcomp fails on index resolution as predicted.)*
4. Full verification suite (above) — the reformat must not change behavior.

## Unit B — CI workflow (parallel with C, after A)

**Files:** new `.github/workflows/ci.yml` only.

Triggers: `push` to `main` + `pull_request` targeting `main`. Jobs:

- **fmt** (ubuntu-latest): `cargo fmt --all --check`.
- **clippy** (ubuntu-latest): `cargo clippy --all-targets --all-features
  -- -D warnings`.
- **docs** (ubuntu-latest): `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
  -p rcomp-core`.
- **test** (matrix: ubuntu-latest, macos-latest, windows-latest):
  `cargo test --workspace --all-features`. Matrix `fail-fast: false` so one
  OS failing doesn't mask the others on the unproven first run.
- **no-default-features** (ubuntu-latest): `cargo build
  --no-default-features -p rcomp` + `cargo test --no-default-features
  -p rcomp-core` (the rar-less build must keep compiling and core must pass
  without optional features).

Conventions: `dtolnay/rust-toolchain@stable` (with `components: rustfmt,
clippy` where needed), `actions/checkout@v4`, `Swatinem/rust-cache@v2`
(per-job, matrix-keyed), `CARGO_TERM_COLOR: always`. Keep it boring and
readable — no YAML anchors, no reusable-workflow indirection. A short
comment header noting the repo has no remote yet and this runs on first
push.

## Unit C — release workflow + releasing doc (parallel with B, after A)

**Files:** new `.github/workflows/release.yml`, new `RELEASING.md`,
`README.md` (one short "Releasing"/install note only if it improves the
existing Install section — optional).

- **Trigger:** `release: types: [published]` (PLAN.md: a tagged GitHub
  release drives publishing). Expected tag form `v<version>` (e.g.
  `v0.1.0`).
- **Job `publish`** (ubuntu-latest, `environment` not needed):
  1. checkout + stable toolchain.
  2. **Tag/version guard:** extract the tag (`github.event.release.tag_name`),
     strip the leading `v`, compare against the workspace version from
     `cargo metadata` (or `cargo pkgid`-style parse). Mismatch → fail fast
     with a clear message before anything is published.
  3. Sanity: `cargo publish -p rcomp-core --dry-run` (full verify works for
     core). NO pre-publish sanity step for `rcomp` — packaging it is
     impossible until `rcomp-core` is in the index (see Facts).
  4. `cargo publish -p rcomp-core` with
     `CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}` — cargo
     blocks until the crate is in the index.
  5. `cargo publish -p rcomp --no-verify` with a small retry loop
     (3 attempts, 30 s apart) as belt-and-braces against index propagation
     lag.
- **`RELEASING.md`:** the release runbook — prerequisites (crates.io
  account, `CARGO_REGISTRY_TOKEN` repo secret, remote exists), normal path
  (bump `[workspace.package].version`, commit, tag `v<version>`, publish a
  GitHub release → workflow does the rest), and the **manual fallback** from
  PLAN.md (guided `cargo publish -p rcomp-core` then
  `cargo publish -p rcomp --no-verify` with the token in the environment),
  plus the note that `rcomp` can't be `--dry-run`-verified before
  `rcomp-core` is published.

## Out of scope for all units

Prebuilt release binaries / cargo-binstall / Homebrew (PLAN_EXTRAS),
attaching completions/man artifacts to releases, `repository` Cargo.toml
field and README badges (need the remote URL — log as follow-up),
coverage/audit/dependabot jobs. Report — do not fix — anything beyond your
unit.
