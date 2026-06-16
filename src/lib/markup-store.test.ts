import { describe, it, expect, vi } from "vitest";
import { MarkupStore } from "./markup-store.svelte";
import type { Markup } from "./ipc";

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
