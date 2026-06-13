<script lang="ts">
  /**
   * App.svelte — root component and state machine for rcomp desktop.
   *
   * States:
   *   idle      — DropZone is shown; user drops or browses a file.
   *   inspected — CompressCard or ExtractCard (togglable when both make sense).
   *   running   — ProgressView during an in-flight operation.
   *   viewing   — ArchiveContents overlay.
   *   done      — SummaryView after a successful operation.
   */

  import DropZone from "./lib/components/DropZone.svelte";
  import CompressCard from "./lib/components/CompressCard.svelte";
  import ExtractCard from "./lib/components/ExtractCard.svelte";
  import ProgressView from "./lib/components/ProgressView.svelte";
  import SummaryView from "./lib/components/SummaryView.svelte";
  import ArchiveContents from "./lib/components/ArchiveContents.svelte";
  import type { InspectResult, ProgressEvent, Report, IpcError } from "./lib/types";

  type AppState = "idle" | "inspected" | "running" | "viewing" | "done";
  type Mode = "compress" | "extract";

  let appState = $state<AppState>("idle");
  let activePath = $state<string | null>(null);
  let inspectResult = $state<InspectResult | null>(null);
  let mode = $state<Mode>("compress");
  let activeJobId = $state<string | null>(null);
  let latestProgress = $state<ProgressEvent | null>(null);
  let doneReport = $state<Report | null>(null);
  let doneDest = $state<string | null>(null);
  let doneVerified = $state(false);
  let doneKind = $state<Mode>("compress");
  let globalError = $state<string | null>(null);

  // Whether the file can also be extracted (i.e. it is a recognised archive).
  const canExtract = $derived(inspectResult?.is_archive === true);
  // Whether the file/folder can be compressed (always true for existing paths).
  const canCompress = $derived(inspectResult?.exists === true);

  function handleInspected(path: string, result: InspectResult): void {
    activePath = path;
    inspectResult = result;
    globalError = null;

    // Default mode: archives default to extract; everything else to compress.
    mode = result.is_archive ? "extract" : "compress";
    appState = "inspected";
  }

  function handleRunning(jobId: string): void {
    activeJobId = jobId;
    latestProgress = null;
    appState = "running";
  }

  function handleProgress(e: ProgressEvent): void {
    latestProgress = e;
  }

  function handleDoneCompress(report: Report, dest: string): void {
    doneReport = report;
    doneDest = dest;
    doneVerified = false;
    doneKind = "compress";
    activeJobId = null;
    appState = "done";
  }

  function handleDoneExtract(report: Report, dest: string, verified: boolean): void {
    doneReport = report;
    doneDest = dest;
    doneVerified = verified;
    doneKind = "extract";
    activeJobId = null;
    appState = "done";
  }

  function handleError(err: IpcError): void {
    globalError = err.message;
    activeJobId = null;
    // Return to inspected state so the user can retry.
    appState = "inspected";
  }

  function handleCancel(): void {
    activeJobId = null;
    latestProgress = null;
    // Go back to the card so the user can adjust options or start fresh.
    appState = "inspected";
  }

  function handleReset(): void {
    appState = "idle";
    activePath = null;
    inspectResult = null;
    activeJobId = null;
    latestProgress = null;
    doneReport = null;
    doneDest = null;
    globalError = null;
  }

  function switchMode(newMode: Mode): void {
    mode = newMode;
    globalError = null;
  }
</script>

