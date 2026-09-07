<script lang="ts">
  /**
   * FileList — the always-visible centre of the workspace.
   *
   * Purely presentational: the parent maps either archive entries (Open mode)
   * or staged inputs (Compose mode) into a uniform list of {@link FileRow}s.
   * Removable rows (Compose) show an ✕ button wired to `onRemove`.
   */
  import { onMount, onDestroy } from "svelte";
  import { humanBytes } from "../types";
  import type { FileRow } from "../types";
  import { onMenuFind } from "../ipc";

  interface Props {
    /** Title shown above the list (e.g. "Archive contents" or "Items to bundle"). */
    title: string;
    rows: FileRow[];
    /** Subtitle line (e.g. the archive path); shown truncated in the footer bar, when present. */
    subtitle?: string | null;
    loading?: boolean;
    error?: string | null;
    emptyMessage?: string;
    onRemove?: (key: string) => void;
    /** Optional summary stats shown in a footer bar below the table. */
    footerStats?: { label: string; value: string }[];
    /** Search box shown next to the title, when both are provided. */
    searchQuery?: string;
    onSearchChange?: (query: string) => void;
    /** When provided, shows an "Add Files" button in the header and a
     *  drag & drop hint above the table (Compose mode only). */
    onAddFiles?: () => void;
  }

  let {
    title,
    rows,
    subtitle = null,
    loading = false,
    error = null,
    emptyMessage = "Nothing here yet.",
    onRemove,
    footerStats,
    searchQuery,
    onSearchChange,
    onAddFiles,
  }: Props = $props();

  function handleDropHintKeydown(e: KeyboardEvent): void {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onAddFiles?.();
    }
  }

  let searchInputEl = $state<HTMLInputElement | null>(null);
  let unlistenMenu: (() => void) | null = null;

  onMount(async () => {
    unlistenMenu = await onMenuFind(() => searchInputEl?.focus());
  });

  onDestroy(() => {
    unlistenMenu?.();
  });
</script>

