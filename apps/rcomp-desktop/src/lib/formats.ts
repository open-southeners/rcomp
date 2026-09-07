/**
 * Format catalog for the compress format picker.
 *
 * RAR is intentionally absent — it is extract-only.
 */

export interface FormatEntry {
  /** Canonical format name understood by rcomp-core (e.g. "tar.gz", "zstd"). */
  name: string;
  /** Human-readable label for display in the UI. */
  label: string;
  /**
   * `true` for single-stream codecs (gzip, bzip2, …).
   * The silent-tar dialog is shown when `codecOnly === true` and the input is
   * a directory.
   */
  codecOnly: boolean;
}

/** All formats available for compression (rar excluded — extract-only). */
export const FORMATS: FormatEntry[] = [
  // --- Layered (tar + codec) ---
  { name: "tar.zst", label: "TAR + Zstandard (.tar.zst)", codecOnly: false },
  { name: "tar.gz", label: "TAR + Gzip (.tar.gz)", codecOnly: false },
  { name: "tar.bz2", label: "TAR + Bzip2 (.tar.bz2)", codecOnly: false },
  { name: "tar.xz", label: "TAR + XZ (.tar.xz)", codecOnly: false },
  { name: "tar.lz4", label: "TAR + LZ4 (.tar.lz4)", codecOnly: false },
  { name: "tar.br", label: "TAR + Brotli (.tar.br)", codecOnly: false },
  // --- Containers ---
  { name: "zip", label: "ZIP (.zip)", codecOnly: false },
  { name: "7z", label: "7-Zip (.7z)", codecOnly: false },
  { name: "tar", label: "TAR (.tar) — uncompressed", codecOnly: false },
  // --- Bare codecs ---
  { name: "zstd", label: "Zstandard (.zst)", codecOnly: true },
  { name: "gzip", label: "Gzip (.gz)", codecOnly: true },
  { name: "bzip2", label: "Bzip2 (.bz2)", codecOnly: true },
  { name: "xz", label: "XZ (.xz)", codecOnly: true },
  { name: "lz4", label: "LZ4 (.lz4)", codecOnly: true },
  { name: "brotli", label: "Brotli (.br)", codecOnly: true },
];

/** Available compression level presets. */
export const LEVELS = ["Fast", "Best", "Edge"] as const;
export type Level = (typeof LEVELS)[number];

/**
 * Codec catalog for the split container/codec picker (compose sidebar).
 *
 * Mirrors rcomp-core's `Codec` enum. `tarSuffix` is the extension used when
 * the codec is layered with a Tar container (e.g. `"zst"` → `"tar.zst"`).
 */
export interface CodecEntry {
  /** Canonical bare-codec format name (e.g. `"zstd"`, `"gzip"`). */
  name: string;
  label: string;
  tarSuffix: string;
}

export const CODECS: CodecEntry[] = [
  { name: "zstd", label: "Zstandard — balanced speed & ratio", tarSuffix: "zst" },
  { name: "gzip", label: "Gzip — universal compatibility", tarSuffix: "gz" },
  { name: "xz", label: "XZ — maximum compression ratio", tarSuffix: "xz" },
  { name: "bzip2", label: "Bzip2", tarSuffix: "bz2" },
  { name: "lz4", label: "LZ4 — ultra-fast decompression", tarSuffix: "lz4" },
  { name: "brotli", label: "Brotli — web & text optimization", tarSuffix: "br" },
];

/** Container catalog for the split container/codec picker. `"none"` means a bare codec (no container). */
export type ContainerName = "none" | "tar" | "zip" | "7z";

export interface ContainerEntry {
  name: ContainerName;
  label: string;
}

export const CONTAINERS: ContainerEntry[] = [
  { name: "none", label: "None" },
  { name: "tar", label: "TAR" },
  { name: "zip", label: "ZIP" },
  { name: "7z", label: "7-Zip" },
];

/** Whether `container` supports picking a separate codec — zip/7z manage their own compression internally. */
export function containerAllowsCodec(container: ContainerName): boolean {
  return container === "none" || container === "tar";
}

/** Combine a container + codec picker choice into the canonical format name rcomp-core expects. */
export function composeFormat(container: ContainerName, codec: string | null): string {
  if (container === "zip") return "zip";
  if (container === "7z") return "7z";
  if (container === "tar") {
    const suffix = CODECS.find((c) => c.name === codec)?.tarSuffix;
    return suffix ? `tar.${suffix}` : "tar";
  }
  // container === "none": a bare codec always needs an algorithm.
  return CODECS.some((c) => c.name === codec) ? (codec as string) : "zstd";
}

/** Split a canonical format name back into container + codec picker state. */
export function decomposeFormat(name: string): { container: ContainerName; codec: string | null } {
  if (name === "zip") return { container: "zip", codec: null };
  if (name === "7z") return { container: "7z", codec: null };
  if (name === "tar") return { container: "tar", codec: null };
  const tarMatch = CODECS.find((c) => name === `tar.${c.tarSuffix}`);
  if (tarMatch) return { container: "tar", codec: tarMatch.name };
  const bareMatch = CODECS.find((c) => c.name === name);
  if (bareMatch) return { container: "none", codec: bareMatch.name };
  return { container: "none", codec: "zstd" };
}

/**
 * Return the default format name for a given input.
 *
 * - Directory input → `"tar.zst"` (safe layered default).
 * - File input → `"zstd"` (simple single-file codec default).
 */
export function defaultFormat(isDir: boolean): string {
  return isDir ? "tar.zst" : "zstd";
}

/** Return the file extension (including the dot) for a given canonical format name. */
export function extensionFor(formatName: string): string {
  const map: Record<string, string> = {
    "tar.zst": ".tar.zst",
    "tar.gz": ".tar.gz",
    "tar.bz2": ".tar.bz2",
    "tar.xz": ".tar.xz",
    "tar.lz4": ".tar.lz4",
    "tar.br": ".tar.br",
    zip: ".zip",
    "7z": ".7z",
    tar: ".tar",
    zstd: ".zst",
    gzip: ".gz",
    bzip2: ".bz2",
    xz: ".xz",
    lz4: ".lz4",
    brotli: ".br",
  };
  return map[formatName] ?? "";
}
