<script lang="ts">
  /**
   * ChangelogView — read-only, in-app rendering of the workspace CHANGELOG.md,
   * opened from the native Help menu's "What's New" item (see
   * `onShowChangelog` in `../ipc`) so non-technical users don't have to visit
   * the GitHub repository to see what changed.
   */
  import { onMount } from "svelte";
  import { getChangelog } from "../ipc";

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let html = $state<string | null>(null);

  onMount(async () => {
    html = await getChangelog();
  });
</script>

<div class="changelog-view">
  <div class="changelog-header">
    <h2 class="title">What's New</h2>
    <button class="btn-secondary" onclick={onClose}>← Back</button>
  </div>

  <div class="changelog-body">
    {#if html === null}
      <p class="loading">Loading…</p>
    {:else}
      {@html html}
    {/if}
  </div>
</div>

<style>
  .changelog-view {
    max-width: 720px;
    width: 100%;
    margin: 0 auto;
    padding: 1.5rem 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    flex: 1;
    min-height: 0;
  }

  .changelog-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .title {
    margin: 0;
    font-size: 1.15rem;
    color: var(--text);
  }

  .btn-secondary {
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.85rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: var(--surface-hover);
  }

  .changelog-body {
    overflow-y: auto;
    min-height: 0;
    font-size: 0.9rem;
    color: var(--text-secondary);
  }

  .loading {
    color: var(--text-faint);
  }

  .changelog-body :global(h1) {
    font-size: 1.3rem;
    color: var(--text);
    margin: 0 0 0.75rem;
  }

  .changelog-body :global(h2) {
    font-size: 1.05rem;
    color: var(--text);
    margin: 1.75rem 0 0.5rem;
    padding-bottom: 0.3rem;
    border-bottom: 1px solid var(--border);
  }

  .changelog-body :global(h3) {
    font-size: 0.92rem;
    font-weight: 600;
    color: var(--text);
    margin: 1.1rem 0 0.4rem;
  }

  .changelog-body :global(p) {
    margin: 0 0 0.75rem;
    line-height: 1.55;
  }

  .changelog-body :global(ul) {
    margin: 0 0 0.75rem;
    padding-left: 1.25rem;
  }

  .changelog-body :global(li) {
    margin-bottom: 0.3rem;
    line-height: 1.5;
  }

  .changelog-body :global(a) {
    color: var(--accent);
  }

  .changelog-body :global(a:hover) {
    color: var(--accent-hover);
  }

  .changelog-body :global(code) {
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.05rem 0.3rem;
    font-size: 0.85em;
  }

  .changelog-body :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 1.5rem 0;
  }
</style>
