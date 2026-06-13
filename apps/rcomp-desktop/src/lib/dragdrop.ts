/**
 * Drag-and-drop helper using the Tauri webview event API.
 *
 * Registers a listener for `DragDropEvent`s on the current webview and calls
 * `cb` with the first dropped path whenever the user drops files.  Returns the
 * unlisten function (a Promise that resolves to `() => void`) so callers can
 * clean up when the component is unmounted.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";

/**
 * Register a drag-drop listener on the current webview.
 *
 * @param cb  Called with the first path whenever a file or folder is dropped.
 * @returns   A Promise resolving to an `unlisten` function.
 */
export function onFileDrop(cb: (path: string) => void): Promise<() => void> {
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "drop") {
      const paths = event.payload.paths;
      if (paths && paths.length > 0) {
        cb(paths[0]);
      }
    }
  });
}
