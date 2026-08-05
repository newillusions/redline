<script lang="ts">
  /**
   * Undo/Redo toolbar buttons over `MarkupStore.undo()`/`redo()` - the model + history
   * stack were fully implemented and unit-tested but had zero UI or keyboard surface
   * (found in the 2026-08-05 GUI validation pass, obs:us5j4ne1r5byjzle8u23). The Cmd/Ctrl+Z
   * / Cmd/Ctrl+Shift+Z / Cmd/Ctrl+Y keyboard bindings live in App.svelte's `handleKeydown`
   * (via `$lib/keyboard-shortcuts`); this component is the visible, clickable surface plus
   * the disabled-state reflection of `store.canUndo`/`canRedo`.
   *
   * A separate leaf component (mirrors ToolChestPanel's `markupStore` prop pattern) rather
   * than inline buttons in App.svelte - App.svelte has no test file (license gate + Tauri
   * IPC make it expensive to mount), so keeping this as its own component is what makes it
   * directly testable with @testing-library/svelte, same as ToolPalette.interaction.test.ts.
   */
  import type { MarkupStore } from "$lib/markup-store.svelte";

  const { store }: { store: MarkupStore | null } = $props();
</script>

<button
  class="btn-toolbar btn-icon"
  onclick={() => store?.undo()}
  disabled={!store || !store.canUndo}
  title="Undo (Cmd/Ctrl+Z)"
>↶ Undo</button>
<button
  class="btn-toolbar btn-icon"
  onclick={() => store?.redo()}
  disabled={!store || !store.canRedo}
  title="Redo (Cmd/Ctrl+Shift+Z)"
>↷ Redo</button>

<style>
  /* Mirrors App.svelte's .btn-toolbar/.btn-icon - Svelte scoped CSS doesn't cross the
     component boundary, so the parent toolbar's styles don't reach these buttons. */
  .btn-toolbar {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-3);
    transition: background 120ms;
  }
  .btn-toolbar:hover:not(:disabled) { background: var(--color-bg-hover); }
  .btn-toolbar:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-toolbar.btn-icon { padding: var(--space-1) var(--space-2); }
</style>
