/**
 * User preferences for Settings, beyond appearance (see `theme.ts`).
 *
 * `null` means "no explicit preference" — callers fall back to their own
 * smart default (e.g. `formats.ts`'s `defaultFormat(isDir)`) rather than a
 * fixed value. Persisted to `localStorage`, same approach as `theme.ts`.
 */

const DEFAULT_FORMAT_KEY = "rcomp:defaultFormat";
const ACCELERATION_KEY = "rcomp:acceleration";

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
