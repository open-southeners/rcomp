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
  import { onMount } from "svelte";
  import { getAppVersion } from "../ipc";

  type WMode = "empty" | "open" | "compose";

  interface Props {
    mode: WMode;
    settingsActive: boolean;
    onOpenSettings: () => void;
    onOpenChangelog: () => void;
  }

  let { mode, settingsActive, onOpenSettings, onOpenChangelog }: Props = $props();

  const isMac =
    typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

  // `tauri dev` serves the frontend through the Vite dev server, so
  // `import.meta.env.DEV` is true there and false in a built app bundle.
  const isDev = import.meta.env.DEV;

  let version = $state<string | null>(null);

  onMount(async () => {
    version = await getAppVersion();
  });
</script>

<header class="title-bar" class:mac-inset={isMac} data-tauri-drag-region>
  <div class="left" data-tauri-drag-region>
    {#if isMac}
      <div class="divider" data-tauri-drag-region></div>
    {/if}
    <div class="brand" data-tauri-drag-region>
      <svg class="logo" viewBox="0 0 48 48" fill="none" aria-hidden="true">
        <rect width="48" height="48" rx="12" fill="#0066FF" />
        <rect x="4" y="4" width="40" height="40" rx="10" stroke="rgba(255,255,255,0.2)" stroke-width="1" />
        <path d="M14 16H34M14 24H34M14 32H34" stroke="white" stroke-width="3" stroke-linecap="round" stroke-dasharray="2 6" />
        <path d="M24 12V36" stroke="white" stroke-width="3.5" stroke-linecap="round" />
        <rect x="20" y="20" width="8" height="8" rx="2" fill="white" />
        <circle cx="24" cy="24" r="1.5" fill="#0066FF" />
      </svg>
      <div class="name-group">
        <span class="name">rcomp</span>
        {#if isDev || version}
          <button
            class="version"
            class:preview={isDev}
            onclick={onOpenChangelog}
            title="What's New"
            aria-label="What's New"
          >{isDev ? "preview" : `v${version}`}</button>
        {/if}
      </div>
    </div>
  </div>

  <div class="mode-label" data-tauri-drag-region>
    {#if mode === "open"}Open archive{:else if mode === "compose"}Compress{/if}
  </div>

  <div class="actions">
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
    gap: 1rem;
    padding: 0.8rem 1.05rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-subtle);
    /* Native macOS traffic lights sit roughly in this space when overlaid. */
    -webkit-app-region: drag;
  }

  .title-bar.mac-inset {
    padding-left: 78px;
  }

  .left {
    display: flex;
    align-items: center;
    gap: 0.85rem;
    min-width: 0;
  }

  .divider {
    width: 1px;
    height: 1.1rem;
    background: var(--border);
    flex-shrink: 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    min-width: 0;
  }

  /* Tauri only treats the exact mousedown target as a drag region, not its
     ancestors, so clicks landing on the logo or text would otherwise miss
     the `data-tauri-drag-region` on `.brand`/`.title-bar`. */
  .brand * {
    pointer-events: none;
  }

  .logo {
    width: 1.87rem;
    height: 1.87rem;
    flex-shrink: 0;
  }

  .name-group {
    display: flex;
    align-items: center;
    gap: 0.55rem;
  }

  .name {
    font-size: 1.05rem;
    font-weight: 700;
    color: var(--text);
    letter-spacing: -0.01em;
  }

  .version {
    padding: 0.15rem 0.4rem;
    border-radius: 5px;
    background: var(--accent-subtle-bg);
    color: var(--accent-subtle-fg);
    border: 1px solid var(--border);
    font-size: 0.7rem;
    font-weight: 600;
    font-family: inherit;
    cursor: pointer;
    /* Opt back into pointer events and out of the drag region — `.brand *`
       disables both so clicks on the logo/name still drag the window. */
    pointer-events: auto;
    -webkit-app-region: no-drag;
  }

  .version:hover {
    background: var(--surface-hover);
    border-color: var(--accent);
  }

  .version.preview {
    background: var(--preview-bg);
    color: var(--preview-fg);
    border-color: var(--preview-border);
  }

  .version.preview:hover {
    background: var(--preview-bg);
    border-color: var(--preview-fg);
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

  .btn-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2.1rem;
    height: 2.1rem;
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
