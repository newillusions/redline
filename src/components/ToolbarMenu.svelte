<script lang="ts" module>
  export interface ToolbarMenuItem {
    id: string;
    label: string;
    title?: string;
    disabled?: boolean;
    /** Render a separator above this item. */
    separator?: boolean;
    onselect: () => void;
  }
</script>

<script lang="ts">
  /**
   * ToolbarMenu - small dropdown used to group toolbar actions (Document menu, overflow
   * "More" menu). UX audit F4, 2026-10-10.
   *
   * Items are plain data so the toolbar can build different menus for wide and narrow
   * windows. Closes on outside click, Escape, or after an item is chosen.
   * Keyboard: Enter/Space on the trigger opens it; ArrowDown/ArrowUp move between items.
   */

  const {
    label,
    items,
    disabled = false,
    title = undefined,
    align = "left",
    testId = undefined,
  }: {
    label: string;
    items: ToolbarMenuItem[];
    disabled?: boolean;
    title?: string;
    align?: "left" | "right";
    testId?: string;
  } = $props();

  let open = $state(false);
  let root = $state<HTMLElement | null>(null);

  function choose(item: ToolbarMenuItem) {
    if (item.disabled) return;
    open = false;
    item.onselect();
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      open = false;
      root?.querySelector<HTMLElement>(".menu-trigger")?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const els = Array.from(root?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
      if (els.length === 0) return;
      const idx = els.indexOf(document.activeElement as HTMLElement);
      const next = e.key === "ArrowDown" ? (idx + 1) % els.length : (idx - 1 + els.length) % els.length;
      els[next].focus();
    }
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="toolbar-menu" bind:this={root} onkeydown={onKeyDown} role="presentation" data-testid={testId}>
  <button
    class="btn-toolbar menu-trigger"
    type="button"
    aria-haspopup="menu"
    aria-expanded={open}
    {title}
    {disabled}
    onclick={() => (open = !open)}
  >
    {label} <span aria-hidden="true">▾</span>
  </button>
  {#if open}
    <div class="menu-list" class:menu-right={align === "right"} role="menu">
      {#each items as item (item.id)}
        {#if item.separator}<div class="menu-sep" role="separator"></div>{/if}
        <button
          class="menu-item"
          type="button"
          role="menuitem"
          title={item.title}
          disabled={item.disabled}
          onclick={() => choose(item)}
        >
          {item.label}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .toolbar-menu {
    position: relative;
    display: inline-block;
  }

  /* Same look as the App toolbar's .btn-toolbar (that rule is scoped to App.svelte). */
  .menu-trigger {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-3);
    transition: background 120ms;
  }
  .menu-trigger:hover:not(:disabled),
  .menu-trigger[aria-expanded="true"] {
    background: var(--color-bg-hover);
  }
  .menu-trigger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .menu-list {
    position: absolute;
    top: calc(100% + 2px);
    left: 0;
    z-index: 800;
    min-width: 220px;
    display: flex;
    flex-direction: column;
    padding: var(--space-1);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.32);
  }
  .menu-list.menu-right {
    left: auto;
    right: 0;
  }

  .menu-item {
    text-align: left;
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-2) var(--space-3);
    white-space: nowrap;
  }
  .menu-item:hover:not(:disabled),
  .menu-item:focus-visible {
    background: var(--color-bg-hover);
  }
  .menu-item:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .menu-sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--color-border);
  }
</style>
