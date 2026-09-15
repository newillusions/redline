import { describe, it, expect } from "vitest";
import {
  computePageLayout,
  pageAtScrollTop,
  scrollTopForPage,
  visiblePageRange,
  parsePageJumpInput,
  pageTileSizeCss,
  READING_PAGE_GAP_CSS,
  type PageDims,
} from "./reading-mode";

const LETTER: PageDims = { width_pts: 612, height_pts: 792 };
const A0_LANDSCAPE: PageDims = { width_pts: 3370, height_pts: 2384 };

describe("computePageLayout", () => {
  it("stacks pages top to bottom with a gap between them", () => {
    const layout = computePageLayout([LETTER, LETTER, LETTER], 1.0);
    expect(layout.topsCss).toEqual([0, 792 + READING_PAGE_GAP_CSS, (792 + READING_PAGE_GAP_CSS) * 2]);
    expect(layout.heightsCss).toEqual([792, 792, 792]);
    expect(layout.widthsCss).toEqual([612, 612, 612]);
  });

  it("scales dimensions by zoom", () => {
    const layout = computePageLayout([LETTER], 0.5);
    expect(layout.heightsCss).toEqual([396]);
    expect(layout.widthsCss).toEqual([306]);
  });

  it("total height has no trailing gap after the last page", () => {
    const layout = computePageLayout([LETTER, LETTER], 1.0);
    expect(layout.totalHeightCss).toBe(792 * 2 + READING_PAGE_GAP_CSS);
  });

  it("handles an empty page list", () => {
    const layout = computePageLayout([], 1.0);
    expect(layout.topsCss).toEqual([]);
    expect(layout.totalHeightCss).toBe(0);
  });

  it("handles mixed page sizes (a sheet set with a title page + large-format drawings)", () => {
    const layout = computePageLayout([LETTER, A0_LANDSCAPE], 1.0);
    expect(layout.topsCss[1]).toBe(792 + READING_PAGE_GAP_CSS);
    expect(layout.widthsCss[1]).toBe(3370);
  });

  it("uses a custom gap when given", () => {
    const layout = computePageLayout([LETTER, LETTER], 1.0, 0);
    expect(layout.topsCss).toEqual([0, 792]);
    expect(layout.totalHeightCss).toBe(792 * 2);
  });
});

describe("pageAtScrollTop", () => {
  const layout = computePageLayout([LETTER, LETTER, LETTER], 1.0); // tops: 0, 804, 1608

  it("returns page 0 at the very top", () => {
    expect(pageAtScrollTop(layout, 0)).toBe(0);
  });

  it("returns the page whose top has just been scrolled past", () => {
    expect(pageAtScrollTop(layout, 804)).toBe(1);
    expect(pageAtScrollTop(layout, 900)).toBe(1);
  });

  it("returns the last page once scrolled to the end", () => {
    expect(pageAtScrollTop(layout, 100000)).toBe(2);
  });

  it("clamps a negative scroll position to page 0", () => {
    expect(pageAtScrollTop(layout, -50)).toBe(0);
  });

  it("returns 0 for an empty layout instead of -1", () => {
    expect(pageAtScrollTop(computePageLayout([], 1.0), 0)).toBe(0);
  });
});

describe("scrollTopForPage", () => {
  const layout = computePageLayout([LETTER, LETTER, LETTER], 1.0);

  it("returns each page's own top offset", () => {
    expect(scrollTopForPage(layout, 0)).toBe(0);
    expect(scrollTopForPage(layout, 1)).toBe(804);
    expect(scrollTopForPage(layout, 2)).toBe(1608);
  });

  it("clamps an out-of-range index", () => {
    expect(scrollTopForPage(layout, 99)).toBe(1608);
    expect(scrollTopForPage(layout, -1)).toBe(0);
  });

  it("returns 0 for an empty layout", () => {
    expect(scrollTopForPage(computePageLayout([], 1.0), 0)).toBe(0);
  });
});

