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

/**
 * Snapshot of user-navigable viewport state saved per tab (feat/tabbed-multi-file).
 * Restored as initialState when a tab is re-activated so zoom/page/scroll are preserved.
 */
export interface ViewportSnapshot {
  zoom: number;
  pageIndex: number;
  scrollX: number;
  scrollY: number;
}

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

/** Zoom-snap presets - 1:1 (actual size / 100%). */
export const ACTUAL_SIZE_ZOOM = 1.0;

// ---------------------------------------------------------------------------
// Wheel-event classification (owner-decided pan/zoom scheme, 2026-09-08)
// ---------------------------------------------------------------------------
// Trackpad two-finger swipe and pinch both arrive as native `wheel` events on every
// platform this app ships to (macOS WKWebView, Windows WebView2/Chromium) - the
// discriminator for a pinch specifically is `ctrlKey`/`metaKey`: the OS sets it on a
// pinch (and on an explicit Cmd/Ctrl+wheel from a mouse) but never on a plain two-finger
// pan swipe. macOS additionally dispatches WebKit-only `gesturestart`/`gesturechange`/
// `gestureend` events during a pinch (handled separately in Viewport.svelte, guarded so
// the two paths never both apply the same pinch) - Windows WebView2 never fires those, so
// the ctrlKey-wheel path here is the ONLY pinch-zoom route there. Shift held is a THIRD
// zoom trigger (owner amendment 2026-09-08) - Shift+wheel zooms live at the cursor,
// exactly like Ctrl/Cmd+wheel; it is not a horizontal-pan modifier in this app.

