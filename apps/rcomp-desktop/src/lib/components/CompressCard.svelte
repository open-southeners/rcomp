<script lang="ts">
  import { pickSavePath, confirmDialog } from "../dialogs";
  import { compress, writeSidecar } from "../ipc";
  import { FORMATS, LEVELS, defaultFormat, extensionFor } from "../formats";
  import type { InspectResult, ProgressEvent, Report, IpcError } from "../types";

  interface Props {
    inputPath: string;
    inspect: InspectResult;
    onRunning: (jobId: string) => void;
    onProgress: (e: ProgressEvent) => void;
    onDone: (report: Report, dest: string) => void;
    onError: (err: IpcError) => void;
  }

  let { inputPath, inspect, onRunning, onProgress, onDone, onError }: Props = $props();

  // Derive a default output path from the input name.
  const inputBaseName = $derived(inputPath.replace(/\\/g, "/").split("/").pop() ?? "output");

  let selectedFormat = $state(defaultFormat(inspect.is_dir));
  let outputPath = $state("");
  let level = $state<"Fast" | "Best" | "Edge">("Best");
  let checksum = $state(false);
  let gitignore = $state(true);
  let overwrite = $state(false);
  let excludeText = $state("");
  let inlineError = $state<string | null>(null);
  let busy = $state(false);

  // Recompute outputPath default when format changes.
  $effect(() => {
    const ext = extensionFor(selectedFormat);
    const stem = stripKnownExtensions(inputBaseName);
    outputPath = `${stem}${ext}`;
  });

  function stripKnownExtensions(name: string): string {
    return name
      .replace(/\.(tar\.(gz|bz2|xz|zst|lz4|br))$/i, "")
      .replace(/\.(zip|7z|tar|gz|bz2|xz|zst|lz4|br)$/i, "");
  }

  function parseExclude(text: string): string[] {
    return text
      .split(/[\n,]+/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
  }

  const selectedFormatEntry = $derived(FORMATS.find((f) => f.name === selectedFormat));

  async function chooseOutput(): Promise<void> {
    const path = await pickSavePath(outputPath || inputBaseName);
    if (path) outputPath = path;
  }

  async function handleCompress(): Promise<void> {
    inlineError = null;

    if (!outputPath) {
      inlineError = "Please specify an output path.";
      return;
    }

    // Silent-tar dialog: codec-only format + directory input.
    if (inspect.is_dir && selectedFormatEntry?.codecOnly) {
      const ok = await confirmDialog(
        `The folder will be archived as tar inside ${outputPath}. Other tools expect a .tar.* name. Continue?`,
        "Folder will be tarred",
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
      const report = await compress(jobId, inputPath, outputPath, opts, onProgress);

      // Write sidecar when checksum was requested and the report has a digest.
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
      if (e.kind === "already-exists") {
        busy = false;
        const ok = await confirmDialog(`Overwrite ${outputPath}?`, "File already exists");
        if (ok) {
          await doCompress(true);
        }
        return;
      }
      if (e.kind === "cancelled") {
        busy = false;
        return;
      }
      if (e.kind === "invalid-glob") {
        busy = false;
        inlineError = `Invalid glob pattern: ${e.message}`;
        return;
      }
      if (e.kind === "unknown-format") {
        busy = false;
        inlineError = `Unknown format: ${e.message}`;
        return;
      }
      busy = false;
      onError(e);
    }
  }
</script>

<div class="compress-card">
  <h2 class="card-title">Compress</h2>

  <div class="input-row">
    <span class="field-label">Input</span>
    <span class="field-value path">{inputPath}</span>
  </div>

  <div class="input-row">
    <label class="field-label" for="output-path">Output</label>
    <div class="path-group">
      <input
        id="output-path"
        class="text-input"
        type="text"
        bind:value={outputPath}
        placeholder="output file path"
      />
      <button class="btn-secondary" onclick={chooseOutput}>Choose…</button>
    </div>
  </div>

  <div class="input-row">
    <label class="field-label" for="format-select">Format</label>
    <select id="format-select" class="select-input" bind:value={selectedFormat}>
      {#each FORMATS as fmt (fmt.name)}
        <option value={fmt.name}>{fmt.label}</option>
      {/each}
    </select>
  </div>

  <div class="input-row">
    <label class="field-label" for="level-select">Level</label>
    <select id="level-select" class="select-input select-small" bind:value={level}>
      {#each LEVELS as l (l)}
        <option value={l}>{l}</option>
      {/each}
    </select>
  </div>

  <div class="input-row">
    <label class="field-label" for="exclude-text">Exclude globs</label>
    <textarea
      id="exclude-text"
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

  <button class="btn-primary" onclick={handleCompress} disabled={busy}>
    {busy ? "Compressing…" : "Compress"}
  </button>
</div>

<style>
  .compress-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    min-width: 360px;
    max-width: 500px;
    padding: 1.5rem 2rem;
  }

  .card-title {
    margin: 0 0 0.25rem;
    font-size: 1.1rem;
    color: #111;
  }

  .input-row {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field-label {
    font-size: 0.82rem;
    font-weight: 600;
    color: #6b7280;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .field-value.path {
    font-size: 0.9rem;
    color: #333;
    word-break: break-all;
  }

  .path-group {
    display: flex;
    gap: 0.4rem;
  }

  .text-input {
    flex: 1;
    padding: 0.35rem 0.6rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    font-size: 0.9rem;
    color: #111;
    background: #fff;
  }

  .text-input:focus {
    outline: 2px solid #0070f3;
    outline-offset: 1px;
  }

  .textarea {
    resize: vertical;
    font-family: inherit;
  }

  .select-input {
    padding: 0.35rem 0.6rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    font-size: 0.9rem;
    background: #fff;
    color: #111;
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
    color: #374151;
    cursor: pointer;
  }

  .toggle input[type="checkbox"] {
    cursor: pointer;
  }

  .inline-error {
    color: #c00;
    font-size: 0.88rem;
    margin: 0;
  }

  .btn-primary {
    align-self: flex-start;
    padding: 0.45rem 1.4rem;
    border: none;
    border-radius: 6px;
    background: #0070f3;
    color: #fff;
    font-size: 0.95rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-primary:hover:not(:disabled) {
    background: #0058c4;
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-secondary {
    padding: 0.35rem 0.75rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    background: #fff;
    color: #374151;
    font-size: 0.9rem;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: #f3f4f6;
  }
</style>
