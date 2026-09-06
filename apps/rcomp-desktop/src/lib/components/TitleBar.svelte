<script lang="ts">
  /**
   * TitleBar — the app's own branded title bar, replacing the design
   * mockup's header (minus the tab nav, search, and dark-mode toggle it
   * showed — mode is inferred rather than tab-picked, and appearance lives
   * in Settings; see `.claude/rcomp_desktop_application.html` for the
   * reference design and `tauri.conf.json`'s `titleBarStyle: "Overlay"` +
   * `hiddenTitle` for how the native title bar is replaced on macOS).
   *
   * On macOS the native traffic lights float over this bar (`titleBarStyle:
   * "Overlay"`), so the left edge reserves space for them and the bar is a
   * `data-tauri-drag-region` — clicking empty space still drags the window,
   * same as the native title bar would. On Windows/Linux the OS keeps its
   * own title bar above this one; the extra left inset is skipped there.
   */
  type WMode = "empty" | "open" | "compose";

  interface Props {
    mode: WMode;
    settingsActive: boolean;
    onReset: () => void;
    onOpenSettings: () => void;
  }

  let { mode, settingsActive, onReset, onOpenSettings }: Props = $props();

  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);
</script>

<header class="title-bar" class:mac-inset={isMac} data-tauri-drag-region>
  <div class="brand" data-tauri-drag-region>
    <svg class="logo" viewBox="0 0 100 100" aria-hidden="true">
      <rect x="2" y="2" width="96" height="96" rx="24" fill="#0066FF" />
      <rect x="7" y="7" width="86" height="86" rx="20" fill="none" stroke="#3884FF" stroke-width="2.5" />
      <rect x="46" y="21" width="8" height="58" rx="4" fill="#FFFFFF" />
      <rect x="26" y="30" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="26" y="46.5" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="26" y="63" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="54" y="30" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="54" y="46.5" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="54" y="63" width="20" height="7" rx="3.5" fill="#FFFFFF" />
      <rect x="41.5" y="41.5" width="17" height="17" rx="5" fill="#FFFFFF" />
      <circle cx="50" cy="50" r="3.5" fill="#0066FF" />
    </svg>
    <span class="name">rcomp</span>
    <span class="version">v0.2.0</span>
  </div>

  <div class="mode-label" data-tauri-drag-region>
    {#if mode === "open"}Open archive{:else if mode === "compose"}Compress{/if}
  </div>

  <div class="actions">
    {#if mode !== "empty"}
      <button class="btn-reset" onclick={onReset} title="Start over">New</button>
    {/if}
    <button
      class="btn-icon"
      class:active={settingsActive}
      onclick={onOpenSettings}
      title="Settings"
      aria-label="Settings"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path
          d="M10.343 3.94c.09-.542.56-.94 1.11-.94h1.093c.55 0 1.02.398 1.11.94l.149.894c.07.424.384.764.78.93.398.164.855.142 1.205-.108l.737-.527a1.125 1.125 0 011.45.12l.774.773c.39.39.44 1.002.12 1.45l-.527.738c-.25.35-.272.806-.107 1.204.165.397.505.71.93.78l.893.15c.543.09.94.56.94 1.109v1.094c0 .55-.397 1.02-.94 1.11l-.893.149c-.425.07-.765.383-.93.78-.165.398-.143.854.107 1.204l.527.738c.32.447.269 1.06-.12 1.45l-.774.773a1.125 1.125 0 01-1.449.12l-.738-.526c-.35-.25-.806-.272-1.203-.107-.397.165-.71.505-.781.929l-.149.894c-.09.542-.56.94-1.11.94h-1.093c-.55 0-1.02-.398-1.11-.94l-.148-.894c-.071-.424-.384-.764-.781-.93-.398-.164-.854-.142-1.204.108l-.738.526c-.447.32-1.06.269-1.45-.12l-.773-.773a1.125 1.125 0 01-.12-1.45l.527-.737c.25-.35.272-.807.108-1.204-.165-.397-.506-.71-.93-.781l-.894-.149c-.542-.09-.94-.56-.94-1.109v-1.094c0-.55.398-1.02.94-1.11l.894-.149c.424-.07.765-.383.93-.78.165-.398.142-.854-.108-1.204l-.526-.738a1.125 1.125 0 01.12-1.45l.773-.773a1.125 1.125 0 011.45-.12l.737.527c.35.25.807.272 1.204.107.397-.165.71-.505.78-.929l.15-.894z"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
        <path d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
  </div>
</header>

<style>
  .title-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.5rem 0.9rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-subtle);
    /* Native macOS traffic lights sit roughly in this space when overlaid. */
    -webkit-app-region: drag;
  }

  .title-bar.mac-inset {
    padding-left: 78px;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }

  .logo {
    width: 1.35rem;
    height: 1.35rem;
    flex-shrink: 0;
  }

  .name {
    font-weight: 700;
    color: var(--text);
    letter-spacing: -0.01em;
  }

  .version {
    padding: 0.05rem 0.4rem;
    border-radius: 5px;
    background: var(--accent-subtle-bg);
    color: var(--accent-subtle-fg);
    border: 1px solid var(--border);
    font-size: 0.7rem;
    font-weight: 600;
  }

  .mode-label {
    flex: 1;
    text-align: center;
    font-size: 0.9rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    -webkit-app-region: no-drag;
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

  .btn-icon.active {
    background: var(--accent-subtle-bg);
    color: var(--accent-subtle-fg);
    border-color: var(--accent);
  }

  .btn-icon svg {
    width: 1.05rem;
    height: 1.05rem;
  }
</style>
