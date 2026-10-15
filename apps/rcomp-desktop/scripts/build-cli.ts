/**
 * Build the `rcomp` command-line tool for bundling inside the desktop app.
 *
 * Run by Tauri before the app is compiled (`beforeBuildCommand`, and on
 * Windows also `beforeDevCommand`), so it reads the target and profile Tauri
 * is building for from `TAURI_ENV_TARGET_TRIPLE` / `TAURI_ENV_DEBUG`. The
 * binary is copied to `<workspace>/target/rcomp-cli/rcomp[.exe]`, the fixed
 * path the platform bundle configs pick it up from:
 *
 * - macOS: `Contents/MacOS/rcomp-cli` (`tauri.macos.conf.json`, signed by
 *   `scripts/macos-sign.sh` before bundling). A universal build lipo-merges
 *   both Apple targets.
 * - Windows: `<install dir>\bin\rcomp.exe` (`tauri.windows.conf.json`).
 * - Linux: `/usr/bin/rcomp` in the .deb/.rpm and `usr/bin/rcomp` in the
 *   AppImage (`tauri.conf.json`).
 *
 * The desktop app links it onto the user's PATH from Settings — see
 * `src-tauri/src/cli_link.rs`.
 */

import { copyFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../../..");
const out = join(root, "target", "rcomp-cli");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const triple = process.env.TAURI_ENV_TARGET_TRIPLE;
const debug = process.env.TAURI_ENV_DEBUG === "true";
const profileDir = debug ? "debug" : "release";
const windows = triple ? triple.includes("windows") : process.platform === "win32";
const exe = windows ? "rcomp.exe" : "rcomp";

function run(cmd: string[]): void {
  console.log(`build-cli: ${cmd.join(" ")}`);
  const result = Bun.spawnSync(cmd, { cwd: root, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) process.exit(result.exitCode ?? 1);
}

/** Build the CLI for `target` (host when undefined) and return the binary's path. */
function build(target: string | undefined): string {
  const cmd = ["cargo", "build", "-p", "rcomp", "--locked"];
  if (!debug) cmd.push("--release");
  if (target) cmd.push("--target", target);
  run(cmd);
  return target ? join(targetDir, target, profileDir, exe) : join(targetDir, profileDir, exe);
}

mkdirSync(out, { recursive: true });
const dest = join(out, exe);

if (triple === "universal-apple-darwin") {
  const slices = ["aarch64-apple-darwin", "x86_64-apple-darwin"].map(build);
  run(["lipo", "-create", ...slices, "-output", dest]);
} else {
  copyFileSync(build(triple), dest);
}

console.log(`build-cli: ${dest}`);
