<script lang="ts">
  /**
   * SettingsView — application preferences.
   *
   * Deliberately minimal: only settings wired to a real, functional
   * preference belong here. See `.claude/plans/PLAN_EXTRAS.md` for settings
   * the design mockup suggested that have no backing feature yet (CPU thread
   * limit, context-menu integration) — they are left out rather than shown
   * as non-functional placeholders.
   */
  import { FORMATS } from "../formats";
  import type { Appearance } from "../theme";

  interface Props {
    appearance: Appearance;
    onChange: (appearance: Appearance) => void;
    /** Pinned default compression format, or `null` for "Auto" (smart
     *  per-content default — see `defaultFormat` in `../formats`). */
    defaultFormat: string | null;
    onChangeDefaultFormat: (format: string | null) => void;
    onClose: () => void;
  }

  let { appearance, onChange, defaultFormat, onChangeDefaultFormat, onClose }: Props = $props();

  const OPTIONS: { value: Appearance; label: string }[] = [
    { value: "auto", label: "Auto" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  function handleFormatChange(e: Event): void {
    const value = (e.currentTarget as HTMLSelectElement).value;
    onChangeDefaultFormat(value === "" ? null : value);
  }
</script>

<div class="settings-view">
  <div class="settings-header">
    <h2 class="title">Settings</h2>
    <button class="btn-secondary" onclick={onClose}>← Back</button>
  </div>

  <div class="setting-row">
    <div class="setting-copy">
      <span class="setting-label">Appearance</span>
      <span class="setting-hint">Follows your OS light/dark setting by default.</span>
    </div>
    <div class="segmented" role="group" aria-label="Appearance">
      {#each OPTIONS as opt (opt.value)}
        <button
          class="segment"
          class:active={appearance === opt.value}
          onclick={() => onChange(opt.value)}
        >
          {opt.label}
        </button>
      {/each}
    </div>
  </div>

  <div class="setting-row">
    <div class="setting-copy">
      <span class="setting-label">Default Compression Codec</span>
      <span class="setting-hint">Auto picks by content: a bundle/folder gets .tar.zst, a single file gets .zst.</span>
    </div>
    <select class="select-input" value={defaultFormat ?? ""} onchange={handleFormatChange}>
      <option value="">Auto</option>
      {#each FORMATS as fmt (fmt.name)}
        <option value={fmt.name}>{fmt.label}</option>
      {/each}
    </select>
  </div>
</div>

<style>
  .settings-view {
    max-width: 640px;
    margin: 0 auto;
    padding: 1.5rem 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .settings-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border);
  }

  .title {
    margin: 0;
    font-size: 1.15rem;
    color: var(--text);
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1rem;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
  }

  .setting-copy {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }

  .setting-label {
    font-size: 0.92rem;
    font-weight: 600;
    color: var(--text);
  }

  .setting-hint {
    font-size: 0.8rem;
    color: var(--text-faint);
  }

  .segmented {
    display: flex;
    padding: 0.2rem;
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 8px;
    gap: 0.15rem;
    flex-shrink: 0;
  }

  .segment {
    padding: 0.3rem 0.75rem;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 0.82rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .segment:hover {
    background: var(--surface-hover);
  }

  .segment.active {
    background: var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
  }

  .select-input {
    flex-shrink: 0;
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    font-size: 0.85rem;
    color: var(--text);
    background: var(--surface);
    max-width: 14rem;
  }

  .btn-secondary {
    padding: 0.35rem 0.75rem;
    border: 1px solid var(--border-input);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font-size: 0.85rem;
    cursor: pointer;
    transition: background 0.12s;
  }

  .btn-secondary:hover {
    background: var(--surface-hover);
  }
</style>
