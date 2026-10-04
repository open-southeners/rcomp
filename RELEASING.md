# Releasing rcomp

This document is the authoritative runbook for publishing `rcomp-core`, the
GPU provider crates `rcomp-wgpu` and `rcomp-metal`, and `rcomp` to
[crates.io](https://crates.io).  The automated path (GitHub Actions)
is preferred; the manual fallback is documented for emergencies.

## Prerequisites

Before any release:

1. **crates.io account** — you must own (or be a co-owner of) `rcomp-core`,
   `rcomp-wgpu`, `rcomp-metal`, and `rcomp` on crates.io.  First-time only: run `cargo login` locally and
   follow the prompts to obtain a token.
2. **`CARGO_REGISTRY_TOKEN` secret** — add your crates.io publish token as a
   repository secret named exactly `CARGO_REGISTRY_TOKEN` under
   *Settings → Secrets and variables → Actions*.
3. **Remote exists** — the GitHub remote must exist and the workflow file must
   be on the default branch (`main`) before a release trigger will fire.
4. **Stable Rust** — `rustup toolchain install stable` locally if you intend to
   run the manual fallback.

## Normal release path (automated)

1. **Bump the version** in `[workspace.package]` inside the root `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "0.2.0"   # was 0.1.0
   ```

   Every crate inherits this via `version.workspace = true`.  Also bump the
   `version = "…"` on the internal path dependencies (`rcomp-core` in each
   dependent crate, and `rcomp-wgpu`/`rcomp-metal` in `crates/rcomp`) so they
   match.

2. **Refresh the lockfile and commit the bump.**  `Cargo.lock` records the
   workspace members' versions, and both CI and the release workflow run
   cargo with `--locked` — a stale lockfile fails the release:

   ```
   cargo build            # updates Cargo.lock to the new version
   git add Cargo.toml Cargo.lock
   git commit -m "chore: bump version to 0.2.0"
   git push
   ```

3. **Create a GitHub release** with tag `v<version>` (e.g. `v0.2.0`).  The tag
   and the release can be created together through the GitHub UI
   (*Releases → Draft a new release → Choose a tag → Create new tag*) or via
   the CLI:

   ```
   gh release create v0.2.0 --title "v0.2.0" --generate-notes
   ```

4. **The workflow does the rest.**  `.github/workflows/release.yml` runs
   automatically:
   - Verifies the tag matches `[workspace.package].version`; fails loudly on
     mismatch before touching crates.io.
   - Packages and build-verifies all four crates together (see below).
   - Publishes `rcomp-core`, `rcomp-wgpu`, `rcomp-metal`, then `rcomp`, each
     with a short retry loop for index propagation lag.

   Monitor progress under *Actions → Release* on GitHub.

### Re-running a half-failed release

The workflow is idempotent.  If it fails mid-way (e.g. rcomp-core published but
the others did not), re-trigger by re-running the failed workflow run in the GitHub
UI.  The publish steps query the crates.io sparse index first and skip any crate
whose exact version is already present and not yanked — so re-running never
produces "already uploaded" errors.

## Pre-publish validation

`cargo package -p rcomp-core -p rcomp-wgpu -p rcomp-metal -p rcomp --locked`
packages the crates into a temporary local registry, so each crate resolves its
siblings without them being on crates.io yet, and builds every package from
its packaged sources.  CI runs this on every push (the `package` job) and the
release workflow runs it before publishing anything, so a manifest crates.io
would reject — such as a dependency on an unpublished crate — fails early.
Run it locally before tagging a release.

## Manual fallback

Use this only if the GitHub Actions workflow is unavailable (e.g. the remote
does not exist yet, or secrets are not configured).

1. Authenticate with crates.io:

   ```
   cargo login          # interactive; prompts for your API token
   # — or —
   export CARGO_REGISTRY_TOKEN=<your-token>
   ```

2. Validate packaging (see above):

   ```
   cargo package -p rcomp-core -p rcomp-wgpu -p rcomp-metal -p rcomp --locked
   ```

3. Publish in dependency order:

   ```
   cargo publish -p rcomp-core --no-verify --locked
   cargo publish -p rcomp-wgpu --no-verify --locked
   cargo publish -p rcomp-metal --no-verify --locked
   cargo publish -p rcomp --no-verify --locked
   ```

   Cargo ≥ 1.66 blocks until each crate is visible in the index before
   returning.  If one reports the crate already exists at this version,
   continue with the next.  If one fails with an index-lag error, wait 30
   seconds and retry.

4. Verify the crates appear on crates.io:

   ```
   cargo search rcomp
   ```
