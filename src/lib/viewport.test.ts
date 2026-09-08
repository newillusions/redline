import { describe, it, expect } from "vitest";
import {
  fitWidthZoom,
  fitHeightZoom,
  ACTUAL_SIZE_ZOOM,
  wheelZoomFactor,
  quantizeZoom,
  clampTileDpr,
  MAX_TILE_DPR,
  ZOOM_MIN,
  ZOOM_MAX,
  classifyWheelEvent,
  normalizeWheelDelta,
  parseZoomPercent,
  type WheelEventLike,
} from "./viewport";

// ---------------------------------------------------------------------------
// Zoom-snap presets (§5: all math in PDF user space / css px — never the raster)
// ---------------------------------------------------------------------------
describe("fitWidthZoom", () => {
  it("returns the zoom at which page width exactly fills the viewport width", () => {
    expect(fitWidthZoom(200, 400)).toBeCloseTo(2);   // 400 css px / 200 pts
    expect(fitWidthZoom(400, 200)).toBeCloseTo(0.5); // 200 css px / 400 pts
    expect(fitWidthZoom(612, 612)).toBeCloseTo(1);   // letter width 1:1
  });

  it("falls back to actual size when the page width is unknown (avoids divide-by-zero)", () => {
    expect(fitWidthZoom(0, 400)).toBe(ACTUAL_SIZE_ZOOM);
    expect(fitWidthZoom(-1, 400)).toBe(ACTUAL_SIZE_ZOOM);
  });
});

describe("fitHeightZoom", () => {
  it("returns the zoom at which page height exactly fills the viewport height", () => {
    expect(fitHeightZoom(400, 200)).toBeCloseTo(0.5); // 200 css px / 400 pts
    expect(fitHeightZoom(200, 400)).toBeCloseTo(2);
    expect(fitHeightZoom(792, 792)).toBeCloseTo(1);   // letter height 1:1
  });

  it("falls back to actual size when the page height is unknown", () => {
    expect(fitHeightZoom(0, 200)).toBe(ACTUAL_SIZE_ZOOM);
  });
});

describe("ACTUAL_SIZE_ZOOM", () => {
  it("is 1.0 (1:1 / 100%)", () => {
    expect(ACTUAL_SIZE_ZOOM).toBe(1.0);
  });
});

// Guard the existing shared curve stays put alongside the new presets.
describe("wheelZoomFactor", () => {
  it("is symmetric around zero delta and clamped to [0.5, 2]", () => {
    expect(wheelZoomFactor(0)).toBeCloseTo(1);
    expect(wheelZoomFactor(-100000)).toBeCloseTo(2);
    expect(wheelZoomFactor(100000)).toBeCloseTo(0.5);
  });
});

// ---------------------------------------------------------------------------
// quantizeZoom - Windows-freeze fix (bounds the number of distinct raster zoom
// levels a smooth wheel-zoom gesture can generate).
// ---------------------------------------------------------------------------
describe("quantizeZoom", () => {
  it("collapses the real Windows-freeze zoom sequence onto a small, bounded set of rungs", () => {
    // The actual sequence captured from the freeze-reproducing Windows log: a run of close
    // fractional zoom values from a smooth wheel gesture, each of which minted a fresh tile
    // set under the old unquantized behaviour.
    const observedSequence = [
      0.9895, 0.9807, 1.0698, 0.8517, 0.8843, 0.8390, 0.8378, 0.9912, 1.0123, 0.9456,
    ];
    const rungs = new Set(observedSequence.map((z) => quantizeZoom(z)));
    // Values within ~12% of each other must collapse onto the same or adjacent rungs -
    // nowhere near one distinct rung per input value.
    expect(rungs.size).toBeLessThan(observedSequence.length);
  });

  it("maps a dense continuous zoom range onto a bounded ladder of discrete levels", () => {
    const rungs = new Set<number>();
    // 200 distinct continuous zoom levels sampled smoothly across the valid range - directly
    // mirrors the TDD scenario feeding the tile cache in tile-cache.test.ts.
    for (let i = 0; i < 200; i++) {
      const z = ZOOM_MIN + ((ZOOM_MAX - ZOOM_MIN) * i) / 199;
      rungs.add(quantizeZoom(z));
    }
    // The ladder spans [ZOOM_MIN, ZOOM_MAX] in ~12% steps - a small, bounded rung count
    // regardless of how many continuous input samples are fed in.
    expect(rungs.size).toBeLessThan(50);
  });

  it("is a pure, gesture-independent function - the same input always yields the same rung", () => {
    expect(quantizeZoom(1.0)).toBe(quantizeZoom(1.0));
    expect(quantizeZoom(0.8517)).toBe(quantizeZoom(0.8517));
  });

  it("clamps output to [min, max]", () => {
    expect(quantizeZoom(0.001)).toBeGreaterThanOrEqual(ZOOM_MIN);
    expect(quantizeZoom(1000)).toBeLessThanOrEqual(ZOOM_MAX);
  });
});

