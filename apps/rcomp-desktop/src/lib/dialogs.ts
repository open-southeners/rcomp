/**
 * Thin wrappers over the Tauri dialog plugin.
 *
 * All dialog interactions are centralised here so the UI components stay free
 * of plugin-specific imports.
 */

import { open, save, confirm } from "@tauri-apps/plugin-dialog";

/** Open a single file picker.  Returns `null` when the user cancels. */
export async function pickFile(): Promise<string | null> {
  const result = await open({ multiple: false, directory: false });
  if (Array.isArray(result)) return result[0] ?? null;
  return result;
}

/** Open a folder picker.  Returns `null` when the user cancels. */
export async function pickFolder(): Promise<string | null> {
  const result = await open({ multiple: false, directory: true });
  if (Array.isArray(result)) return result[0] ?? null;
  return result;
}

/**
 * Open a save-file dialog with an optional suggested default name.
 * Returns `null` when the user cancels.
 */
export async function pickSavePath(defaultName?: string): Promise<string | null> {
  return save({ defaultPath: defaultName });
}

/**
 * Show a native confirm dialog (Ok / Cancel).
 * Returns `true` when the user clicks Ok.
 */
export async function confirmDialog(message: string, title?: string): Promise<boolean> {
  return confirm(message, title ? { title } : undefined);
}
