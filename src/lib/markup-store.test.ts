import { describe, it, expect, vi } from "vitest";
import { MarkupStore, reconstructCountSets } from "./markup-store.svelte";
import type { CountSet, Markup } from "./ipc";

function countMk(id: string, set: CountSet | null): Markup {
  return {
    id, markup_type: "MeasurementCount", page: 0,
    geometry: { Point: { x: 0, y: 0 } },
    appearance: { color: set?.color ?? "#f00", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: { user_id: "u", display_name: "U" }, created_at: "", modified_by: { user_id: "u", display_name: "U" }, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 1, custom_columns: {} },
    count_set: set,
  };
}

function mk(id: string, contents: string | null = null): Markup {
  return {
    id, markup_type: "Rectangle", page: 0,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
    appearance: { color: "#f00", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents, group_id: null,
    audit: { created_by: { user_id: "u", display_name: "U" }, created_at: "", modified_by: { user_id: "u", display_name: "U" }, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] }, measurement: null,
  };
}

function fakeIpc() {
  return {
    add: vi.fn(async (_d: string, _m: Markup) => {}),
    update: vi.fn(async (_d: string, _m: Markup) => {}),
    remove: vi.fn(async (_d: string, _id: string) => {}),
  };
}

describe("MarkupStore", () => {
  it("create adds to markups and mirrors an add op", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    expect(s.markups.length).toBe(1);
    await s.flush();
    expect(ipc.add).toHaveBeenCalledTimes(1);
    expect(ipc.add).toHaveBeenCalledWith("doc1", expect.objectContaining({ id: "a" }));
  });

  it("update then delete mirror in order", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a");
    s.create(a);
    s.update(a, { ...a, contents: "x" });
    s.delete("a");
    expect(s.markups.length).toBe(0);
    await s.flush();
    expect(ipc.add.mock.invocationCallOrder[0]).toBeLessThan(ipc.update.mock.invocationCallOrder[0]);
    expect(ipc.update.mock.invocationCallOrder[0]).toBeLessThan(ipc.remove.mock.invocationCallOrder[0]);
  });

  it("undo of a create mirrors a delete", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.undo();
    expect(s.markups.length).toBe(0);
    await s.flush();
    expect(ipc.remove).toHaveBeenCalledWith("doc1", "a");
  });

  it("seed loads markups without enqueuing mirror ops", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.seed([mk("a"), mk("b")]);
    expect(s.markups.length).toBe(2);
    await s.flush();
    expect(ipc.add).not.toHaveBeenCalled();
  });

  it("a failed op records mirrorError and halts the drain", async () => {
    const ipc = fakeIpc();
    ipc.add.mockRejectedValueOnce(new Error("boom"));
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    await expect(s.flush()).rejects.toThrow("boom");
    expect(s.mirrorError).toContain("boom");
  });

  it("enqueue during drain still drains in order", async () => {
    let resolveFirst!: () => void;
    const ipc = {
      add: vi.fn(() => new Promise<void>((res) => { resolveFirst = res; })),
      update: vi.fn(async () => {}),
      remove: vi.fn(async () => {}),
    };
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.update(mk("a"), { ...mk("a"), contents: "x" });
    resolveFirst();
    await s.flush();
    expect(ipc.add.mock.invocationCallOrder[0]).toBeLessThan(ipc.update.mock.invocationCallOrder[0]);
  });
});

describe("MarkupStore.selectedMarkups", () => {
  it("returns only the currently selected markups", () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.create(mk("b"));
    s.create(mk("c"));
    s.selectedIds = new Set(["a", "c"]);
    expect(s.selectedMarkups.map((m) => m.id)).toEqual(["a", "c"]);
  });

  it("returns empty array when nothing is selected", () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    expect(s.selectedMarkups).toEqual([]);
  });
});

describe("MarkupStore.applyBatch", () => {
  it("applies 2 update pairs, both afters are reflected in markups", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a", "old-a");
    const b = mk("b", "old-b");
    s.create(a);
    s.create(b);
    await s.flush(); // drain the creates first

    const afterA = mk("a", "new-a");
    const afterB = mk("b", "new-b");
    s.applyBatch([{ before: a, after: afterA }, { before: b, after: afterB }]);

    expect(s.markups.find((m) => m.id === "a")!.contents).toBe("new-a");
    expect(s.markups.find((m) => m.id === "b")!.contents).toBe("new-b");
  });

  it("ipc.update is called twice after flush", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a", "old-a");
    const b = mk("b", "old-b");
    s.create(a);
    s.create(b);
    await s.flush();
    ipc.update.mockClear();

    s.applyBatch([{ before: a, after: mk("a", "new-a") }, { before: b, after: mk("b", "new-b") }]);
    await s.flush();
    expect(ipc.update).toHaveBeenCalledTimes(2);
  });

  it("ONE undo reverts both updates", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a", "old-a");
    const b = mk("b", "old-b");
    s.create(a);
    s.create(b);
    await s.flush();

    s.applyBatch([{ before: a, after: mk("a", "new-a") }, { before: b, after: mk("b", "new-b") }]);
    expect(s.canUndo).toBe(true);

    s.undo();
    expect(s.markups.find((m) => m.id === "a")!.contents).toBe("old-a");
    expect(s.markups.find((m) => m.id === "b")!.contents).toBe("old-b");
    expect(s.canUndo).toBe(true); // the two creates are still on the stack
  });

  it("redo re-applies both updates", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a", "old-a");
    const b = mk("b", "old-b");
    s.create(a);
    s.create(b);
    await s.flush();

    s.applyBatch([{ before: a, after: mk("a", "new-a") }, { before: b, after: mk("b", "new-b") }]);
    s.undo();
    s.redo();

    expect(s.markups.find((m) => m.id === "a")!.contents).toBe("new-a");
    expect(s.markups.find((m) => m.id === "b")!.contents).toBe("new-b");
  });

  it("empty applyBatch is a no-op", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    await s.flush();
    ipc.update.mockClear();

    s.applyBatch([]);
    await s.flush();
    expect(ipc.update).not.toHaveBeenCalled();
    expect(s.canUndo).toBe(true); // only the create remains
  });
});

