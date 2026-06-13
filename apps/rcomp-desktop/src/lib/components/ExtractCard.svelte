<script lang="ts">
  import { onMount } from "svelte";
  import { pickFolder, confirmDialog } from "../dialogs";
  import { extract, wrapInfo, readSidecar } from "../ipc";
  import type {
    InspectResult,
    ProgressEvent,
    Report,
    IpcError,
    WrapInfo,
    SidecarData,
  } from "../types";

  interface Props {
    inputPath: string;
    inspect: InspectResult;
    onRunning: (jobId: string) => void;
    onProgress: (e: ProgressEvent) => void;
    onDone: (report: Report, dest: string, verified: boolean) => void;
    onError: (err: IpcError) => void;
    onViewContents: () => void;
  }

  let {
    inputPath,
    inspect,
    onRunning,
    onProgress,
    onDone,
    onError,
    onViewContents,
  }: Props = $props();

  // Derive parent directory for default destination.
  function deriveParent(p: string): string {
    const normalized = p.replace(/\\/g, "/");
    const idx = normalized.lastIndexOf("/");
    return idx > 0 ? normalized.slice(0, idx) : ".";
  }

  let destPath = $state(deriveParent(inputPath));
  let wrapData = $state<WrapInfo | null>(null);
  let sidecar = $state<SidecarData | null>(null);
  let unwrap = $state(false); // false = keep wrap dir; true = extract directly
  let busy = $state(false);
  let inlineError = $state<string | null>(null);

  const effectiveDest = $derived(
    wrapData && !unwrap && wrapData.roots > 1
      ? `${destPath}/${wrapData.wrap_dir}`
      : destPath,
  );

  const hasSidecar = $derived(sidecar !== null && sidecar.artifact_sha256 !== null);

  onMount(async () => {
    try {
      [wrapData, sidecar] = await Promise.all([
        wrapInfo(inputPath),
        readSidecar(inputPath),
      ]);
    } catch {
      // Non-fatal; wrapData / sidecar stay null.
    }
  });

  async function chooseDest(): Promise<void> {
    const path = await pickFolder();
    if (path) destPath = path;
  }

  async function handleExtract(): Promise<void> {
    inlineError = null;
    await doExtract(false);
  }

  async function doExtract(forceOverwrite: boolean): Promise<void> {
    busy = true;
    const jobId = crypto.randomUUID();
    onRunning(jobId);

    const opts = {
      format: inspect.format_name ?? undefined,
      overwrite: forceOverwrite,
      verify_sha256: hasSidecar ? (sidecar?.artifact_sha256 ?? null) : null,
      verify_content_sha256: hasSidecar ? (sidecar?.content_sha256 ?? null) : null,
    };

    try {
      const report = await extract(jobId, inputPath, effectiveDest, opts, onProgress);
      onDone(report, effectiveDest, hasSidecar);
    } catch (err: unknown) {
      const e = err as IpcError;
      if (e.kind === "already-exists") {
        busy = false;
        const ok = await confirmDialog(
          `Overwrite existing files in ${effectiveDest}?`,
          "Destination already exists",
        );
        if (ok) {
          await doExtract(true);
        }
        return;
      }
      if (e.kind === "cancelled") {
        busy = false;
        return;
      }
      if (e.kind === "checksum-mismatch") {
        busy = false;
        const data = e.data as { digest?: string } | undefined;
        const digestLabel = data?.digest ?? "checksum";
        inlineError = `${digestLabel} mismatch — the archive may be corrupt or tampered with.`;
        return;
      }
      busy = false;
      onError(e);
    }
  }
</script>

<div class="extract-card">
  <h2 class="card-title">Extract</h2>

  <div class="input-row">
    <span class="field-label">Archive</span>
    <span class="field-value path">{inputPath}</span>
  </div>

  {#if inspect.format_name}
    <div class="format-badge">{inspect.format_name}</div>
  {/if}

  <div class="input-row">
    <label class="field-label" for="dest-path">Destination</label>
    <div class="path-group">
      <input
        id="dest-path"
        class="text-input"
        type="text"
        bind:value={destPath}
        placeholder="destination folder"
      />
      <button class="btn-secondary" onclick={chooseDest}>Choose…</button>
    </div>
  </div>

  {#if wrapData && wrapData.roots > 1}
    <div class="wrap-section">
      <label class="toggle">
        <input type="checkbox" bind:checked={unwrap} />
        Extract directly here (unwrap)
      </label>
      {#if !unwrap}
        <p class="wrap-note">
          Will extract into: <strong>{effectiveDest}</strong>
        </p>
      {/if}
    </div>
  {/if}

  {#if hasSidecar}
    <div class="sidecar-badge">Auto-verify checksum from sidecar</div>
  {/if}

  {#if inlineError}
    <p class="inline-error">{inlineError}</p>
  {/if}

  <div class="actions">
    <button class="btn-primary" onclick={handleExtract} disabled={busy}>
      {busy ? "Extracting…" : "Extract"}
    </button>
    <button class="btn-secondary" onclick={onViewContents}>View contents</button>
  </div>
</div>

<style>
  .extract-card {
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

  .format-badge {
    align-self: flex-start;
    background: #eff6ff;
    color: #1d4ed8;
    border-radius: 4px;
    padding: 0.15rem 0.5rem;
    font-size: 0.8rem;
    font-weight: 600;
    font-family: monospace;
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

  .wrap-section {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    background: #fffbeb;
    border: 1px solid #fde68a;
    border-radius: 6px;
    padding: 0.6rem 0.8rem;
  }

  .wrap-note {
    margin: 0;
    font-size: 0.85rem;
    color: #555;
    word-break: break-all;
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

  .sidecar-badge {
    align-self: flex-start;
    background: #dcfce7;
    color: #166534;
    border-radius: 4px;
    padding: 0.15rem 0.5rem;
    font-size: 0.8rem;
    font-weight: 600;
  }

  .inline-error {
    color: #c00;
    font-size: 0.88rem;
    margin: 0;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.25rem;
  }

  .btn-primary {
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
    padding: 0.4rem 0.9rem;
    border: 1px solid #d1d5db;
    border-radius: 6px;
    background: #fff;
    color: #374151;
    font-size: 0.9rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: #f3f4f6;
  }
</style>
