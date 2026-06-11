# Releasing rcomp

This document is the authoritative runbook for publishing `rcomp-core` and
`rcomp` to [crates.io](https://crates.io).  The automated path (GitHub Actions)
is preferred; the manual fallback is documented for emergencies.

## Prerequisites

Before any release:

1. **crates.io account** — you must own (or be a co-owner of) both `rcomp-core`
   and `rcomp` on crates.io.  First-time only: run `cargo login` locally and
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

   Both crates inherit this via `version.workspace = true`, so no other
   `Cargo.toml` files need editing.

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
   - Dry-run-validates `rcomp-core` packaging (see the note below about
     `rcomp`).
   - Publishes `rcomp-core`, then waits for it to be visible in the index.
   - Publishes `rcomp --no-verify` with a short retry loop.

   Monitor progress under *Actions → Release* on GitHub.

### Re-running a half-failed release

The workflow is idempotent.  If it fails mid-way (e.g. rcomp-core published but
rcomp did not), re-trigger by re-running the failed workflow run in the GitHub
UI.  The publish steps query the crates.io sparse index first and skip any crate
whose exact version is already present and not yanked — so re-running never
produces "already uploaded" errors.

## Why `rcomp` cannot be dry-run-validated before publishing

`cargo publish --dry-run` for `rcomp` fails even locally if `rcomp-core` is not
already on crates.io.  When cargo packages `rcomp` it strips the local `path =`
from the `rcomp-core` dependency and must resolve the crate from the registry
index — even `--no-verify --offline` cannot satisfy that.  There is no way to
pre-validate `rcomp` packaging before `rcomp-core` is live.

The consequence: the first time `rcomp` is successfully packaged is the real
`cargo publish -p rcomp --no-verify` step in the workflow.  `rcomp-core` is
fully dry-run-validated as a compensating control.

## Manual fallback

Use this only if the GitHub Actions workflow is unavailable (e.g. the remote
does not exist yet, or secrets are not configured).

1. Authenticate with crates.io:

   ```
   cargo login          # interactive; prompts for your API token
   # — or —
   export CARGO_REGISTRY_TOKEN=<your-token>
   ```

2. Publish `rcomp-core` first and wait for it to be indexed:

   ```
   cargo publish -p rcomp-core --locked
   ```

   Cargo ≥ 1.66 blocks until the crate is visible in the index before
   returning.  If it returns an error about the crate already existing at this
   version, you can safely continue to step 3.

3. Publish `rcomp`:

   ```
   cargo publish -p rcomp --no-verify --locked
   ```

   `--no-verify` is required because cargo cannot package `rcomp` for
   verification before `rcomp-core` is in the index — see the section above.
   If this fails with an index-lag error, wait 30 seconds and retry.

4. Verify both crates appear on crates.io:

   ```
   cargo search rcomp
   ```