describe("MarkupStore.deleteSelected", () => {
  it("deletes both selected markups and clears selectedIds", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.create(mk("b"));
    s.create(mk("c"));
    s.selectedIds = new Set(["a", "b"]);
    await s.flush();
    ipc.remove.mockClear();

    s.deleteSelected();
    expect(s.markups.map((m) => m.id)).toEqual(["c"]);
    expect(s.selectedIds.size).toBe(0);
  });

  it("ipc.remove called twice after flush", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.create(mk("b"));
    s.selectedIds = new Set(["a", "b"]);
    await s.flush();
    ipc.remove.mockClear();

    s.deleteSelected();
    await s.flush();
    expect(ipc.remove).toHaveBeenCalledTimes(2);
  });

  it("ONE undo restores both deleted markups", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.create(mk("b"));
    s.selectedIds = new Set(["a", "b"]);
    await s.flush();

    s.deleteSelected();
    expect(s.markups).toHaveLength(0);

    s.undo();
    expect(s.markups).toHaveLength(2);
    expect(s.markups.map((m) => m.id).sort()).toEqual(["a", "b"]);
    // selection is NOT restored — that is acceptable
  });

  it("deleteSelected with empty selection is a no-op", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    await s.flush();
    ipc.remove.mockClear();

    s.deleteSelected(); // selectedIds is empty
    await s.flush();
    expect(ipc.remove).not.toHaveBeenCalled();
    expect(s.markups).toHaveLength(1);
  });
});

describe("MarkupStore count sets", () => {
  it("seeds one default active count set on construction", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    expect(s.countSets).toHaveLength(1);
    expect(s.activeCountSetId).toBe(s.countSets[0].id);
    expect(s.activeCountSet?.symbol).toBe("Circle");
  });

  it("addCountSet appends a set and makes it active", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    const set = s.addCountSet("Type-B", "Square", "#00875a");
    expect(s.countSets).toHaveLength(2);
    expect(s.activeCountSetId).toBe(set.id);
    expect(s.activeCountSet).toMatchObject({ name: "Type-B", color: "#00875a", symbol: "Square" });
  });

  it("setActiveCountSet selects a known set and ignores unknown ids", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    const first = s.countSets[0].id;
    s.addCountSet("Type-B", "Square");
    s.setActiveCountSet(first);
    expect(s.activeCountSetId).toBe(first);
    s.setActiveCountSet("nope");
    expect(s.activeCountSetId).toBe(first);
  });

  it("reconstructCountSets returns the unique sets embedded on markups", () => {
    const A: CountSet = { id: "a", name: "A", color: "#0066ff", symbol: "Triangle" };
    const B: CountSet = { id: "b", name: "B", color: "#00875a", symbol: "Square" };
    const sets = reconstructCountSets([countMk("1", A), countMk("2", B), countMk("3", A), countMk("4", null)]);
    expect(sets.map((s) => s.id)).toEqual(["a", "b"]);
  });

  it("seed restores the document's count sets and activates one", () => {
    const A: CountSet = { id: "a", name: "Type-A", color: "#0066ff", symbol: "Triangle" };
    const s = new MarkupStore("doc1", fakeIpc());
    s.seed([countMk("1", A)]);
    expect(s.countSets.some((set) => set.id === "a")).toBe(true);
    expect(s.activeCountSetId).toBe("a");
  });
});

describe("MarkupStore dirty tracking", () => {
  it("is clean on construction", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    expect(s.dirty).toBe(false);
  });

  it("becomes dirty after create", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after update", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    const m = mk("a");
    s.create(m);
    s.clearDirty();
    s.update(m, { ...m, contents: "changed" });
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after delete", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    s.clearDirty();
    s.delete("a");
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after undo", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    s.clearDirty();
    s.undo();
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after redo", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    s.undo();
    s.clearDirty();
    s.redo();
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after applyBatch", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    const m = mk("a");
    s.create(m);
    s.clearDirty();
    s.applyBatch([{ before: m, after: { ...m, contents: "x" } }]);
    expect(s.dirty).toBe(true);
  });

  it("becomes dirty after deleteSelected", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    s.selectedIds = new Set(["a"]);
    s.clearDirty();
    s.deleteSelected();
    expect(s.dirty).toBe(true);
  });

  it("clearDirty resets to false", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    expect(s.dirty).toBe(true);
    s.clearDirty();
    expect(s.dirty).toBe(false);
  });

  it("seed resets dirty to false", () => {
    const s = new MarkupStore("doc1", fakeIpc());
    s.create(mk("a"));
    expect(s.dirty).toBe(true);
    s.seed([mk("b")]);
    expect(s.dirty).toBe(false);
  });
});
