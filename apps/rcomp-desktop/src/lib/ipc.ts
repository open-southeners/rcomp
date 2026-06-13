/**
 * Typed wrappers around every Tauri IPC command used by the rcomp desktop UI.
 *
 * All `invoke` calls are centralised here so that any arg-casing fix is made
 * in one place.  Top-level invoke argument keys use camelCase (Tauri 2 converts
 * them to the Rust snake_case params); DTO object fields remain snake_case to
 * match serde deserialization on the Rust side.
 */

import { invoke, Channel } from "@tauri-apps/api/core";
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
