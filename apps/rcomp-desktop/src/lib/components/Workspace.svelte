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
   *
   * Drag & drop with an archive open edits it: files dropped onto the list are
   * added to the archive (into the folder row under the cursor, if any), and
   * rows dragged out of the list are extracted where they are dropped. Files
   * dropped right after compressing are added to the archive just created.
   */
  import { onMount, onDestroy } from "svelte";
  import DropZone from "./DropZone.svelte";
  import FileList from "./FileList.svelte";
  import ComposePanel from "./ComposePanel.svelte";
  import ExtractBar from "./ExtractBar.svelte";
  import ProgressView from "./ProgressView.svelte";
  import SummaryView from "./SummaryView.svelte";
  import SettingsView from "./SettingsView.svelte";
  import ChangelogView from "./ChangelogView.svelte";
  import TitleBar from "./TitleBar.svelte";
  import { onFileDrag } from "../dragdrop";
  import type { DragPoint } from "../dragdrop";
  import { pickFile, pickFiles, pickFolder, confirmDialog } from "../dialogs";
  import {
    inspectPath,
    listEntries,
    getLaunchPaths,
    onOpenPaths,
    onShowChangelog,
    onMenuOpenArchive,
    onMenuNewArchiveFromFiles,
    onMenuNewArchiveFromFolder,
    onMenuCloseArchive,
    addToArchive,
    dragOut,
    dragStagingRoot,
  } from "../ipc";
  import { loadAppearance, applyAppearance } from "../theme";
  import {
    loadAcceleration,
    loadDefaultFormat,
    saveAcceleration,
    saveDefaultFormat,
  } from "../preferences";
  import type { AccelerationPreference } from "../preferences";
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
  let showChangelog = $state(false);
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

  let defaultFormatPref = $state<string | null>(loadDefaultFormat());

  function setDefaultFormatPref(next: string | null): void {
    defaultFormatPref = next;
    saveDefaultFormat(next);
  }

  let acceleration = $state<AccelerationPreference>(loadAcceleration());

  function setAcceleration(next: AccelerationPreference): void {
    acceleration = next;
    saveAcceleration(next);
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

  // --- drag & drop editing ---
  /** Folder drag-out files are staged in; drops from there are our own. */
  let stagingRoot: string | null = null;
  /** An OS file drag over the window that would add to an archive. */
  let fileDrag = $state<{ key: string | null; prefix: string | null } | null>(null);
  /** Entries being extracted for a drag-out, before the native drag starts. */
  let dragOutCount = $state(0);

  let unlistenDrop: (() => void) | null = null;
  let unlistenOpen: (() => void) | null = null;
  let unlistenChangelog: (() => void) | null = null;
  let menuUnlisten: (() => void)[] = [];

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

  // Footer stats for the archive file list: total uncompressed size, the
  // on-disk (compressed) archive size, and the per-entry size, computed from
  // what list_entries/inspect already gives us (no extra IPC round-trip).
  const uncompressedEntries = $derived(entries.filter((e) => !e.is_dir));
  const totalUncompressed = $derived(
    uncompressedEntries.reduce((sum, e) => sum + e.size, 0),
  );
  const openFooterStats = $derived(
    mode === "open" && uncompressedEntries.length > 0
      ? [
          { label: "Original", value: humanBytes(totalUncompressed) },
          ...(archiveInspect?.size != null
            ? [{ label: "On disk", value: humanBytes(archiveInspect.size) }]
            : []),
          {
            // A single-entry archive has nothing to average — it's just that
            // file's size, so only call it "Avg" once there's more than one.
            label: uncompressedEntries.length === 1 ? "File size" : "Avg file size",
            value: humanBytes(totalUncompressed / uncompressedEntries.length),
          },
        ]
      : undefined,
  );

  // -------------------------------------------------------------------------
  // Drag & drop editing of an archive.
  // -------------------------------------------------------------------------

  /** An entry path without `./` prefixes or a trailing separator. */
  function normEntry(p: string): string {
    return p.replace(/\\/g, "/").replace(/^(\.\/)+/, "").replace(/\/+$/, "");
  }

  /** The archive a drop would add to right now, if any. */
  function dropArchive(): string | null {
    if (run || showSettings || showChangelog) return null;
    if (mode === "open" && !result && !entriesLoading && !entriesError) return archivePath;
    if (result?.kind === "compress") return result.dest;
    return null;
  }

  function isOwnDragOut(paths: string[]): boolean {
    const root = stagingRoot;
    return root !== null && paths.every((p) => p.startsWith(root));
  }

  /**
   * Resolve a drop position to the folder inside the open archive it lands
   * in: a folder row is the folder itself, a file row its parent folder,
   * anywhere else the archive root.
   */
  function dropTargetAt(at: DragPoint): { key: string | null; prefix: string | null } {
    if (mode !== "open") return { key: null, prefix: null };
    const el = document.elementFromPoint(at.x, at.y)?.closest("[data-row-key]");
    const key = el?.getAttribute("data-row-key") ?? null;
    const row = key ? rows.find((r) => r.key === key) : undefined;
    if (!key || !row) return { key: null, prefix: null };
    if (row.isDir) return { key, prefix: normEntry(key) };
    const parent = normEntry(key).split("/").slice(0, -1).join("/");
    return { key: null, prefix: parent || null };
  }

  async function handleDrop(paths: string[], at: DragPoint): Promise<void> {
    const archive = dropArchive();
    if (!archive) {
      await routePaths(paths);
      return;
    }
    if (mode === "open") {
      await addItemsToArchive(archive, paths, dropTargetAt(at).prefix);
      return;
    }
    // Just compressed: switch to the new archive, then grow it.
    globalError = null;
    try {
      await openArchive(archive, await inspectPath(archive));
    } catch (err: unknown) {
      globalError = (err as { message?: string }).message ?? "Failed to open the new archive.";
      return;
    }
    if (!entriesError) await addItemsToArchive(archive, paths, null);
  }

  async function addItemsToArchive(
    archive: string,
    paths: string[],
    prefix: string | null,
  ): Promise<void> {
    globalError = null;
    const name = baseName(archive);
    const where = prefix ? ` (in ${prefix}/)` : "";
    const existing = entries.map((e) => normEntry(e.path));
    const clashes = paths.map(baseName).filter((n) => {
      const target = prefix ? `${prefix}/${n}` : n;
      return existing.some((e) => e === target || e.startsWith(`${target}/`));
    });

    const ok =
      clashes.length > 0
        ? await confirmDialog(
            `${clashes.join(", ")} already ${clashes.length === 1 ? "exists" : "exist"} in ${name}${where}. Replace ${clashes.length === 1 ? "it" : "them"}?`,
            "Replace items?",
          )
        : await confirmDialog(
            `Add ${paths.length} item${paths.length === 1 ? "" : "s"} to ${name}${where}? The archive is rewritten with the new items.`,
            "Add to archive",
          );
    if (!ok) return;

    const jobId = crypto.randomUUID();
    handleRunning(jobId);
    try {
      await addToArchive(
        jobId,
        archive,
        paths,
        { prefix, replace: clashes.length > 0, acceleration },
        handleProgress,
      );
      run = null;
      latestProgress = null;
      archiveInspect = await inspectPath(archive);
      entries = await listEntries(archive);
    } catch (err: unknown) {
      const e = err as IpcError;
      if (e.kind === "cancelled") handleCancel();
      else handleError(e);
    }
  }

  async function handleDragOut(keys: string[]): Promise<void> {
    if (!archivePath || dragOutCount > 0 || run) return;
    globalError = null;
    dragOutCount = keys.length;
    try {
      await dragOut(crypto.randomUUID(), archivePath, keys);
    } catch (err: unknown) {
      globalError = (err as { message?: string }).message ?? "Failed to drag the items out.";
    } finally {
      dragOutCount = 0;
    }
  }

  const dropLabel = $derived.by(() => {
    if (!fileDrag) return null;
    const archive = dropArchive();
    if (!archive) return null;
    const into = fileDrag.prefix ? ` › ${fileDrag.prefix}/` : "";
    return `Drop to add to ${baseName(archive)}${into}`;
  });

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

  // -------------------------------------------------------------------------
  // Opening a file the OS handed us directly (double-click / "Open With…" /
  // launch args), as opposed to a path picked or dropped from inside the app.
  //
  // Unlike `routePaths`, this never falls back to Compose: the user asked the
  // OS to open this specific file with rcomp, so an unsupported format should
  // surface as an error, not silently start a new bundle from it. It also
  // ignores the "add to the in-progress bundle" case `routePaths` has for
  // drag & drop — an OS open always means "open", even mid-compose.
  // -------------------------------------------------------------------------
  async function openFromOS(paths: string[]): Promise<void> {
    if (paths.length === 0) return;
    globalError = null;
    const problems: string[] = [];

    for (const path of paths) {
      let res: InspectResult;
      try {
        res = await inspectPath(path);
      } catch (err: unknown) {
        problems.push((err as { message?: string }).message ?? `Failed to inspect ${baseName(path)}.`);
        continue;
      }
      if (!res.exists) {
        problems.push(`Path not found: ${path}`);
        continue;
      }
      if (res.is_archive) {
        await openArchive(path, res);
      } else {
        problems.push(`"${baseName(path)}" isn't a supported archive format.`);
      }
    }

    if (problems.length > 0) globalError = problems.join(" ");
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

  // Empty-state affordances (DropZone buttons).
  async function handleOpen(): Promise<void> {
    const path = await pickFile();
    if (path) await routePaths([path]);
  }

  async function handleCompressFiles(): Promise<void> {
    const paths = await pickFiles();
    if (paths.length > 0) await routePaths(paths);
  }

  async function handleNewArchiveFromFolder(): Promise<void> {
    const path = await pickFolder();
    if (path) await routePaths([path]);
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
    try {
      stagingRoot = await dragStagingRoot();
    } catch {
      // Drag-out is unavailable on this platform; nothing to filter.
    }

    unlistenDrop = await onFileDrag({
      enter: (paths, at) => {
        if (isOwnDragOut(paths) || !dropArchive()) return;
        fileDrag = dropTargetAt(at);
      },
      over: (at) => {
        if (fileDrag) fileDrag = dropTargetAt(at);
      },
      leave: () => {
        fileDrag = null;
      },
      drop: (paths, at) => {
        fileDrag = null;
        if (isOwnDragOut(paths)) return;
        void handleDrop(paths, at);
      },
    });

    // Subscribe to live OS "open with" deliveries BEFORE draining the startup
    // buffer, so no delivery is lost in the gap.
    unlistenOpen = await onOpenPaths((paths) => {
      void openFromOS(paths);
    });

    unlistenChangelog = await onShowChangelog(() => {
      showChangelog = true;
    });

    menuUnlisten = await Promise.all([
      onMenuOpenArchive(() => void handleOpen()),
      onMenuNewArchiveFromFiles(() => void handleCompressFiles()),
      onMenuNewArchiveFromFolder(() => void handleNewArchiveFromFolder()),
      onMenuCloseArchive(() => {
        if (mode !== "empty") reset();
      }),
    ]);

    // Drain any files the app was launched with (also flips the backend's
    // "ready" flag so subsequent deliveries arrive via the event above).
    const launch = await getLaunchPaths();
    if (launch.length > 0) await openFromOS(launch);
  });

  onDestroy(() => {
    if (unlistenDrop) unlistenDrop();
    if (unlistenOpen) unlistenOpen();
    if (unlistenChangelog) unlistenChangelog();
    menuUnlisten.forEach((fn) => fn());
  });
</script>

<div class="workspace">
  <TitleBar
    {mode}
    settingsActive={showSettings}
    onOpenSettings={() => (showSettings = true)}
    onOpenChangelog={() => (showChangelog = true)}
  />

  {#if dragOutCount > 0}
    <div class="info-banner" role="status">
      Preparing {dragOutCount} item{dragOutCount === 1 ? "" : "s"} to drag out…
    </div>
  {/if}

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
        defaultFormat={defaultFormatPref}
        onChangeDefaultFormat={setDefaultFormatPref}
        {acceleration}
        onChangeAcceleration={setAcceleration}
        onClose={() => (showSettings = false)}
      />
    </div>
  {:else if showChangelog}
    <div class="full-area">
      <ChangelogView onClose={() => (showChangelog = false)} />
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
          {acceleration}
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
        searchQuery={searchQuery}
        onSearchChange={(q) => (searchQuery = q)}
        onDragOut={(keys) => void handleDragOut(keys)}
        {dropLabel}
        dropTargetKey={fileDrag?.key ?? null}
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
          onAddFiles={mode === "compose" ? handleAddFiles : undefined}
          {dropLabel}
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
            defaultFormatPref={defaultFormatPref}
            {acceleration}
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

  .info-banner {
    background: var(--accent-subtle-bg);
    border-bottom: 1px solid var(--border);
    padding: 0.5rem 1.2rem;
    color: var(--accent-subtle-fg);
    font-size: 0.85rem;
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
