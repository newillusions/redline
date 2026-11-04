import { describe, it, expect } from "vitest";
import {
  PANEL_IDS,
  PANEL_MIN_HEIGHT,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  BOTTOM_MIN_HEIGHT,
  defaultLayout,
  normalizeLayout,
  movePanel,
  movePanelBy,
  setCollapsed,
  setSidebarWidth,
  setSidebarHidden,
  setBottomHeight,
  splitWeights,
  dropIndex,
  sideOf,
  clampSidebarWidth,
  clampBottomHeight,
} from "./panel-layout";

function allPanels(l: ReturnType<typeof defaultLayout>) {
  return [...l.left.panels, ...l.right.panels].sort();
}

describe("defaultLayout", () => {
  it("places every panel exactly once, Properties on the right", () => {
    const l = defaultLayout();
    expect(allPanels(l)).toEqual([...PANEL_IDS].sort());
    expect(l.right.panels).toEqual(["properties"]);
    expect(l.left.panels[0]).toBe("search");
  });

  it("keeps the bottom panel hidden and Links collapsed, as before", () => {
    const l = defaultLayout();
    expect(l.bottom.hidden).toBe(true);
    expect(l.collapsed.links).toBe(true);
    expect(l.collapsed.toolchest).toBe(false);
  });

  it("gives Search a larger height share than the panels below it", () => {
    const l = defaultLayout();
    expect(l.weights.search).toBeGreaterThan(l.weights.recent);
  });
});

describe("normalizeLayout", () => {
  it("returns the default for null, non-objects and garbage", () => {
    for (const raw of [null, undefined, 42, "x", []]) {
      expect(normalizeLayout(raw)).toEqual(defaultLayout());
    }
  });

  it("round-trips a valid layout unchanged", () => {
    const l = movePanel(setSidebarWidth(defaultLayout(), "left", 333), "layers", "right", 0);
    expect(normalizeLayout(JSON.parse(JSON.stringify(l)))).toEqual(l);
  });

  it("drops unknown and duplicate ids and restores missing ones to their default sidebar", () => {
    const l = normalizeLayout({
      left: { panels: ["toolchest", "bogus", "toolchest", "properties"] },
      right: { panels: ["recent"] },
    });
    expect(l.left.panels.slice(0, 2)).toEqual(["toolchest", "properties"]);
    expect(l.right.panels[0]).toBe("recent");
    expect(allPanels(l)).toEqual([...PANEL_IDS].sort());
    // "search" was missing - it goes back to the left (its default sidebar)
    expect(sideOf(l, "search")).toBe("left");
  });

  it("clamps out-of-range sizes and ignores non-numeric ones", () => {
    const l = normalizeLayout({
      left: { width: 5 },
      right: { width: 99999 },
      bottom: { height: 1 },
      weights: { toolchest: -3, recent: "big", layers: 1e9 },
    });
    expect(l.left.width).toBe(SIDEBAR_MIN_WIDTH);
    expect(l.right.width).toBe(SIDEBAR_MAX_WIDTH);
    expect(l.bottom.height).toBe(BOTTOM_MIN_HEIGHT);
    expect(l.weights.toolchest).toBeGreaterThan(0);
    expect(l.weights.recent).toBe(1);
    expect(Number.isFinite(l.weights.layers)).toBe(true);
  });
});

describe("movePanel", () => {
  it("moves a panel to the other sidebar at the given index", () => {
    const l = movePanel(defaultLayout(), "toolchest", "right", 0);
    expect(l.right.panels).toEqual(["toolchest", "properties"]);
    expect(l.left.panels).not.toContain("toolchest");
  });

  it("reorders within a sidebar using the post-removal index", () => {
    const base = defaultLayout();
    const l = movePanel(base, "search", "left", 2);
    expect(l.left.panels.slice(0, 3)).toEqual(["recent", "toolchest", "search"]);
  });

  it("clamps the index and does not mutate the input", () => {
    const base = defaultLayout();
    const snapshot = JSON.stringify(base);
    const l = movePanel(base, "properties", "left", 999);
    expect(l.left.panels.at(-1)).toBe("properties");
    expect(l.right.panels).toEqual([]);
    expect(JSON.stringify(base)).toBe(snapshot);
  });

  it("shows a hidden sidebar when a panel is moved into it", () => {
    const hidden = setSidebarHidden(defaultLayout(), "right", true);
    expect(movePanel(hidden, "layers", "right", 0).right.hidden).toBe(false);
  });
});

describe("movePanelBy", () => {
  it("moves up and down within the sidebar and is a no-op at the ends", () => {
    const base = defaultLayout();
    expect(movePanelBy(base, "recent", -1).left.panels.slice(0, 2)).toEqual(["recent", "search"]);
    expect(movePanelBy(base, "search", -1)).toBe(base);
    expect(movePanelBy(base, "history", 1)).toBe(base);
    expect(movePanelBy(base, "properties", 1)).toBe(base);
  });
});

describe("setCollapsed", () => {
  it("only changes the named panel", () => {
    const l = setCollapsed(defaultLayout(), "toolchest", true);
    expect(l.collapsed.toolchest).toBe(true);
    expect(l.collapsed.recent).toBe(false);
  });
});

describe("sidebar width / bottom height clamps", () => {
  it("never lets a sidebar exceed 40% of the window", () => {
    expect(clampSidebarWidth(600, 1024)).toBe(409);
    expect(clampSidebarWidth(100, 1024)).toBe(SIDEBAR_MIN_WIDTH);
    // tiny window: the minimum still wins
    expect(clampSidebarWidth(300, 300)).toBe(SIDEBAR_MIN_WIDTH);
  });

  it("never lets the bottom panel exceed 70% of the window", () => {
    expect(clampBottomHeight(900, 640)).toBe(448);
    expect(setBottomHeight(defaultLayout(), 50).bottom.height).toBe(BOTTOM_MIN_HEIGHT);
  });
});

describe("splitWeights", () => {
  it("moves height from B to A while keeping their combined weight", () => {
    const [a, b] = splitWeights(1, 1, 200, 200, 100);
    expect(a + b).toBeCloseTo(2);
    expect(a / (a + b)).toBeCloseTo(300 / 400);
  });

  it("stops at the minimum height on either side", () => {
    const [a] = splitWeights(1, 1, 200, 200, 10_000);
    expect((a / 2) * 400).toBeCloseTo(400 - PANEL_MIN_HEIGHT);
    const [a2] = splitWeights(1, 1, 200, 200, -10_000);
    expect((a2 / 2) * 400).toBeCloseTo(PANEL_MIN_HEIGHT);
  });

  it("changes nothing when the two panels cannot both fit their minimum", () => {
    expect(splitWeights(1, 3, 50, 60, 20)).toEqual([1, 3]);
  });
});

describe("dropIndex", () => {
  const frames = [
    { id: "search" as const, mid: 50 },
    { id: "recent" as const, mid: 150 },
    { id: "toolchest" as const, mid: 250 },
  ];

  it("inserts above the first frame whose midpoint is below the pointer", () => {
    expect(dropIndex(frames, "layers", 10)).toBe(0);
    expect(dropIndex(frames, "layers", 120)).toBe(1);
    expect(dropIndex(frames, "layers", 999)).toBe(3);
  });

  it("ignores the dragged panel's own frame", () => {
    expect(dropIndex(frames, "search", 200)).toBe(1);
    expect(dropIndex(frames, "search", 999)).toBe(2);
  });
});
