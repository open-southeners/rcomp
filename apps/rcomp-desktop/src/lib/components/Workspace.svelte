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
   * throughout. Settings is a separate overlay, reachable from any mode.
   */
  import { onMount, onDestroy } from "svelte";
  import DropZone from "./DropZone.svelte";
  import FileList from "./FileList.svelte";
  import ComposePanel from "./ComposePanel.svelte";
  import ExtractBar from "./ExtractBar.svelte";
  import ProgressView from "./ProgressView.svelte";
  import SummaryView from "./SummaryView.svelte";
  import SettingsView from "./SettingsView.svelte";
  import { onFileDrop } from "../dragdrop";
  import { pickFile, pickFiles, pickFolder } from "../dialogs";
  import { inspectPath, listEntries, getLaunchPaths, onOpenPaths } from "../ipc";
  import { loadAppearance, applyAppearance } from "../theme";
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
  import type { Appearance } from "../theme";

  type WMode = "empty" | "open" | "compose";

  let mode = $state<WMode>("empty");
  let showSettings = $state(false);
  let searchQuery = $state("");

  // Applied immediately (not just in an effect) so the pinned theme, if any,
  // takes effect before first paint rather than flashing the OS default.
  const initialAppearance = loadAppearance();
  applyAppearance(initialAppearance);
  let appearance = $state<Appearance>(initialAppearance);

  function setAppearance(next: Appearance): void {
    appearance = next;
    applyAppearance(next);
  }

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
  <div class="toolbar">
    <span class="mode-label">
      {#if mode === "open"}Open archive{:else if mode === "compose"}Compress{/if}
    </span>
    <div class="toolbar-actions">
      {#if mode !== "empty"}
        <button class="btn-reset" onclick={reset} title="Start over">New</button>
      {/if}
      <button
        class="btn-icon"
        onclick={() => (showSettings = true)}
        title="Settings"
        aria-label="Settings"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path
            d="M10.343 3.94c.09-.542.56-.94 1.11-.94h1.093c.55 0 1.02.398 1.11.94l.149.894c.07.424.384.764.78.93.398.164.855.142 1.205-.108l.737-.527a1.125 1.125 0 011.45.12l.774.773c.39.39.44 1.002.12 1.45l-.527.738c-.25.35-.272.806-.107 1.204.165.397.505.71.93.78l.893.15c.543.09.94.56.94 1.109v1.094c0 .55-.397 1.02-.94 1.11l-.893.149c-.425.07-.765.383-.93.78-.165.398-.143.854.107 1.204l.527.738c.32.447.269 1.06-.12 1.45l-.774.773a1.125 1.125 0 01-1.449.12l-.738-.526c-.35-.25-.806-.272-1.203-.107-.397.165-.71.505-.781.929l-.149.894c-.09.542-.56.94-1.11.94h-1.093c-.55 0-1.02-.398-1.11-.94l-.148-.894c-.071-.424-.384-.764-.781-.93-.398-.164-.854-.142-1.204.108l-.738.526c-.447.32-1.06.269-1.45-.12l-.773-.773a1.125 1.125 0 01-.12-1.45l.527-.737c.25-.35.272-.807.108-1.204-.165-.397-.506-.71-.93-.781l-.894-.149c-.542-.09-.94-.56-.94-1.109v-1.094c0-.55.398-1.02.94-1.11l.894-.149c.424-.07.765-.383.93-.78.165-.398.142-.854-.108-1.204l-.526-.738a1.125 1.125 0 01.12-1.45l.773-.773a1.125 1.125 0 011.45-.12l.737.527c.35.25.807.272 1.204.107.397-.165.71-.505.78-.929l.15-.894z"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
          <path
            d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </button>
    </div>
  </div>

  {#if globalError}
    <div class="error-banner">
      <strong>Error:</strong>
      {globalError}
      <button class="error-dismiss" onclick={() => (globalError = null)}>✕</button>
    </div>
  {/if}

  {#if showSettings}
    <div class="full-area">
      <SettingsView
        appearance={appearance}
        onChange={setAppearance}
        onClose={() => (showSettings = false)}
      />
    </div>
  {:else if mode === "empty"}
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

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem 1.2rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-subtle);
  }

  .mode-label {
    font-size: 0.9rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .toolbar-actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .btn-reset {
    padding: 0.25rem 0.7rem;
    border: 1px solid var(--border-input);
    border-radius: 5px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.85rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-reset:hover {
    background: var(--surface-hover);
  }

  .btn-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.9rem;
    height: 1.9rem;
    padding: 0;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-icon:hover {
    background: var(--surface-hover);
  }

  .btn-icon svg {
    width: 1.05rem;
    height: 1.05rem;
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
