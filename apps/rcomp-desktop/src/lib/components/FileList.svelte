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
    /** Subtitle line (e.g. the archive path or item count); optional. */
    subtitle?: string | null;
    loading?: boolean;
    error?: string | null;
    emptyMessage?: string;
    onRemove?: (key: string) => void;
  }

  let {
    title,
    rows,
    subtitle = null,
    loading = false,
    error = null,
    emptyMessage = "Nothing here yet.",
    onRemove,
  }: Props = $props();
</script>

<div class="file-list">
  <div class="header">
    <h2 class="title">{title}</h2>
    {#if rows.length > 0}
      <span class="count">{rows.length} item{rows.length === 1 ? "" : "s"}</span>
    {/if}
  </div>

  {#if subtitle}
    <p class="subtitle" title={subtitle}>{subtitle}</p>
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
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .title {
    margin: 0;
    font-size: 1rem;
    color: var(--text);
  }

  .count {
    font-size: 0.8rem;
    color: var(--text-faint);
    white-space: nowrap;
  }

  .subtitle {
    margin: 0;
    font-size: 0.78rem;
    color: var(--text-faint);
    word-break: break-all;
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
</style>
