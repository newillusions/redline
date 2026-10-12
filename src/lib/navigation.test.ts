import { describe, expect, it } from "vitest";
import type { BookmarkView, LinkView, NavTarget } from "./navigation-ipc";
import type { Markup } from "./ipc";
import {
  ancestorIds,
  bookmarkForMarkup,
  bookmarksOnPage,
  choiceOf,
  currentViewDest,
  defaultBookmarkTitle,
  dropSpec,
  filterLinks,
  formatLabelNumber,
  hasLabels,
  isFollowable,
  jumpFor,
  labelOf,
  linkAt,
  moveSpec,
  openIds,
  pageText,
  pagesForRange,
  previewLabels,
  rectFromCorners,
  rectIsUsable,
  reconcileExpanded,
  resolvePageInput,
  targetText,
  viewFromChoice,
  visibleRows,
} from "./navigation";

const page = (p: number): NavTarget => ({
  kind: "page",
  page: p,
  view: { kind: "xyz", left: null, top: null, zoom: null },
});

function bm(id: string, title: string, children: BookmarkView[] = [], open = false, target = page(0)): BookmarkView {
  return { id, title, open, target, color: null, bold: false, italic: false, children };
}

/** A(0) [B(0.0), C(0.1) [D(0.1.0)]], E(1) */
function tree(): BookmarkView[] {
  return [
    bm("0", "A", [bm("0.0", "B"), bm("0.1", "C", [bm("0.1.0", "D")])], true),
    bm("1", "E"),
  ];
}

describe("page labels", () => {
  const labels = ["", "E-101", "E-102", ""];
  it("shows the label, else the 1-based number", () => {
    expect(labelOf(labels, 1)).toBe("E-101");
    expect(labelOf(labels, 99)).toBe("");
    expect(pageText(labels, 1)).toBe("E-101");
    expect(pageText(labels, 0)).toBe("1");
    expect(pageText(["  "], 0)).toBe("1");
    expect(hasLabels(labels)).toBe(true);
    expect(hasLabels(["", " "])).toBe(false);
  });

  it("resolves typed input as a label first, then a page number", () => {
    expect(resolvePageInput("e-102", labels, 4)).toBe(2);
    expect(resolvePageInput(" E-101 ", labels, 4)).toBe(1);
    expect(resolvePageInput("4", labels, 4)).toBe(3);
    expect(resolvePageInput("5", labels, 4)).toBeNull();
    expect(resolvePageInput("0", labels, 4)).toBeNull();
    expect(resolvePageInput("nope", labels, 4)).toBeNull();
    expect(resolvePageInput("", labels, 4)).toBeNull();
    expect(resolvePageInput("1", labels, 0)).toBeNull();
    // A set labelled with plain numbers: the label wins over the position.
    expect(resolvePageInput("3", ["", "", "3"], 3)).toBe(2);
  });
});

describe("targets", () => {
  it("describes every target kind", () => {
    expect(targetText(page(2), ["", "", "E-3"])).toBe("Page E-3");
    expect(targetText({ kind: "uri", uri: "https://example.com" })).toBe("https://example.com");
    expect(targetText({ kind: "unsupported", action: "Launch" })).toContain("Launch");
    expect(targetText({ kind: "blocked", reason: "the scheme is not allowed" })).toContain("Blocked");
    expect(targetText({ kind: "broken" })).toContain("no longer exists");
    expect(targetText({ kind: "none" })).toBe("No action");
  });

  it("only pages and addresses are followable", () => {
    expect(isFollowable(page(0))).toBe(true);
    expect(isFollowable({ kind: "uri", uri: "https://a.example" })).toBe(true);
    for (const t of [
      { kind: "none" },
      { kind: "broken" },
      { kind: "blocked", reason: "x" },
      { kind: "unsupported", action: "JavaScript" },
    ] as NavTarget[]) {
      expect(isFollowable(t)).toBe(false);
    }
  });

  it("turns a destination into a viewport request", () => {
    expect(jumpFor(page(3))).toEqual({ page: 3 });
    expect(
      jumpFor({ kind: "page", page: 1, view: { kind: "xyz", left: 10, top: 700, zoom: 2 } }),
    ).toEqual({ page: 1, view: { left: 10, top: 700, zoom: 2 } });
    expect(
      jumpFor({ kind: "page", page: 1, view: { kind: "fit_r", left: 1, bottom: 2, right: 3, top: 4 } }),
    ).toEqual({ page: 1, rect: [1, 2, 3, 4] });
    expect(jumpFor({ kind: "page", page: 1, view: { kind: "fit" } })).toEqual({ page: 1 });
    expect(jumpFor({ kind: "uri", uri: "https://a.example" })).toBeNull();
  });

  it("maps view choices both ways", () => {
    for (const c of ["inherit", "fit_page", "fit_width", "actual_size"] as const) {
      expect(choiceOf(viewFromChoice(c))).toBe(c);
    }
    expect(viewFromChoice("actual_size")).toEqual({ kind: "xyz", left: null, top: null, zoom: 1 });
  });

  it("captures the current view as the top-left of the screen", () => {
    expect(currentViewDest(2, 2, 200, 400, 792)).toEqual({
      kind: "page",
      page: 2,
      view: { kind: "xyz", left: 100, top: 592, zoom: 2 },
    });
    // Nonsense zoom or negative scroll cannot produce NaN or out-of-page values.
    const d = currentViewDest(0, 0, -50, -50, 792);
    expect(d.kind === "page" && d.view.kind === "xyz" && Number.isFinite(d.view.zoom as number)).toBe(true);
  });
});

