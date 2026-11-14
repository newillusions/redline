// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import { invoke } from "@tauri-apps/api/core";
import SymbolSearchPanel from "./SymbolSearchPanel.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { buildMarkup } from "$lib/markup-tools";
import type { Appearance, UserRef } from "$lib/ipc";

const mockInvoke = vi.mocked(invoke);
const by: UserRef = { user_id: "u", display_name: "T" };
const APPEARANCE = {
  color: "#112233", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null,
} as Appearance;
const ipc = () => ({ add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
const m = (cx: number, cy: number) => ({ center: [cx, cy], rect: { x: cx - 5, y: cy - 5, w: 10, h: 10 }, angle: 0, score: 1, scale: 1 });
const RESP = {
  truncated: false,
  method: "vector", template_segments: 12, skipped: [], elapsed_ms: 9,
  pages: [{ page: 0, matches: [m(100, 100), m(200, 200), m(300, 300)], truncated: false }],
};

function setup(select = true) {
  const store = new MarkupStore("d", ipc());
  const box = buildMarkup({
    markupType: "Rectangle", page: 0, geometry: { Rect: { min: { x: 90, y: 90 }, max: { x: 110, y: 110 } } },
    appearance: APPEARANCE, identity: by, now: "t", id: "box",
  });
  store.seed([box]);
  if (select) store.selectedIds = new Set(["box"]);
  const r = render(SymbolSearchPanel, { props: { store, docId: "d", currentPage: 0, pageCount: 4, identity: by } });
  return { store, ...r };
}
const settle = async () => { await tick(); await Promise.resolve(); await tick(); };

describe("SymbolSearchPanel", () => {
  beforeEach(() => {
    mockInvoke.mockReset(); // braces: a returned mock would be run by vitest as a cleanup hook
  });

  it("asks for a box when nothing is selected and disables Find", () => {
    const { getByTestId } = setup(false);
    expect(getByTestId("ss-template").textContent).toContain("Draw a Rectangle");
    expect((getByTestId("ss-find") as HTMLButtonElement).disabled).toBe(true);
  });

  it("searches with the selected rectangle and the chosen options, then lists every hit accepted", async () => {
    mockInvoke.mockResolvedValue(RESP as never);
    const { getByTestId, container } = setup();
    expect(getByTestId("ss-template").textContent).toContain("page 1");
    await fireEvent.click(getByTestId("ss-rotations"));
    await fireEvent.change(getByTestId("ss-scope"), { target: { value: "all" } });
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(mockInvoke).toHaveBeenCalledWith("symbol_search", {
      request: expect.objectContaining({
        doc_id: "d", template_page: 0, rect: { x: 90, y: 90, w: 20, h: 20 }, pages: [0, 1, 2, 3], rotations: true, min_score: 0.9,
      }),
    });
    expect(container.querySelectorAll("[data-hit]")).toHaveLength(3);
    expect(getByTestId("ss-summary").textContent).toContain("3 found");
    expect(getByTestId("ss-place").textContent).toContain("Place 3 counts");
  });

  it("rejecting a hit lowers the count and Place creates only the accepted markers in one undo step", async () => {
    mockInvoke.mockResolvedValue(RESP as never);
    const { getByTestId, container, store } = setup();
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    const boxes = container.querySelectorAll<HTMLInputElement>("[data-hit] input[type=checkbox]");
    await fireEvent.click(boxes[1]);
    expect(getByTestId("ss-place").textContent).toContain("Place 2 counts");
    const before = store.markups.length;
    await fireEvent.click(getByTestId("ss-place"));
    const added = store.markups.filter((x) => x.markup_type === "MeasurementCount");
    expect(store.markups.length - before).toBe(2);
    expect(added.map((x) => (x.geometry as { Point: { x: number } }).Point.x).sort()).toEqual([100, 300]);
    expect(new Set(added.map((x) => x.count_group)).size).toBe(1);
    store.undo();
    expect(store.markups.filter((x) => x.markup_type === "MeasurementCount")).toHaveLength(0);
  });

  it("shows the failure and keeps no stale hits", async () => {
    const { getByTestId, container } = setup();
    // Set after setup: other store code may call invoke too. Tauri rejects with the command's Err(String), not an Error.
    mockInvoke.mockImplementation((() => Promise.reject("the box holds too little line work")) as never);
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(getByTestId("ss-error").textContent).toContain("too little line work");
    expect(container.querySelectorAll("[data-hit]")).toHaveLength(0);
  });

  it("tells the user when nothing matched", async () => {
    mockInvoke.mockResolvedValue({ ...RESP, pages: [{ page: 0, matches: [] }] } as never);
    const { getByTestId } = setup();
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(getByTestId("ss-message").textContent).toContain("No matching symbols");
  });

  it("lists why each page was skipped, with 1-based page numbers", async () => {
    mockInvoke.mockResolvedValue({
      ...RESP,
      skipped: [{ page: 2, reason: "page too large to rasterise at this resolution" }, { page: 4, reason: "matching failed: boom" }],
    } as never);
    const { getByTestId } = setup();
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(getByTestId("ss-skipped").textContent).toContain("2 pages skipped");
    const list = getByTestId("ss-skipped-list").textContent ?? "";
    expect(list).toContain("Page 3: page too large to rasterise");
    expect(list).toContain("Page 5: matching failed: boom");
  });

  it("still shows the skip reasons when nothing matched", async () => {
    mockInvoke.mockResolvedValue({ ...RESP, pages: [], skipped: [{ page: 0, reason: "unreadable" }] } as never);
    const { getByTestId } = setup();
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(getByTestId("ss-skipped-list").textContent).toContain("Page 1: unreadable");
  });

  it("warns when the search was cut short by a limit", async () => {
    mockInvoke.mockResolvedValue({ ...RESP, truncated: true } as never);
    const { getByTestId, queryByTestId } = setup();
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(getByTestId("ss-truncated").textContent).toContain("cut short");
    mockInvoke.mockResolvedValue(RESP as never);
    await fireEvent.click(getByTestId("ss-find"));
    await settle();
    expect(queryByTestId("ss-truncated")).toBeNull();
  });
});
