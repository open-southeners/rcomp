<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { cancelJob, onMenuCancelJob } from "../ipc";
  import { humanBytes } from "../types";
  import type { ProgressEvent } from "../types";

  interface Props {
    progress: ProgressEvent | null;
    jobId: string;
    onCancel: () => void;
  }

  let { progress, jobId, onCancel }: Props = $props();

  let cancelling = $state(false);

  let unlistenMenu: (() => void) | null = null;

  onMount(async () => {
    unlistenMenu = await onMenuCancelJob(() => void handleCancel());
  });

  onDestroy(() => {
    unlistenMenu?.();
  });

  const pct = $derived(
    progress && progress.bytes_total != null && progress.bytes_total > 0
      ? Math.min(100, (progress.bytes_done / progress.bytes_total) * 100)
      : null,
  );

  const doneStr = $derived(progress ? humanBytes(progress.bytes_done) : "0 B");
  const totalStr = $derived(
    progress && progress.bytes_total != null ? humanBytes(progress.bytes_total) : null,
  );

  async function handleCancel(): Promise<void> {
    if (cancelling) return;
    cancelling = true;
    try {
      await cancelJob(jobId);
    } catch {
      // ignore; the backend will surface cancellation via the invoke rejection
    }
    onCancel();
  }
</script>

<div class="progress-view">
  <h2 class="title">Working…</h2>

  <div class="progress-bar-track">
    {#if pct !== null}
      <div class="progress-bar-fill" style="width: {pct.toFixed(1)}%"></div>
    {:else}
      <div class="progress-bar-indeterminate"></div>
    {/if}
  </div>

  <div class="progress-info">
    <span class="bytes">
      {doneStr}{totalStr ? ` / ${totalStr}` : ""}
    </span>
    {#if pct !== null}
      <span class="pct">{pct.toFixed(0)}%</span>
    {/if}
  </div>

  {#if progress?.current_entry}
    <p class="current-entry" title={progress.current_entry}>
      {progress.current_entry}
    </p>
  {/if}

  <button class="btn-cancel" onclick={handleCancel} disabled={cancelling}>
    {cancelling ? "Cancelling…" : "Cancel"}
  </button>
</div>

<style>
  .progress-view {
    min-width: 360px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 2rem;
  }

  .title {
    margin: 0;
    font-size: 1.1rem;
    color: var(--text);
  }

  .progress-bar-track {
    width: 100%;
    height: 10px;
    background: var(--border);
    border-radius: 5px;
    overflow: hidden;
    position: relative;
  }

  .progress-bar-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 5px;
    transition: width 0.1s;
  }

  @keyframes indeterminate {
    0%   { left: -40%; width: 40%; }
    100% { left: 100%; width: 40%; }
  }

  .progress-bar-indeterminate {
    position: absolute;
    height: 100%;
    background: var(--accent);
    border-radius: 5px;
    animation: indeterminate 1.4s ease-in-out infinite;
  }

  .progress-info {
    display: flex;
    gap: 1rem;
    font-size: 0.9rem;
    color: var(--text-muted);
  }

  .pct {
    font-weight: 600;
  }

  .current-entry {
    font-size: 0.8rem;
    color: var(--text-faint);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }

  .btn-cancel {
    margin-top: 0.5rem;
    padding: 0.4rem 1.1rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.9rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-cancel:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  .btn-cancel:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