<div class="file-list">
  <div class="header">
    <h2 class="title">{title}</h2>
    <div class="header-right">
      {#if onSearchChange}
        <input
          class="search-input"
          type="text"
          placeholder="Search files…"
          value={searchQuery ?? ""}
          oninput={(e) => onSearchChange?.((e.currentTarget as HTMLInputElement).value)}
          bind:this={searchInputEl}
        />
      {/if}
      {#if onAddFiles}
        <button type="button" class="btn-add-files" onclick={onAddFiles}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M12 5v14M5 12h14" stroke-linecap="round" />
          </svg>
          Add Files
        </button>
      {/if}
      {#if rows.length > 0}
        <span class="count">{rows.length} item{rows.length === 1 ? "" : "s"}</span>
      {/if}
    </div>
  </div>

  {#if onAddFiles}
    <div
      class="drop-hint"
      role="button"
      tabindex="0"
      onclick={onAddFiles}
      onkeydown={handleDropHintKeydown}
    >
      <span class="drop-hint-icon" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path
            d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5m-13.5-9L12 3m0 0l4.5 4.5M12 3v13.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </span>
      <p class="drop-hint-label">Drag &amp; drop files or folders here</p>
      <p class="drop-hint-sub">or click to browse files</p>
    </div>
  {/if}

  {#if loading}
    <p class="state-msg">Loading…</p>
  {:else if error}
    <p class="error-msg">{error}</p>
  {:else if rows.length === 0}
    <p class="state-msg">{emptyMessage}</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th class="col-name">Name</th>
            <th class="col-size">Size</th>
            {#if onRemove}
              <th class="col-actions" aria-label="Actions"></th>
            {/if}
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.key)}
            <tr>
              <td class="col-name">
                <span class="row-icon">{row.isDir ? "📁" : "📄"}</span>
                {row.isDir && !row.name.endsWith("/") ? row.name + "/" : row.name}
              </td>
              <td class="col-size">
                {row.isDir || row.size === null ? "—" : humanBytes(row.size)}
              </td>
              {#if onRemove}
                <td class="col-actions">
                  {#if row.removable}
                    <button
                      class="remove"
                      onclick={() => onRemove?.(row.key)}
                      aria-label="Remove {row.name}"
                      title="Remove"
                    >
                      ✕
                    </button>
                  {/if}
                </td>
              {/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    {#if (footerStats && footerStats.length > 0) || subtitle}
      <div class="footer-bar">
        <div class="footer-stats">
          {#each footerStats ?? [] as stat, i (stat.label)}
            {#if i > 0}<span class="dot">•</span>{/if}
            <span class="stat">{stat.label}: <strong>{stat.value}</strong></span>
          {/each}
        </div>
        {#if subtitle}
          <p class="footer-path" title={subtitle}>{subtitle}</p>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .file-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 0;
    height: 100%;
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .title {
    margin: 0;
    font-size: 1rem;
    color: var(--text);
    white-space: nowrap;
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .search-input {
    width: 12rem;
    max-width: 40vw;
    padding: 0.3rem 0.55rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    font-size: 0.8rem;
    color: var(--text);
    background: var(--surface);
  }

  .search-input:focus {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .count {
    font-size: 0.8rem;
    color: var(--text-faint);
    white-space: nowrap;
  }

  .btn-add-files {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.7rem;
    border: 1px solid var(--accent-subtle-fg);
    border-radius: 8px;
    background: var(--accent-subtle-bg);
    color: var(--accent-subtle-fg);
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s, color 0.12s, border-color 0.12s;
  }

  .btn-add-files:hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
    color: var(--accent-contrast);
  }

  .btn-add-files svg {
    width: 0.9rem;
    height: 0.9rem;
  }

  .drop-hint {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.3rem;
    padding: 1.4rem 1rem;
    border: 2px dashed var(--border-input);
    border-radius: 16px;
    background: var(--surface-subtle);
    cursor: pointer;
    transition: border-color 0.12s, background 0.12s;
  }

  .drop-hint:hover {
    border-color: var(--accent);
    background: var(--accent-subtle-bg);
  }

  .drop-hint-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2.75rem;
    height: 2.75rem;
    margin-bottom: 0.15rem;
    border-radius: 999px;
    background: var(--accent-subtle-bg);
    color: var(--accent);
  }

  .drop-hint-icon svg {
    width: 1.35rem;
    height: 1.35rem;
  }

  .drop-hint-label {
    margin: 0;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .drop-hint-sub {
    margin: 0;
    font-size: 0.78rem;
    color: var(--text-faint);
  }

  .state-msg {
    color: var(--text-muted);
    font-size: 0.9rem;
    margin: 0.5rem 0;
  }

  .error-msg {
    color: var(--danger);
    font-size: 0.9rem;
    margin: 0.5rem 0;
  }

  .table-wrap {
    flex: 1;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    min-height: 0;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }

  thead {
    position: sticky;
    top: 0;
    background: var(--surface-subtle);
    z-index: 1;
  }

  th {
    text-align: left;
    padding: 0.4rem 0.6rem;
    color: var(--text-muted);
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }

  td {
    padding: 0.3rem 0.6rem;
    color: var(--text);
    border-bottom: 1px solid var(--surface-hover);
  }

  tr:last-child td {
    border-bottom: none;
  }

  .col-name {
    word-break: break-all;
  }

  .row-icon {
    margin-right: 0.35rem;
  }

  .col-size {
    text-align: right;
    white-space: nowrap;
    color: var(--text-muted);
    width: 6rem;
  }

  .col-actions {
    width: 2rem;
    text-align: center;
  }

  .remove {
    border: none;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    font-size: 0.8rem;
    padding: 0.1rem 0.3rem;
    border-radius: 4px;
  }

  .remove:hover {
    background: var(--danger-soft-bg);
    color: var(--danger-strong);
  }

  .footer-bar {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.4rem 0.1rem 0;
  }

  .footer-stats {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    flex-shrink: 0;
    font-size: 0.78rem;
    color: var(--text-muted);
  }

  .footer-stats .dot {
    margin: 0 0.4rem;
    color: var(--text-faint);
  }

  .footer-stats strong {
    color: var(--text-secondary);
    font-weight: 600;
  }

  .footer-path {
    flex: 1 1 auto;
    min-width: 0;
    margin: 0;
    text-align: right;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.78rem;
    color: var(--text-faint);
  }
</style>
