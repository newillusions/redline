// @vitest-environment jsdom
/**
 * Viewport G3 drag-draw interaction tests.
 *
 * Strategy:
 * - Mock $lib/ipc so onMount IPC calls resolve without Tauri.
 * - Mount the REAL Viewport.svelte with a REAL MarkupStore (fake ipc injected).
 * - Stub ResizeObserver to drive containerWidth/Height synchronously.
 * - Stub getBoundingClientRect on containerEl so screen→PDF math is deterministic.
 * - Drive pointer gestures via a lightweight PointerEvent shim (jsdom lacks it).
 *
 * Coordinate setup (zoom=1, scroll=0, page=200×200 pts, container=200×200px at 0,0):
 *   screenToPdfUserSpace(50, 50)  → PDF(50, 150)  [y-flip: 200 - 50 = 150]
 *   screenToPdfUserSpace(100, 100) → PDF(100, 100)
 *   These two points always produce a non-zero rect geometry.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor } from "@testing-library/svelte";
import { fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Viewport from "./Viewport.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { TakeoffStore } from "$lib/takeoff-store.svelte";
import { wheelZoomFactor } from "$lib/viewport";
import { buildMarkup } from "$lib/markup-tools";

// ---------------------------------------------------------------------------
// jsdom does not implement PointerEvent — install a minimal shim.
// ---------------------------------------------------------------------------
if (typeof PointerEvent === "undefined") {
  (globalThis as Record<string, unknown>).PointerEvent = class PointerEvent extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 1;
    }
  };
}

// ---------------------------------------------------------------------------
// Mock $lib/ipc — vi.mock is hoisted above imports by Vitest.
// Factory returns plain vi.fn() so tests can re-configure per-test in beforeEach.
// ---------------------------------------------------------------------------
vi.mock("$lib/ipc", () => ({
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
  processRssMb: vi.fn(),
  getUserIdentity: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
  addMarkup: vi.fn(),
  listMarkups: vi.fn(),
  loadMarkups: vi.fn(),
  saveDocument: vi.fn(),
  saveDocumentAs: vi.fn(),
  updateMarkup: vi.fn(),
  deleteMarkup: vi.fn(),
  // M3 takeoff IPC
  addScale: vi.fn(async () => ({
    id: "scale-1", applies_to: { kind: "DocumentDefault" }, method: "TwoPoint",
    ratio: 0.001, unit: "m", label: "1:1000", precision: 2,
  })),
  listScales: vi.fn(async () => []),
  deleteScale: vi.fn(async () => true),
  exportMarkupList: vi.fn(async () => {}),
}));

// Import the module AFTER vi.mock so we get the mocked version.
import * as ipcMocks from "$lib/ipc";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FAKE_DOC = { doc_id: "d1", path: "/fake.pdf", page_count: 1 };
const FAKE_IDENTITY = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
const FAKE_PAGE_SIZE = { doc_id: "d1", page_index: 0, width_pts: 200, height_pts: 200 };

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fakeIpc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  };
}

/**
 * Stub ResizeObserver — capture the callback and return a trigger function.
 * The trigger fires the resize with the given container dimensions.
 */
function stubResizeObserver(): (w: number, h: number) => void {
  let capturedCb: ResizeObserverCallback | null = null;
  let capturedTarget: Element | null = null;

  (globalThis as Record<string, unknown>).ResizeObserver = class {
    constructor(cb: ResizeObserverCallback) { capturedCb = cb; }
    observe(el: Element) { capturedTarget = el; }
    unobserve() {}
    disconnect() {}
  };

  return (w: number, h: number) => {
    if (capturedCb && capturedTarget) {
      const entry: ResizeObserverEntry = {
        contentRect: { width: w, height: h, top: 0, left: 0, bottom: h, right: w, x: 0, y: 0, toJSON() { return {}; } } as DOMRectReadOnly,
        target: capturedTarget,
        borderBoxSize: [],
        contentBoxSize: [],
        devicePixelContentBoxSize: [],
      };
      capturedCb([entry], {} as ResizeObserver);
    }
  };
}

/** Fire a PointerEvent on an element. */
function ptr(target: Element, type: string, x: number, y: number) {
  fireEvent(target, new PointerEvent(type, {
    bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 1,
  }));
}

/** Mount Viewport and wait for onMount IPC calls to complete. */
async function mountViewport(store: MarkupStore, takeoffStore?: TakeoffStore) {
  const triggerResize = stubResizeObserver();

  // setPointerCapture is called by the overlay on pointerdown capture.
  Element.prototype.setPointerCapture = vi.fn();

  const props = takeoffStore ? { docInfo: FAKE_DOC, store, takeoffStore } : { docInfo: FAKE_DOC, store };
  const { container } = render(Viewport, { props });

  // Wait for getUserIdentity (the last onMount async call) to have been invoked.
  await waitFor(() => {
    expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled();
  });

  // Drive the ResizeObserver so containerWidth/Height become 200×200.
  const containerEl = container.querySelector(".viewport-root") as HTMLElement;
  if (containerEl) {
    vi.spyOn(containerEl, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, right: 200, bottom: 200,
      width: 200, height: 200, x: 0, y: 0,
      toJSON() { return {}; },
    } as DOMRect);
    triggerResize(200, 200);
  }

  const overlay = container.querySelector("svg.markup-overlay") as SVGElement;
  return { container, containerEl, overlay };
}

// ---------------------------------------------------------------------------
// G3 drag-draw tests
// ---------------------------------------------------------------------------

