<script lang="ts">
  /**
   * ComposePanel — global compression parameters and the Compress action for
   * the workspace's Compose mode.
   *
   * The staged items themselves are rendered by the always-visible `FileList`;
   * this panel only owns the parameters (format/level/exclude/…), the output
   * path, and the bundle-into-one-archive action via `compress_many`. Adding
   * and removing items is delegated to the parent workspace.
   */
  import { pickSavePath, confirmDialog } from "../dialogs";
  import { compressMany, writeSidecar } from "../ipc";
  import { FORMATS, LEVELS, extensionFor } from "../formats";
  import type { StagedItem, ProgressEvent, Report, IpcError } from "../types";

  interface Props {
    items: StagedItem[];
    onAddFiles: () => void;
    onAddFolder: () => void;
    onRunning: (jobId: string) => void;
    onProgress: (e: ProgressEvent) => void;
    onDone: (report: Report, dest: string) => void;
    onError: (err: IpcError) => void;
  }

  let { items, onAddFiles, onAddFolder, onRunning, onProgress, onDone, onError }: Props = $props();

  let selectedFormat = $state("tar.zst");
  let outputPath = $state("");
  let outputTouched = $state(false);
  let level = $state<"Fast" | "Best" | "Edge">("Best");
  let checksum = $state(false);
  let gitignore = $state(true);
  let overwrite = $state(false);
  let excludeText = $state("");
  let inlineError = $state<string | null>(null);
  let busy = $state(false);

  const selectedFormatEntry = $derived(FORMATS.find((f) => f.name === selectedFormat));

  function dirName(p: string): string {
    const normalized = p.replace(/\\/g, "/");
    const idx = normalized.lastIndexOf("/");
    return idx > 0 ? normalized.slice(0, idx) : "";
  }

  // Default the output path from the first staged item until the user edits it.
  $effect(() => {
    const ext = extensionFor(selectedFormat);
    const dir = items.length > 0 ? dirName(items[0].path) : "";
    const def = dir ? `${dir}/archive${ext}` : `archive${ext}`;
    if (!outputTouched) outputPath = def;
  });

  function parseExclude(text: string): string[] {
    return text
      .split(/[\n,]+/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
  }

  async function chooseOutput(): Promise<void> {
    const suggested = outputPath || `archive${extensionFor(selectedFormat)}`;
    const path = await pickSavePath(suggested);
    if (path) {
      outputPath = path;
      outputTouched = true;
    }
  }

  async function handleCompress(): Promise<void> {
    inlineError = null;

    if (items.length === 0) {
      inlineError = "Add at least one file or folder to compress.";
      return;
    }
    if (!outputPath) {
      inlineError = "Please specify an output path.";
      return;
    }

    // Silent-tar dialog: a codec-only format holds only one stream, so a
    // multi-input or directory bundle is transparently tarred first.
    const needsTar = items.length > 1 || items.some((it) => it.isDir);
    if (selectedFormatEntry?.codecOnly && needsTar) {
      const ok = await confirmDialog(
        `These items will be archived as tar inside ${outputPath}. Other tools expect a .tar.* name. Continue?`,
        "Items will be tarred",
      );
      if (!ok) return;
    }

    await doCompress(false);
  }

  async function doCompress(forceOverwrite: boolean): Promise<void> {
    busy = true;
    const jobId = crypto.randomUUID();
    onRunning(jobId);

    const opts = {
      format: selectedFormat || null,
      level,
      overwrite: forceOverwrite || overwrite,
      gitignore,
      exclude: parseExclude(excludeText),
      checksum,
    };

    try {
      const report = await compressMany(
        jobId,
        items.map((it) => it.path),
        outputPath,
        opts,
        onProgress,
      );

      if (checksum && report.sha256) {
        try {
          await writeSidecar(outputPath, report.sha256, report.content_sha256);
        } catch {
          // Sidecar write failure is non-fatal; continue to show summary.
        }
      }

      onDone(report, outputPath);
    } catch (err: unknown) {
      const e = err as IpcError;
      busy = false;
      if (e.kind === "already-exists") {
        const ok = await confirmDialog(`Overwrite ${outputPath}?`, "File already exists");
        if (ok) await doCompress(true);
        return;
      }
      if (e.kind === "cancelled") return;
      if (e.kind === "invalid-glob") {
        inlineError = `Invalid glob pattern: ${e.message}`;
        return;
      }
      if (e.kind === "unknown-format") {
        inlineError = `Unknown format: ${e.message}`;
        return;
      }
      if (e.kind === "duplicate-input") {
        const data = e.data as { name?: string } | undefined;
        const name = data?.name ?? "an item";
        inlineError = `Two items share the name "${name}". Rename or remove one before bundling.`;
        return;
      }
      onError(e);
    }
  }
</script>

<div class="compose-panel">
  <h2 class="card-title">Compress</h2>

  <div class="staged-actions">
    <button class="btn-secondary" onclick={onAddFiles}>Add files…</button>
    <button class="btn-secondary" onclick={onAddFolder}>Add folder…</button>
  </div>

  <div class="input-row">
    <label class="field-label" for="compose-output">Output</label>
    <div class="path-group">
      <input
        id="compose-output"
        class="text-input"
        type="text"
        bind:value={outputPath}
        oninput={() => (outputTouched = true)}
        placeholder="output archive path"
      />
      <button class="btn-secondary" onclick={chooseOutput}>Choose…</button>
    </div>
  </div>

  <div class="input-row">
    <label class="field-label" for="compose-format">Format</label>
    <select id="compose-format" class="select-input" bind:value={selectedFormat}>
      {#each FORMATS as fmt (fmt.name)}
        <option value={fmt.name}>{fmt.label}</option>
      {/each}
    </select>
  </div>

  <div class="input-row">
    <label class="field-label" for="compose-level">Level</label>
    <select id="compose-level" class="select-input select-small" bind:value={level}>
      {#each LEVELS as l (l)}
        <option value={l}>{l}</option>
      {/each}
    </select>
  </div>

  <div class="input-row">
    <label class="field-label" for="compose-exclude">Exclude globs</label>
    <textarea
      id="compose-exclude"
      class="text-input textarea"
      bind:value={excludeText}
      placeholder="*.log, .DS_Store (comma or newline)"
      rows={2}
    ></textarea>
  </div>

  <div class="toggles">
    <label class="toggle">
      <input type="checkbox" bind:checked={checksum} />
      Compute checksum (.sha256 sidecar)
    </label>
    <label class="toggle">
      <input type="checkbox" bind:checked={gitignore} />
      Respect .gitignore
    </label>
    <label class="toggle">
      <input type="checkbox" bind:checked={overwrite} />
      Overwrite existing output
    </label>
  </div>

  {#if inlineError}
    <p class="inline-error">{inlineError}</p>
  {/if}

  <button class="btn-primary" onclick={handleCompress} disabled={busy || items.length === 0}>
    {busy ? "Compressing…" : "Compress"}
  </button>
</div>

<style>
  .compose-panel {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .card-title {
    margin: 0;
    font-size: 1.1rem;
    color: var(--text);
  }

  .staged-actions {
    display: flex;
    gap: 0.4rem;
  }

  .input-row {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field-label {
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .path-group {
    display: flex;
    gap: 0.4rem;
  }

  .text-input {
    flex: 1;
    min-width: 0;
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    font-size: 0.9rem;
    color: var(--text);
    background: var(--surface);
  }

  .text-input:focus {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .textarea {
    resize: vertical;
    font-family: inherit;
  }

  .select-input {
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    font-size: 0.9rem;
    background: var(--surface);
    color: var(--text);
  }

  .select-small {
    width: 10rem;
  }

  .toggles {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.9rem;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .toggle input[type="checkbox"] {
    cursor: pointer;
  }

  .inline-error {
    color: var(--danger);
    font-size: 0.88rem;
    margin: 0;
  }

  .btn-primary {
    align-self: flex-start;
    padding: 0.45rem 1.4rem;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: 0.95rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-secondary {
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.9rem;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: var(--surface-hover);
  }
</style>
