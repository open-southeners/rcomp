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