describe("bookmark tree", () => {
  it("moves up, down, indent and outdent", () => {
    const t = tree();
    expect(moveSpec(t, "0.1", "up")).toMatchObject({ place: "before", anchor: { id: "0.0" } });
    expect(moveSpec(t, "0.0", "up")).toBeNull();
    expect(moveSpec(t, "0", "down")).toMatchObject({ place: "after", anchor: { id: "1" } });
    expect(moveSpec(t, "1", "down")).toBeNull();
    expect(moveSpec(t, "0.1", "indent")).toMatchObject({ place: "child", anchor: { id: "0.0" } });
    expect(moveSpec(t, "0.0", "indent")).toBeNull();
    expect(moveSpec(t, "0.1.0", "outdent")).toMatchObject({ place: "after", anchor: { id: "0.1" } });
    expect(moveSpec(t, "0", "outdent")).toBeNull();
  });

  it("drops by row position and refuses a drop into itself", () => {
    const t = tree();
    expect(dropSpec(t, "1", "0", 0.1)).toMatchObject({ place: "before" });
    expect(dropSpec(t, "1", "0", 0.9)).toMatchObject({ place: "after" });
    expect(dropSpec(t, "1", "0", 0.5)).toMatchObject({ place: "child" });
    expect(dropSpec(t, "0", "0", 0.5)).toBeNull();
    expect(dropSpec(t, "0", "0.1.0", 0.5)).toBeNull();
    expect(dropSpec(t, "1", "9", 0.5)).toBeNull();
    // "0.1" is not inside "0.10".
    const wide = [bm("0", "A", [...Array.from({ length: 12 }, (_, i) => bm(`0.${i}`, `c${i}`))])];
    expect(dropSpec(wide, "0.1", "0.10", 0.5)).not.toBeNull();
  });

  it("lists visible rows, honouring collapse", () => {
    const t = tree();
    expect(visibleRows(t, new Set()).map((r) => r.node.title)).toEqual(["A", "E"]);
    expect(visibleRows(t, new Set(["0"])).map((r) => r.node.title)).toEqual(["A", "B", "C", "E"]);
    const all = visibleRows(t, new Set(["0", "0.1"]));
    expect(all.map((r) => [r.node.title, r.depth])).toEqual([
      ["A", 0], ["B", 1], ["C", 1], ["D", 2], ["E", 0],
    ]);
  });

  it("keeps the user's expansion and opens a parent that just gained a child", () => {
    const before = [bm("0", "A", [bm("0.0", "B")], false)];
    const after = [bm("0", "A", [bm("0.0", "B"), bm("0.1", "New")], true)];
    expect([...reconcileExpanded(before, after, new Set())]).toEqual(["0"]);
    // The user collapsed it and the file still says open: it stays collapsed.
    const stillOpen = [bm("0", "A", [bm("0.0", "B")], true)];
    expect([...reconcileExpanded(stillOpen, stillOpen, new Set())]).toEqual([]);
    // A node that lost its children drops out.
    expect([...reconcileExpanded(after, [bm("0", "A")], new Set(["0"]))]).toEqual([]);
  });

  it("finds open ids, bookmarks on a page and ancestors", () => {
    const t = tree();
    expect([...openIds(t)]).toEqual(["0"]);
    const t2 = [bm("0", "A", [bm("0.0", "B", [], false, page(4))], true, page(2)), bm("1", "E", [], false, { kind: "uri", uri: "https://a.example" })];
    expect([...bookmarksOnPage(t2, 4)]).toEqual(["0.0"]);
    expect([...bookmarksOnPage(t2, 2)]).toEqual(["0"]);
    expect(ancestorIds("0.1.0")).toEqual(["0", "0.1"]);
    expect(ancestorIds("3")).toEqual([]);
  });

  it("titles a new bookmark with the page's label", () => {
    expect(defaultBookmarkTitle(["E-1", ""], 0)).toBe("E-1");
    expect(defaultBookmarkTitle(["E-1", ""], 1)).toBe("Page 2");
  });
});

