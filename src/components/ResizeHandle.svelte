<script lang="ts">
  /**
   * ResizeHandle - a draggable, keyboard-operable splitter (WAI-ARIA "separator" pattern).
   *
   * Reports raw movement only; the caller decides what it resizes and in which direction
   * (a left sidebar grows with +dx, a right sidebar with -dx, the bottom panel with -dy).
   *
   * - orientation "vertical": a vertical bar between columns, dragged left/right, reports dx.
   *   Keys: ArrowLeft/ArrowRight.
   * - orientation "horizontal": a horizontal bar between rows, dragged up/down, reports dy.
   *   Keys: ArrowUp/ArrowDown.
   * Shift+Arrow takes a larger step. Home/End are deliberately not handled (no fixed
   * "min/max" endpoint is meaningful for a weight split).
   *
   * Pointer events + setPointerCapture, not the HTML5 drag API - same reasoning as
   * TabBar.svelte (WKWebView drag-image quirks, and moves keep arriving when the pointer
   * leaves the 4px bar).
   */
  import { RESIZE_KEY_STEP, RESIZE_KEY_STEP_LARGE } from "$lib/panel-layout";

  interface Props {
    orientation: "vertical" | "horizontal";
    label: string;
    /** Incremental movement in CSS px since the last call. */
    onresize: (delta: number) => void;
    /** Called once when a drag ends (pointer up / cancel). */
    onresizeend?: () => void;
    /** Current size for assistive tech (aria-valuenow), and its bounds. */
    value?: number;
    min?: number;
    max?: number;
    testId?: string;
  }

  const { orientation, label, onresize, onresizeend, value, min, max, testId }: Props = $props();

  let dragging = $state(false);
  let last = 0;

  function coord(e: PointerEvent): number {
    return orientation === "vertical" ? e.clientX : e.clientY;
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    dragging = true;
    last = coord(e);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const c = coord(e);
    const d = c - last;
    if (d !== 0) {
      last = c;
      onresize(d);
    }
  }

  function endDrag(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    onresizeend?.();
  }

  function onKeyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? RESIZE_KEY_STEP_LARGE : RESIZE_KEY_STEP;
    const back = orientation === "vertical" ? "ArrowLeft" : "ArrowUp";
    const fwd = orientation === "vertical" ? "ArrowRight" : "ArrowDown";
    if (e.key !== back && e.key !== fwd) return;
    e.preventDefault();
    e.stopPropagation();
    onresize(e.key === fwd ? step : -step);
    onresizeend?.();
  }
</script>

<!-- A focusable separator is the WAI-ARIA window-splitter pattern; svelte's a11y lint does
     not know role="separator" becomes interactive when focusable. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="resize-handle"
  class:resize-handle--vertical={orientation === "vertical"}
  class:resize-handle--horizontal={orientation === "horizontal"}
  class:resize-handle--active={dragging}
  role="separator"
  tabindex="0"
  aria-label={label}
  aria-orientation={orientation}
  aria-valuenow={value === undefined ? undefined : Math.round(value)}
  aria-valuemin={min}
  aria-valuemax={max}
  title={label}
  data-testid={testId}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={endDrag}
  onpointercancel={endDrag}
  onkeydown={onKeyDown}
></div>

<style>
  .resize-handle {
    flex: 0 0 auto;
    background: transparent;
    position: relative;
    z-index: 2;
    touch-action: none;
    transition: background 120ms;
  }
  .resize-handle--vertical {
    width: var(--splitter-size);
    cursor: col-resize;
  }
  .resize-handle--horizontal {
    height: var(--splitter-size);
    cursor: row-resize;
  }
  .resize-handle:hover,
  .resize-handle--active {
    background: var(--color-primary);
  }
  .resize-handle:focus-visible {
    outline: none;
    background: var(--color-primary);
  }
</style>