<div class="app-shell">
  <header class="app-header">
    <span class="app-logo">rcomp</span>
    {#if appState !== "idle"}
      <button class="btn-reset" onclick={handleReset} title="Start over">New</button>
    {/if}
  </header>

  <main class="app-main">
    {#if globalError}
      <div class="error-banner">
        <strong>Error:</strong> {globalError}
        <button class="error-dismiss" onclick={() => { globalError = null; }}>✕</button>
      </div>
    {/if}

    {#if appState === "idle"}
      <DropZone onInspected={handleInspected} />

    {:else if appState === "inspected" && activePath && inspectResult}
      <div class="card-wrapper">
        <!-- Mode toggle when both compress and extract make sense. -->
        {#if canCompress && canExtract}
          <div class="mode-tabs">
            <button
              class="mode-tab"
              class:active={mode === "compress"}
              onclick={() => switchMode("compress")}
            >
              Compress
            </button>
            <button
              class="mode-tab"
              class:active={mode === "extract"}
              onclick={() => switchMode("extract")}
            >
              Extract
            </button>
          </div>
        {:else}
          <div class="mode-tabs">
            <button class="mode-tab active" disabled>
              {mode === "compress" ? "Compress" : "Extract"}
            </button>
            <button
              class="mode-tab"
              disabled={mode === "compress" ? !canExtract : !canCompress}
              onclick={() => switchMode(mode === "compress" ? "extract" : "compress")}
            >
              {mode === "compress" ? "Extract" : "Compress"}
            </button>
          </div>
        {/if}

        {#if mode === "compress"}
          <CompressCard
            inputPath={activePath}
            inspect={inspectResult}
            onRunning={handleRunning}
            onProgress={handleProgress}
            onDone={handleDoneCompress}
            onError={handleError}
          />
        {:else}
          <ExtractCard
            inputPath={activePath}
            inspect={inspectResult}
            onRunning={handleRunning}
            onProgress={handleProgress}
            onDone={handleDoneExtract}
            onError={handleError}
            onViewContents={() => { appState = "viewing"; }}
          />
        {/if}
      </div>

    {:else if appState === "running" && activeJobId}
      <ProgressView
        progress={latestProgress}
        jobId={activeJobId}
        onCancel={handleCancel}
      />

    {:else if appState === "viewing" && activePath}
      <ArchiveContents
        path={activePath}
        onClose={() => { appState = "inspected"; }}
      />

    {:else if appState === "done" && doneReport && doneDest}
      <SummaryView
        report={doneReport}
        kind={doneKind}
        dest={doneDest}
        verified={doneVerified}
        onReset={handleReset}
      />
    {/if}
  </main>
</div>

<style>
  .app-shell {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    background: #fff;
  }

  .app-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.6rem 1.2rem;
    border-bottom: 1px solid #e5e7eb;
    background: #f9fafb;
  }

  .app-logo {
    font-size: 1rem;
    font-weight: 700;
    color: #0070f3;
    letter-spacing: 0.04em;
  }

  .btn-reset {
    padding: 0.25rem 0.7rem;
    border: 1px solid #d1d5db;
    border-radius: 5px;
    background: #fff;
    color: #374151;
    font-size: 0.85rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-reset:hover {
    background: #f3f4f6;
  }

  .app-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 2rem 1rem;
    gap: 1rem;
  }

  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    background: #fef2f2;
    border: 1px solid #fca5a5;
    border-radius: 6px;
    padding: 0.6rem 1rem;
    color: #991b1b;
    font-size: 0.9rem;
    max-width: 520px;
    width: 100%;
  }

  .error-dismiss {
    border: none;
    background: transparent;
    cursor: pointer;
    color: inherit;
    font-size: 0.9rem;
    padding: 0;
    flex-shrink: 0;
  }

  .card-wrapper {
    display: flex;
    flex-direction: column;
    gap: 0;
    border: 1px solid #e5e7eb;
    border-radius: 10px;
    overflow: hidden;
    background: #fff;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.06);
    min-width: 360px;
  }

  .mode-tabs {
    display: flex;
    border-bottom: 1px solid #e5e7eb;
    background: #f9fafb;
  }

  .mode-tab {
    flex: 1;
    padding: 0.55rem 1rem;
    border: none;
    background: transparent;
    font-size: 0.9rem;
    color: #6b7280;
    cursor: pointer;
    transition: background 0.1s, color 0.1s;
    font-weight: 500;
  }

  .mode-tab:hover:not(:disabled):not(.active) {
    background: #f3f4f6;
    color: #374151;
  }

  .mode-tab.active {
    color: #0070f3;
    background: #fff;
    border-bottom: 2px solid #0070f3;
  }

  .mode-tab:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
