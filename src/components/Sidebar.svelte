<script lang="ts" module>
  import type { PanelId, Side } from "$lib/panel-layout";

  /** Shared between the two sidebars so the one the pointer is over can draw the drop
   *  marker for a panel dragged out of the other one. */
  const drag = $state<{ id: PanelId | null; side: Side | null; index: number }>({
    id: null,
    side: null,
    index: -1,
  });
</script>

<script lang="ts">
  /**
   * Sidebar - one side column hosting a stack of PANELS (owner terminology: the column is
   * the SIDEBAR, the stacked modules are PANELS).
   *
   * Owner defect 2026-11-04: "the tool chests in the side bars cannot be resized or
   * repositioned, and when open they often cannot show their overflow". Root cause of the
   * clipping: the old stack wrapped each panel in Accordion, whose scoped
   * `.accordion-body { overflow: hidden }` (two-class specificity once Svelte adds its hash)
   * beat the global `.panel-body-flush { overflow-y: auto }` (one class), so no panel body
   * could ever scroll - and every panel was `flex: 1` with no minimum, so at 1024x640 each
   * got ~20px. This component replaces that stack:
   *
   *  - every expanded panel is `flex: <weight> 1 0` with a minimum height, and its body is
   *    the scroll container (`min-height: 0; overflow: auto`), so content always scrolls;
   *  - if the window is too short for every expanded panel's minimum, the stack itself
   *    scrolls rather than squashing panels to nothing;
   *  - a splitter between neighbouring expanded panels moves height between just those two;
   *  - a handle on the inner edge resizes the sidebar width;
   *  - panels move by dragging the grip in the header (within or across sidebars) or from
   *    the header's "..." menu (keyboard path);
   *  - all of it lives in the PanelLayoutStore and persists across restarts.
   */
  import type { Snippet } from "svelte";
  import { tick } from "svelte";
  import ResizeHandle from "./ResizeHandle.svelte";
  import type { PanelLayoutStore } from "$lib/panel-layout-store.svelte";
  import {
    PANEL_MIN_HEIGHT,
    PANEL_TITLES,
    SIDEBAR_MIN_WIDTH,
    SIDEBAR_MAX_WIDTH,
    clampSidebarWidth,
    dropIndex,
    movePanel,
    movePanelBy,
    setCollapsed,
    setSidebarWidth,
    setWeights,
    splitWeights,
  } from "$lib/panel-layout";

  interface Props {
    side: Side;
    store: PanelLayoutStore;
    /** Body content for each panel this app knows how to render. */
    content: Partial<Record<PanelId, Snippet>>;
    /** Panels that only exist while switched on elsewhere (Search). Default: always shown. */
    shown?: (id: PanelId) => boolean;
    /** Panels with a close button (Search) - called when it is pressed. */
    onclose?: Partial<Record<PanelId, () => void>>;
    testId?: string;
  }

  const { side, store, content, shown = () => true, onclose = {}, testId }: Props = $props();

  /** Existing data-testids kept stable for harnesses and tests written against the old stack. */
  const HEADER_TEST_IDS: Partial<Record<PanelId, string>> = {
    recent: "accordion-recent-docs",
    toolchest: "accordion-tool-chest",
    bookmarks: "accordion-bookmarks",
    links: "accordion-links",
    navigator: "accordion-navigator",
    layers: "accordion-layers",
    history: "accordion-history",
    properties: "accordion-properties",
  };

  const DRAG_THRESHOLD = 5;

  let innerWidth = $state(typeof window === "undefined" ? 1280 : window.innerWidth);

  const layout = $derived(store.layout);
  const panels = $derived(layout[side].panels.filter((id) => content[id] && shown(id)));
  const width = $derived(clampSidebarWidth(layout[side].width, innerWidth));
  const otherSide: Side = $derived(side === "left" ? "right" : "left");

  let rootEl = $state<HTMLElement | null>(null);
  function frameEl(id: PanelId): HTMLElement | null {
    return rootEl?.querySelector<HTMLElement>(`[data-panel-id="${id}"]`) ?? null;
  }

  /** The next expanded panel below `id` in this sidebar, if any (splitter partner). */
  function nextExpanded(id: PanelId): PanelId | null {
    const i = panels.indexOf(id);
    for (let j = i + 1; j < panels.length; j++) {
      if (!layout.collapsed[panels[j]]) return panels[j];
    }
    return null;
  }

  function resizeSplit(a: PanelId, b: PanelId, delta: number) {
    const ea = frameEl(a);
    const eb = frameEl(b);
    if (!ea || !eb) return;
    const [wa, wb] = splitWeights(
      layout.weights[a],
      layout.weights[b],
      ea.getBoundingClientRect().height,
      eb.getBoundingClientRect().height,
      delta,
    );
    store.update((l) => setWeights(l, a, wa, b, wb));
  }

  /** Share of the pair's height taken by the upper panel, 0-100 (aria-valuenow). */
  function splitShare(a: PanelId, b: PanelId): number {
    const wa = layout.weights[a];
    return (wa / (wa + layout.weights[b])) * 100;
  }

  function resizeWidth(delta: number) {
    const grow = side === "left" ? delta : -delta;
    store.update((l) => setSidebarWidth(l, side, width + grow, innerWidth));
  }

  function toggleCollapsed(id: PanelId) {
    store.update((l) => setCollapsed(l, id, !l.collapsed[id]));
  }

  // --- Panel "..." menu (keyboard-accessible move path) ---------------------------------
  let menuFor = $state<PanelId | null>(null);
  let menuStyle = $state("");
  let menuEl = $state<HTMLElement | null>(null);

  async function openMenu(id: PanelId, trigger: HTMLElement) {
    if (menuFor === id) {
      menuFor = null;
      return;
    }
    const r = trigger.getBoundingClientRect();
    const menuWidth = 220;
    const left = Math.max(8, Math.min(r.right - menuWidth, innerWidth - menuWidth - 8));
    menuStyle = `top: ${r.bottom + 2}px; left: ${left}px;`;
    menuFor = id;
    await tick();
    menuEl?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')?.focus();
  }

  function closeMenu(refocus = true) {
    const id = menuFor;
    menuFor = null;
    if (refocus && id) frameEl(id)?.querySelector<HTMLElement>(".panel-menu-trigger")?.focus();
  }

  function menuItems(id: PanelId) {
    const list = layout[side].panels;
    const i = list.indexOf(id);
    return [
      { key: "up", label: "Move up", disabled: i <= 0, run: () => store.update((l) => movePanelBy(l, id, -1)) },
      { key: "down", label: "Move down", disabled: i === -1 || i >= list.length - 1, run: () => store.update((l) => movePanelBy(l, id, 1)) },
      {
        key: "side",
        label: `Move to ${otherSide} sidebar`,
        disabled: false,
        run: () => store.update((l) => movePanel(l, id, otherSide, l[otherSide].panels.length)),
      },
      { key: "reset", label: "Reset panel layout", disabled: false, run: () => store.reset() },
    ];
  }

  function onMenuKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const els = Array.from(menuEl?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
      if (els.length === 0) return;
      const idx = els.indexOf(document.activeElement as HTMLElement);
      els[e.key === "ArrowDown" ? (idx + 1) % els.length : (idx - 1 + els.length) % els.length].focus();
    }
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (menuFor && menuEl && !menuEl.contains(e.target as Node) && !(e.target as Element).closest?.(".panel-menu-trigger")) {
      closeMenu(false);
    }
  }

  // --- Drag a panel by its grip (within or across sidebars) -----------------------------
  let pending: { id: PanelId; x: number; y: number } | null = null;

  function onGripDown(e: PointerEvent, id: PanelId) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    pending = { id, x: e.clientX, y: e.clientY };
  }

  function sidebarUnder(x: number): HTMLElement | null {
    const bars = Array.from(document.querySelectorAll<HTMLElement>("[data-sidebar-side]"));
    for (const b of bars) {
      const r = b.getBoundingClientRect();
      if (x >= r.left && x <= r.right) return b;
    }
    return null;
  }

  function onGripMove(e: PointerEvent) {
    if (!pending) return;
    if (drag.id === null) {
      if (Math.hypot(e.clientX - pending.x, e.clientY - pending.y) < DRAG_THRESHOLD) return;
      drag.id = pending.id;
    }
    const bar = sidebarUnder(e.clientX);
    if (!bar) {
      drag.side = null;
      drag.index = -1;
      return;
    }
    const frames = Array.from(bar.querySelectorAll<HTMLElement>("[data-panel-id]")).map((el) => {
      const r = el.getBoundingClientRect();
      return { id: el.dataset.panelId as PanelId, mid: r.top + r.height / 2 };
    });
    drag.side = bar.dataset.sidebarSide as Side;
    drag.index = dropIndex(frames, drag.id, e.clientY);
  }

  function resetDrag() {
    pending = null;
    drag.id = null;
    drag.side = null;
    drag.index = -1;
  }

  /** pointercancel (OS gesture, lost capture): abandon the drag - never commit a drop. */
  function onGripCancel(e: PointerEvent) {
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    resetDrag();
  }

  function onWindowKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape" && (pending !== null || drag.id !== null)) {
      e.preventDefault();
      resetDrag();
    }
  }

  function onGripUp(e: PointerEvent) {
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    const id = drag.id;
    const target = drag.side;
    const index = drag.index;
    resetDrag();
    if (!id || !target || index < 0) return;
    // `index` counts only the panels rendered in the target sidebar; map it onto the full
    // persisted list (which may hold panels not shown right now, e.g. a closed Search).
    store.update((l) => {
      const rendered = Array.from(
        document.querySelectorAll<HTMLElement>(`[data-sidebar-side="${target}"] [data-panel-id]`),
      )
        .map((el) => el.dataset.panelId as PanelId)
        .filter((p) => p !== id);
      const full = l[target].panels.filter((p) => p !== id);
      const anchor = rendered[index];
      const at = anchor === undefined ? full.length : full.indexOf(anchor);
      return movePanel(l, id, target, at === -1 ? full.length : at);
    });
  }

  function onGripKeyDown(e: KeyboardEvent, id: PanelId) {
    // Keyboard equivalent of dragging the grip: Up/Down reorder, Left/Right change sidebar.
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      store.update((l) => movePanelBy(l, id, e.key === "ArrowUp" ? -1 : 1));
    } else if ((e.key === "ArrowLeft" && side === "right") || (e.key === "ArrowRight" && side === "left")) {
      e.preventDefault();
      store.update((l) => movePanel(l, id, otherSide, l[otherSide].panels.length));
    } else {
      return;
    }
    afterMove(id, e.key);
  }

  /** Polite screen-reader message for the last keyboard/menu move. */
  let announcement = $state("");

  /** After a keyboard or menu move: keep focus on the moved panel's grip (so repeated
   *  presses keep moving it) and announce where it ended up. */
  function afterMove(id: PanelId, how: string) {
    void tick().then(() => {
      document.querySelector<HTMLElement>(`[data-panel-id="${id}"] .panel-grip`)?.focus();
      if (how === "reset") {
        announcement = "Panel layout reset to default";
        return;
      }
      const sideNow: Side = layout.right.panels.includes(id) ? "right" : "left";
      const list = layout[sideNow].panels;
      announcement = `${PANEL_TITLES[id]} panel moved to ${sideNow} sidebar, position ${list.indexOf(id) + 1} of ${list.length}`;
    });
  }

  const dropMarkerIndex = $derived(drag.id !== null && drag.side === side ? drag.index : -1);
  /** Panels in render order without the one being dragged - drop indices count these. */
  const markerPanels = $derived(drag.id ? panels.filter((p) => p !== drag.id) : panels);
