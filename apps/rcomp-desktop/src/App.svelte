<script lang="ts">
  /**
   * App.svelte — application shell.
   *
   * Hosts the {@link Workspace}, which owns the always-visible archive-browser
   * file list and all open/compose state. The app name is shown by the OS
   * window title bar, so no in-app title is rendered here.
   */
  import Workspace from "./lib/components/Workspace.svelte";
</script>

<div class="app-shell">
  <Workspace />
</div>

<style>
  .app-shell {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg);
  }
</style>
