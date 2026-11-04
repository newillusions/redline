<script lang="ts">
  /**
   * ContextMenu - the one right-click menu (markup, empty page area, Navigator thumbnail).
   * The caller builds the items (`$lib/context-menu`) and runs the chosen id; this component
   * only presents them: fixed-positioned and kept inside the window, arrow / Home / End /
   * Enter / Space keys, Escape or an outside press closes it, and focus returns to where it
   * was. Theme tokens only.
   */
  import { onMount, tick } from "svelte";
  import { clampMenuPosition, firstEnabled, lastEnabled, nextEnabled, type MenuItem } from "$lib/context-menu";

  const {
    items,
    x,
    y,
    label = "Context menu",
    onselect,
    onclose,
  }: {
    items: MenuItem[];
    /** Viewport coordinates (CSS px) the menu was opened at. */
    x: number;
    y: number;
    label?: string;
    onselect: (id: string) => void;
    onclose: () => void;
  } = $props();

  let menuEl = $state<HTMLDivElement | null>(null);
  let pos = $state<{ left: number; top: number }>({ left: 0, top: 0 });
  let active = $state(-1);
  let previousFocus: Element | null = null;

  function place() {
    if (!menuEl) return;
    pos = clampMenuPosition(x, y, menuEl.offsetWidth, menuEl.offsetHeight, window.innerWidth, window.innerHeight);
  }

  onMount(() => {
    previousFocus = document.activeElement;
    place();
    active = firstEnabled(items);
    void tick().then(() => menuEl?.focus({ preventScroll: true }));
    const onOutside = (e: Event) => {
      if (menuEl && e.target instanceof Node && menuEl.contains(e.target)) return;
      onclose();
    };
    // Capture: the press that dismisses the menu must not also reach what it landed on first.
    window.addEventListener("pointerdown", onOutside, true);
    window.addEventListener("blur", onclose);
    window.addEventListener("resize", onclose);
    return () => {
      window.removeEventListener("pointerdown", onOutside, true);
      window.removeEventListener("blur", onclose);
      window.removeEventListener("resize", onclose);
      if (previousFocus instanceof HTMLElement) previousFocus.focus({ preventScroll: true });
    };
  });

  // Re-place if the item list changes size while open.
  $effect(() => {
    void items;
    void tick().then(place);
  });

  function choose(i: number) {
    const it = items[i];
    if (!it || it.kind !== "item" || it.disabled) return;
    onselect(it.id);
  }

  function onKeyDown(e: KeyboardEvent) {
    // The menu owns every key while open: nothing may leak to the viewport's window handler.
    e.stopPropagation();
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        onclose();
        return;
      case "ArrowDown":
        e.preventDefault();
        active = nextEnabled(items, active, 1);
        return;
      case "ArrowUp":
        e.preventDefault();
        active = nextEnabled(items, active, -1);
        return;
      case "Home":
        e.preventDefault();
        active = firstEnabled(items);
        return;
      case "End":
        e.preventDefault();
        active = lastEnabled(items);
        return;
      case "Enter":
      case " ":
        e.preventDefault();
        choose(active);
        return;
      case "Tab":
        e.preventDefault();
        onclose();
        return;
      default:
        // Shift+F10 / the Menu key again just closes it.
        if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
          e.preventDefault();
          onclose();
        }
    }
  }
</script>

<div
  class="ctx-menu"
  role="menu"
  aria-label={label}
  tabindex="-1"
  data-testid="context-menu"
  bind:this={menuEl}
  style:left="{pos.left}px"
  style:top="{pos.top}px"
  aria-activedescendant={active >= 0 ? `ctx-item-${active}` : undefined}
  onkeydown={onKeyDown}
  oncontextmenu={(e) => e.preventDefault()}
>
  {#each items as it, i (i)}
    {#if it.kind === "separator"}
      <div class="ctx-sep" role="separator"></div>
    {:else if it.kind === "heading"}
      <div class="ctx-heading" role="presentation">{it.label}</div>
    {:else}
      <div
        id={`ctx-item-${i}`}
        class="ctx-item"
        class:active={active === i}
        class:danger={it.danger}
        class:disabled={it.disabled}
        role="menuitem"
        tabindex="-1"
        aria-disabled={it.disabled ? "true" : undefined}
        data-menu-id={it.id}
        onpointermove={() => { if (!it.disabled) active = i; }}
        onclick={() => choose(i)}
        onkeydown={() => {}}
      >
        <span class="ctx-label">{it.label}</span>
        {#if it.shortcut}<span class="ctx-shortcut">{it.shortcut}</span>{/if}
      </div>
    {/if}
  {/each}
</div>

<style>
  .ctx-menu {
    position: fixed;
    z-index: 1000;
    min-width: 200px;
    max-height: calc(100vh - 2 * var(--space-2));
    overflow-y: auto;
    padding: var(--space-1);
    background: var(--color-bg-panel);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-menu);
    font-size: var(--font-size-sm);
    outline: none;
    user-select: none;
  }
  .ctx-heading {
    padding: var(--space-1) var(--space-3);
    color: var(--color-text-secondary);
    font-size: var(--font-size-xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .ctx-sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--color-border-subtle);
  }
  .ctx-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    cursor: pointer;
    white-space: nowrap;
  }
  .ctx-item.active { background: var(--color-bg-hover); }
  .ctx-item.danger { color: var(--color-danger); }
  .ctx-item.disabled {
    color: var(--color-text-muted);
    cursor: default;
  }
  .ctx-shortcut { color: var(--color-text-secondary); font-size: var(--font-size-xs); }
  .ctx-item.disabled .ctx-shortcut { color: var(--color-text-muted); }
</style>