describe("links", () => {
  const link = (page: number, rect: [number, number, number, number], target: NavTarget): LinkView => ({
    id: "obj:1:0", page, rect, target,
  });

  it("hits the topmost link containing a point", () => {
    const links = [
      link(0, [0, 0, 100, 100], page(1)),
      link(0, [50, 50, 150, 150], page(2)),
    ];
    expect(linkAt(links, 60, 60)?.target).toEqual(page(2));
    expect(linkAt(links, 10, 10)?.target).toEqual(page(1));
    expect(linkAt(links, 500, 500)).toBeNull();
    expect(linkAt([], 1, 1)).toBeNull();
  });

  it("normalises drawn corners and rejects tiny areas", () => {
    expect(rectFromCorners({ x: 30, y: 40 }, { x: 10, y: 5 })).toEqual([10, 5, 30, 40]);
    expect(rectIsUsable([0, 0, 10, 10])).toBe(true);
    expect(rectIsUsable([0, 0, 1, 10])).toBe(false);
    expect(rectIsUsable([0, 0, Number.NaN, 10])).toBe(false);
  });

  it("filters the list by page and target text", () => {
    const links = [
      link(0, [0, 0, 9, 9], { kind: "uri", uri: "https://example.com/spec" }),
      link(1, [0, 0, 9, 9], page(2)),
    ];
    expect(filterLinks(links, "SPEC", [])).toHaveLength(1);
    expect(filterLinks(links, "e-102", ["", "E-102"])).toHaveLength(1);
    expect(filterLinks(links, "", [])).toHaveLength(2);
    expect(filterLinks(links, "zzz", [])).toHaveLength(0);
  });
});

describe("page ranges and label previews", () => {
  it("resolves All, Current, Selected and Custom ranges", () => {
    expect(pagesForRange("all", 3, 0, [], "")).toEqual([0, 1, 2]);
    expect(pagesForRange("current", 3, 1, [], "")).toEqual([1]);
    expect(pagesForRange("current", 3, null, [], "")).toBeNull();
    expect(pagesForRange("selected", 5, 0, [3, 1, 3, 9], "")).toEqual([1, 3]);
    expect(pagesForRange("selected", 5, 0, [], "")).toBeNull();
    expect(pagesForRange("custom", 10, 0, [], "1-3, 5, 9")).toEqual([0, 1, 2, 4, 8]);
    expect(pagesForRange("custom", 5, 0, [], "1-9")).toBeNull();
    expect(pagesForRange("custom", 5, 0, [], "banana")).toBeNull();
    expect(pagesForRange("custom", 5, 0, [], "")).toBeNull();
    expect(pagesForRange("all", 0, null, [], "")).toBeNull();
  });

  it("previews the same text the backend writes", () => {
    expect(previewLabels("D", "E-", 101, 3)).toEqual(["E-101", "E-102", "E-103"]);
    expect(previewLabels("r", "", 3, 2)).toEqual(["iii", "iv"]);
    expect(previewLabels("A", "", 26, 2)).toEqual(["Z", "AA"]);
    expect(previewLabels(null, "Cover", 1, 2)).toEqual(["Cover", "Cover"]);
    expect(formatLabelNumber("R", 4000)).toBe("4000");
    expect(formatLabelNumber("a", 26 * 8 + 1)).toBe(String(26 * 8 + 1));
  });
});

describe("bookmark for a mark-up", () => {
  const rect = (min: [number, number], max: [number, number], over: Partial<Markup> = {}) =>
    ({
      id: "m", page: 3, markup_type: "Rectangle", subject: null, contents: null,
      geometry: { Rect: { min: { x: min[0], y: min[1] }, max: { x: max[0], y: max[1] } } },
      ...over,
    }) as unknown as Markup;

  it("points at the page and a box around the mark-up", () => {
    const { title, target } = bookmarkForMarkup(rect([100, 200], [300, 260], { subject: "Fixture A" }));
    expect(title).toBe("Fixture A");
    expect(target).toEqual({
      kind: "page", page: 3,
      view: { kind: "fit_r", left: 88, bottom: 170, right: 312, top: 290 }, // 12pt margin, grown to 120pt tall
    });
  });

  it("never makes the box smaller than a readable area, however small the mark-up", () => {
    const { target } = bookmarkForMarkup(rect([50, 50], [51, 51]));
    expect(target.kind === "page" && target.view.kind === "fit_r").toBe(true);
    if (target.kind === "page" && target.view.kind === "fit_r") {
      expect(target.view.right - target.view.left).toBeGreaterThanOrEqual(120);
      expect(target.view.top - target.view.bottom).toBeGreaterThanOrEqual(120);
    }
  });

  it("titles from the comment, then the type, and bounds the length", () => {
    expect(bookmarkForMarkup(rect([0, 0], [9, 9], { contents: "  check\n clearance  " })).title).toBe("check clearance");
    expect(bookmarkForMarkup(rect([0, 0], [9, 9])).title).toBe("Rectangle");
    expect(bookmarkForMarkup(rect([0, 0], [9, 9], { subject: "x".repeat(500) })).title.length).toBe(80);
  });
});