// ---------------------------------------------------------------------------
// clampTileDpr - Windows-freeze fix (bounds per-tile decoded byte size on
// high display-scaling Windows machines, e.g. 250% -> dpr 2.5).
// ---------------------------------------------------------------------------
describe("clampTileDpr", () => {
  it("passes through dpr values at or below the cap", () => {
    expect(clampTileDpr(1)).toBe(1);
    expect(clampTileDpr(2)).toBe(2);
  });

  it("clamps the real Windows 250% scaling case (dpr 2.5) down to MAX_TILE_DPR", () => {
    expect(clampTileDpr(2.5)).toBe(MAX_TILE_DPR);
    expect(clampTileDpr(3)).toBe(MAX_TILE_DPR);
  });
});

// ---------------------------------------------------------------------------
// normalizeWheelDelta - deltaMode-aware px conversion (0=pixel, 1=line, 2=page).
// ---------------------------------------------------------------------------
describe("normalizeWheelDelta", () => {
  it("passes deltaMode 0 (pixel) through unchanged", () => {
    expect(normalizeWheelDelta(42, 0)).toBe(42);
    expect(normalizeWheelDelta(-7.5, 0)).toBe(-7.5);
  });

  it("scales deltaMode 1 (line) up to an approximate px value", () => {
    expect(normalizeWheelDelta(1, 1)).toBeGreaterThan(1);
    expect(normalizeWheelDelta(-2, 1)).toBeLessThan(-2);
  });

  it("scales deltaMode 2 (page) up further than line mode", () => {
    expect(normalizeWheelDelta(1, 2)).toBeGreaterThan(normalizeWheelDelta(1, 1));
  });

  it("preserves sign", () => {
    expect(normalizeWheelDelta(-3, 1)).toBeLessThan(0);
    expect(normalizeWheelDelta(3, 1)).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// classifyWheelEvent - owner-decided pan/zoom scheme (2026-09-08): a two-finger
// trackpad swipe pans, a pinch (or explicit Ctrl/Cmd+wheel) zooms.
// ---------------------------------------------------------------------------
describe("classifyWheelEvent", () => {
  function wheel(overrides: Partial<WheelEventLike>): WheelEventLike {
    return { deltaX: 0, deltaY: 0, deltaMode: 0, ctrlKey: false, metaKey: false, shiftKey: false, ...overrides };
  }

  it("mac trackpad two-finger swipe (ctrlKey false, small deltaX/deltaY, deltaMode 0) pans diagonally", () => {
    const action = classifyWheelEvent(wheel({ deltaX: 3.2, deltaY: -1.7 }));
    expect(action).toEqual({ kind: "pan", dx: 3.2, dy: -1.7 });
  });

  it("mac pinch (ctrlKey true, small deltaY - WebKit's synthetic pinch-wheel shape) zooms", () => {
    const action = classifyWheelEvent(wheel({ deltaY: -12, ctrlKey: true }));
    expect(action.kind).toBe("zoom");
    if (action.kind === "zoom") {
      expect(action.factor).toBeCloseTo(wheelZoomFactor(-12));
      expect(action.factor).toBeGreaterThan(1); // negative deltaY → zoom in
    }
  });

  it("Windows precision-touchpad pan (same shape as mac trackpad) pans", () => {
    const action = classifyWheelEvent(wheel({ deltaX: -5, deltaY: 4 }));
    expect(action).toEqual({ kind: "pan", dx: -5, dy: 4 });
  });

  it("Windows precision-touchpad pinch (ctrlKey true) zooms, same as mac", () => {
    const action = classifyWheelEvent(wheel({ deltaY: 8, ctrlKey: true }));
    expect(action.kind).toBe("zoom");
  });

  it("plain mouse wheel (deltaMode 1 lines, deltaY only) pans vertically", () => {
    const action = classifyWheelEvent(wheel({ deltaY: 3, deltaMode: 1 }));
    expect(action).toEqual({ kind: "pan", dx: 0, dy: normalizeWheelDelta(3, 1) });
  });

  it("plain mouse wheel with a large raw deltaY (deltaMode 0) pans vertically", () => {
    const action = classifyWheelEvent(wheel({ deltaY: 240 }));
    expect(action).toEqual({ kind: "pan", dx: 0, dy: 240 });
  });

  it("Shift+wheel zooms (owner amendment 2026-09-08) - Shift is a zoom trigger, not a horizontal-pan remap", () => {
    const action = classifyWheelEvent(wheel({ deltaY: 50, shiftKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: wheelZoomFactor(50) });
  });

  it("Shift+two-finger-swipe (deltaX and deltaY both nonzero) still zooms - deltaY is preferred over deltaX", () => {
    const action = classifyWheelEvent(wheel({ deltaX: 12, deltaY: -8, shiftKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: wheelZoomFactor(-8) });
  });

  it("Shift+wheel on a Windows mouse (Chromium/WebView2 remaps the delta to deltaX, zeroing deltaY) falls back to deltaX so it isn't a silent no-op", () => {
    // Regression case (review finding 2026-09-08, PR #111 round 2, BLOCKING): before the
    // deltaY-or-deltaX fallback, this shape classified as wheelZoomFactor(0) === 1, a
    // deterministic no-op that silently broke Shift+wheel zoom for every Windows mouse.
    const action = classifyWheelEvent(wheel({ deltaX: 120, deltaY: 0, shiftKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: wheelZoomFactor(120) });
    expect((action as { factor: number }).factor).not.toBe(1);
  });

  it("Shift+wheel on a Windows mouse scrolling the other direction (negative deltaX, deltaY zero) zooms in, not out", () => {
    const action = classifyWheelEvent(wheel({ deltaX: -120, deltaY: 0, shiftKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: wheelZoomFactor(-120) });
    expect((action as { factor: number }).factor).toBeGreaterThan(1); // negative delta -> zoom in
  });

  it("Shift+wheel with both deltas zero (a genuinely empty event) is the one legitimate no-op - not a regression, just nothing to classify", () => {
    const action = classifyWheelEvent(wheel({ deltaX: 0, deltaY: 0, shiftKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: 1 });
  });

  it("Cmd+wheel (metaKey, macOS convention) zooms same as ctrlKey", () => {
    const action = classifyWheelEvent(wheel({ deltaY: -20, metaKey: true }));
    expect(action.kind).toBe("zoom");
  });

  it("Ctrl+wheel from an explicit mouse zooms using the shared wheelZoomFactor curve", () => {
    const action = classifyWheelEvent(wheel({ deltaY: 100, ctrlKey: true }));
    expect(action).toEqual({ kind: "zoom", factor: wheelZoomFactor(100) });
  });

  it("Ctrl+Shift+wheel still zooms - both modifiers independently trigger zoom", () => {
    const action = classifyWheelEvent(wheel({ deltaY: -20, ctrlKey: true, shiftKey: true }));
    expect(action.kind).toBe("zoom");
  });

  it("an unmodified two-finger swipe with a horizontal component pans horizontally (deltaX from the trackpad itself)", () => {
    const action = classifyWheelEvent(wheel({ deltaX: 50, deltaY: 0 }));
    expect(action).toEqual({ kind: "pan", dx: 50, dy: 0 });
  });

  it("a mouse tilt-wheel's deltaX (no modifier) pans horizontally", () => {
    const action = classifyWheelEvent(wheel({ deltaX: 30, deltaY: 0, deltaMode: 1 }));
    expect(action).toEqual({ kind: "pan", dx: normalizeWheelDelta(30, 1), dy: 0 });
  });
});

// ---------------------------------------------------------------------------
// parseZoomPercent - toolbar zoom-percent input parsing.
// ---------------------------------------------------------------------------
describe("parseZoomPercent", () => {
  it("parses a plain integer percent into a zoom multiplier", () => {
    expect(parseZoomPercent("150")).toBeCloseTo(1.5);
    expect(parseZoomPercent("100")).toBeCloseTo(1.0);
    expect(parseZoomPercent("50")).toBeCloseTo(0.5);
  });

  it("tolerates a trailing % sign and surrounding whitespace", () => {
    expect(parseZoomPercent("150%")).toBeCloseTo(1.5);
    expect(parseZoomPercent("  150  ")).toBeCloseTo(1.5);
  });

  it("clamps to [ZOOM_MIN, ZOOM_MAX]", () => {
    expect(parseZoomPercent("1")).toBeCloseTo(ZOOM_MIN);
    expect(parseZoomPercent("99999")).toBeCloseTo(ZOOM_MAX);
  });

  it("returns null for empty, non-numeric, zero, or negative input", () => {
    expect(parseZoomPercent("")).toBeNull();
    expect(parseZoomPercent("   ")).toBeNull();
    expect(parseZoomPercent("abc")).toBeNull();
    expect(parseZoomPercent("0")).toBeNull();
    expect(parseZoomPercent("-50")).toBeNull();
  });
});