describe("Viewport G3 drag-draw", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  beforeEach(() => {
    // Re-install mock implementations before each test.
    // (vi.clearAllMocks in setup.ts clears call history but keeps implementations;
    //  we set them here explicitly so they don't depend on factory defaults.)
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // T1: Draw creates a markup
  // -------------------------------------------------------------------------
  it("T1: drag-draw creates one Rectangle markup and calls ipc.add", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Rectangle";

    // Gesture: down(50,50) → move(100,100) → up(100,100).
    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointerup", 100, 100);

    expect(store.markups.length).toBe(1);

    const m = store.markups[0];
    expect(m.markup_type).toBe("Rectangle");
    expect(m.page).toBe(0);

    // Geometry is a normalized Rect at the EXACT PDF coordinates.
    // With zoom 1, scroll 0, page 200×200, container 200×200 at origin:
    //   down(50,50)  → screenToPdfUserSpace → PDF(50, 150)  [y-flip: 200-50]
    //   up(100,100)  → PDF(100, 100)
    //   normalized:    min(50, 100)  max(100, 150)
    // Exact assertions (not just min<max) catch y-flip / rect-offset / zoom
    // regressions in the screen→PDF math — the spec §5 precision invariant.
    const rect = (m.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    expect(rect).toBeDefined();
    expect(rect.min.x).toBeCloseTo(50);
    expect(rect.min.y).toBeCloseTo(100);
    expect(rect.max.x).toBeCloseTo(100);
    expect(rect.max.y).toBeCloseTo(150);

    // Mirror op drained to ipc.add.
    await waitFor(() => expect(ipc.add).toHaveBeenCalledTimes(1));
  });

  // -------------------------------------------------------------------------
  // T6: §5 precision invariant — markup stays glued to PDF space across zoom
  // -------------------------------------------------------------------------
  it("T6: drawn markup stays glued to PDF space across zoom (no drift)", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Rectangle";

    // Draw a 50pt-wide rect at zoom 1.
    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointerup", 100, 100);

    const rectEl = container.querySelector("svg.markup-overlay rect") as SVGRectElement;
    const w0 = parseFloat(rectEl.getAttribute("width")!);
    expect(w0).toBeCloseTo(50); // 50pt × zoom 1 = 50 screen px

    // Zoom in one wheel step. Viewport.onWheel: deltaY<0 → zoom += 0.1 → 1.1.
    fireEvent.wheel(container.querySelector(".viewport-root")!, { deltaY: -100 });
    await tick(); // let the $derived overlay re-derive from the new viewState

    const rectEl2 = container.querySelector("svg.markup-overlay rect") as SVGRectElement;
    const w1 = parseFloat(rectEl2.getAttribute("width")!);
    // Width must scale with zoom (50pt × 1.1) — proving the overlay re-derives
    // from PDF user space / viewState, not pinned to fixed screen pixels.
    expect(w1).toBeCloseTo(50 * wheelZoomFactor(-100), 1);
  });

  // -------------------------------------------------------------------------
  // T2: Preview not committed early
  // -------------------------------------------------------------------------
  it("T2: after pointerdown+move (before up) store is empty but preview SVG is present", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Rectangle";

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);

    // No committed markup yet.
    expect(store.markups.length).toBe(0);

    // A preview <rect> inside the SVG overlay must be visible.
    const svgRect = container.querySelector("svg.markup-overlay rect");
    expect(svgRect).not.toBeNull();
  });

  // -------------------------------------------------------------------------
  // T3: Hand tool doesn't draw
  // -------------------------------------------------------------------------
  it("T3: hand tool gesture does not create any markup", async () => {
    const { overlay } = await mountViewport(store);
    // Default activeTool is "hand".
    expect(store.activeTool).toBe("hand");

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointerup", 100, 100);

    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  // -------------------------------------------------------------------------
  // T4: Zero-size is a no-op
  // -------------------------------------------------------------------------
  it("T4: pointerdown+up at the same point does not create a markup", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Rectangle";

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);

    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  // -------------------------------------------------------------------------
  // T5: Undo removes the markup
  // -------------------------------------------------------------------------
  it("T5: undo after a draw removes the markup from the store", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Rectangle";

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointerup", 100, 100);

    expect(store.markups.length).toBe(1);

    store.undo();

    expect(store.markups.length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// G4 multi-click + ink + cloud interaction tests
// ---------------------------------------------------------------------------

describe("Viewport G4 multi-click tools", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // T7: Polygon — 3 clicks + Enter commits
  // -------------------------------------------------------------------------
  it("T7: polygon — 3 clicks + Enter creates one Polygon markup with exact PDF coords", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Polygon";
    await tick();

    // Click 3 distinct points. Coordinate transform (zoom=1, scroll=0, page=200x200):
    //   click(50, 50)   → PDF(50, 150)  [y-flip: 200-50]
    //   click(100, 50)  → PDF(100, 150)
    //   click(100, 100) → PDF(100, 100)
    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 100 });

    // Finish with Enter (avoids the click-on-dblclick double-count problem).
    fireEvent.keyDown(window, { key: "Enter" });
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Polygon");
    expect(m.page).toBe(0);

    // Exact PDF-space geometry (spec §5 precision invariant).
    const poly = (m.geometry as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(poly).toHaveLength(3);
    expect(poly[0].x).toBeCloseTo(50);
    expect(poly[0].y).toBeCloseTo(150);
    expect(poly[1].x).toBeCloseTo(100);
    expect(poly[1].y).toBeCloseTo(150);
    expect(poly[2].x).toBeCloseTo(100);
    expect(poly[2].y).toBeCloseTo(100);

    await waitFor(() => expect(ipc.add).toHaveBeenCalledTimes(1));
  });

  // -------------------------------------------------------------------------
  // T8: Polyline — 2 clicks + Enter commits
  // -------------------------------------------------------------------------
  it("T8: polyline — 2 clicks + Enter creates a Polyline markup", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Polyline";
    await tick();

    // click(50, 50) → PDF(50, 150); click(150, 100) → PDF(150, 100)
    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 150, clientY: 100 });
    fireEvent.keyDown(window, { key: "Enter" });
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Polyline");

    const poly = (m.geometry as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(poly).toHaveLength(2);
    expect(poly[0].x).toBeCloseTo(50);
    expect(poly[0].y).toBeCloseTo(150);
    expect(poly[1].x).toBeCloseTo(150);
    expect(poly[1].y).toBeCloseTo(100);
  });

  // -------------------------------------------------------------------------
  // T9: Esc cancels in-progress multi-click (no markup committed)
  // -------------------------------------------------------------------------
  it("T9: Esc cancels an in-progress Polygon — no markup created", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Polygon";
    await tick();

    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 50 });

    // Cancel before reaching minimum vertices (3 for Polygon) — but Esc should
    // cancel regardless.
    fireEvent.keyDown(window, { key: "Escape" });
    await tick();

    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  // -------------------------------------------------------------------------
  // T10: Cloud renders a <path> element in the SVG overlay
  // -------------------------------------------------------------------------
  it("T10: cloud — 3 clicks + Enter renders a <path> in the markup overlay", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Cloud";
    await tick();

    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 150, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 100 });
    fireEvent.keyDown(window, { key: "Enter" });
    await tick();

    expect(store.markups.length).toBe(1);
    expect(store.markups[0].markup_type).toBe("Cloud");

    // The cloud shape should render as a <path> element (scalloped arc path).
    const pathEl = container.querySelector("svg.markup-overlay path");
    expect(pathEl).not.toBeNull();
  });

  // -------------------------------------------------------------------------
  // T11: Ink freehand — pointerdown + moves + pointerup creates an Ink markup
  // -------------------------------------------------------------------------
  it("T11: ink — pointerdown + pointermoves + pointerup creates an Ink markup with ≥2 sampled points", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Ink";
    await tick();

    // Drive a freehand stroke via pointer events.
    ptr(overlay, "pointerdown", 20, 50);
    // Move several pixels apart (>1px each) to pass throttle.
    ptr(overlay, "pointermove", 40, 55);
    ptr(overlay, "pointermove", 60, 60);
    ptr(overlay, "pointermove", 80, 65);
    ptr(overlay, "pointerup", 100, 70);
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Ink");

    // Ink geometry: strokes array with at least one stroke of ≥2 points.
    const ink = (m.geometry as { Ink: { x: number; y: number }[][] }).Ink;
    expect(ink).toHaveLength(1);
    expect(ink[0].length).toBeGreaterThanOrEqual(2);

    // The committed stroke renders as ≥1 <polyline> in the overlay.
    const polylines = container.querySelectorAll("svg.markup-overlay polyline");
    expect(polylines.length).toBeGreaterThanOrEqual(1);
  });

  // -------------------------------------------------------------------------
  // T12: Glued-on-zoom — polyline coords scale with zoom (spec §5 no-drift)
  // -------------------------------------------------------------------------
  it("T12: polyline stays glued to PDF space across zoom (no drift)", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Polyline";
    await tick();

    // Draw a polyline from (50,50) to (150,100) in screen space at zoom 1.
    // PDF: (50,150) → (150,100). Screen at zoom 1: (50,50) → (150,100).
    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 150, clientY: 100 });
    fireEvent.keyDown(window, { key: "Enter" });
    await tick();

    expect(store.markups.length).toBe(1);

    // Read the rendered SVG points at zoom=1.
    const polylineEl = container.querySelector("svg.markup-overlay polyline") as SVGPolylineElement;
    expect(polylineEl).not.toBeNull();
    const points0 = polylineEl.getAttribute("points")!;
    // Expected at zoom 1: "50,50 150,100"
    expect(points0).toContain("50");

    // Zoom in one wheel step: deltaY < 0 → zoom += 0.1 → zoom = 1.1.
    fireEvent.wheel(container.querySelector(".viewport-root")!, { deltaY: -100 });
    await tick();

    // After zoom, the screen coordinates must scale with zoom.
    // PDF(50,150) at zoom 1.1: screen_x = 50*1.1 = 55, screen_y = (200-150)*1.1 = 55
    // PDF(150,100) at zoom 1.1: screen_x = 150*1.1 = 165, screen_y = (200-100)*1.1 = 110
    const points1 = polylineEl.getAttribute("points")!;
    // Parse the first point's x coordinate and verify it scaled by 1.1.
    const firstX = parseFloat(points1.split(",")[0]);
    expect(firstX).toBeCloseTo(50 * wheelZoomFactor(-100), 0); // scales with the actual zoom factor
    // The values changed (they weren't pinned to screen pixels).
    expect(points1).not.toBe(points0);
  });

  // -------------------------------------------------------------------------
  // T13: dblclick finishes with the correct vertex count (I1 dedup)
  // -------------------------------------------------------------------------
  it("T13: polygon finished via dblclick yields exactly 3 vertices (no dup from the dblclick's clicks)", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Polygon";
    await tick();

    // Two distinct vertices.
    //   click(50, 50)   → PDF(50, 150)
    //   click(100, 50)  → PDF(100, 150)
    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 50 });

    // Reproduce the real browser dblclick sequence at the 3rd point:
    // the OS fires click→click→dblclick, and jsdom's fireEvent.dblClick does NOT
    // auto-fire the constituent clicks — so we fire the two clicks explicitly,
    // then dblClick. Both clicks land at PDF(100, 100) [click(100,100) → y-flip].
    // Without I1's dedup this would leave 4 verts (50,150 / 100,150 / 100,100 / 100,100);
    // the dedup drops the trailing duplicate, leaving 3.
    fireEvent.click(overlay, { clientX: 100, clientY: 100 });
    fireEvent.click(overlay, { clientX: 100, clientY: 100 });
    fireEvent.dblClick(overlay, { clientX: 100, clientY: 100 });
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Polygon");
    const poly = (m.geometry as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(poly).toHaveLength(3);
    expect(poly[0].x).toBeCloseTo(50);
    expect(poly[0].y).toBeCloseTo(150);
    expect(poly[1].x).toBeCloseTo(100);
    expect(poly[1].y).toBeCloseTo(150);
    expect(poly[2].x).toBeCloseTo(100);
    expect(poly[2].y).toBeCloseTo(100);
  });

  // -------------------------------------------------------------------------
  // T14: tool-switch clears in-progress multi-click verts (I2 $effect)
  // -------------------------------------------------------------------------
  it("T14: switching tools clears in-progress verts (created Polygon has only the new 3)", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Polygon";
    await tick();

    // Start an incomplete polygon (2 verts), then switch away and back.
    fireEvent.click(overlay, { clientX: 10, clientY: 10 });
    fireEvent.click(overlay, { clientX: 20, clientY: 20 });

    store.activeTool = "hand";
    await tick(); // let the $effect run and clear gesture state
    store.activeTool = "Polygon";
    await tick();

    // Three fresh vertices + Enter.
    //   click(50, 50)   → PDF(50, 150)
    //   click(100, 50)  → PDF(100, 150)
    //   click(100, 100) → PDF(100, 100)
    fireEvent.click(overlay, { clientX: 50, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 50 });
    fireEvent.click(overlay, { clientX: 100, clientY: 100 });
    fireEvent.keyDown(window, { key: "Enter" });
    await tick();

    expect(store.markups.length).toBe(1);
    const poly = (store.markups[0].geometry as { Polyline: { x: number; y: number }[] }).Polyline;
    // Exactly the 3 new verts — proving the old 2 were cleared (not 5).
    expect(poly).toHaveLength(3);
    expect(poly[0].x).toBeCloseTo(50);
    expect(poly[0].y).toBeCloseTo(150);
  });
});

