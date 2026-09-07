<script lang="ts">
  /**
   * ExtractBar — sits above the always-visible file list in Open mode.
   *
   * Replaces the old sidebar-based extraction panel: a narrow window has no
   * room for a side panel, so archive info (format/codec, checksum sidecar
   * availability, space saved) and the destination + Extract action live in a
   * bar above the list instead. Mirrors the CLI's wrap behaviour automatically
   * (see rcomp-core's silent-tar / wrap-folder rule) via `wrap_info`.
   */
  import { onMount, onDestroy } from "svelte";
  import { pickFolder, confirmDialog } from "../dialogs";
  import { extract, wrapInfo, readSidecar, onMenuExtractNow } from "../ipc";
  import type {
    InspectResult,
    Entry,
    ProgressEvent,
    Report,
    IpcError,
    WrapInfo,
    SidecarData,
  } from "../types";

  interface Props {
    inputPath: string;
    inspect: InspectResult;
    entries: Entry[];
    onRunning: (jobId: string) => void;
    onProgress: (e: ProgressEvent) => void;
    onDone: (report: Report, dest: string, verified: boolean) => void;
    onError: (err: IpcError) => void;
    /** Called when an in-flight run should return to idle without a result
     *  (the user declined an overwrite prompt, or the job was cancelled). */
    onIdle: () => void;
  }

  let { inputPath, inspect, entries, onRunning, onProgress, onDone, onError, onIdle }: Props =
    $props();

  let wrapData = $state<WrapInfo | null>(null);
  let sidecar = $state<SidecarData | null>(null);
  let busy = $state(false);

  let unlistenMenu: (() => void) | null = null;

  onMount(async () => {
    unlistenMenu = await onMenuExtractNow(() => void handleExtractNow());
  });

  onDestroy(() => {
    unlistenMenu?.();
  });

  function dirName(p: string): string {
    const normalized = p.replace(/\\/g, "/");
    const idx = normalized.lastIndexOf("/");
    return idx > 0 ? normalized.slice(0, idx) : "";
  }

  // Defaults to the archive's own directory until the user edits it. Reruns
  // when `inputPath` changes: the component stays mounted across archives
  // opened in succession (Workspace doesn't remount it), so this can't just
  // be a one-time initializer.
  let destinationPath = $state("");
  let destinationTouched = $state(false);

  $effect(() => {
    if (!destinationTouched) destinationPath = dirName(inputPath);
  });

  $effect(() => {
    const path = inputPath;
    wrapData = null;
    sidecar = null;
    Promise.all([wrapInfo(path), readSidecar(path)])
      .then(([w, s]) => {
        wrapData = w;
        sidecar = s;
      })
      .catch(() => {
        // Non-fatal; wrapData / sidecar stay null.
      });
  });

  const hasSidecar = $derived(sidecar !== null && sidecar.artifact_sha256 !== null);

  const uncompressedEntries = $derived(entries.filter((e) => !e.is_dir));
  const totalUncompressed = $derived(
    uncompressedEntries.reduce((sum, e) => sum + e.size, 0),
  );
  const percentSaved = $derived(
    inspect.size != null && inspect.size > 0 && totalUncompressed > 0
      ? Math.max(0, (1 - inspect.size / totalUncompressed) * 100)
      : null,
  );

  function effectiveDest(base: string): string {
    return wrapData && wrapData.roots > 1 ? `${base}/${wrapData.wrap_dir}` : base;
  }

  async function chooseDestination(): Promise<void> {
    const picked = await pickFolder();
    if (picked) {
      destinationPath = picked;
      destinationTouched = true;
    }
  }

  async function handleExtractNow(): Promise<void> {
    if (busy || !destinationPath) return;
    await doExtract(destinationPath, false);
  }

  async function doExtract(base: string, forceOverwrite: boolean): Promise<void> {
    busy = true;
    const jobId = crypto.randomUUID();
    const dest = effectiveDest(base);
    onRunning(jobId);

    const opts = {
      format: inspect.format_name ?? undefined,
      overwrite: forceOverwrite,
      verify_sha256: hasSidecar ? (sidecar?.artifact_sha256 ?? null) : null,
      verify_content_sha256: hasSidecar ? (sidecar?.content_sha256 ?? null) : null,
    };

    try {
      const report = await extract(jobId, inputPath, dest, opts, onProgress);
      onDone(report, dest, hasSidecar);
    } catch (err: unknown) {
      const e = err as IpcError;
      if (e.kind === "already-exists") {
        busy = false;
        const ok = await confirmDialog(`Overwrite existing files in ${dest}?`, "Destination already exists");
        if (ok) {
          await doExtract(base, true);
        } else {
          onIdle();
        }
        return;
      }
      if (e.kind === "cancelled") {
        busy = false;
        onIdle();
        return;
      }
      if (e.kind === "checksum-mismatch") {
        busy = false;
        const data = e.data as { digest?: string } | undefined;
        const digestLabel = data?.digest ?? "checksum";
        onError({ ...e, message: `${digestLabel} mismatch — the archive may be corrupt or tampered with.` });
        return;
      }
      busy = false;
      onError(e);
    }
  }
</script>

<div class="extract-bar">
  <div class="badges-row">
    {#if inspect.format?.container}
      <span class="badge">{inspect.format.container}</span>
    {/if}
    {#if inspect.format?.codec}
      <span class="badge badge-accent">{inspect.format.codec}</span>
    {/if}
    {#if hasSidecar}
      <span class="badge badge-info">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        SHA-256 available
      </span>
    {/if}
    {#if percentSaved !== null}
      <span class="badge">{percentSaved.toFixed(1)}% saved</span>
    {/if}
  </div>

  <div class="destination-row">
    <span class="destination-label">Extract to</span>
    <input
      class="text-input"
      type="text"
      bind:value={destinationPath}
      oninput={() => (destinationTouched = true)}
      placeholder="Destination folder"
    />
    <button class="btn-secondary" onclick={chooseDestination}>Choose…</button>
    <button class="btn-primary" onclick={handleExtractNow} disabled={busy || !destinationPath}>
      {busy ? "Extracting…" : "Extract Now"}
    </button>
  </div>
</div>

<style>
  .extract-bar {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.7rem 0.85rem;
    margin-bottom: 0.75rem;
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 8px;
    font-size: 0.85rem;
  }

  .badges-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.15rem 0.5rem;
    border-radius: 5px;
    background: var(--surface-hover);
    color: var(--text-secondary);
    font-size: 0.75rem;
    font-weight: 600;
  }

  .badge-accent {
    background: var(--accent-subtle-bg);
    color: var(--accent-subtle-fg);
  }

  .badge-info {
    background: var(--success-icon-bg);
    color: var(--success-icon-fg);
  }

  .badge-info svg {
    width: 0.8rem;
    height: 0.8rem;
  }

  .destination-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .destination-label {
    flex-shrink: 0;
    color: var(--text-muted);
    font-weight: 600;
  }

  .text-input {
    flex: 1;
    min-width: 0;
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    font-size: 0.85rem;
    color: var(--text);
    background: var(--surface);
  }

  .text-input:focus {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .btn-secondary {
    flex-shrink: 0;
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.85rem;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: var(--surface-hover);
  }

  .btn-primary {
    flex-shrink: 0;
    padding: 0.4rem 1rem;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s;
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
