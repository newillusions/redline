/**
 * Viewport geometry helpers — coordinate transforms between screen space
 * and PDF user space (spec §5).
 *
 * All measurement / snapping math runs in PDF user space at f64.
 * This module provides the screen ↔ PDF user space mapping so the
 * frontend can:
 *  - Convert a cursor position to PDF user space before querying snap targets.
 *  - Determine which tiles are visible at the current scroll/zoom.
 *
 * PDF user space: origin bottom-left, y increases upward, units = points.
 * Screen space:   origin top-left,    y increases downward, units = CSS pixels.
 */

export interface ViewportState {
  /** Width of the visible canvas area in CSS pixels. */
  canvasWidthCss: number;
  /** Height of the visible canvas area in CSS pixels. */
  canvasHeightCss: number;
  /** Current zoom level (1.0 = 100%). */
  zoom: number;
  /** Device pixel ratio (window.devicePixelRatio). */
  dpr: number;
  /** Scroll offset in CSS pixels (top-left of visible area). */
  scrollX: number;
  scrollY: number;
  /** Page size in PDF points. */
  pageWidthPts: number;
  pageHeightPts: number;
}

/**
 * Convert a screen-space CSS pixel position to PDF user space.
 * Accounts for scroll offset, zoom, and the PDF y-axis flip.
 */
export function screenToPdfUserSpace(
  screenX: number,
  screenY: number,
  v: ViewportState
): { x: number; y: number } {
  const ptsPerPx = 1.0 / v.zoom; // CSS px → PDF points factor
  const pdfX = (screenX + v.scrollX) * ptsPerPx;
  // PDF y: flip from top-left origin to bottom-left origin
  const pdfY =
    v.pageHeightPts - (screenY + v.scrollY) * ptsPerPx;
  return { x: pdfX, y: pdfY };
}

/**
 * Convert a PDF user-space point to screen CSS pixels.
 */
export function pdfUserSpaceToScreen(
  pdfX: number,
  pdfY: number,
  v: ViewportState
): { x: number; y: number } {
  const pxPerPt = v.zoom;
  return {
    x: pdfX * pxPerPt - v.scrollX,
    y: (v.pageHeightPts - pdfY) * pxPerPt - v.scrollY,
  };
}

/** Tile size in CSS pixels (fixed for M1; made adaptive later). */
export const TILE_SIZE_CSS = 512;

/**
 * Wheel deltaY → multiplicative zoom factor. Proportional (exp) and symmetric, clamped per
 * event so a fast flick can't jump to the zoom limit in a few events. Shared by
 * Viewport.onWheel and the glued-on-zoom tests so neither hard-codes the curve.
 */
export function wheelZoomFactor(deltaY: number): number {
  return Math.min(2, Math.max(0.5, Math.exp(-deltaY * 0.0015)));
}

/** Zoom-snap presets — 1:1 (actual size / 100%). */
export const ACTUAL_SIZE_ZOOM = 1.0;

/**
 * Zoom level at which the page WIDTH exactly fills the viewport width.
 * Pure §5 math (PDF points vs css px) — never reads the raster. Falls back to actual
 * size when the page size is not yet known, so callers can't divide by zero.
 */
export function fitWidthZoom(pageWidthPts: number, canvasWidthCss: number): number {
  if (pageWidthPts <= 0) return ACTUAL_SIZE_ZOOM;
  return canvasWidthCss / pageWidthPts;
}

/** Zoom level at which the page HEIGHT exactly fills the viewport height (see fitWidthZoom). */
export function fitHeightZoom(pageHeightPts: number, canvasHeightCss: number): number {
  if (pageHeightPts <= 0) return ACTUAL_SIZE_ZOOM;
  return canvasHeightCss / pageHeightPts;
}

/**
 * Compute which tiles are visible in the current viewport.
 * Returns an array of (tile_x, tile_y) pairs that intersect the visible area.
 */
export function visibleTiles(v: ViewportState): Array<{ tx: number; ty: number }> {
  const pxPerPt = v.zoom;
  const fullW = v.pageWidthPts * pxPerPt;
  const fullH = v.pageHeightPts * pxPerPt;

  const cols = Math.ceil(fullW / TILE_SIZE_CSS);
  const rows = Math.ceil(fullH / TILE_SIZE_CSS);

  const firstCol = Math.max(0, Math.floor(v.scrollX / TILE_SIZE_CSS));
  const firstRow = Math.max(0, Math.floor(v.scrollY / TILE_SIZE_CSS));
  const lastCol = Math.min(cols - 1, Math.floor((v.scrollX + v.canvasWidthCss) / TILE_SIZE_CSS));
  const lastRow = Math.min(rows - 1, Math.floor((v.scrollY + v.canvasHeightCss) / TILE_SIZE_CSS));

  const tiles: Array<{ tx: number; ty: number }> = [];
  for (let row = firstRow; row <= lastRow; row++) {
    for (let col = firstCol; col <= lastCol; col++) {
      tiles.push({ tx: col, ty: row });
    }
  }
  return tiles;
}
