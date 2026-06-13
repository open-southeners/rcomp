/** All shared DTO types for the rcomp desktop IPC bridge. */

/** A parsed archive/codec format descriptor. Variant strings are PascalCase. */
export interface FormatObj {
  container: string | null;
  codec: string | null;
}

/** Result of the `inspect` command. */
export interface InspectResult {
  exists: boolean;
  is_dir: boolean;
  is_archive: boolean;
  format: FormatObj | null;
  /** Canonical display name, e.g. `"tar.gz"` or `"zstd"`. Prefer this for UI. */
  format_name: string | null;
  /** File size in bytes for regular files; null for directories. */
  size: number | null;
}

/** A single archive entry returned by `list_entries`. */
export interface Entry {
  path: string;
  size: number;
  is_dir: boolean;
}

/** Wrap-folder decision returned by `wrap_info`. */
export interface WrapInfo {
  /** Number of distinct top-level roots in the archive. */
  roots: number;
  /** Suggested wrap directory name derived from the archive file name. */
  wrap_dir: string;
}

/** Parsed checksums from a sidecar file returned by `read_sidecar`. */
export interface SidecarData {
  artifact_sha256: string | null;
  content_sha256: string | null;
}

/** A Rust `Duration` as serialised by serde. */
export interface RustDuration {
  secs: number;
  nanos: number;
}

/** Report returned on successful compress or extract. */
export interface Report {
  input_bytes: number;
  output_bytes: number;
  entries: number;
  entries_excluded: number;
  duration: RustDuration;
  sha256: string | null;
  content_sha256: string | null;
}

/** Convert a RustDuration to milliseconds. */
export function durationMs(d: RustDuration): number {
  return d.secs * 1000 + d.nanos / 1e6;
}

/** A progress event streamed via the Channel during compress/extract. */
export interface ProgressEvent {
  bytes_done: number;
  bytes_total: number | null;
  current_entry: string | null;
}

/** Error shape that Tauri IPC rejects with. */
export interface IpcError {
  kind: string;
  message: string;
  data?: unknown;
}

/** Options for a compress operation. Field names are snake_case (serde). */
export interface CompressOpts {
  format?: string | null;
  level?: "Fast" | "Best" | "Edge" | null;
  overwrite: boolean;
  gitignore: boolean;
  exclude: string[];
  checksum: boolean;
}

/** Options for an extract operation. Field names are snake_case (serde). */
export interface ExtractOpts {
  format?: string | null;
  overwrite: boolean;
  verify_sha256?: string | null;
  verify_content_sha256?: string | null;
}

/**
 * Format a byte count as a human-readable string.
 * E.g. 1536 → "1.5 KB", 2097152 → "2.0 MB".
 */
export function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}