// ---------------------------------------------------------------------------
// Zoom-snap presets (Fit-Width / Fit-Height / 100%) — buttons + key-commands
// ---------------------------------------------------------------------------
describe("Viewport zoom-snap controls", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  // Page 100×400 pts against a 200×200 container makes the three presets distinct:
  //   fit-width  → 200/100 = 2.0 → "200%"
  //   fit-height → 200/400 = 0.5 → "50%"
  //   actual     → 1.0           → "100%"
  const TALL_PAGE = { doc_id: "d1", page_index: 0, width_pts: 100, height_pts: 400 };

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(TALL_PAGE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function zoomPercent(container: HTMLElement): string {
    return container.querySelector(".zoom-indicator")?.textContent ?? "";
  }

  it("Z1: Cmd+1 fits page width to the viewport (zoom → 200%)", async () => {
    const { container } = await mountViewport(store);
    fireEvent.keyDown(window, { key: "1", metaKey: true });
    await tick();
    expect(zoomPercent(container)).toContain("200%");
  });

  it("Z2: Cmd+2 fits page height to the viewport (zoom → 50%)", async () => {
    const { container } = await mountViewport(store);
    fireEvent.keyDown(window, { key: "2", metaKey: true });
    await tick();
    expect(zoomPercent(container)).toContain("50%");
  });

  it("Z3: Cmd+0 snaps back to actual size (100%) after a fit", async () => {
    const { container } = await mountViewport(store);
    fireEvent.keyDown(window, { key: "1", metaKey: true }); // → 200%
    await tick();
    expect(zoomPercent(container)).toContain("200%");
    fireEvent.keyDown(window, { key: "0", metaKey: true }); // → 100%
    await tick();
    expect(zoomPercent(container)).toContain("100%");
  });

  it("Z4: Ctrl variants work too (cross-platform)", async () => {
    const { container } = await mountViewport(store);
    fireEvent.keyDown(window, { key: "2", ctrlKey: true });
    await tick();
    expect(zoomPercent(container)).toContain("50%");
  });

  it("Z5: the Fit-Width / Fit-Height / 100% buttons snap zoom", async () => {
    const { container } = await mountViewport(store);
    const byTitle = (frag: string) =>
      Array.from(container.querySelectorAll(".zoom-controls button")).find((b) =>
        (b.getAttribute("title") ?? "").includes(frag),
      ) as HTMLButtonElement;

    fireEvent.click(byTitle("Fit width"));
    await tick();
    expect(zoomPercent(container)).toContain("200%");

    fireEvent.click(byTitle("Fit height"));
    await tick();
    expect(zoomPercent(container)).toContain("50%");

    fireEvent.click(byTitle("Actual size"));
    await tick();
    expect(zoomPercent(container)).toContain("100%");
  });
});