</script>

<svelte:window bind:innerWidth onpointerdown={onWindowPointerDown} onkeydown={onWindowKeyDown} />

<aside
  bind:this={rootEl}
  class="sidebar sidebar-{side}"
  style="width: {width}px"
  data-testid={testId}
  data-sidebar-side={side}
  aria-label="{side === 'left' ? 'Left' : 'Right'} sidebar"
>
  {#if side === "right"}
    <ResizeHandle
      orientation="vertical"
      label="Resize right sidebar"
      value={width}
      min={SIDEBAR_MIN_WIDTH}
      max={SIDEBAR_MAX_WIDTH}
      onresize={resizeWidth}
      testId="resize-sidebar-right"
    />
  {/if}
  <div class="sr-only" role="status" aria-live="polite" data-testid="panel-move-status-{side}">{announcement}</div>
  <div class="sidebar-stack" data-testid="sidebar-stack-{side}">
    {#each panels as id (id)}
      {@const expanded = !layout.collapsed[id]}
      {@const below = expanded ? nextExpanded(id) : null}
      {@const markerHere = dropMarkerIndex !== -1 && markerPanels[dropMarkerIndex] === id}
      {#if markerHere}<div class="drop-marker" aria-hidden="true"></div>{/if}
      <section
        class="panel-frame panel-frame--{id}"
        class:panel-frame--collapsed={!expanded}
        class:panel-frame--dragging={drag.id === id}
        class:panel-section--search={id === "search"}
        style={expanded ? `flex: ${layout.weights[id]} 1 0px; min-height: ${PANEL_MIN_HEIGHT}px;` : undefined}
        data-panel-id={id}
        data-testid="panel-frame-{id}"
        aria-label={PANEL_TITLES[id]}
      >
        <header class="panel-frame-header">
          <span
            class="panel-grip"
            role="button"
            tabindex="0"
            aria-label="Move {PANEL_TITLES[id]} panel (drag, or arrow keys)"
            title="Drag to move this panel; arrow keys also move it"
            data-testid="panel-grip-{id}"
            onpointerdown={(e) => onGripDown(e, id)}
            onpointermove={onGripMove}
            onpointerup={onGripUp}
            onpointercancel={onGripCancel}
            onkeydown={(e) => onGripKeyDown(e, id)}
          >⠿</span>
          <button
            type="button"
            class="panel-toggle"
            aria-expanded={expanded}
            data-testid={HEADER_TEST_IDS[id] ?? `panel-toggle-${id}`}
            onclick={() => toggleCollapsed(id)}
          >
            <span class="panel-caret" aria-hidden="true">{expanded ? "▾" : "▸"}</span>
            <span class="panel-title">{PANEL_TITLES[id]}</span>
          </button>
          <button
            type="button"
            class="panel-icon-btn panel-menu-trigger"
            aria-haspopup="menu"
            aria-expanded={menuFor === id}
            aria-label="{PANEL_TITLES[id]} panel options"
            title="Panel options"
            data-testid="panel-menu-{id}"
            onclick={(e) => openMenu(id, e.currentTarget)}
          >⋯</button>
          {#if onclose[id]}
            <button
              type="button"
              class="panel-icon-btn"
              aria-label="Close {PANEL_TITLES[id].toLowerCase()}"
              onclick={() => onclose[id]?.()}
            >✕</button>
          {/if}
        </header>
        {#if expanded}
          <div class="panel-frame-body" data-testid="panel-body-{id}">
            {@render content[id]?.()}
          </div>
        {/if}
      </section>
      {#if below}
        <ResizeHandle
          orientation="horizontal"
          label="Resize {PANEL_TITLES[id]} and {PANEL_TITLES[below]}"
          value={splitShare(id, below)}
          min={0}
          max={100}
          onresize={(d) => resizeSplit(id, below, d)}
          testId="resize-split-{id}"
        />
      {/if}
    {/each}
    {#if dropMarkerIndex !== -1 && dropMarkerIndex >= markerPanels.length}
      <div class="drop-marker" aria-hidden="true"></div>
    {/if}
    {#if panels.length === 0}
      <p class="sidebar-empty">
        No panels here. Drag a panel's grip into this sidebar, or use a panel's ⋯ menu.
      </p>
    {/if}
  </div>
  {#if side === "left"}
    <ResizeHandle
      orientation="vertical"
      label="Resize left sidebar"
      value={width}
      min={SIDEBAR_MIN_WIDTH}
      max={SIDEBAR_MAX_WIDTH}
      onresize={resizeWidth}
      testId="resize-sidebar-left"
    />
  {/if}
</aside>

{#if menuFor}
  {@const id = menuFor}
  <div
    class="panel-menu"
    role="menu"
    tabindex="-1"
    aria-label="{PANEL_TITLES[id]} panel options"
    style={menuStyle}
    bind:this={menuEl}
    onkeydown={onMenuKeyDown}
    data-testid="panel-menu-list"
  >
    {#each menuItems(id) as item (item.key)}
      {#if item.key === "reset"}<div class="panel-menu-sep" role="separator"></div>{/if}
      <button
        type="button"
        class="panel-menu-item"
        role="menuitem"
        disabled={item.disabled}
        onclick={() => {
          const moved = id;
          item.run();
          closeMenu(false);
          afterMove(moved, item.key);
        }}
      >{item.label}</button>
    {/each}
  </div>
{/if}

<style>
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
  }
  .sidebar {
    display: flex;
    flex-direction: row;
    flex-shrink: 0;
    min-width: 0;
    background: var(--color-bg-panel);
    overflow: hidden;
  }
  .sidebar-left {
    border-right: 1px solid var(--color-border);
  }
  .sidebar-right {
    border-left: 1px solid var(--color-border);
  }

  /* The stack scrolls only when the window is too short for every expanded panel's
     minimum height; normally the panels share the height and scroll individually. */
  .sidebar-stack {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-x: hidden;
    overflow-y: auto;
  }

  .panel-frame {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-bottom: 1px solid var(--color-border);
  }
  .panel-frame--collapsed {
    flex: 0 0 auto;
  }
  .panel-frame--dragging {
    opacity: 0.5;
  }

  .panel-frame-header {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    gap: var(--space-1);
    padding: 0 var(--space-1) 0 0;
    border-bottom: 1px solid var(--color-border);
  }
  .panel-frame--collapsed .panel-frame-header {
    border-bottom: none;
  }

  .panel-grip {
    flex-shrink: 0;
    cursor: grab;
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    line-height: 1;
    padding: var(--space-2) 0 var(--space-2) var(--space-2);
    touch-action: none;
  }
  .panel-grip:active {
    cursor: grabbing;
  }
  .panel-grip:hover,
  .panel-grip:focus-visible {
    color: var(--color-text);
    outline: none;
  }

  .panel-toggle {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    margin: 0;
    padding: var(--space-2) var(--space-1);
    cursor: pointer;
    font: inherit;
    font-size: var(--font-size-xs);
    font-weight: 600;
    color: var(--color-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    text-align: left;
  }
  .panel-toggle:hover,
  .panel-toggle:focus-visible {
    color: var(--color-text);
  }
  .panel-caret {
    flex-shrink: 0;
    width: 1em;
    color: var(--color-text-muted);
  }
  .panel-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .panel-icon-btn {
    flex-shrink: 0;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: var(--font-size-sm);
    line-height: 1;
    padding: var(--space-1);
  }
  .panel-icon-btn:hover,
  .panel-icon-btn:focus-visible,
  .panel-icon-btn[aria-expanded="true"] {
    color: var(--color-text);
    background: var(--color-bg-hover);
  }

  /* The body is the scroll container: min-height 0 lets it shrink inside the frame, and
     flex column lets a panel whose root uses height:100% / flex:1 fill it exactly. */
  .panel-frame-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: auto;
  }

  .drop-marker {
    flex: 0 0 auto;
    height: var(--space-1);
    margin: 0 var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--color-primary);
  }

  .sidebar-empty {
    margin: var(--space-3);
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
  }

  .panel-menu {
    position: fixed;
    z-index: 800;
    width: 220px;
    display: flex;
    flex-direction: column;
    padding: var(--space-1);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    box-shadow: var(--shadow-menu);
  }
  .panel-menu-item {
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
  .panel-menu-item:hover:not(:disabled),
  .panel-menu-item:focus-visible {
    background: var(--color-bg-hover);
    outline: none;
  }
  .panel-menu-item:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .panel-menu-sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--color-border);
  }
</style>
