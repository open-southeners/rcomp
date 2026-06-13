<script lang="ts">
  import { onMount } from "svelte";
  import { listEntries } from "../ipc";
  import { humanBytes } from "../types";
  import type { Entry } from "../types";

  interface Props {
    path: string;
    onClose: () => void;
  }

  let { path, onClose }: Props = $props();

  let entries = $state<Entry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  onMount(async () => {
    try {
      entries = await listEntries(path);
    } catch (err: unknown) {
      const e = err as { message?: string };
      error = e.message ?? "Failed to list archive contents.";
    } finally {
      loading = false;
    }
  });
</script>

<div class="archive-contents">
  <div class="header">
    <h2 class="title">Archive Contents</h2>
    <button class="btn-close" onclick={onClose} aria-label="Close">✕</button>
  </div>

  <p class="path">{path}</p>

  {#if loading}
    <p class="state-msg">Loading…</p>
  {:else if error}
    <p class="error-msg">{error}</p>
  {:else if entries.length === 0}
    <p class="state-msg">Archive is empty.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th class="col-path">Path</th>
            <th class="col-size">Size</th>
          </tr>
        </thead>
        <tbody>
          {#each entries as entry (entry.path)}
            <tr>
              <td class="col-path">
                {entry.is_dir ? entry.path + "/" : entry.path}
              </td>
              <td class="col-size">
                {entry.is_dir ? "—" : humanBytes(entry.size)}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="entry-count">{entries.length} entries</p>
  {/if}
</div>

<style>
  .archive-contents {
    min-width: 360px;
    max-width: 620px;
    padding: 1.5rem 2rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .title {
    margin: 0;
    font-size: 1.1rem;
    color: #111;
  }

  .btn-close {
    border: none;
    background: transparent;
    font-size: 1rem;
    cursor: pointer;
    color: #6b7280;
    padding: 0.2rem 0.4rem;
    border-radius: 4px;
  }

  .btn-close:hover {
    background: #f3f4f6;
  }

  .path {
    margin: 0;
    font-size: 0.8rem;
    color: #888;
    word-break: break-all;
  }

  .state-msg {
    color: #666;
    font-size: 0.9rem;
    margin: 0;
  }

  .error-msg {
    color: #c00;
    font-size: 0.9rem;
    margin: 0;
  }

  .table-wrap {
    overflow-y: auto;
    max-height: 360px;
    border: 1px solid #e5e7eb;
    border-radius: 6px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }

  thead {
    position: sticky;
    top: 0;
    background: #f9fafb;
  }

  th {
    text-align: left;
    padding: 0.4rem 0.6rem;
    color: #6b7280;
    font-weight: 600;
    border-bottom: 1px solid #e5e7eb;
  }

  td {
    padding: 0.3rem 0.6rem;
    color: #333;
    border-bottom: 1px solid #f3f4f6;
  }

  tr:last-child td {
    border-bottom: none;
  }

  .col-path {
    word-break: break-all;
  }

  .col-size {
    text-align: right;
    white-space: nowrap;
    color: #555;
    width: 6rem;
  }

  .entry-count {
    font-size: 0.8rem;
    color: #999;
    margin: 0;
    text-align: right;
  }
</style>
