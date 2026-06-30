<script lang="ts">
  /**
   * TabBar — horizontal tab strip for multi-document editing (feat/tabbed-multi-file).
   *
   * Shows one tab per open document with: filename label, active highlight, × close button.
   * Drag-and-drop reorder via pointer events (pointerdown/pointermove/pointerup) —
   * chosen over HTML5 drag events because WKWebView can be finicky with custom drag
   * images; pointer events are fully spec-compliant and reliable in all WebKit contexts.
   *
   * Accessibility: existing keyboard nav (Enter/Space to activate) is preserved.
   * Tabs are intentionally not draggable via keyboard — arrow-key reorder is tracked as
   * a follow-up enhancement (low priority given the desktop Tauri context).
   *
   * Svelte 5 runes. Matches the app dark theme (CSS custom properties, no Tailwind).
   */
  import type { DocTab } from "$lib/doc-tabs.svelte";

  const {
    tabs,
    activeDocId,
    ontabclick,
    ontabclose,
    onmoveTab,
  }: {
    tabs: DocTab[];
    activeDocId: string | null;
    /** Called when the user clicks a tab to switch to it. */
    ontabclick: (docId: string) => void;
    /** Called when the user clicks the × button on a tab. */
    ontabclose: (docId: string) => void;
    /** Called when the user drags a tab to a new position. */
    onmoveTab: (fromIndex: number, toIndex: number) => void;
  } = $props();

  function filename(path: string): string {
    return path.split(/[\\/]/).at(-1) ?? path;
  }

  // ---------------------------------------------------------------------------
  // Drag-and-drop via pointer events
  //
  // Why pointer events over HTML5 drag API:
  //   - Custom drag images in HTML5 are unreliable in macOS WKWebView (the
  //     webview used by Tauri). `dataTransfer.setDragImage` can be ignored or
  //     rendered incorrectly.
  //   - Pointer events (`pointerdown` / `pointermove` / `pointerup`) are
  //     first-class in all WebKit versions that ship on macOS 10.15+ and are
  //     unaffected by the sandboxing layer around `WKWebView`.
  //   - setPointerCapture() ensures we receive `pointermove` events even when
  //     the pointer travels across other tabs or outside the bar entirely.
  //
  // Protocol:
  //   1. pointerdown on a tab div: record dragSrcIndex, capture pointer.
  //   2. pointermove on the same tab (capture routes events here):
  //      activate drag after DRAG_THRESHOLD px, then update dropIndex.
  //   3. pointerup / pointercancel: commit or discard.
  //
  // dropIndex is an INSERTION position (0 = before first tab, N = after last).
  // It is converted to a target array index before calling onmoveTab.
  // ---------------------------------------------------------------------------

  const DRAG_THRESHOLD = 5; // px movement before drag activates

  let barEl = $state<HTMLElement | null>(null);
  let dragSrcIndex = $state<number | null>(null);
  let dropIndex = $state<number | null>(null);
  let dragActive = $state(false);
  let dragStartX = 0;

  /**
   * Guard against a ghost onclick firing on the dragged tab after pointerup.
   * The browser emits a click event after every pointerdown+pointerup pair;
   * this flag lets the onclick handler skip that synthetic click.
   */
  let suppressNextClick = false;

  function computeDropIndex(clientX: number): number {
    if (!barEl) return 0;
    const tabEls = Array.from(barEl.querySelectorAll<HTMLElement>(".tab"));
    for (let i = 0; i < tabEls.length; i++) {
      const rect = tabEls[i].getBoundingClientRect();
      if (clientX < rect.left + rect.width / 2) return i;
    }
    return tabEls.length;
  }

  /** Returns true when the drop indicator should render before tab at index i. */
  function showDropBefore(i: number): boolean {
    if (!dragActive || dragSrcIndex === null || dropIndex === null) return false;
    if (dropIndex !== i) return false;
    // Dropping before i would be a no-op if src is already at i or i-1.
    if (dragSrcIndex === i || dragSrcIndex === i - 1) return false;
    return true;
  }

  /** Returns true when the drop indicator should render after the last tab. */
  function showDropAfterLast(): boolean {
    if (!dragActive || dragSrcIndex === null || dropIndex === null) return false;
    if (dropIndex !== tabs.length) return false;
    if (dragSrcIndex === tabs.length - 1) return false;
    return true;
  }

  function handlePointerDown(e: PointerEvent, i: number) {
    // Primary button only (left-click / touch).
    if (e.pointerType === "mouse" && e.button !== 0) return;
    // If the pointer-down originated on the close button, bail out immediately —
    // do NOT set pointer capture so the button's native click event can fire.
    if ((e.target as HTMLElement).closest(".tab-close")) return;
    dragSrcIndex = i;
    dropIndex = i;
    dragStartX = e.clientX;
    dragActive = false;
    suppressNextClick = false;
    // Capture to the tab element so pointermove fires here during the drag.
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function handlePointerMove(e: PointerEvent, i: number) {
    // Only the captured tab (the drag source) drives this.
    if (dragSrcIndex !== i) return;
    if (!dragActive && Math.abs(e.clientX - dragStartX) > DRAG_THRESHOLD) {
      dragActive = true;
    }
    if (dragActive) {
      dropIndex = computeDropIndex(e.clientX);
    }
  }

  function handlePointerUp(e: PointerEvent, i: number) {
    if (dragSrcIndex !== i) return;
    if (dragActive && dropIndex !== null && dragSrcIndex !== null) {
      // Convert insertion position to target array index:
      //   if inserting after the source's original position, subtract 1 because
      //   removing the source shifts subsequent items left by 1.
      const targetIndex =
        dropIndex > dragSrcIndex ? dropIndex - 1 : dropIndex;
      if (targetIndex !== dragSrcIndex) {
        onmoveTab(dragSrcIndex, targetIndex);
      }
      suppressNextClick = true;
    }
    dragSrcIndex = null;
    dropIndex = null;
    dragActive = false;
  }

  function handlePointerCancel(i: number) {
    if (dragSrcIndex !== i) return;
    dragSrcIndex = null;
    dropIndex = null;
    dragActive = false;
    suppressNextClick = false;
  }

  /** Click handler that ignores the ghost click emitted right after a drag. */
  function handleTabClick(docId: string) {
    if (suppressNextClick) {
      suppressNextClick = false;
      return;
    }
    ontabclick(docId);
  }
</script>

{#if tabs.length > 0}
  <div
    class="tab-bar"
    class:tab-bar-dragging={dragActive}
    role="tablist"
    aria-label="Open documents"
    bind:this={barEl}
  >
    {#each tabs as tab, i (tab.docId)}
      {#if showDropBefore(i)}
        <div class="drop-indicator" aria-hidden="true"></div>
      {/if}
      <div
        class="tab"
        class:tab-active={tab.docId === activeDocId}
        class:tab-dragging={dragActive && dragSrcIndex === i}
        role="tab"
        aria-selected={tab.docId === activeDocId}
        tabindex={tab.docId === activeDocId ? 0 : -1}
        title={tab.doc.path}
        onclick={() => handleTabClick(tab.docId)}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            ontabclick(tab.docId);
          }
        }}
        onpointerdown={(e) => handlePointerDown(e, i)}
        onpointermove={(e) => handlePointerMove(e, i)}
        onpointerup={(e) => handlePointerUp(e, i)}
        onpointercancel={() => handlePointerCancel(i)}
      >
        <span class="tab-label">{filename(tab.doc.path)}</span>
        <button
          class="tab-close"
          aria-label={`Close ${filename(tab.doc.path)}`}
          title="Close tab (Cmd/Ctrl+W)"
          onclick={(e) => {
            e.stopPropagation();
            ontabclose(tab.docId);
          }}
        >×</button>
      </div>
    {/each}
    {#if showDropAfterLast()}
      <div class="drop-indicator" aria-hidden="true"></div>
    {/if}
  </div>
{/if}

<style>
  .tab-bar {
    display: flex;
    align-items: stretch;
    height: 34px;
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    overflow-x: auto;
    overflow-y: hidden;
    flex-shrink: 0;
    /* Hide scrollbar but keep scrollability for many tabs */
    scrollbar-width: none;
  }
  .tab-bar::-webkit-scrollbar { display: none; }

  /* Suppress default browser text-selection cursor during drag */
  .tab-bar-dragging {
    cursor: grabbing;
    user-select: none;
    -webkit-user-select: none;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: 0 var(--space-2) 0 var(--space-3);
    min-width: 100px;
    max-width: 200px;
    border-right: 1px solid var(--color-border);
    border-bottom: 2px solid transparent;
    cursor: grab;
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    white-space: nowrap;
    user-select: none;
    -webkit-user-select: none;
    transition: background 100ms, color 100ms, opacity 100ms;
    flex-shrink: 0;
    outline: none;
    touch-action: none; /* prevent scroll interference during pointer drag */
  }
  .tab:hover {
    background: var(--color-bg-hover);
    color: var(--color-text-secondary);
  }
  .tab:focus-visible {
    outline: 2px solid var(--color-primary);
    outline-offset: -2px;
  }
  .tab.tab-active {
    background: var(--color-bg);
    color: var(--color-text);
    border-bottom-color: var(--color-primary);
  }

  /* Visual feedback: the tab being dragged fades so the insertion point is clear */
  .tab.tab-dragging {
    opacity: 0.45;
    cursor: grabbing;
  }

  .tab-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tab-close {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0;
    flex-shrink: 0;
    transition: background 100ms, color 100ms;
  }
  .tab-close:hover {
    background: var(--color-bg-active);
    color: var(--color-text);
  }
  .tab.tab-active .tab-close {
    color: var(--color-text-secondary);
  }
  .tab.tab-active .tab-close:hover {
    background: var(--color-bg-active);
    color: var(--color-danger);
  }

  /* Drop insertion indicator — thin vertical stripe that appears between tabs */
  .drop-indicator {
    width: 3px;
    flex-shrink: 0;
    align-self: stretch;
    background: var(--color-primary);
    border-radius: 2px;
    /* Slight glow so it's visible against both light and dark tabs */
    box-shadow: 0 0 4px var(--color-primary);
    pointer-events: none;
  }
</style>
