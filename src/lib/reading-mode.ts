/**
 * Continuous reading-mode layout + virtualization helpers (owner request 2026-09-15).
 *
 * Pure, unit-tested functions used by ReadingView.svelte to lay pages out in a single
 * vertically scrolling column, decide which pages are visible/should be rendered, and
 * resolve the page-indicator's editable jump input. Kept separate from viewport.ts
 * (single-page tile math) rather than merged into it — reading mode uses a simpler
 * one-raster-per-page render model (see ReadingView.svelte's header comment), and giving
 * it its own module keeps that scoping decision visible instead of tangled into the
 * existing tiled-zoom code path that single-page mode still relies on unchanged.
 */

/** Minimal page-size shape (mirrors ipc.ts's PageSize, without doc_id/page_index). */
export interface PageDims {
  width_pts: number;
  height_pts: number;
}

export interface PageLayout {
  /** Top offset of each page, in CSS px, within the scrollable column. */
  topsCss: number[];
  /** Rendered height of each page, in CSS px, at the given zoom. */
  heightsCss: number[];
  /** Rendered width of each page, in CSS px, at the given zoom. */
  widthsCss: number[];
  /** Total scrollable content height, in CSS px (last page's bottom edge). */
  totalHeightCss: number;
}

/** Gap between stacked pages in the column, CSS px. */
export const READING_PAGE_GAP_CSS = 12;

/**
 * Lay out every page top-to-bottom at a single shared zoom. Pages keep their own
 * aspect ratio (a mixed-size sheet set is not forced to a common width), so page
 * widths can differ — the column centers each page independently in the view, that
 * part is CSS in ReadingView.svelte, not this module.
 */
export function computePageLayout(
  pages: readonly PageDims[],
  zoom: number,
  gapCss: number = READING_PAGE_GAP_CSS,
): PageLayout {
  const topsCss: number[] = [];
  const heightsCss: number[] = [];
  const widthsCss: number[] = [];
  let cursor = 0;
  for (const page of pages) {
    const h = Math.max(0, page.height_pts) * zoom;
    const w = Math.max(0, page.width_pts) * zoom;
    topsCss.push(cursor);
    heightsCss.push(h);
    widthsCss.push(w);
    cursor += h + gapCss;
  }
  // No trailing gap after the last page.
  const totalHeightCss = pages.length > 0 ? cursor - gapCss : 0;
  return { topsCss, heightsCss, widthsCss, totalHeightCss: Math.max(0, totalHeightCss) };
}

/**
 * The "current page" for the page indicator: the topmost page whose visible span
 * contains (or is below) the current scroll position. Matches the common continuous-
 * scroll-reader convention (Chrome's built-in PDF viewer, Bluebeam's continuous mode) —
 * the page indicator tracks whichever page you'd land on if you stopped scrolling now,
 * not whichever page happens to fill the most pixels.
 *
 * Returns 0 for an empty layout (nothing to point at) rather than -1, so callers can
 * always index into pages[] / a 1-based display without a separate empty-state branch.
 */
export function pageAtScrollTop(layout: PageLayout, scrollTopCss: number): number {
  const { topsCss } = layout;
  if (topsCss.length === 0) return 0;
  const clamped = Math.max(0, scrollTopCss);
  // topsCss is sorted ascending by construction (computePageLayout) — the current page
  // is the last one whose top has already been scrolled past.
  let page = 0;
  for (let i = 0; i < topsCss.length; i++) {
    if (topsCss[i] <= clamped) page = i;
    else break;
  }
  return page;
}

/** Scroll-top (CSS px) that puts `pageIndex`'s top flush with the viewport top. */
export function scrollTopForPage(layout: PageLayout, pageIndex: number): number {
  if (layout.topsCss.length === 0) return 0;
  const clampedIdx = Math.max(0, Math.min(pageIndex, layout.topsCss.length - 1));
  return layout.topsCss[clampedIdx];
}