describe("visiblePageRange", () => {
  // 10 identical Letter pages, tops at 0, 804, 1608, ... (804 apart).
  const pages = Array.from({ length: 10 }, () => LETTER);
  const layout = computePageLayout(pages, 1.0);

  it("includes only the strictly-visible pages when lookahead is 0", () => {
    // Viewport [0, 800): only page 0 (top 0..792) is visible.
    const range = visiblePageRange(layout, 0, 800, 0);
    expect(range).toEqual({ first: 0, last: 0 });
  });

  it("expands by lookaheadPages on both sides, clamped to the document", () => {
    const range = visiblePageRange(layout, 0, 800, 1);
    expect(range).toEqual({ first: 0, last: 1 }); // no page -1, but page 1 included
  });

  it("tracks a scroll position in the middle of the document", () => {
    // Scrolled to top=1700 (mid page 2, tops 1608..2400), viewport height 200.
    const range = visiblePageRange(layout, 1700, 200, 1);
    // Page 2 (1608..2400) is visible; lookahead adds pages 1 and 3.
    expect(range.first).toBe(1);
    expect(range.last).toBe(3);
  });

  it("clamps the trailing lookahead at the last page", () => {
    const range = visiblePageRange(layout, 100000, 800, 1);
    expect(range.last).toBe(9);
  });

  it("never renders every page for a large document (virtualization holds)", () => {
    const many = Array.from({ length: 500 }, () => LETTER);
    const bigLayout = computePageLayout(many, 1.0);
    const range = visiblePageRange(bigLayout, 40000, 900, 1);
    const windowSize = range.last - range.first + 1;
    expect(windowSize).toBeLessThan(10);
  });

  it("returns an empty range for an empty document", () => {
    const range = visiblePageRange(computePageLayout([], 1.0), 0, 800, 1);
    expect(range.last).toBeLessThan(range.first);
  });
});

describe("parsePageJumpInput", () => {
  it("parses a plain 1-based page number", () => {
    expect(parsePageJumpInput("5", 10)).toBe(4);
    expect(parsePageJumpInput("1", 10)).toBe(0);
    expect(parsePageJumpInput("10", 10)).toBe(9);
  });

  it("tolerates surrounding whitespace", () => {
    expect(parsePageJumpInput("  7  ", 10)).toBe(6);
  });

  it("rejects empty input", () => {
    expect(parsePageJumpInput("", 10)).toBeNull();
    expect(parsePageJumpInput("   ", 10)).toBeNull();
  });

  it("rejects non-numeric input", () => {
    expect(parsePageJumpInput("abc", 10)).toBeNull();
    expect(parsePageJumpInput("5.5", 10)).toBeNull();
    expect(parsePageJumpInput("-3", 10)).toBeNull();
  });

  it("rejects out-of-range input rather than clamping", () => {
    expect(parsePageJumpInput("0", 10)).toBeNull();
    expect(parsePageJumpInput("11", 10)).toBeNull();
  });

  it("rejects any input when the document has no pages", () => {
    expect(parsePageJumpInput("1", 0)).toBeNull();
  });
});

describe("pageTileSizeCss", () => {
  it("sizes the tile to the larger of width/height at the given zoom", () => {
    expect(pageTileSizeCss(612, 792, 1.0)).toBe(792);
    expect(pageTileSizeCss(2000, 500, 1.0)).toBe(2000);
  });

  it("scales with zoom", () => {
    expect(pageTileSizeCss(612, 792, 0.5)).toBe(396);
  });

  it("caps at maxTileCss for an extreme zoom on a large page", () => {
    expect(pageTileSizeCss(3370, 2384, 4.0, 4096)).toBe(4096);
  });

  it("never returns 0 or a negative size", () => {
    expect(pageTileSizeCss(0, 0, 1.0)).toBeGreaterThan(0);
  });
});
