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