export interface RenderWindow {
  /** First page index that should have tiles requested/kept, inclusive. */
  first: number;
  /** Last page index that should have tiles requested/kept, inclusive. */
  last: number;
}

/**
 * Which pages should actually be rendered right now: the strictly-visible range plus a
 * small lookahead on each side, clamped to the document. This is the virtualization
 * boundary the render-cache eviction (tile-cache.ts's evictExcept) and the tile-fetch
 * loop both key off — pages outside this window are neither fetched nor kept cached, so
 * a long scroll through a large sheet set never holds more than a handful of pages'
 * rasters in memory at once (owner constraint: never render all pages at once).
 */
export function visiblePageRange(
  layout: PageLayout,
  scrollTopCss: number,
  viewportHeightCss: number,
  lookaheadPages: number = 1,
): RenderWindow {
  const { topsCss, heightsCss } = layout;
  const pageCount = topsCss.length;
  if (pageCount === 0) return { first: 0, last: -1 }; // empty range — caller iterates 0 times

  const viewTop = Math.max(0, scrollTopCss);
  const viewBottom = viewTop + Math.max(0, viewportHeightCss);

  let firstVisible = 0;
  let lastVisible = pageCount - 1;
  let foundFirst = false;
  for (let i = 0; i < pageCount; i++) {
    const top = topsCss[i];
    const bottom = top + heightsCss[i];
    const intersects = bottom >= viewTop && top <= viewBottom;
    if (intersects) {
      if (!foundFirst) {
        firstVisible = i;
        foundFirst = true;
      }
      lastVisible = i;
    } else if (foundFirst && top > viewBottom) {
      // topsCss is sorted ascending — once we've passed the viewport with no more
      // intersections, every later page is further away still.
      break;
    }
  }
  if (!foundFirst) {
    // Scrolled past the end (or an empty viewport height) — clamp to the nearest page
    // rather than returning an empty window, so something is always rendered.
    firstVisible = lastVisible = pageAtScrollTop(layout, scrollTopCss);
  }

  return {
    first: Math.max(0, firstVisible - lookaheadPages),
    last: Math.min(pageCount - 1, lastVisible + lookaheadPages),
  };
}

/**
 * Parse the page-indicator's editable jump field. Accepts a 1-based page number
 * ("5"), tolerates surrounding whitespace, and rejects non-numeric/out-of-range input
 * by returning null so the caller can reject the edit instead of silently clamping to
 * a wrong page (mirrors viewport.ts's parseZoomPercent — same "reject, don't clamp"
 * contract for a mistyped value).
 *
 * Returns a 0-based page index on success.
 */
export function parsePageJumpInput(raw: string, pageCount: number): number | null {
  const trimmed = raw.trim();
  if (trimmed === "" || pageCount <= 0) return null;
  if (!/^\d+$/.test(trimmed)) return null;
  const oneBased = Number(trimmed);
  if (!Number.isFinite(oneBased) || oneBased < 1 || oneBased > pageCount) return null;
  return oneBased - 1;
}

/**
 * Tile size (CSS px, square) needed to raster an entire page in ONE tile at `zoom` —
 * reading mode's per-page render model (see ReadingView.svelte). Capped at
 * `maxTileCss` so an extreme zoom on a very tall/narrow custom page size can't request
 * an unbounded raster; past the cap the page is still fully visible, just not at full
 * fidelity — single-page mode (Viewport.svelte's real tile grid) is the path for
 * detailed inspection at high zoom, this is a deliberate v1 scoping decision.
 */
export function pageTileSizeCss(
  pageWidthPts: number,
  pageHeightPts: number,
  zoom: number,
  maxTileCss: number = 4096,
): number {
  const w = Math.max(0, pageWidthPts) * zoom;
  const h = Math.max(0, pageHeightPts) * zoom;
  return Math.min(maxTileCss, Math.ceil(Math.max(w, h, 1)));
}
