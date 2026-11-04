import { describe, it, expect, beforeEach } from "vitest";
import { runMarkupCommand, selectAllOnPage, setSelectionStatus } from "./context-actions";
import type { ArrangeCtx } from "./arrange-actions";
import { markupClipboard } from "./markup-clipboard.svelte";
import { BY, mk, seqIds, storeWith } from "./arrange.test-util";

function ctx(ms = [mk("a"), mk("b", { rect: [20, 20, 30, 30] }), mk("c", { rect: [40, 40, 50, 50] })]) {
  const store = storeWith(ms);
  const mint = seqIds("n");
  const c: ArrangeCtx = {
    store, identity: BY, scales: [], pageIndex: 0, pageCount: 3, pageWidthPts: 600, pageHeightPts: 400,
    cursor: { x: 100, y: 100 }, viewCenter: { x: 300, y: 200 }, now: () => "2026-11-04T10:00:00Z", mintId: mint,
  };
  return c;
}

beforeEach(() => markupClipboard.clear());

describe("setSelectionStatus", () => {
  it("sets the status on every selected markup as one undo step", () => {
    const c = ctx();
    c.store.selectedIds = new Set(["a", "b"]);
    const r = setSelectionStatus(c, "Accepted");
    expect(r).toEqual({ ok: true, message: "Status Accepted on 2 markups" });
    expect(c.store.getById("a")?.workflow.status).toBe("Accepted");
    expect(c.store.getById("b")?.workflow.status).toBe("Accepted");
    expect(c.store.getById("c")?.workflow.status).toBe("None");
    c.store.undo();
    expect(c.store.getById("a")?.workflow.status).toBe("None");
    expect(c.store.getById("b")?.workflow.status).toBe("None");
  });
  it("skips locked markups and says so", () => {
    const c = ctx([mk("a", { annot_flags: 4 | 128 }), mk("b")]);
    c.store.selectedIds = new Set(["a", "b"]);
    const r = setSelectionStatus(c, "Rejected");
    expect(r.message).toBe("Status Rejected on 1 markup; 1 locked skipped");
    expect(c.store.getById("a")?.workflow.status).toBe("None");
    expect(c.store.getById("b")?.workflow.status).toBe("Rejected");
  });
  it("refuses with nothing selected", () => {
    expect(setSelectionStatus(ctx(), "Accepted").ok).toBe(false);
  });
});

describe("runMarkupCommand", () => {
  it("routes status ids", () => {
    const c = ctx();
    c.store.selectedIds = new Set(["c"]);
    expect(runMarkupCommand("status:Completed", c)?.ok).toBe(true);
    expect(c.store.getById("c")?.workflow.status).toBe("Completed");
  });
  it("copy then paste lands a copy; duplicate adds one", () => {
    const c = ctx();
    c.store.selectedIds = new Set(["a"]);
    expect(runMarkupCommand("copy", c)?.ok).toBe(true);
    expect(markupClipboard.size).toBe(1);
    expect(runMarkupCommand("paste", c)?.ok).toBe(true);
    expect(c.store.markups.length).toBe(4);
    c.store.selectedIds = new Set(["a"]);
    expect(runMarkupCommand("duplicate", c)?.ok).toBe(true);
    expect(c.store.markups.length).toBe(5);
  });
  it("cut removes, and bring-to-front / send-to-back reorder", () => {
    const c = ctx();
    c.store.selectedIds = new Set(["a"]);
    runMarkupCommand("bring-to-front", c);
    expect(c.store.markups.map((m) => m.id)).toEqual(["b", "c", "a"]);
    runMarkupCommand("send-to-back", c);
    expect(c.store.markups.map((m) => m.id)).toEqual(["a", "b", "c"]);
    runMarkupCommand("cut", c);
    expect(c.store.markups.map((m) => m.id)).toEqual(["b", "c"]);
  });
  it("returns null for ids the caller owns", () => {
    expect(runMarkupCommand("properties", ctx())).toBeNull();
    expect(runMarkupCommand("delete", ctx())).toBeNull();
  });
});

describe("selectAllOnPage", () => {
  it("selects the given ids, or refuses when there are none", () => {
    const c = ctx();
    expect(selectAllOnPage(c, ["a", "b"]).ok).toBe(true);
    expect([...c.store.selectedIds]).toEqual(["a", "b"]);
    expect(selectAllOnPage(c, []).ok).toBe(false);
  });
});
