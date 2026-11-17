/**
 * User preferences for Settings, beyond appearance (see `theme.ts`).
 *
 * `null` means "no explicit preference" — callers fall back to their own
 * smart default (e.g. `formats.ts`'s `defaultFormat(isDir)`) rather than a
 * fixed value. Persisted to `localStorage`, same approach as `theme.ts`.
 */

const DEFAULT_FORMAT_KEY = "rcomp:defaultFormat";
const ACCELERATION_KEY = "rcomp:acceleration";
/** localStorage key for the "show hidden files" preference. */
export const SHOW_HIDDEN_KEY = "rcomp:showHidden";

export type AccelerationPreference = "cpu" | "auto" | "required";

/** Read the user's pinned default compression format, or `null` for "Auto". */
export function loadDefaultFormat(): string | null {
  try {
    return localStorage.getItem(DEFAULT_FORMAT_KEY);
  } catch {
    return null;
  }
}

/** Persist the user's default compression format preference (`null` clears it, back to "Auto"). */
export function saveDefaultFormat(format: string | null): void {
  try {
    if (format) {
      localStorage.setItem(DEFAULT_FORMAT_KEY, format);
    } else {
      localStorage.removeItem(DEFAULT_FORMAT_KEY);
    }
  } catch {
    // Non-fatal; the preference just won't survive a restart.
  }
}

/** Read the shared core acceleration preference. CPU preserves legacy behavior. */
export function loadAcceleration(): AccelerationPreference {
  try {
    const value = localStorage.getItem(ACCELERATION_KEY);
    return value === "auto" || value === "required" ? value : "cpu";
  } catch {
    return "cpu";
  }
}

/** Persist the acceleration preference used for compression and extraction. */
export function saveAcceleration(value: AccelerationPreference): void {
  try {
    localStorage.setItem(ACCELERATION_KEY, value);
  } catch {
    // Non-fatal; the preference just won't survive a restart.
  }
}

/** Whether archive listings show hidden entries (off by default). */
export function loadShowHidden(): boolean {
  try {
    return localStorage.getItem(SHOW_HIDDEN_KEY) === "true";
  } catch {
    return false;
  }
}

/** Persist the "show hidden files" preference. */
export function saveShowHidden(value: boolean): void {
  try {
    if (value) {
      localStorage.setItem(SHOW_HIDDEN_KEY, "true");
    } else {
      localStorage.removeItem(SHOW_HIDDEN_KEY);
    }
  } catch {
    // Non-fatal; the preference just won't survive a restart.
  }
}

/**
 * Whether an archive entry is hidden: a dotfile or dot-folder, macOS's
 * `__MACOSX` resource-fork folder, Windows's `Thumbs.db` / `desktop.ini`, or
 * anything inside one of those. Matches the Quick Look preview.
 */
export function isHiddenEntry(path: string): boolean {
  return path
    .replace(/\\/g, "/")
    .split("/")
    .filter((part) => part !== "" && part !== ".")
    .some((part) => {
      const lower = part.toLowerCase();
      return (
        part.startsWith(".") ||
        part === "__MACOSX" ||
        lower === "thumbs.db" ||
        lower === "desktop.ini"
      );
    });
}
