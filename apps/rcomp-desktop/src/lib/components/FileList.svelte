<script lang="ts">
  /**
   * FileList — the always-visible centre of the workspace.
   *
   * Purely presentational: the parent maps either archive entries (Open mode)
   * or staged inputs (Compose mode) into a uniform list of {@link FileRow}s.
   * Removable rows (Compose) show an ✕ button wired to `onRemove`.
   */
  import { humanBytes } from "../types";
  import type { FileRow } from "../types";

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
  }: Props = $props();
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
        />
      {/if}
      {#if rows.length > 0}
        <span class="count">{rows.length} item{rows.length === 1 ? "" : "s"}</span>
      {/if}
    </div>
  </div>

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