// ---------------------------------------------------------------------------
// Auth guard: identity unavailable prevents draw
// ---------------------------------------------------------------------------
describe("Viewport draw guard — identity unavailable", () => {
  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    // getUserIdentity rejects — identity stays null in component.
    vi.mocked(ipcMocks.getUserIdentity).mockRejectedValue(new Error("no identity"));
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("draw gesture does nothing when getUserIdentity rejects", async () => {
    Element.prototype.setPointerCapture = vi.fn();
    const triggerResize = stubResizeObserver();

    const ipc = fakeIpc();
    const store = new MarkupStore("d1", ipc);

    const { container } = render(Viewport, { props: { docInfo: FAKE_DOC, store } });

    await waitFor(() => expect(vi.mocked(ipcMocks.getUserIdentity)).toHaveBeenCalled());

    const containerEl = container.querySelector(".viewport-root") as HTMLElement;
    if (containerEl) {
      vi.spyOn(containerEl, "getBoundingClientRect").mockReturnValue({
        left: 0, top: 0, right: 200, bottom: 200,
        width: 200, height: 200, x: 0, y: 0,
        toJSON() { return {}; },
      } as DOMRect);
      triggerResize(200, 200);
    }

    store.activeTool = "Rectangle";
    const overlay = container.querySelector("svg.markup-overlay") as SVGElement;

    ptr(overlay, "pointerdown", 50, 50);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointerup", 100, 100);

    expect(store.markups.length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// G5 Text + Callout interaction tests
// ---------------------------------------------------------------------------

describe("Viewport G5 text + callout", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // T15: Text place + commit
  // -------------------------------------------------------------------------
  it("T15: Text tool — click opens textarea, typing+blur commits a Text markup", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Text";
    await tick();

    // Click the overlay at (60, 80) → PDF(60, 120)
    fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();

    // A textarea should appear.
    const textarea = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(textarea).not.toBeNull();

    // Set value and trigger input event so the Svelte bind:value syncs.
    textarea.value = "My annotation";
    fireEvent.input(textarea);
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Text");
    expect(m.contents).toBe("My annotation");
    // Geometry is a Rect
    expect("Rect" in m.geometry).toBe(true);
    // No textarea in DOM after commit
    expect(container.querySelector("textarea.text-editor")).toBeNull();

    // The text markup renders an SVG <text> element.
    await tick();
    const svgText = container.querySelector("svg.markup-overlay text");
    expect(svgText).not.toBeNull();
    expect(svgText!.textContent).toBe("My annotation");

    await waitFor(() => expect(ipc.add).toHaveBeenCalledTimes(1));
  });

  // -------------------------------------------------------------------------
  // T16: Empty text = no-op
  // -------------------------------------------------------------------------
  it("T16: Text tool — empty textarea blur creates no markup", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Text";
    await tick();

    fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();

    const textarea = document.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(textarea).not.toBeNull();

    // Blur with no content.
    textarea.value = "";
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  it("T16b: Text tool — whitespace-only textarea blur creates no markup", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Text";
    await tick();

    fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();

    const textarea = document.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(textarea).not.toBeNull();

    // Whitespace-only → trimmed to empty → no-op (plan: empty/whitespace-only = no-op).
    textarea.value = "   ";
    fireEvent.input(textarea);
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(0);
    expect(ipc.add).not.toHaveBeenCalled();
  });

  // -------------------------------------------------------------------------
  // T17: Callout — two clicks + commit
  // -------------------------------------------------------------------------
  it("T17: Callout tool — two clicks open editor, commit creates Callout with Polyline leader", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Callout";
    await tick();

    // First click: leader target at (30, 40) → PDF(30, 160)
    fireEvent.click(overlay, { clientX: 30, clientY: 40 });
    await tick();
    // After first click, no textarea yet.
    expect(container.querySelector("textarea.text-editor")).toBeNull();

    // Second click: text anchor at (80, 60) → PDF(80, 140)
    fireEvent.click(overlay, { clientX: 80, clientY: 60 });
    await tick();

    const textarea = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(textarea).not.toBeNull();

    textarea.value = "see note";
    fireEvent.input(textarea);
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(1);
    const m = store.markups[0];
    expect(m.markup_type).toBe("Callout");
    expect(m.contents).toBe("see note");
    // Geometry is a Polyline with 2 verts (leader target → text anchor).
    const poly = (m.geometry as { Polyline: { x: number; y: number }[] }).Polyline;
    expect(poly).toHaveLength(2);
    expect(poly[0].x).toBeCloseTo(30);   // leader target x
    expect(poly[0].y).toBeCloseTo(160);  // PDF y-flip: 200-40
    expect(poly[1].x).toBeCloseTo(80);   // text anchor x
    expect(poly[1].y).toBeCloseTo(140);  // PDF y-flip: 200-60

    // The callout renders a <polyline> + <text> in the SVG overlay.
    await tick();
    const svgPolyline = container.querySelector("svg.markup-overlay polyline");
    const svgText = container.querySelector("svg.markup-overlay text");
    expect(svgPolyline).not.toBeNull();
    expect(svgText).not.toBeNull();
    expect(svgText!.textContent).toBe("see note");
  });

  // -------------------------------------------------------------------------
  // T18: Font defaulted to Helvetica 12
  // -------------------------------------------------------------------------
  it("T18: created Text markup has DEFAULT_TEXT_FONT (Helvetica 12)", async () => {
    const { overlay } = await mountViewport(store);
    store.activeTool = "Text";
    await tick();

    fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();

    const textarea = document.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    textarea.value = "hello";
    fireEvent.input(textarea);
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(1);
    const font = store.markups[0].appearance.font;
    expect(font).toEqual({ family: "Helvetica", size_pt: 12 });
  });

  // -------------------------------------------------------------------------
  // T19: Glued-on-zoom (text) — position + font-size scale with zoom
  // -------------------------------------------------------------------------
  it("T19: Text markup x/y and font-size scale with zoom (no drift)", async () => {
    const { container, overlay } = await mountViewport(store);
    store.activeTool = "Text";
    await tick();

    // Place a text at (60, 80) → PDF anchor (60, 120).
    fireEvent.click(overlay, { clientX: 60, clientY: 80 });
    await tick();
    const textarea = document.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    textarea.value = "zoom test";
    fireEvent.input(textarea);
    fireEvent.blur(textarea);
    await tick();

    expect(store.markups.length).toBe(1);

    // Read x/y and font-size at zoom=1.
    const textEl = container.querySelector("svg.markup-overlay text") as SVGTextElement;
    expect(textEl).not.toBeNull();
    const x0 = parseFloat(textEl.getAttribute("x")!);
    const fontSize0 = parseFloat(textEl.getAttribute("font-size")!);
    expect(x0).toBeGreaterThan(0);
    expect(fontSize0).toBeCloseTo(12 * 1); // 12pt × zoom 1

    // Zoom in one step (deltaY < 0 → zoom = 1.1).
    fireEvent.wheel(container.querySelector(".viewport-root")!, { deltaY: -100 });
    await tick();

    const textEl2 = container.querySelector("svg.markup-overlay text") as SVGTextElement;
    const x1 = parseFloat(textEl2.getAttribute("x")!);
    const fontSize1 = parseFloat(textEl2.getAttribute("font-size")!);

    // x/y and font-size must scale (proving re-derivation from PDF space + viewState).
    expect(x1).toBeCloseTo(x0 * wheelZoomFactor(-100), 0);
    expect(fontSize1).toBeCloseTo(12 * wheelZoomFactor(-100), 1);
  });
});

// ---------------------------------------------------------------------------
// G6 select tool interaction tests
// ---------------------------------------------------------------------------

/**
 * Coordinate reference for G6 tests (zoom=1, scroll=0, page=200×200, container=200×200 at 0,0):
 *   screenToPdfUserSpace(50, 50)   -> PDF(50,  150)
 *   screenToPdfUserSpace(100, 100) -> PDF(100, 100)
 *   screenToPdfUserSpace(150, 150) -> PDF(150, 50)
 *
 * Rect A: PDF { min:(40,90), max:(110,160) }
 *   screen top-left  = (40, 200-160) = (40, 40)
 *   screen bot-right = (110, 200-90)  = (110, 110)
 *   centre screen    = (75, 75) — inside the rect
 *
 * Rect B: PDF { min:(120,30), max:(180,80) }
 *   screen top-left  = (120, 200-80) = (120, 120)
 *   screen bot-right = (180, 200-30)  = (180, 170)
 *   centre screen    = (150, 145) — inside the rect
 *
 * Polyline C: PDF points (10,180) and (30,160)
 *   screen points ~ (10,20) and (30,40)
 */

describe("Viewport G6 select", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  const DEFAULT_APPEARANCE = {
    color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid" as const, font: null,
  };

  // Seed a rect markup into the store (no IPC side-effects needed for selection tests).
  function seedRect(s: MarkupStore, id: string, minX: number, minY: number, maxX: number, maxY: number) {
    s.markups.push(buildMarkup({
      markupType: "Rectangle",
      page: 0,
      geometry: { Rect: { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } } },
      appearance: DEFAULT_APPEARANCE,
      identity: FAKE_IDENTITY,
      now: "2026-01-01T00:00:00Z",
      id,
    }));
  }

  function seedPolyline(s: MarkupStore, id: string) {
    s.markups.push(buildMarkup({
      markupType: "Polyline",
      page: 0,
      geometry: { Polyline: [{ x: 10, y: 180 }, { x: 30, y: 160 }] },
      appearance: DEFAULT_APPEARANCE,
      identity: FAKE_IDENTITY,
      now: "2026-01-01T00:00:00Z",
      id,
    }));
  }

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // S1: Click on a markup selects it; selection-box appears in SVG.
  // -------------------------------------------------------------------------
  it("S1: click on a rect with select tool selects it and shows .selection-box", async () => {
    const { container, overlay } = await mountViewport(store);
    // Seed rect A: PDF { min:(40,90), max:(110,160) } — screen centre (75,75).
    seedRect(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    // Click in the centre of rect A at screen (75, 75).
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    expect(store.selectedIds.has("rect-a")).toBe(true);
    expect(store.selectedIds.size).toBe(1);

    const box = container.querySelector("svg.markup-overlay .selection-box");
    expect(box).not.toBeNull();
  });

  // -------------------------------------------------------------------------
  // S2: Shift-click a second markup adds it; selection grows to size 2.
  // -------------------------------------------------------------------------
  it("S2: shift-click second markup adds to selection, selection-box spans both", async () => {
    const { container, overlay } = await mountViewport(store);
    seedRect(store, "rect-a", 40, 90, 110, 160);
    seedRect(store, "rect-b", 120, 30, 180, 80);

    store.activeTool = "select";
    await tick();

    // Select rect A at screen (75, 75).
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.size).toBe(1);

    // Shift-click rect B at screen (150, 145).
    fireEvent(overlay, new PointerEvent("pointerdown", {
      bubbles: true, cancelable: true, clientX: 150, clientY: 145, pointerId: 1, shiftKey: true,
    }));
    fireEvent(overlay, new PointerEvent("pointerup", {
      bubbles: true, cancelable: true, clientX: 150, clientY: 145, pointerId: 1, shiftKey: true,
    }));
    await tick();

    expect(store.selectedIds.size).toBe(2);
    expect(store.selectedIds.has("rect-a")).toBe(true);
    expect(store.selectedIds.has("rect-b")).toBe(true);

    // Selection box must be present (spans both rects).
    expect(container.querySelector("svg.markup-overlay .selection-box")).not.toBeNull();

    // With 2 markups selected, no resize handles should appear.
    const handles = container.querySelectorAll("svg.markup-overlay .selection-handle");
    expect(handles.length).toBe(0);
  });

  // -------------------------------------------------------------------------
  // S3: Click empty space clears selection.
  // -------------------------------------------------------------------------
  it("S3: click empty space clears selection and removes .selection-box", async () => {
    const { container, overlay } = await mountViewport(store);
    seedRect(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    // Select rect A.
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.size).toBe(1);

    // Click in empty space at screen (190, 10) — far outside rect A.
    ptr(overlay, "pointerdown", 190, 10);
    ptr(overlay, "pointerup", 190, 10);
    await tick();

    expect(store.selectedIds.size).toBe(0);
    expect(container.querySelector("svg.markup-overlay .selection-box")).toBeNull();
  });

  // -------------------------------------------------------------------------
  // S4: Marquee drag over two rects selects both.
  // -------------------------------------------------------------------------
  it("S4: marquee drag over two rects selects both", async () => {
    const { overlay } = await mountViewport(store);
    seedRect(store, "rect-a", 40, 90, 110, 160);
    seedRect(store, "rect-b", 120, 30, 180, 80);

    store.activeTool = "select";
    await tick();

    // Drag from (10, 10) to (195, 195) — encompasses both rects.
    ptr(overlay, "pointerdown", 10, 10);
    ptr(overlay, "pointermove", 100, 100);
    ptr(overlay, "pointermove", 195, 195);
    ptr(overlay, "pointerup", 195, 195);
    await tick();

    expect(store.selectedIds.has("rect-a")).toBe(true);
    expect(store.selectedIds.has("rect-b")).toBe(true);
    expect(store.selectedIds.size).toBe(2);
  });

  // -------------------------------------------------------------------------
  // S5: Single rect selection shows 8 handles; polyline shows 0.
  // -------------------------------------------------------------------------
  it("S5a: single Rect selection shows 8 .selection-handle elements", async () => {
    const { container, overlay } = await mountViewport(store);
    seedRect(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    const handles = container.querySelectorAll("svg.markup-overlay .selection-handle");
    expect(handles.length).toBe(8);
  });

  it("S5b: single non-rect (Polyline) selection shows 0 .selection-handle elements", async () => {
    const { container, overlay } = await mountViewport(store);
    // Polyline C: PDF points (10,180) and (30,160) -> screen ~(10,20) and (30,40).
    // Hit test at screen (15, 25): close to the segment.
    seedPolyline(store, "poly-c");

    store.activeTool = "select";
    await tick();

    // Click near the polyline segment at screen (15, 25).
    ptr(overlay, "pointerdown", 15, 25);
    ptr(overlay, "pointerup", 15, 25);
    await tick();

    // Should have selected the polyline.
    expect(store.selectedIds.has("poly-c")).toBe(true);

    const handles = container.querySelectorAll("svg.markup-overlay .selection-handle");
    expect(handles.length).toBe(0);
  });

  // -------------------------------------------------------------------------
  // S6: Selection box scales with zoom (glued-on-zoom).
  // -------------------------------------------------------------------------
  it("S6: selection-box width scales with zoom (glued to PDF space)", async () => {
    const { container, overlay } = await mountViewport(store);
    // Rect A: PDF { min:(40,90), max:(110,160) } -> PDF width = 70pt.
    seedRect(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    const box0 = container.querySelector("svg.markup-overlay .selection-box") as SVGRectElement;
    expect(box0).not.toBeNull();
    const w0 = parseFloat(box0.getAttribute("width")!);
    expect(w0).toBeCloseTo(70); // 70pt × zoom 1

    // Zoom in one wheel step.
    fireEvent.wheel(container.querySelector(".viewport-root")!, { deltaY: -100 });
    await tick();

    const box1 = container.querySelector("svg.markup-overlay .selection-box") as SVGRectElement;
    expect(box1).not.toBeNull();
    const w1 = parseFloat(box1.getAttribute("width")!);
    // Width must scale by the zoom factor.
    expect(w1).toBeCloseTo(70 * wheelZoomFactor(-100), 0);
  });

  // -------------------------------------------------------------------------
  // S7: Switching tool away from select clears the selection.
  // -------------------------------------------------------------------------
  it("S7: switching from select to another tool clears selection and removes .selection-box", async () => {
    const { container, overlay } = await mountViewport(store);
    seedRect(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.size).toBe(1);
    expect(container.querySelector("svg.markup-overlay .selection-box")).not.toBeNull();

    // Switch to Rectangle draw tool.
    store.activeTool = "Rectangle";
    await tick();

    expect(store.selectedIds.size).toBe(0);
    expect(container.querySelector("svg.markup-overlay .selection-box")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// G6 move / resize / delete
// ---------------------------------------------------------------------------

describe("Viewport G6 move/resize/delete", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  const DEFAULT_APP = {
    color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid" as const, font: null,
  };

  function seedRect2(s: MarkupStore, id: string, minX: number, minY: number, maxX: number, maxY: number) {
    s.markups.push(buildMarkup({
      markupType: "Rectangle", page: 0,
      geometry: { Rect: { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } } },
      appearance: DEFAULT_APP, identity: FAKE_IDENTITY, now: "2026-01-01T00:00:00Z", id,
    }));
  }

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // SM1: Move single rect
  // -------------------------------------------------------------------------
  it("SM1: move single rect — geometry translated by exact PDF delta, one ipc.update, undo reverts", async () => {
    const { overlay } = await mountViewport(store);
    // Rect A: PDF { min:(40,90), max:(110,160) }, screen centre (75,75)
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    // Select rect A
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.has("rect-a")).toBe(true);

    // Move: down at (75,75) -> PDF(75,125), up at (95,65) -> PDF(95,135)
    // dx=20, dy=10 -> new min=(60,100), max=(130,170)
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointermove", 95, 65);
    ptr(overlay, "pointerup", 95, 65);
    await tick();

    const m = store.markups[0];
    const rect = (m.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    expect(rect.min.x).toBeCloseTo(60);
    expect(rect.min.y).toBeCloseTo(100);
    expect(rect.max.x).toBeCloseTo(130);
    expect(rect.max.y).toBeCloseTo(170);

    await waitFor(() => expect(ipc.update).toHaveBeenCalledTimes(1));

    store.undo();
    const orig = store.markups[0];
    const origRect = (orig.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    expect(origRect.min.x).toBeCloseTo(40);
    expect(origRect.min.y).toBeCloseTo(90);
    expect(origRect.max.x).toBeCloseTo(110);
    expect(origRect.max.y).toBeCloseTo(160);
  });

  // -------------------------------------------------------------------------
  // SM2: Move multi (2 rects)
  // -------------------------------------------------------------------------
  it("SM2: move 2 rects via marquee — both translated, ipc.update x2, one undo reverts both", async () => {
    const { overlay } = await mountViewport(store);
    // Rect A: screen centre (75,75). Rect B: screen centre (150,145)
    seedRect2(store, "rect-a", 40, 90, 110, 160);
    seedRect2(store, "rect-b", 120, 30, 180, 80);

    store.activeTool = "select";
    await tick();

    // Marquee-select both
    ptr(overlay, "pointerdown", 5, 5);
    ptr(overlay, "pointermove", 195, 195);
    ptr(overlay, "pointerup", 195, 195);
    await tick();
    expect(store.selectedIds.size).toBe(2);

    // Move by screen (+20,-10) = PDF (+20,+10)
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointermove", 95, 65);
    ptr(overlay, "pointerup", 95, 65);
    await tick();

    const rects = store.markups.map((m) => (m.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect);
    const a = rects.find((_, i) => store.markups[i].id === "rect-a")!;
    const b = rects.find((_, i) => store.markups[i].id === "rect-b")!;
    expect(a.min.x).toBeCloseTo(60);
    expect(a.min.y).toBeCloseTo(100);
    expect(b.min.x).toBeCloseTo(140);
    expect(b.min.y).toBeCloseTo(40);

    await waitFor(() => expect(ipc.update).toHaveBeenCalledTimes(2));

    store.undo();
    const rects2 = store.markups.map((m) => (m.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect);
    const a2 = rects2.find((_, i) => store.markups[i].id === "rect-a")!;
    const b2 = rects2.find((_, i) => store.markups[i].id === "rect-b")!;
    expect(a2.min.x).toBeCloseTo(40);
    expect(b2.min.x).toBeCloseTo(120);
  });

  // -------------------------------------------------------------------------
  // SM3: No-op click (no drag)
  // -------------------------------------------------------------------------
  it("SM3: pointerdown+up at same point — no geometry change, no ipc.update", async () => {
    const { overlay } = await mountViewport(store);
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    // First click: select
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    ipc.update.mockClear();

    // Second click: same point, no move
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    const rect = (store.markups[0].geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    expect(rect.min.x).toBeCloseTo(40);
    expect(ipc.update).not.toHaveBeenCalled();
    expect(store.canUndo).toBe(false);
  });

  // -------------------------------------------------------------------------
  // SR1: Resize SE handle
  // -------------------------------------------------------------------------
  it("SR1: drag SE handle — SE corner moves, NW corner fixed, ipc.update once", async () => {
    const { overlay } = await mountViewport(store);
    // Rect A: PDF { min:(40,90), max:(110,160) }
    // SE handle: PDF(110, 90) -> screen(110, 110)
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    // Select rect A
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.has("rect-a")).toBe(true);

    // Drag SE handle from (110,110) to (130,130)
    // screen(130,130) -> PDF(130, 70)
    // SE moves maxX -> 130, minY -> 70; NW (minX=40, maxY=160) unchanged
    ptr(overlay, "pointerdown", 110, 110);
    ptr(overlay, "pointermove", 130, 130);
    ptr(overlay, "pointerup", 130, 130);
    await tick();

    const rect = (store.markups[0].geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    // NW corner unchanged
    expect(rect.min.x).toBeCloseTo(40);
    expect(rect.max.y).toBeCloseTo(160);
    // SE corner moved
    expect(rect.max.x).toBeCloseTo(130);
    expect(rect.min.y).toBeCloseTo(70);

    await waitFor(() => expect(ipc.update).toHaveBeenCalledTimes(1));
  });

  // -------------------------------------------------------------------------
  // SR2: Resize min-size clamp
  // -------------------------------------------------------------------------
  it("SR2: dragging handle past opposite edge clamps to MIN_RESIZE_PTS", async () => {
    const { overlay } = await mountViewport(store);
    // Small rect: PDF { min:(50,50), max:(60,60) } - 10pt wide/tall
    // SE handle: PDF(60,50) -> screen(60, 150)
    seedRect2(store, "small", 50, 50, 60, 60);

    store.activeTool = "select";
    await tick();

    // Select (click inside rect: screen centre of PDF(50..60, 50..60) = screen(55, 145))
    ptr(overlay, "pointerdown", 55, 145);
    ptr(overlay, "pointerup", 55, 145);
    await tick();
    expect(store.selectedIds.has("small")).toBe(true);

    // Drag SE handle past NW: screen(60,150) -> screen(45,155) = PDF(45,45)
    ptr(overlay, "pointerdown", 60, 150);
    ptr(overlay, "pointermove", 45, 155);
    ptr(overlay, "pointerup", 45, 155);
    await tick();

    const rect = (store.markups[0].geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    const width = rect.max.x - rect.min.x;
    const height = rect.max.y - rect.min.y;
    expect(width).toBeGreaterThanOrEqual(4);
    expect(height).toBeGreaterThanOrEqual(4);
  });

  // -------------------------------------------------------------------------
  // SD1: Delete
  // -------------------------------------------------------------------------
  it("SD1: Delete key removes selected markups, ipc.remove x2, undo restores both", async () => {
    const { overlay } = await mountViewport(store);
    seedRect2(store, "rect-a", 40, 90, 110, 160);
    seedRect2(store, "rect-b", 120, 30, 180, 80);

    store.activeTool = "select";
    await tick();

    // Select both via marquee
    ptr(overlay, "pointerdown", 5, 5);
    ptr(overlay, "pointermove", 195, 195);
    ptr(overlay, "pointerup", 195, 195);
    await tick();
    expect(store.selectedIds.size).toBe(2);

    fireEvent.keyDown(window, { key: "Delete" });
    await tick();

    expect(store.markups.length).toBe(0);
    await waitFor(() => expect(ipc.remove).toHaveBeenCalledTimes(2));

    store.undo();
    expect(store.markups.length).toBe(2);
  });

  // -------------------------------------------------------------------------
  // SD2: Backspace also deletes
  // -------------------------------------------------------------------------
  it("SD2: Backspace key deletes selected markup", async () => {
    const { overlay } = await mountViewport(store);
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.has("rect-a")).toBe(true);

    fireEvent.keyDown(window, { key: "Backspace" });
    await tick();

    expect(store.markups.length).toBe(0);
  });

  // -------------------------------------------------------------------------
  // SE1: Escape clears selection (no delete)
  // -------------------------------------------------------------------------
  it("SE1: Escape clears selection without deleting markups", async () => {
    const { overlay } = await mountViewport(store);
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();
    expect(store.selectedIds.has("rect-a")).toBe(true);

    fireEvent.keyDown(window, { key: "Escape" });
    await tick();

    expect(store.selectedIds.size).toBe(0);
    expect(store.markups.length).toBe(1);
  });

  // -------------------------------------------------------------------------
  // SA1: Audit bump on move
  // -------------------------------------------------------------------------
  it("SA1: move bumps audit.revision by 1 and updates modified_at", async () => {
    const { overlay } = await mountViewport(store);
    seedRect2(store, "rect-a", 40, 90, 110, 160);

    store.activeTool = "select";
    await tick();

    const origModifiedAt = store.markups[0].audit.modified_at;

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointermove", 95, 65);
    ptr(overlay, "pointerup", 95, 65);
    await tick();

    expect(store.markups[0].audit.revision).toBe(1);
    expect(store.markups[0].audit.modified_at).not.toBe(origModifiedAt);
  });
});

// ---------------------------------------------------------------------------
// G8 grouping interaction tests
// ---------------------------------------------------------------------------

/**
 * Coordinate setup (same as G6 tests):
 *   Rect A: PDF { min:(40,90), max:(110,160) }, screen centre (75,75)
 *   Rect B: PDF { min:(120,30), max:(180,80) }, screen centre (150,145)
 *   Rect C: PDF { min:(5,5), max:(20,15) }, screen centre (12,191) — small rect, loner
 */

describe("Viewport G8 grouping", () => {
  let ipc: ReturnType<typeof fakeIpc>;
  let store: MarkupStore;

  const DEFAULT_APP = {
    color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid" as const, font: null,
  };

  function seedRect3(s: MarkupStore, id: string, minX: number, minY: number, maxX: number, maxY: number) {
    s.markups.push(buildMarkup({
      markupType: "Rectangle", page: 0,
      geometry: { Rect: { min: { x: minX, y: minY }, max: { x: maxX, y: maxY } } },
      appearance: DEFAULT_APP, identity: FAKE_IDENTITY, now: "2026-01-01T00:00:00Z", id,
    }));
  }

  beforeEach(() => {
    vi.mocked(ipcMocks.getPageSize).mockResolvedValue(FAKE_PAGE_SIZE);
    vi.mocked(ipcMocks.renderTile).mockResolvedValue({
      doc_id: "d1", page_index: 0, tile_x: 0, tile_y: 0,
      width_px: 512, height_px: 512, zoom: 1, dpr: 1,
      png_base64: "", render_ms: 1,
    });
    vi.mocked(ipcMocks.processRssMb).mockResolvedValue(0);
    vi.mocked(ipcMocks.getUserIdentity).mockResolvedValue(FAKE_IDENTITY);

    ipc = fakeIpc();
    store = new MarkupStore("d1", ipc);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // G8-1: Cmd+G groups selected markups (≥2) into one undo frame
  // -------------------------------------------------------------------------
  it("G8-1: Cmd+G groups 2 selected markups — both get same non-null group_id, loner stays null, one undo frame", async () => {
    const { overlay } = await mountViewport(store);
    seedRect3(store, "a", 40, 90, 110, 160);
    seedRect3(store, "b", 120, 30, 180, 80);
    seedRect3(store, "c", 5, 5, 20, 15);

    store.activeTool = "select";
    await tick();

    // Select a and b via marquee (they are both within (5,5)-(195,195) drag but c is very small bottom-left)
    // Easier: directly set selectedIds for a and b.
    store.selectedIds = new Set(["a", "b"]);

    // Dispatch Cmd+G.
    fireEvent.keyDown(window, { key: "g", metaKey: true });
    await tick();

    const mA = store.markups.find((m) => m.id === "a")!;
    const mB = store.markups.find((m) => m.id === "b")!;
    const mC = store.markups.find((m) => m.id === "c")!;

    expect(mA.group_id).not.toBeNull();
    expect(mB.group_id).not.toBeNull();
    expect(mA.group_id).toBe(mB.group_id);
    expect(mC.group_id).toBeNull();

    // One undo frame reverts both group_ids to null.
    store.undo();
    const mA2 = store.markups.find((m) => m.id === "a")!;
    const mB2 = store.markups.find((m) => m.id === "b")!;
    expect(mA2.group_id).toBeNull();
    expect(mB2.group_id).toBeNull();
  });

  // -------------------------------------------------------------------------
  // G8-2: Group-aware select — clicking one group member selects both
  // -------------------------------------------------------------------------
  it("G8-2: clicking one member of a grouped pair selects both members", async () => {
    const { overlay } = await mountViewport(store);
    seedRect3(store, "a", 40, 90, 110, 160);   // screen centre (75,75)
    seedRect3(store, "b", 120, 30, 180, 80);   // screen centre (150,145)

    // Pre-group them directly (simulates state after Cmd+G).
    const GID = "gggg0000-0000-0000-0000-000000000001";
    store.markups[0] = { ...store.markups[0], group_id: GID };
    store.markups[1] = { ...store.markups[1], group_id: GID };

    store.activeTool = "select";
    await tick();

    // Click rect A at screen (75, 75).
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointerup", 75, 75);
    await tick();

    // Both a and b should be selected.
    expect(store.selectedIds.has("a")).toBe(true);
    expect(store.selectedIds.has("b")).toBe(true);
    expect(store.selectedIds.size).toBe(2);
  });

  // -------------------------------------------------------------------------
  // G8-3: Group move glued — selecting via group click and moving translates both
  // -------------------------------------------------------------------------
  it("G8-3: move via group-aware select translates both markups by the same delta", async () => {
    const { overlay } = await mountViewport(store);
    seedRect3(store, "a", 40, 90, 110, 160);   // screen centre (75,75)
    seedRect3(store, "b", 120, 30, 180, 80);   // screen centre (150,145)

    const GID = "gggg0000-0000-0000-0000-000000000002";
    store.markups[0] = { ...store.markups[0], group_id: GID };
    store.markups[1] = { ...store.markups[1], group_id: GID };

    store.activeTool = "select";
    await tick();

    // Click on A (selects both via group expand), then drag.
    // Screen (75,75) -> PDF(75,125); screen (95,65) -> PDF(95,135). dx=+20, dy=+10.
    ptr(overlay, "pointerdown", 75, 75);
    ptr(overlay, "pointermove", 95, 65);
    ptr(overlay, "pointerup", 95, 65);
    await tick();

    const mA = store.markups.find((m) => m.id === "a")!;
    const mB = store.markups.find((m) => m.id === "b")!;

    const rA = (mA.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;
    const rB = (mB.geometry as { Rect: { min: { x: number; y: number }; max: { x: number; y: number } } }).Rect;

    // Both translated by (+20, +10) PDF delta.
    expect(rA.min.x).toBeCloseTo(60);   // 40+20
    expect(rA.min.y).toBeCloseTo(100);  // 90+10
    expect(rB.min.x).toBeCloseTo(140);  // 120+20
    expect(rB.min.y).toBeCloseTo(40);   // 30+10
  });

  // -------------------------------------------------------------------------
  // M3-1: calibrate tool — two clicks advance calibrationState
  // -------------------------------------------------------------------------
  it("M3-1: calibrate tool — two pointer-down clicks advance calibration state to waiting_p2", async () => {
    const takeoffStore = new TakeoffStore();
    takeoffStore.startCalibration({ page: 0, appliesToPage: null });
    const { overlay } = await mountViewport(store, takeoffStore);
    store.activeTool = "calibrate";
    await tick();

    // First click on the overlay: calibration state should advance to waiting_p1 -> waiting_p2
    ptr(overlay, "pointerdown", 20, 20);
    await tick();
    // After p1 click the calibrationState step should be waiting_p2
    expect(takeoffStore.calibrationState?.step).toBe("waiting_p2");
  });

  // -------------------------------------------------------------------------
  // M3-2: MeasurementLength tool creates a markup with measurement payload
  // -------------------------------------------------------------------------
  it("M3-2: MeasurementLength drag-draw creates a MeasurementLength markup", async () => {
    const takeoffStore = new TakeoffStore();
    const { overlay } = await mountViewport(store, takeoffStore);
    store.activeTool = "MeasurementLength";
    await tick();

    ptr(overlay, "pointerdown", 50, 50);
    await tick();
    ptr(overlay, "pointermove", 100, 50);
    await tick();
    ptr(overlay, "pointerup", 100, 50);
    await tick();

    expect(store.markups).toHaveLength(1);
    expect(store.markups[0].markup_type).toBe("MeasurementLength");
  });

  // -------------------------------------------------------------------------
  // M3-3: MeasurementCount single click creates a Count markup
  // -------------------------------------------------------------------------
  it("M3-3: MeasurementCount single click places a count markup", async () => {
    const takeoffStore = new TakeoffStore();
    const { overlay } = await mountViewport(store, takeoffStore);
    store.activeTool = "MeasurementCount";
    await tick();

    fireEvent.click(overlay, { clientX: 50, clientY: 50, bubbles: true });
    await tick();

    expect(store.markups).toHaveLength(1);
    expect(store.markups[0].markup_type).toBe("MeasurementCount");
  });

  // -------------------------------------------------------------------------
  // G8-4: Cmd+Shift+G ungroups — group_id back to null, one undo frame
  // -------------------------------------------------------------------------
  it("G8-4: Cmd+Shift+G ungroups selected markups — group_id set to null, one undo frame", async () => {
    const { overlay } = await mountViewport(store);
    seedRect3(store, "a", 40, 90, 110, 160);
    seedRect3(store, "b", 120, 30, 180, 80);

    const GID = "gggg0000-0000-0000-0000-000000000003";
    store.markups[0] = { ...store.markups[0], group_id: GID };
    store.markups[1] = { ...store.markups[1], group_id: GID };

    store.activeTool = "select";
    store.selectedIds = new Set(["a", "b"]);
    await tick();

    // Dispatch Cmd+Shift+G.
    fireEvent.keyDown(window, { key: "G", metaKey: true, shiftKey: true });
    await tick();

    const mA = store.markups.find((m) => m.id === "a")!;
    const mB = store.markups.find((m) => m.id === "b")!;
    expect(mA.group_id).toBeNull();
    expect(mB.group_id).toBeNull();

    // One undo frame reverts both back to having the group_id.
    store.undo();
    const mA2 = store.markups.find((m) => m.id === "a")!;
    const mB2 = store.markups.find((m) => m.id === "b")!;
    expect(mA2.group_id).toBe(GID);
    expect(mB2.group_id).toBe(GID);
  });
});
