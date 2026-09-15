<script lang="ts">
  /**
   * ThumbnailPanel — vertical strip of page thumbnails with drag-to-reorder.
   *
   * M4 S1: page thumbnail strip, drag-to-reorder, delete, rotate controls.
   * Calls page-op IPC functions on user action.
   *
   * Owner request 2026-09-15 (reading-mode/thumbnails/extract feature set, PR-B): real
   * thumbnail rendering (was a "p{n}" text placeholder), the current-page highlight, and
   * click-to-jump. Reuses the SAME render engine as everything else in this app - one
   * whole-page raster per thumbnail via `render_tile`, sized with `fitWidthZoom` /
   * `pageTileSizeCss` (the exact helpers ReadingView.svelte uses for its own one-raster-
   * per-page render model) - no new rendering path, just a much smaller target width.
   * Rendered LAZILY via IntersectionObserver as thumbnails scroll into view, so opening a
   * large sheet set doesn't fire hundreds of renderTile calls at once; fetched thumbnails
   * are kept for the component's lifetime (small, cheap - unlike ReadingView's full-page
   * rasters, no eviction needed) and invalidated wholesale on any page-structure-changing
   * op (rotate/delete/reorder), since indices/aspect-ratios shift under those.
   *
   * PR-C (2026-09-15): page SELECTION - click selects just that page (and still
   * navigates there, same as PR-B), ctrl/cmd-click toggles a page in/out of the
   * selection (does NOT navigate - you're building a multi-select, not browsing),
   * shift-click selects the contiguous range from the last-clicked page to this one
   * (standard file-explorer convention; does not move the shift-click anchor). A
   * floating action bar appears once anything is selected, calling `onextract` with
   * the raw (unsorted, possibly-duplicate-free-but-unordered) selected indices -
   * ExtractPagesDialog normalizes them to ascending/deduped order before extracting.
   */
  import { reorderPages, deletePage, rotatePage, getPageSize, renderTile } from "$lib/ipc";
  import { fitWidthZoom } from "$lib/viewport";
  import { pageTileSizeCss } from "$lib/reading-mode";
  import { onDestroy } from "svelte";

  interface Props {
    docId: string;
    pageCount: number;
    /** 0-based index of the page currently shown in the main viewport, or null when no
     *  document is open — drives the current-page highlight. */
    currentPage?: number | null;
    /** Called with a 0-based page index when a thumbnail is clicked (navigate there). */
    onjump?: (pageIndex: number) => void;
    /** Called with the currently-selected 0-based page indices when the user clicks the
     *  "Extract…" action bar button. */
    onextract?: (pageIndices: number[]) => void;
    /** Optional callback when pages change (e.g. to trigger re-render). */
    onPageOp?: () => void;
  }

  const { docId, pageCount, currentPage = null, onjump, onextract, onPageOp }: Props = $props();

  // ---------------------------------------------------------------------------
  // Selection (PR-C)
  // ---------------------------------------------------------------------------

  let selected = $state<Set<number>>(new Set());
  /** Anchor for the next shift-click range - the last page clicked WITHOUT shift. */
  let selectionAnchor: number | null = null;

  function handleThumbnailClick(e: MouseEvent, idx: number) {
    const toggle = e.ctrlKey || e.metaKey;
    const range = e.shiftKey;

    if (range && selectionAnchor !== null) {
      const lo = Math.min(selectionAnchor, idx);
      const hi = Math.max(selectionAnchor, idx);
      const next = new Set<number>();
      for (let i = lo; i <= hi; i++) next.add(i);
      selected = next;
      // Anchor stays put - a second shift-click extends/shrinks from the SAME anchor,
      // matching Explorer/Finder rather than accumulating from the last shift target.
      return;
    }

    if (toggle) {
      const next = new Set(selected);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      selected = next;
      selectionAnchor = idx;
      return; // ctrl/cmd-click builds a multi-select without changing the viewed page
    }

    // Plain click: select just this page and navigate there (PR-B's original behaviour,
    // now also establishing the selection anchor for a future shift-click).
    selected = new Set([idx]);
    selectionAnchor = idx;
    onjump?.(idx);
  }

  function clearSelection() {
    selected = new Set();
  }

  function handleExtractClick() {
    onextract?.(Array.from(selected));
  }

  // ---------------------------------------------------------------------------
  // Thumbnail rendering (lazy, cached per page index)
  // ---------------------------------------------------------------------------

  /** Target CSS width of a thumbnail's rendered raster - the panel's own width (see
   *  --panel-left-width) sets the visual size; this only needs to be "small enough to
   *  render cheaply, sharp enough to read at that size". */
  const THUMBNAIL_WIDTH_CSS = 140;
  /** Thumbnails never need more than one small tile - caps pageTileSizeCss well below
   *  ReadingView's full-resolution cap. */
  const THUMBNAIL_MAX_TILE_CSS = 600;

  interface ThumbRaster {
    src: string;
    widthCss: number;
    heightCss: number;
  }

  let thumbs = $state<Record<number, ThumbRaster>>({});
  const pendingThumbs = new Set<number>();
  /** Indices whose thumbnail element is currently observed as visible (or, in a jsdom/
   *  no-IntersectionObserver test environment, every mounted index — see lazyThumbnail's
   *  fail-open branch). Used by invalidateAllThumbnails() to re-fetch what's currently on
   *  screen: a keyed {#each} block reuses the SAME DOM node across a page-count-unchanged
   *  re-render (rotate), so the lazy-load action's one-shot observer setup never re-fires
   *  on its own — clearing `thumbs` alone would otherwise leave already-visible
   *  thumbnails stuck on their stale (pre-invalidation) raster until the user scrolls
   *  them out of and back into view. */
  const visibleIndices = new Set<number>();

  async function ensureThumbnail(idx: number) {
    if (idx < 0 || idx >= pageCount) return;
    if (thumbs[idx] || pendingThumbs.has(idx)) return;
    pendingThumbs.add(idx);
    try {
      const size = await getPageSize(docId, idx);
      const zoom = fitWidthZoom(size.width_pts, THUMBNAIL_WIDTH_CSS);
      const tileSizeCss = pageTileSizeCss(size.width_pts, size.height_pts, zoom, THUMBNAIL_MAX_TILE_CSS);
      const result = await renderTile({
        doc_id: docId,
        page_index: idx,
        tile_size_css: tileSizeCss,
        tile_x: 0,
        tile_y: 0,
        zoom,
        dpr: 1, // thumbnails are display-only and tiny - no need for device-pixel sharpening
      });
      thumbs = {
        ...thumbs,
        [idx]: {
          src: `data:image/png;base64,${result.png_base64}`,
          widthCss: size.width_pts * zoom,
          heightCss: size.height_pts * zoom,
        },
      };
    } catch (e) {
      console.error(`Thumbnail render failed for page ${idx}:`, e);
    } finally {
      pendingThumbs.delete(idx);
    }
  }

  /** Wholesale cache reset - any op that changes page structure invalidates every index
   *  (a delete/reorder shifts which page an index refers to; a rotate changes aspect
   *  ratio) - simpler and safer than trying to patch individual entries. */
  function invalidateAllThumbnails() {
    thumbs = {};
    pendingThumbs.clear();
    for (const idx of visibleIndices) void ensureThumbnail(idx);
  }

  // Initialized to the CURRENT docId (not null) so this effect is a no-op on first
  // mount - it only invalidates on a later, genuine docId change (switching tabs to a
  // different document), never doubling up with the lazy-load action's own initial
  // fetch of every visible thumbnail.
  let lastDocId: string | null = docId;
  $effect(() => {
    if (docId !== lastDocId) {
      lastDocId = docId;
      invalidateAllThumbnails();
    }
  });

  /**
   * Lazy-load action: observes the thumbnail element and fetches its raster once it
   * scrolls near the viewport. Falls back to fetching immediately when
   * IntersectionObserver isn't available (fail open to correctness, not silently never
   * loading — also what makes this trivially testable in jsdom, which has no
   * IntersectionObserver implementation at all).
   */
  function lazyThumbnail(node: HTMLElement, idx: number) {
    if (typeof IntersectionObserver === "undefined") {
      visibleIndices.add(idx);
      void ensureThumbnail(idx);
      return {};
    }
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            visibleIndices.add(idx);
            void ensureThumbnail(idx);
          } else {
            visibleIndices.delete(idx);
          }
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(node);
    return {
      destroy() {
        observer.disconnect();
        visibleIndices.delete(idx);
      },
    };
  }

  onDestroy(() => {
    pendingThumbs.clear();
  });

  // ---------------------------------------------------------------------------
  // Drag-to-reorder state
  // ---------------------------------------------------------------------------

  let dragSrcIdx = $state<number | null>(null);
  let dragOverIdx = $state<number | null>(null);

  function handleDragStart(e: DragEvent, idx: number) {
    dragSrcIdx = idx;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", String(idx));
    }
  }

  function handleDragOver(e: DragEvent, idx: number) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = "move";
    }
    dragOverIdx = idx;
  }

  function handleDragLeave() {
    dragOverIdx = null;
  }

  function handleDragEnd() {
    dragSrcIdx = null;
    dragOverIdx = null;
  }

  async function handleDrop(e: DragEvent, targetIdx: number) {
    e.preventDefault();
    const src = dragSrcIdx;
    dragSrcIdx = null;
    dragOverIdx = null;
    if (src === null || src === targetIdx) return;

    // Build new_order permutation: move src page to targetIdx.
    const order = Array.from({ length: pageCount }, (_, i) => i);
    order.splice(src, 1);
    order.splice(targetIdx, 0, src);

    await reorderPages({ doc_id: docId, new_order: order });
    invalidateAllThumbnails();
    // Indices shifted under any selection - clear it rather than let a stale index
    // silently point at the wrong page for a later "Extract…".
    clearSelection();
    onPageOp?.();
  }

  // ---------------------------------------------------------------------------
  // Rotation
  // ---------------------------------------------------------------------------

  async function handleRotate(idx: number, degrees: number) {
    await rotatePage({ doc_id: docId, page_idx: idx, degrees });
    invalidateAllThumbnails();
    // Rotation doesn't shift any index - selection stays valid and is left alone.
    onPageOp?.();
  }

  // ---------------------------------------------------------------------------
  // Delete with confirmation
  // ---------------------------------------------------------------------------

  async function handleDelete(idx: number) {
    if (pageCount <= 1) return; // guard: cannot delete the only page
    const confirmed = window.confirm(`Delete page ${idx + 1}? This cannot be undone.`);
    if (!confirmed) return;
    await deletePage({ doc_id: docId, page_idx: idx });
    invalidateAllThumbnails();
    clearSelection(); // every index at/after the deleted page has shifted
    onPageOp?.();
  }
