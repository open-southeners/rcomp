<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { onFileDrop } from "../dragdrop";
  import { pickFile } from "../dialogs";
  import { inspectPath } from "../ipc";
  import type { InspectResult } from "../types";

  interface Props {
    onInspected: (path: string, result: InspectResult) => void;
  }

  let { onInspected }: Props = $props();

  let isDragOver = $state(false);
  let errorMessage = $state<string | null>(null);
  let unlisten: (() => void) | null = null;

  async function handlePath(path: string): Promise<void> {
    errorMessage = null;
    try {
      const result = await inspectPath(path);
      if (!result.exists) {
        errorMessage = `Path not found: ${path}`;
        return;
      }
      onInspected(path, result);
    } catch (err: unknown) {
      const e = err as { message?: string };
      errorMessage = e.message ?? "Failed to inspect path.";
    }
  }

  async function handleBrowse(): Promise<void> {
    const path = await pickFile();
    if (path) {
      await handlePath(path);
    }
  }

  onMount(async () => {
    unlisten = await onFileDrop(async (path) => {
      isDragOver = false;
      await handlePath(path);
    });
  });

  onDestroy(() => {
    if (unlisten) unlisten();
  });
</script>

<div class="dropzone" class:drag-over={isDragOver}>
  <div class="dropzone-content">
    <svg class="drop-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5m-13.5-9L12 3m0 0l4.5 4.5M12 3v13.5" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
    <p class="drop-label">Drop a file or folder here</p>
    <p class="drop-sub">or</p>
    <button class="btn-primary" onclick={handleBrowse}>Browse…</button>
  </div>
  {#if errorMessage}
    <p class="error-msg">{errorMessage}</p>
  {/if}
</div>

<style>
  .dropzone {
    border: 2px dashed #ccc;
    border-radius: 12px;
    padding: 3rem 2rem;
    text-align: center;
    background: #fafafa;
    transition: border-color 0.15s, background 0.15s;
    min-width: 360px;
  }

  .dropzone.drag-over {
    border-color: #0070f3;
    background: #eff6ff;
  }

  .dropzone-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
  }

  .drop-icon {
    width: 3rem;
    height: 3rem;
    color: #aaa;
    margin-bottom: 0.5rem;
  }

  .drop-label {
    font-size: 1.05rem;
    color: #444;
    margin: 0;
  }

  .drop-sub {
    color: #999;
    margin: 0;
    font-size: 0.9rem;
  }

  .btn-primary {
    margin-top: 0.25rem;
    padding: 0.45rem 1.2rem;
    border: none;
    border-radius: 6px;
    background: #0070f3;
    color: #fff;
    font-size: 0.95rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-primary:hover {
    background: #0058c4;
  }

  .error-msg {
    margin-top: 1rem;
    color: #c00;
    font-size: 0.9rem;
  }
</style>
