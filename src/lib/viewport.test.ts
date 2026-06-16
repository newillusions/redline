import { describe, it, expect } from "vitest";
import {
  fitWidthZoom,
  fitHeightZoom,
  ACTUAL_SIZE_ZOOM,
  wheelZoomFactor,
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
