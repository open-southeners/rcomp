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
  import {
    CODECS,
    CONTAINERS,
    LEVELS,
    composeFormat,
    containerAllowsCodec,
    decomposeFormat,
    extensionFor,
    defaultFormat,
    type ContainerName,
  } from "../formats";
  import type { StagedItem, ProgressEvent, Report, IpcError } from "../types";

  interface Props {
    items: StagedItem[];
    /** User's pinned default format from Settings, or `null` for "Auto"
     *  (per-content smart default — see `defaultFormat` in `../formats`). */
    defaultFormatPref: string | null;
    onRunning: (jobId: string) => void;
    onProgress: (e: ProgressEvent) => void;
    onDone: (report: Report, dest: string) => void;
    onError: (err: IpcError) => void;
  }

  let { items, defaultFormatPref, onRunning, onProgress, onDone, onError }: Props = $props();

  let container = $state<ContainerName>("tar");
  let codec = $state<string | null>("zstd");
  let formatTouched = $state(false);
  let outputPath = $state("");
  let outputTouched = $state(false);
  let levelIndex = $state(1);
  let checksum = $state(false);
  let gitignore = $state(true);
  let overwrite = $state(false);
  let excludeText = $state("");
  let inlineError = $state<string | null>(null);
  let busy = $state(false);

  const level = $derived(LEVELS[levelIndex]);
  const selectedFormat = $derived(composeFormat(container, codec));
  // A bare codec (no container) is the "codec-only" case the silent-tar
  // dialog cares about — a single stream can't hold more than one input.
  const isBareCodec = $derived(container === "none");

  // Follows the pinned Settings preference, or a smart per-content default
  // (folder/multi-item bundles need a container; a single file doesn't) —
  // until the user picks a format themselves.
  $effect(() => {
    if (formatTouched) return;
    const isDirLike = items.length > 1 || items.some((it) => it.isDir);
    const name = defaultFormatPref ?? defaultFormat(isDirLike);
    const decomposed = decomposeFormat(name);
    container = decomposed.container;
    codec = decomposed.codec;
  });

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
    if (isBareCodec && needsTar) {
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
  <div class="panel-header">
    <h2 class="card-title">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" aria-hidden="true">
        <line x1="4" y1="6" x2="20" y2="6" stroke-linecap="round" />
        <line x1="4" y1="12" x2="20" y2="12" stroke-linecap="round" />
        <line x1="4" y1="18" x2="20" y2="18" stroke-linecap="round" />
        <circle cx="9" cy="6" r="2" fill="var(--surface)" />
        <circle cx="15" cy="12" r="2" fill="var(--surface)" />
        <circle cx="11" cy="18" r="2" fill="var(--surface)" />
      </svg>
      <span>Compression Parameters</span>
    </h2>
    <p class="card-subtitle">Configure the archive format, codec, and compression level</p>
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
    <span class="field-label" id="compose-container-label">Archive Container Format</span>
    <div class="container-grid" role="group" aria-labelledby="compose-container-label">
      {#each CONTAINERS as c (c.name)}
        <button
          type="button"
          class="container-btn"
          class:active={container === c.name}
          onclick={() => {
            container = c.name;
            formatTouched = true;
          }}
        >
          {c.label}
        </button>
      {/each}
    </div>
  </div>

  <div class="input-row">
    <label class="field-label" for="compose-codec">Compression Algorithm</label>
    {#if containerAllowsCodec(container)}
      <select
        id="compose-codec"
        class="select-input"
        value={codec ?? ""}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          codec = v === "" ? null : v;
          formatTouched = true;
        }}
      >
        {#if container === "tar"}
          <option value="">None — plain, uncompressed tar</option>
        {/if}
        {#each CODECS as c (c.name)}
          <option value={c.name}>{c.label}</option>
        {/each}
      </select>
    {:else}
      <p class="field-hint">
        {CONTAINERS.find((c) => c.name === container)?.label} manages its own compression.
      </p>
    {/if}
  </div>

  <div class="level-card">
    <div class="level-row">
      <span class="level-title">Compression Level</span>
      <span class="level-value">{level}</span>
    </div>
    <input
      type="range"
      min="0"
      max="2"
      step="1"
      bind:value={levelIndex}
      class="level-slider"
      aria-label="Compression level"
    />
    <div class="level-ticks">
      {#each LEVELS as l, i (l)}
        <span class:active={i === levelIndex}>{l}</span>
      {/each}
    </div>
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
    <svg class="btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" aria-hidden="true">
      <path
        d="M4 8h16M6 8v10a1 1 0 001 1h10a1 1 0 001-1V8M10 12h4"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
    <span>{busy ? "Compressing…" : "Compress & Save Archive"}</span>
  </button>
</div>

<style>
  .compose-panel {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .panel-header {
    padding-bottom: 0.85rem;
    border-bottom: 1px solid var(--border);
  }

  .card-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    margin: 0;
    font-size: 1.05rem;
    color: var(--text);
  }

  .panel-icon {
    width: 1rem;
    height: 1rem;
    color: var(--accent);
    flex-shrink: 0;
  }

  .card-subtitle {
    margin: 0.25rem 0 0;
    font-size: 0.8rem;
    color: var(--text-faint);
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

  .container-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.35rem;
  }

  .container-btn {
    padding: 0.4rem 0.3rem;
    border: 1px solid var(--border-input);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.12s, color 0.12s, border-color 0.12s;
  }

  .container-btn:hover {
    background: var(--surface-hover);
  }

  .container-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-contrast);
  }

  .field-hint {
    margin: 0;
    padding: 0.35rem 0.6rem;
    font-size: 0.82rem;
    color: var(--text-faint);
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .level-card {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    padding: 0.65rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
  }

  .level-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 0.85rem;
  }

  .level-title {
    font-weight: 600;
    color: var(--text-secondary);
  }

  .level-value {
    font-weight: 700;
    color: var(--accent);
  }

  .level-slider {
    width: 100%;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .level-ticks {
    display: flex;
    justify-content: space-between;
    font-size: 0.72rem;
    color: var(--text-faint);
  }

  .level-ticks .active {
    color: var(--text-secondary);
    font-weight: 600;
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
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.7rem 1rem;
    border: none;
    border-radius: 12px;
    background: linear-gradient(135deg, var(--accent), var(--accent-hover));
    color: var(--accent-contrast);
    font-size: 0.92rem;
    font-weight: 600;
    cursor: pointer;
    box-shadow: 0 10px 22px -10px rgba(0, 0, 0, 0.45);
    transition: filter 0.12s, transform 0.06s;
  }

  .btn-primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }

  .btn-primary:active:not(:disabled) {
    transform: scale(0.99);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
    box-shadow: none;
  }

  .btn-icon {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
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
