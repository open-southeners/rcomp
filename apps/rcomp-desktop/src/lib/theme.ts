/**
 * Appearance preference for Settings.
 *
 * `"auto"` (the default) follows the OS light/dark setting via the
 * `prefers-color-scheme` media query in `app.css`. `"light"` / `"dark"` pin a
 * theme regardless of the OS by setting `data-theme` on `<html>`, which
 * `app.css`'s pinned-theme rules key off. Persisted to `localStorage` (no
 * Tauri store plugin dependency) so the choice survives a restart.
 */
export type Appearance = "auto" | "light" | "dark";

const STORAGE_KEY = "rcomp:appearance";

/** Read the persisted appearance preference, defaulting to `"auto"`. */
export function loadAppearance(): Appearance {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "auto") return v;
  } catch {
    // localStorage unavailable (e.g. private browsing); fall back to auto.
  }
  return "auto";
}

/** Apply an appearance preference to the document and persist it. */
export function applyAppearance(appearance: Appearance): void {
  const root = document.documentElement;
  if (appearance === "auto") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = appearance;
  }
  try {
    localStorage.setItem(STORAGE_KEY, appearance);
  } catch {
    // Non-fatal; the preference just won't survive a restart.
  }
}