export interface WheelEventLike {
  deltaX: number;
  deltaY: number;
  /** 0 = pixel, 1 = line, 2 = page (WheelEvent.DOM_DELTA_*). */
  deltaMode: number;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

export type WheelAction =
  | { kind: "zoom"; factor: number }
  | { kind: "pan"; dx: number; dy: number };

/** Approximate CSS-px-per-unit for line/page-mode wheel deltas (rare outside old mouse
 *  drivers; most trackpads and modern mice report deltaMode 0/pixel). */
const WHEEL_LINE_PX = 16;
const WHEEL_PAGE_PX = 800;

/** Normalize a wheel delta to CSS pixels regardless of the event's deltaMode. */
export function normalizeWheelDelta(delta: number, deltaMode: number): number {
  if (deltaMode === 1) return delta * WHEEL_LINE_PX;
  if (deltaMode === 2) return delta * WHEEL_PAGE_PX;
  return delta;
}

/**
 * Classify a wheel event into a pan or a zoom action.
 *   - Ctrl/Cmd held (pinch, or an explicit Ctrl/Cmd+wheel from a mouse) -> zoom, using the
 *     existing `wheelZoomFactor` curve on the normalized deltaY.
 *   - Shift held (owner amendment 2026-09-08, supersedes the original Shift-pans-horizontally
 *     draft of this scheme) -> ALSO zoom, live at the cursor, through the same
 *     `wheelZoomFactor` curve - Shift+two-finger-swipe and Shift+mouse-wheel are both a
 *     zoom gesture, not a horizontal-pan one, in this app's final scheme. The zoom source
 *     prefers deltaY but FALLS BACK to deltaX when deltaY is zero (review finding 2026-09-08,
 *     PR #111 round 2, blocking): Chromium/WebView2 - i.e. every Windows build of this app -
 *     remaps Shift+vertical-wheel into deltaX and zeroes deltaY at the browser level (the
 *     same axis-swap convention that makes Shift+wheel scroll horizontally on an ordinary
 *     web page), so a Windows user's plain mouse wheel would otherwise classify as
 *     wheelZoomFactor(0) === 1 - a silent, deterministic no-op.
 *   - Otherwise -> pan. A plain mouse wheel reports only deltaY, so it pans vertically; a
 *     two-finger trackpad swipe reports both axes, so it pans diagonally. Horizontal pan
 *     comes only from deltaX on an UNMODIFIED wheel event - a trackpad's own two-finger
 *     horizontal component, or a mouse's tilt-wheel deltaX when the hardware reports one.
 */
export function classifyWheelEvent(e: WheelEventLike): WheelAction {
  if (e.ctrlKey || e.metaKey) {
    return { kind: "zoom", factor: wheelZoomFactor(normalizeWheelDelta(e.deltaY, e.deltaMode)) };
  }
  const dx = normalizeWheelDelta(e.deltaX, e.deltaMode);
  const dy = normalizeWheelDelta(e.deltaY, e.deltaMode);
  if (e.shiftKey) {
    return { kind: "zoom", factor: wheelZoomFactor(dy !== 0 ? dy : dx) };
  }
  return { kind: "pan", dx, dy };
}

/**
 * Parse a user-typed zoom-percent string (toolbar input, e.g. "150" or "150%") into a
 * clamped zoom multiplier. Returns null for empty, non-numeric, or non-positive input so
 * the caller can reject the edit instead of silently jumping to a clamped extreme.
 */
export function parseZoomPercent(input: string, min = ZOOM_MIN, max = ZOOM_MAX): number | null {
  const trimmed = input.trim().replace(/%$/, "");
  if (trimmed === "") return null;
  const pct = Number(trimmed);
  if (!Number.isFinite(pct) || pct <= 0) return null;
  return Math.max(min, Math.min(max, pct / 100));
}

/**
 * Discrete geometric zoom ladder used to bound tile-cache churn during a smooth zoom gesture
 * (Windows-freeze fix, 2026-07). Snapping the RASTER zoom to this ladder means many nearby
 * continuous zoom values (a run of wheel ticks a few percent apart) collapse onto the SAME
 * tile-cache key, instead of each minting a fresh full-resolution tile set. The live
 * (continuous) `zoom` is left untouched for the SVG overlay / markup math (section 5 precision
 * invariant) and for the CSS-transform placeholder, which already absorbs the residual gap
 * between a quantized raster and the true zoom (see Viewport.svelte applyZoomPlaceholder).
 */
export const ZOOM_QUANT_STEP = 1.12; // ~12% per rung

/** Zoom bounds shared with Viewport.svelte's ZOOM_MIN/ZOOM_MAX (kept in sync; see quantizeZoom). */
export const ZOOM_MIN = 0.1;
export const ZOOM_MAX = 8.0;

/**
 * Snap a continuous zoom value onto the nearest rung of the geometric ladder
 * (ZOOM_QUANT_STEP ^ n), clamped to [min, max]. Pure function of `zoom` alone - independent
 * of gesture history - so a smooth zoom-in/out always lands on the same rungs.
 */
export function quantizeZoom(zoom: number, min = ZOOM_MIN, max = ZOOM_MAX): number {
  const clamped = Math.max(min, Math.min(max, zoom));
  const rung = Math.round(Math.log(clamped) / Math.log(ZOOM_QUANT_STEP));
  return Math.pow(ZOOM_QUANT_STEP, rung);
}

/**
 * Cap on the dpr used for tile RASTERIZATION only (spec section 20 Windows-freeze fix). At
 * 250%+ Windows display scaling, devicePixelRatio can be 2.5+; rasterizing every tile at that
 * resolution inflates decoded tile bytes by (dpr/2)^2 for no visible benefit - 2x oversampling
 * is already visually sharp. CSS layout keeps using the real (unclamped) devicePixelRatio.
 */
export const MAX_TILE_DPR = 2.0;

/** Clamp a devicePixelRatio value for use in tile rasterization / pixel-budget math only. */
export function clampTileDpr(dpr: number, max = MAX_TILE_DPR): number {
  return Math.min(dpr, max);
}

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

// ---------------------------------------------------------------------------
// Space-drag / click-drag pan: forgiving axis lock (owner feedback 2026-09-15)
// ---------------------------------------------------------------------------
// The mouse-drag pan locks to a single axis (straight H or V panning, the classic
// "hand tool" feel) rather than always following the pointer diagonally - but the
// direction detection must be forgiving, not a hair-trigger dominant-axis snap:
//   - Decide from the first ~9px of movement (PAN_LOCK_DETECT_PX), not the very first
//     event - a couple of noisy sub-pixel deltas at drag-start must not decide the lock.
//   - Lock to an axis only when the drag vector falls within a ~30-35deg cone of it
//     (PAN_LOCK_CONE_DEG); a genuinely diagonal drag (e.g. ~45deg) pans freely in 2D
//     instead of being forced onto whichever axis is nominally "closer".
//   - The lock can be re-evaluated mid-drag: after a pause (no move event for
//     PAN_LOCK_PAUSE_MS) or a sharp turn against the current lock, a fresh detection
//     window starts from the current point - so a user who pans right then stops and
//     pans down isn't stuck panning right.
// Kept as pure functions (no DOM/component state) so the decision logic is unit
// tested directly; Viewport.svelte owns the segment state (origin/scroll0/lock) and
// calls these on every mousemove.

/** Axis-lock state for one pan "segment" (from drag-start, or from the last re-lock). */
export type PanAxisLock = "undecided" | "horizontal" | "vertical" | "free";

/** Movement magnitude (px, from the segment's origin) needed before a lock decision is
 *  made. Below this, movement is applied freely in 2D (no lock has committed yet). */
export const PAN_LOCK_DETECT_PX = 9;

/** Half-angle (degrees) of the cone around each axis that locks to it; a vector outside
 *  both cones (i.e. within [PAN_LOCK_CONE_DEG, 90 - PAN_LOCK_CONE_DEG] of horizontal)
 *  pans freely instead of snapping to the nearer axis. */
export const PAN_LOCK_CONE_DEG = 32;

/** Gap (ms, wall-clock) between move events past which a fresh detection segment starts
 *  at the current point, discarding the old lock decision. */
export const PAN_LOCK_PAUSE_MS = 150;

/**
 * Decide (or keep) a segment's axis lock from its cumulative movement since origin.
 * A no-op once a decision has been made ("horizontal"/"vertical"/"free") for this
 * segment - re-locking only happens by starting a NEW segment (see isSharpTurnAgainstLock
 * and the pause check below), never by silently overriding an existing decision.
 */
export function resolvePanLock(dx: number, dy: number, currentLock: PanAxisLock): PanAxisLock {
  if (currentLock !== "undecided") return currentLock;
  if (Math.hypot(dx, dy) < PAN_LOCK_DETECT_PX) return "undecided";
  const angleFromHorizontalDeg = Math.atan2(Math.abs(dy), Math.abs(dx)) * (180 / Math.PI);
  if (angleFromHorizontalDeg <= PAN_LOCK_CONE_DEG) return "horizontal";
  if (angleFromHorizontalDeg >= 90 - PAN_LOCK_CONE_DEG) return "vertical";
  return "free";
}

/**
 * Whether a single frame's movement (delta since the LAST processed move event, not
 * the segment origin) is a decisive turn against the currently locked axis - e.g. the
 * user was panning horizontally and just flicked sharply downward. Ignored while
 * "free" or "undecided" (nothing to turn against). Uses the same cone as the initial
 * lock decision, so a turn "sharp" enough to have locked the OPPOSITE axis if it were
 * the first movement is what triggers a re-evaluation - not any perpendicular drift.
 */
export function isSharpTurnAgainstLock(frameDx: number, frameDy: number, lock: PanAxisLock): boolean {
  if (lock !== "horizontal" && lock !== "vertical") return false;
  if (Math.hypot(frameDx, frameDy) < PAN_LOCK_DETECT_PX) return false;
  const angleFromHorizontalDeg = Math.atan2(Math.abs(frameDy), Math.abs(frameDx)) * (180 / Math.PI);
  return lock === "horizontal"
    ? angleFromHorizontalDeg >= 90 - PAN_LOCK_CONE_DEG
    : angleFromHorizontalDeg <= PAN_LOCK_CONE_DEG;
}

/** Which scroll axes a lock state allows to move. "undecided"/"free" both move both -
 *  the difference between them is only about whether a FUTURE lock decision is still
 *  pending, not about what's currently allowed to move. */
export function panLockAxes(lock: PanAxisLock): { applyX: boolean; applyY: boolean } {
  if (lock === "horizontal") return { applyX: true, applyY: false };
  if (lock === "vertical") return { applyX: false, applyY: true };
  return { applyX: true, applyY: true };
}

// ---------------------------------------------------------------------------
// Zoom toolbar: collapse + reposition persistence (owner feedback 2026-09-15)
// ---------------------------------------------------------------------------

/** Which corner of the viewport the zoom toolbar is anchored to. */
export type ToolbarCorner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

const TOOLBAR_PREFS_STORAGE_KEY = "redline.viewport.zoomToolbar";
const VALID_CORNERS: readonly ToolbarCorner[] = ["top-left", "top-right", "bottom-left", "bottom-right"];
const DEFAULT_TOOLBAR_CORNER: ToolbarCorner = "bottom-right";

export interface ToolbarPrefs {
  collapsed: boolean;
  corner: ToolbarCorner;
}

/** Load persisted toolbar prefs (per-viewer convenience, not critical state) - mirrors
 *  $lib/search-store.svelte.ts's localStorage pattern. Never throws; a missing/corrupt/
 *  blocked store just yields the defaults. */
export function loadToolbarPrefs(): ToolbarPrefs {
  const defaults: ToolbarPrefs = { collapsed: false, corner: DEFAULT_TOOLBAR_CORNER };
  try {
    if (typeof localStorage === "undefined") return defaults;
    const raw = localStorage.getItem(TOOLBAR_PREFS_STORAGE_KEY);
    if (!raw) return defaults;
    const parsed = JSON.parse(raw) as Partial<ToolbarPrefs>;
    return {
      collapsed: typeof parsed.collapsed === "boolean" ? parsed.collapsed : defaults.collapsed,
      corner: (VALID_CORNERS as readonly string[]).includes(parsed.corner ?? "")
        ? (parsed.corner as ToolbarCorner)
        : defaults.corner,
    };
  } catch {
    return defaults;
  }
}

/** Persist toolbar prefs. Best-effort only - a blocked/private-mode store must not break
 *  the toolbar itself. */
export function persistToolbarPrefs(prefs: ToolbarPrefs): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(TOOLBAR_PREFS_STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // best-effort
  }
}

/**
 * Nearest corner of a width x height rect to a dropped point (clientX/Y relative to the
 * rect's own origin, i.e. already offset by the rect's top-left) - used to snap the zoom
 * toolbar to one of the 4 corners on drag-release. Pure geometry, no DOM.
 */
export function nearestCorner(x: number, y: number, width: number, height: number): ToolbarCorner {
  const isLeft = x < width / 2;
  const isTop = y < height / 2;
  return `${isTop ? "top" : "bottom"}-${isLeft ? "left" : "right"}` as ToolbarCorner;
}