</script>

{#if selected.size > 0}
  <div class="extract-action-bar">
    <span class="extract-count">{selected.size} page{selected.size === 1 ? "" : "s"} selected</span>
    <button class="btn-extract" onclick={handleExtractClick}>Extract…</button>
    <button class="btn-clear-selection" onclick={clearSelection} title="Clear selection">✕</button>
  </div>
{/if}
<aside class="thumbnail-panel" aria-label="Page thumbnails">
  {#each Array.from({ length: pageCount }, (_, i) => i) as idx (idx)}
    <div
      class="thumbnail"
      class:drag-over={dragOverIdx === idx}
      class:drag-src={dragSrcIdx === idx}
      class:current-page={currentPage === idx}
      class:selected={selected.has(idx)}
      draggable="true"
      aria-label={`Page ${idx + 1}`}
      aria-current={currentPage === idx ? "page" : undefined}
      aria-selected={selected.has(idx)}
      role="listitem"
      onclick={(e) => handleThumbnailClick(e, idx)}
      ondragstart={(e) => handleDragStart(e, idx)}
      ondragover={(e) => handleDragOver(e, idx)}
      ondragleave={handleDragLeave}
      ondragend={handleDragEnd}
      ondrop={(e) => handleDrop(e, idx)}
    >
      <div class="thumbnail-number">{idx + 1}</div>
      <div class="thumbnail-preview" aria-hidden="true" use:lazyThumbnail={idx}>
        {#if thumbs[idx]}
          <img src={thumbs[idx].src} alt="" class="thumbnail-image" />
        {:else}
          <span class="preview-placeholder">p{idx + 1}</span>
        {/if}
      </div>
      <div class="thumbnail-controls">
        <button
          class="ctrl-btn"
          title="Rotate 90° clockwise"
          aria-label={`Rotate page ${idx + 1} 90 degrees clockwise`}
          onclick={(e) => { e.stopPropagation(); handleRotate(idx, 90); }}
        >↻</button>
        <button
          class="ctrl-btn danger"
          title="Delete page"
          aria-label={`Delete page ${idx + 1}`}
          disabled={pageCount <= 1}
          onclick={(e) => { e.stopPropagation(); handleDelete(idx); }}
        >✕</button>
      </div>
    </div>
  {/each}
</aside>

<style>
  .thumbnail-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--color-bg-panel);
    border-right: 1px solid var(--color-border);
    overflow-y: auto;
    width: var(--panel-left-width);
    flex-shrink: 0;
  }

  .thumbnail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-2);
    background: var(--color-bg-panel-alt);
    border: 1px solid var(--color-border-subtle);
    border-radius: var(--radius-md);
    cursor: pointer;
    user-select: none;
    transition: border-color 120ms, background 120ms;
  }

  .thumbnail:hover {
    border-color: var(--color-border);
    background: var(--color-bg-hover);
  }

  .thumbnail.drag-over {
    border-color: var(--color-primary);
    background: var(--color-bg-active);
  }

  .thumbnail.drag-src {
    opacity: 0.5;
  }

  .thumbnail.current-page {
    border-color: var(--color-primary);
    border-width: 2px;
    background: var(--color-bg-active);
  }

  /* Selection (PR-C) is a distinct visual from current-page - a page can be selected
     without being the one currently shown, and vice versa. Selection uses a filled
     background wash + accent outline so it reads clearly even when combined with
     .current-page's border-width bump on the same element. */
  .thumbnail.selected {
    outline: 2px solid var(--color-primary);
    outline-offset: -1px;
    background: var(--color-bg-active);
  }

  .extract-action-bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--color-bg-active);
    border-bottom: 1px solid var(--color-border);
  }

  .extract-count {
    flex: 1;
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
  }

  .btn-extract {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    font-weight: 600;
    padding: 2px var(--space-2);
    cursor: pointer;
  }
  .btn-extract:hover {
    background: var(--color-primary-hover);
    border-color: var(--color-primary-hover);
  }

  .btn-clear-selection {
    background: transparent;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 2px var(--space-1);
    line-height: 1;
  }
  .btn-clear-selection:hover {
    background: var(--color-bg-hover);
    color: var(--color-text);
  }

  .thumbnail-number {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    align-self: flex-start;
  }

  .thumbnail-preview {
    width: 100%;
    aspect-ratio: 3 / 4;
    background: var(--color-bg);
    border: 1px solid var(--color-border-subtle);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }

  .thumbnail-image {
    display: block;
    max-width: 100%;
    max-height: 100%;
  }

  .preview-placeholder {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
  }

  .thumbnail-controls {
    display: flex;
    gap: var(--space-1);
    width: 100%;
    justify-content: flex-end;
  }

  .ctrl-btn {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 2px var(--space-1);
    line-height: 1;
    transition: background 120ms;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .ctrl-btn.danger:hover:not(:disabled) {
    background: var(--color-danger);
    border-color: var(--color-danger);
    color: var(--color-text-inverse);
  }

  .ctrl-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
</style>
