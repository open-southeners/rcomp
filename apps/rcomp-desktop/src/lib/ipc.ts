/**
 * Typed wrappers around every Tauri IPC command used by the rcomp desktop UI.
 *
 * All `invoke` calls are centralised here so that any arg-casing fix is made
 * in one place.  Top-level invoke argument keys use camelCase (Tauri 2 converts
 * them to the Rust snake_case params); DTO object fields remain snake_case to
 * match serde deserialization on the Rust side.
 */

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import type {
  InspectResult,
  Entry,
  WrapInfo,
  SidecarData,
  Report,
  ProgressEvent,
  CompressOpts,
  ExtractOpts,
} from "./types";

/**
 * Drain the file paths the app was launched with (OS "open with" / argv) and
 * mark the frontend ready for live `open-paths` events.  Call once on startup.
 */
export async function getLaunchPaths(): Promise<string[]> {
  return invoke<string[]>("get_launch_paths");
}

/**
 * Subscribe to file paths delivered by the OS while the app is running
 * (macOS `Opened` events and Windows/Linux second-launch argv forwarding).
 * Returns the unlisten function.
 */
export function onOpenPaths(cb: (paths: string[]) => void): Promise<UnlistenFn> {
  return listen<string[]>("open-paths", (event) => cb(event.payload));
}

/**
 * Subscribe to the native Help menu's "What's New" item. Returns the
 * unlisten function.
 */
export function onShowChangelog(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("show-changelog", () => cb());
}

/** Fetch the workspace changelog, pre-rendered to HTML. */
export async function getChangelog(): Promise<string> {
  return invoke<string>("get_changelog");
}

/** Fetch the running app's version, as set on the `rcomp-desktop` crate. */
export async function getAppVersion(): Promise<string> {
  return getVersion();
}

/** Subscribe to the native **File → Open Archive…** item. */
export function onMenuOpenArchive(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-open-archive", () => cb());
}

/** Subscribe to the native **File → New Archive from Files…** item. */
export function onMenuNewArchiveFromFiles(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-new-archive-files", () => cb());
}

/** Subscribe to the native **File → New Archive from Folder…** item. */
export function onMenuNewArchiveFromFolder(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-new-archive-folder", () => cb());
}

/** Subscribe to the native **File → Close Archive** item. */
export function onMenuCloseArchive(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-close-archive", () => cb());
}

/** Subscribe to the native **File → Extract Now** item. */
export function onMenuExtractNow(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-extract-now", () => cb());
}

/** Subscribe to the native **File → Compress Archive** item. */
export function onMenuCompress(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-compress", () => cb());
}

/** Subscribe to the native **Edit → Find** item. */
export function onMenuFind(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-find", () => cb());
}

/** Subscribe to the native **Edit → Cancel Job** item. */
export function onMenuCancelJob(cb: () => void): Promise<UnlistenFn> {
  return listen<void>("menu-cancel-job", () => cb());
}

/** Inspect the filesystem entry at `path`. */
export async function inspectPath(path: string): Promise<InspectResult> {
  return invoke<InspectResult>("inspect", { path });
}

/** List entries inside an archive at `path`. */
export async function listEntries(path: string): Promise<Entry[]> {
  return invoke<Entry[]>("list_entries", { path });
}

/** Return wrap-folder information for the archive at `path`. */
export async function wrapInfo(path: string): Promise<WrapInfo> {
  return invoke<WrapInfo>("wrap_info", { path });
}

/** Cancel the in-flight job identified by `jobId`. */
export async function cancelJob(jobId: string): Promise<void> {
  return invoke<void>("cancel_job", { jobId });
}

/** Read a sidecar file for the archive at `path`. */
export async function readSidecar(path: string): Promise<SidecarData> {
  return invoke<SidecarData>("read_sidecar", { path });
}

/**
 * Write a sidecar file for the archive at `output`.
 *
 * `content_sha256` is `null` for formats that have no content stream
 * (zip, 7z, rar).
 */
export async function writeSidecar(
  output: string,
  artifact_sha256: string,
  content_sha256: string | null,
): Promise<void> {
  return invoke<void>("write_sidecar", { output, artifact_sha256, content_sha256 });
}

/**
 * Compress `input` to `output`, streaming progress via `onProgress`.
 *
 * A `Channel<ProgressEvent>` is created internally and wired to `onProgress`
 * before calling invoke so the backend can start sending events immediately.
 */
export async function compress(
  jobId: string,
  input: string,
  output: string,
  opts: CompressOpts,
  onProgress: (e: ProgressEvent) => void,
): Promise<Report> {
  const channel = new Channel<ProgressEvent>();
  channel.onmessage = onProgress;
  return invoke<Report>("compress", { jobId, input, output, opts, channel });
}

/**
 * Bundle multiple `inputs` into a single archive at `output`, streaming
 * progress via `onProgress`.
 *
 * Each element of `inputs` becomes a top-level root in the resulting archive.
 * A `Channel<ProgressEvent>` is created internally and wired to `onProgress`
 * before calling invoke so the backend can start sending events immediately.
 */
export async function compressMany(
  jobId: string,
  inputs: string[],
  output: string,
  opts: CompressOpts,
  onProgress: (e: ProgressEvent) => void,
): Promise<Report> {
  const channel = new Channel<ProgressEvent>();
  channel.onmessage = onProgress;
  return invoke<Report>("compress_many", { jobId, inputs, output, opts, channel });
}

/**
 * Extract `input` into `dest`, streaming progress via `onProgress`.
 *
 * A `Channel<ProgressEvent>` is created internally and wired to `onProgress`
 * before calling invoke so the backend can start sending events immediately.
 */
export async function extract(
  jobId: string,
  input: string,
  dest: string,
  opts: ExtractOpts,
  onProgress: (e: ProgressEvent) => void,
): Promise<Report> {
  const channel = new Channel<ProgressEvent>();
  channel.onmessage = onProgress;
  return invoke<Report>("extract", { jobId, input, dest, opts, channel });
}
