<script lang="ts">
  import { humanBytes, durationMs } from "../types";
  import type { Report } from "../types";

  interface Props {
    report: Report;
    /** "compress" or "extract" */
    kind: "compress" | "extract";
    /** Output path (compress) or destination (extract). */
    dest: string;
    /** Whether a sidecar verify was performed and passed. */
    verified?: boolean;
    onReset: () => void;
  }

  let { report, kind, dest, verified = false, onReset }: Props = $props();

  const ms = $derived(durationMs(report.duration));
  const durationLabel = $derived(
    ms < 1000 ? `${ms.toFixed(0)} ms` : `${(ms / 1000).toFixed(2)} s`,
  );

  const ratio = $derived(
    report.input_bytes > 0
      ? ((report.output_bytes / report.input_bytes) * 100).toFixed(1)
      : null,
  );
</script>

<div class="summary-view">
  <div class="summary-icon" class:compress={kind === "compress"} class:extract={kind === "extract"}>
    {#if kind === "compress"}
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M20.25 7.5l-.625 10.632a2.25 2.25 0 01-2.247 2.118H6.622a2.25 2.25 0 01-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
    {:else}
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M9 8.25H7.5a2.25 2.25 0 00-2.25 2.25v9a2.25 2.25 0 002.25 2.25h9a2.25 2.25 0 002.25-2.25v-9a2.25 2.25 0 00-2.25-2.25H15m0-3l-3-3m0 0l-3 3m3-3V15" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
    {/if}
  </div>

  <h2 class="title">{kind === "compress" ? "Compressed" : "Extracted"}</h2>

  <p class="dest">{dest}</p>

  <div class="stats">
    <div class="stat-row">
      <span class="stat-label">Input</span>
      <span class="stat-value">{humanBytes(report.input_bytes)}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Output</span>
      <span class="stat-value">{humanBytes(report.output_bytes)}</span>
    </div>
    {#if ratio !== null}
      <div class="stat-row">
        <span class="stat-label">Ratio</span>
        <span class="stat-value">{ratio}%</span>
      </div>
    {/if}
    <div class="stat-row">
      <span class="stat-label">Duration</span>
      <span class="stat-value">{durationLabel}</span>
    </div>
    {#if report.entries > 0}
      <div class="stat-row">
        <span class="stat-label">Entries</span>
        <span class="stat-value">{report.entries}</span>
      </div>
    {/if}
    {#if report.entries_excluded > 0}
      <div class="stat-row">
        <span class="stat-label">Excluded</span>
        <span class="stat-value">{report.entries_excluded} paths</span>
      </div>
    {/if}
    {#if report.sha256}
      <div class="stat-row digest-row">
        <span class="stat-label">SHA-256</span>
        <span class="stat-value digest">{report.sha256}</span>
      </div>
    {/if}
    {#if report.content_sha256}
      <div class="stat-row digest-row">
        <span class="stat-label">Content SHA-256</span>
        <span class="stat-value digest">{report.content_sha256}</span>
      </div>
    {/if}
    {#if verified}
      <div class="verified-badge">Checksum verified</div>
    {/if}
  </div>

  <button class="btn-primary" onclick={onReset}>New operation</button>
</div>

<style>
  .summary-view {
    min-width: 360px;
    max-width: 520px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 2rem;
  }

  .summary-icon {
    width: 3rem;
    height: 3rem;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .summary-icon.compress {
    background: var(--accent-subtle-bg);
    color: var(--accent);
  }

  .summary-icon.extract {
    background: var(--success-icon-bg);
    color: var(--success-icon-fg);
  }

  .summary-icon svg {
    width: 1.75rem;
    height: 1.75rem;
  }

  .title {
    margin: 0;
    font-size: 1.2rem;
    color: var(--text);
  }

  .dest {
    margin: 0;
    font-size: 0.85rem;
    color: var(--text-muted);
    word-break: break-all;
    text-align: center;
    max-width: 100%;
  }

  .stats {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    background: var(--surface-subtle);
    border-radius: 8px;
    padding: 0.75rem 1rem;
    border: 1px solid var(--border);
  }

  .stat-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.9rem;
  }

  .stat-label {
    color: var(--text-muted);
  }

  .stat-value {
    font-weight: 500;
    color: var(--text);
  }

  .digest-row {
    flex-direction: column;
    gap: 0.1rem;
  }

  .digest {
    font-family: monospace;
    font-size: 0.75rem;
    word-break: break-all;
    color: var(--text-secondary);
    font-weight: 400;
  }

  .verified-badge {
    margin-top: 0.25rem;
    align-self: flex-start;
    background: var(--success-bg);
    color: var(--success-fg);
    border-radius: 4px;
    padding: 0.15rem 0.5rem;
    font-size: 0.8rem;
    font-weight: 600;
  }

  .btn-primary {
    margin-top: 0.5rem;
    padding: 0.5rem 1.4rem;
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
</style>
