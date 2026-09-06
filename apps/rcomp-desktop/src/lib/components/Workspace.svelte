<script lang="ts">
  /**
   * Workspace — the archive-centric root content of rcomp desktop.
   *
   * The always-visible `FileList` IS the archive:
   *   - **open**    — the list shows the contents of an opened archive; an
   *                   `ExtractBar` above it (not a side panel — narrow windows
   *                   have no room for one) offers extraction.
   *   - **compose** — the list shows the files being assembled into one new
   *                   archive; a side panel sets global params and compresses.
   *   - **empty**   — a drop/browse affordance.
   *
   * An in-flight operation (`run`) and the post-operation `result` render in a
   * side panel (shared by both modes) so the file list stays in view
   * throughout.
   */
  import { onMount, onDestroy } from "svelte";
  import DropZone from "./DropZone.svelte";
  import FileList from "./FileList.svelte";
  import ComposePanel from "./ComposePanel.svelte";
  import ExtractBar from "./ExtractBar.svelte";
  import ProgressView from "./ProgressView.svelte";
  import SummaryView from "./SummaryView.svelte";
  import { onFileDrop } from "../dragdrop";
  import { pickFile, pickFiles, pickFolder } from "../dialogs";
  import { inspectPath, listEntries, getLaunchPaths, onOpenPaths } from "../ipc";
  import { humanBytes } from "../types";
  import type {
    InspectResult,
    Entry,
    StagedItem,
    FileRow,
    ProgressEvent,
    Report,
    IpcError,
  } from "../types";

  type WMode = "empty" | "open" | "compose";

  let mode = $state<WMode>("empty");
  let searchQuery = $state("");

  // --- open mode ---
  let archivePath = $state<string | null>(null);
  let archiveInspect = $state<InspectResult | null>(null);
  let entries = $state<Entry[]>([]);
  let entriesLoading = $state(false);
  let entriesError = $state<string | null>(null);

  // --- compose mode ---
  let composeItems = $state<StagedItem[]>([]);

  // --- run / result overlays ---
  let run = $state<{ jobId: string } | null>(null);
  let latestProgress = $state<ProgressEvent | null>(null);
  let result = $state<{
    report: Report;
    dest: string;
    kind: "compress" | "extract";
    verified: boolean;
  } | null>(null);

  let globalError = $state<string | null>(null);

  let unlistenDrop: (() => void) | null = null;
  let unlistenOpen: (() => void) | null = null;

  function baseName(p: string): string {
    return p.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? p;
  }

  // --- rows for the always-visible list ---
  const rows = $derived<FileRow[]>(
    mode === "open"
      ? entries.map((e) => ({
          key: e.path,
          name: e.path,
          size: e.size,
          isDir: e.is_dir,
          removable: false,
        }))
      : composeItems.map((it) => ({
          key: it.path,
          name: it.name,
          size: it.size,
          isDir: it.isDir,
          removable: true,
        })),
  );

  const listTitle = $derived(mode === "open" ? "Archive contents" : "Items to bundle");
  const listSubtitle = $derived(mode === "open" ? archivePath : null);
  const listEmpty = $derived(
    mode === "open" ? "Archive is empty." : "Add files or a folder to bundle.",
  );

  // Client-side filter for the search box in ExtractBar (Open mode only).
  const filteredRows = $derived(
    searchQuery
      ? rows.filter((r) => r.name.toLowerCase().includes(searchQuery.toLowerCase()))
      : rows,
  );

  // Footer stats for the archive file list: total uncompressed size and the
  // average uncompressed entry size, computed from what list_entries already
  // gives us (no extra IPC round-trip).
  const uncompressedEntries = $derived(entries.filter((e) => !e.is_dir));
  const totalUncompressed = $derived(
    uncompressedEntries.reduce((sum, e) => sum + e.size, 0),
  );
  const openFooterStats = $derived(
    mode === "open" && uncompressedEntries.length > 0
      ? [
          { label: "Original", value: humanBytes(totalUncompressed) },
          {
            label: "Avg file size",
            value: humanBytes(totalUncompressed / uncompressedEntries.length),
          },
        ]
      : undefined,
  );

  // -------------------------------------------------------------------------
  // Routing: decide open vs compose from a set of incoming paths.
  // -------------------------------------------------------------------------
  async function routePaths(paths: string[]): Promise<void> {
    if (paths.length === 0) return;
    globalError = null;

    // While composing, any new paths are added as more inputs.
    if (mode === "compose" && !run && !result) {
      await addToCompose(paths);
      return;
    }

    if (paths.length === 1) {
      let res: InspectResult;
      try {
        res = await inspectPath(paths[0]);
      } catch (err: unknown) {
        globalError = (err as { message?: string }).message ?? "Failed to inspect path.";
        return;
      }
      if (!res.exists) {
        globalError = `Path not found: ${paths[0]}`;
        return;
      }
      if (res.is_archive) {
        await openArchive(paths[0], res);
        return;
      }
    }

    // One non-archive, several items, or multiple archives → bundle.
    await startCompose(paths);
  }

  async function openArchive(path: string, inspect: InspectResult): Promise<void> {
    archivePath = path;
    archiveInspect = inspect;
    composeItems = [];
    result = null;
    searchQuery = "";
    mode = "open";

    entriesLoading = true;
    entriesError = null;
    entries = [];
    try {
      entries = await listEntries(path);
    } catch (err: unknown) {
      entriesError = (err as { message?: string }).message ?? "Failed to list archive contents.";
    } finally {
      entriesLoading = false;
    }
  }

  async function startCompose(paths: string[]): Promise<void> {
    composeItems = [];
    archivePath = null;
    archiveInspect = null;
    result = null;
    searchQuery = "";
    mode = "compose";
    await addToCompose(paths);
  }

  async function addToCompose(paths: string[]): Promise<void> {
    for (const path of paths) {
      if (composeItems.some((it) => it.path === path)) continue;
      try {
        const res = await inspectPath(path);
        if (!res.exists) {
          globalError = `Path not found: ${path}`;
          continue;
        }
        composeItems.push({
          path,
          name: baseName(path),
          isDir: res.is_dir,
          isArchive: res.is_archive,
          size: res.size,
        });
      } catch (err: unknown) {
        globalError = (err as { message?: string }).message ?? "Failed to inspect path.";
      }
    }
  }

  function removeItem(key: string): void {
    composeItems = composeItems.filter((it) => it.path !== key);
  }

  async function handleAddFiles(): Promise<void> {
    const paths = await pickFiles();
    if (paths.length > 0) await addToCompose(paths);
  }

  async function handleAddFolder(): Promise<void> {
    const path = await pickFolder();
    if (path) await addToCompose([path]);
  }

  // Empty-state affordances (DropZone buttons).
  async function handleOpen(): Promise<void> {
    const path = await pickFile();
    if (path) await routePaths([path]);
  }

  async function handleCompressFiles(): Promise<void> {
    const paths = await pickFiles();
    if (paths.length > 0) await routePaths(paths);
  }

  // -------------------------------------------------------------------------
  // Run / result handlers (shared by ExtractBar and ComposePanel).
  // -------------------------------------------------------------------------
  function handleRunning(jobId: string): void {
    run = { jobId };
    latestProgress = null;
  }

  function handleProgress(e: ProgressEvent): void {
    latestProgress = e;
  }

  function handleDoneCompress(report: Report, dest: string): void {
    result = { report, dest, kind: "compress", verified: false };
    run = null;
  }

  function handleDoneExtract(report: Report, dest: string, verified: boolean): void {
    result = { report, dest, kind: "extract", verified };
    run = null;
  }

  function handleError(err: IpcError): void {
    globalError = err.message;
    run = null;
  }

  function handleCancel(): void {
    run = null;
    latestProgress = null;
  }

  function reset(): void {
    mode = "empty";
    archivePath = null;
    archiveInspect = null;
    entries = [];
    entriesError = null;
    composeItems = [];
    run = null;
    latestProgress = null;
    result = null;
    globalError = null;
    searchQuery = "";
  }

  onMount(async () => {
    unlistenDrop = await onFileDrop((paths) => {
      void routePaths(paths);
    });

    // Subscribe to live OS "open with" deliveries BEFORE draining the startup
    // buffer, so no delivery is lost in the gap.
    unlistenOpen = await onOpenPaths((paths) => {
      void routePaths(paths);
    });

    // Drain any files the app was launched with (also flips the backend's
    // "ready" flag so subsequent deliveries arrive via the event above).
    const launch = await getLaunchPaths();
    if (launch.length > 0) await routePaths(launch);
  });

  onDestroy(() => {
    if (unlistenDrop) unlistenDrop();
    if (unlistenOpen) unlistenOpen();
  });
