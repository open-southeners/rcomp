/**
 * User preferences for Settings, beyond appearance (see `theme.ts`).
 *
 * `null` means "no explicit preference" — callers fall back to their own
 * smart default (e.g. `formats.ts`'s `defaultFormat(isDir)`) rather than a
 * fixed value. Persisted to `localStorage`, same approach as `theme.ts`.
 */

const DEFAULT_FORMAT_KEY = "rcomp:defaultFormat";

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
