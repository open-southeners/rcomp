<script lang="ts">
  /**
   * DropZone — the empty-state affordance shown before anything is open.
   *
   * Drag-and-drop is handled globally by the parent workspace (the file list is
   * always visible, so drops are accepted anywhere). This component only offers
   * the two explicit entry points: open one item, or pick several to bundle.
   */
  interface Props {
    /** Open a single file/folder (→ view/extract an archive, or compress one). */
    onOpen: () => void;
    /** Pick several files to bundle into one archive. */
    onCompressFiles: () => void;
  }

  let { onOpen, onCompressFiles }: Props = $props();
</script>

<div class="dropzone">
  <div class="dropzone-content">
    <svg class="drop-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5m-13.5-9L12 3m0 0l4.5 4.5M12 3v13.5" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
    <p class="drop-label">Drop files or folders here</p>
    <p class="drop-sub">open one to view or extract, or drop several to bundle</p>
    <div class="drop-actions">
      <button class="btn-primary" onclick={onOpen}>Open…</button>
      <button class="btn-secondary" onclick={onCompressFiles}>Compress files…</button>
    </div>
  </div>
</div>

<style>
  .dropzone {
    border: 2px dashed var(--border-input);
    border-radius: 12px;
    padding: 3rem 2rem;
    text-align: center;
    background: var(--surface-subtle);
    min-width: 360px;
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
    color: var(--text-faint);
    margin-bottom: 0.5rem;
  }

  .drop-label {
    font-size: 1.05rem;
    color: var(--text-secondary);
    margin: 0;
  }

  .drop-sub {
    color: var(--text-faint);
    margin: 0;
    font-size: 0.9rem;
  }

  .drop-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .btn-primary {
    padding: 0.45rem 1.2rem;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: 0.95rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-primary:hover {
    background: var(--accent-hover);
  }

  .btn-secondary {
    padding: 0.45rem 1.2rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.95rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: var(--surface-hover);
  }
</style>