</script>

<div class="workspace">
  {#if globalError}
    <div class="error-banner">
      <strong>Error:</strong>
      {globalError}
      <button class="error-dismiss" onclick={() => (globalError = null)}>✕</button>
    </div>
  {/if}

  {#if mode === "empty"}
    <div class="empty-area">
      <DropZone onOpen={handleOpen} onCompressFiles={handleCompressFiles} />
    </div>
  {:else if mode === "open" && !run && !result}
    <div class="full-area">
      {#if archivePath && archiveInspect}
        <ExtractBar
          inputPath={archivePath}
          inspect={archiveInspect}
          entries={entries}
          searchQuery={searchQuery}
          onSearchChange={(q) => (searchQuery = q)}
          onRunning={handleRunning}
          onProgress={handleProgress}
          onDone={handleDoneExtract}
          onError={handleError}
          onIdle={handleCancel}
        />
      {/if}
      <FileList
        title={listTitle}
        rows={filteredRows}
        subtitle={listSubtitle}
        loading={entriesLoading}
        error={entriesError}
        emptyMessage={listEmpty}
        footerStats={openFooterStats}
      />
    </div>
  {:else}
    <div class="split">
      <section class="pane list-pane">
        <FileList
          title={listTitle}
          rows={filteredRows}
          subtitle={listSubtitle}
          loading={entriesLoading}
          error={entriesError}
          emptyMessage={listEmpty}
          onRemove={mode === "compose" ? removeItem : undefined}
        />
      </section>

      <section class="pane action-pane">
        {#if run}
          <ProgressView progress={latestProgress} jobId={run.jobId} onCancel={handleCancel} />
        {:else if result}
          <SummaryView
            report={result.report}
            kind={result.kind}
            dest={result.dest}
            verified={result.verified}
            onReset={reset}
          />
        {:else if mode === "compose"}
          <ComposePanel
            items={composeItems}
            onAddFiles={handleAddFiles}
            onAddFolder={handleAddFolder}
            onRunning={handleRunning}
            onProgress={handleProgress}
            onDone={handleDoneCompress}
            onError={handleError}
          />
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .workspace {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: var(--danger-banner-bg);
    border-bottom: 1px solid var(--danger-banner-border);
    padding: 0.6rem 1.2rem;
    color: var(--danger-banner-fg);
    font-size: 0.9rem;
  }

  .error-dismiss {
    margin-left: auto;
    border: none;
    background: transparent;
    cursor: pointer;
    color: inherit;
    font-size: 0.9rem;
    padding: 0;
  }

  .empty-area {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
  }

  .full-area {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-height: 0;
    padding: 1rem 1.2rem;
  }

  .full-area :global(.file-list) {
    flex: 1;
    min-height: 0;
  }

  .split {
    flex: 1;
    display: flex;
    gap: 1rem;
    padding: 1rem 1.2rem;
    min-height: 0;
  }

  .pane {
    min-height: 0;
  }

  .list-pane {
    flex: 1 1 55%;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .action-pane {
    flex: 0 0 340px;
    overflow-y: auto;
    border-left: 1px solid var(--border);
    padding-left: 1rem;
  }
</style>
