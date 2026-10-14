/**
 * Drag-and-drop helper using the Tauri webview event API.
 *
 * Registers a listener for `DragDropEvent`s on the current webview and
 * forwards each phase of an OS file drag to the caller.  Positions are
 * converted to CSS pixels so they can be fed to `document.elementFromPoint`.
 * Returns the unlisten function (a Promise that resolves to `() => void`) so
 * callers can clean up when the component is unmounted.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";

/** A drag position in CSS pixels, relative to the webview. */
export interface DragPoint {
  x: number;
  y: number;
}

/** Callbacks for each phase of an OS file drag over the window. */
export interface FileDragHandlers {
  /** Files entered the window; `paths` is always non-empty. */
  enter?: (paths: string[], at: DragPoint) => void;
  /** The drag moved within the window. */
  over?: (at: DragPoint) => void;
  /** The drag left the window or was cancelled. */
  leave?: () => void;
  /** Files were dropped; `paths` is always non-empty. */
  drop: (paths: string[], at: DragPoint) => void;
}

function toCss(position: { x: number; y: number }): DragPoint {
  const scale = window.devicePixelRatio || 1;
  return { x: position.x / scale, y: position.y / scale };
}

/**
 * Register a drag-drop listener on the current webview.
 *
 * @returns   A Promise resolving to an `unlisten` function.
 */
export function onFileDrag(handlers: FileDragHandlers): Promise<() => void> {
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    switch (p.type) {
      case "enter":
        if (p.paths.length > 0) handlers.enter?.(p.paths, toCss(p.position));
        break;
      case "over":
        handlers.over?.(toCss(p.position));
        break;
      case "leave":
        handlers.leave?.();
        break;
      case "drop":
        if (p.paths.length > 0) handlers.drop(p.paths, toCss(p.position));
        else handlers.leave?.();
        break;
    }
  });
}
