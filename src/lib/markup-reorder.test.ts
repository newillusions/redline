import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import * as ipc from "./ipc";
import { refillOrder, ReorderCmd, History, type MarkupSink } from "./markup-commands";
import type { Markup } from "./ipc";
import { MarkupStore, type MarkupIpc } from "./markup-store.svelte";
import { mk, storeWith } from "./arrange.test-util";

const ids = (ms: Markup[]) => ms.map((m) => m.id);
const five = () => ["a", "b", "c", "d", "e"].map((i) => mk(i));

describe("refillOrder (mirrors Rust MarkupStore::reorder)", () => {
  it("refills the slots of the listed ids and leaves the rest where they are", () => {
    expect(ids(refillOrder(five(), ["e", "b", "d"]))).toEqual(["a", "e", "c", "b", "d"]);
  });
  it("ignores unknown ids and repeats", () => {
    expect(ids(refillOrder(five(), ["zz", "c", "b", "c"]))).toEqual(["a", "c", "b", "d", "e"]);
  });
  it("returns the same array when nothing moves", () => {
    const ms = five();
    expect(refillOrder(ms, ["b", "c"])).toBe(ms);
    expect(refillOrder(ms, [])).toBe(ms);
  });
});

describe("ReorderCmd in History: one undo step", () => {
  it("applies, undoes and redoes through the sink", () => {
    let list = five();
    const sink: MarkupSink = {
      insert: () => {}, replace: () => {}, removeById: () => {}, getById: () => undefined,
      reorder: (order) => { list = refillOrder(list, order); },
    };
    const h = new History(sink);
    const ops = h.pushBatch([new ReorderCmd(["a", "b"], ["b", "a"]), new ReorderCmd(["d", "e"], ["e", "d"])]);
    expect(ops).toEqual([{ kind: "reorder", ids: ["b", "a"] }, { kind: "reorder", ids: ["e", "d"] }]);
    expect(ids(list)).toEqual(["b", "a", "c", "e", "d"]);
    expect(h.undo()).toEqual([{ kind: "reorder", ids: ["d", "e"] }, { kind: "reorder", ids: ["a", "b"] }]);
    expect(ids(list)).toEqual(["a", "b", "c", "d", "e"]);
    h.redo();
    expect(ids(list)).toEqual(["b", "a", "c", "e", "d"]);
  });
});

describe("MarkupStore.applyReorder", () => {
  function recording() {
    const calls: string[][] = [];
    const log: string[] = [];
    const io: MarkupIpc = {
      add: async (_d, m) => { log.push(`add ${m.id}`); },
      update: async () => {}, remove: async () => {},
      reorder: async (_d, order) => { calls.push(order); log.push("reorder"); },
    };
    return { io, calls, log };
  }

  it("reorders the array, marks dirty, mirrors the ids in order, and undo/redo are one step each", async () => {
    const { io, calls } = recording();
    const s = storeWith(five(), "d1", io);
    s.applyReorder([{ before: ["a", "b", "c"], after: ["c", "a", "b"] }]);
    expect(ids(s.markups)).toEqual(["c", "a", "b", "d", "e"]);
    expect(s.dirty).toBe(true);
    await s.flush();
    expect(calls).toEqual([["c", "a", "b"]]);
    s.undo();
    expect(ids(s.markups)).toEqual(["a", "b", "c", "d", "e"]);
    s.redo();
    expect(ids(s.markups)).toEqual(["c", "a", "b", "d", "e"]);
    await s.flush();
    expect(calls).toEqual([["c", "a", "b"], ["a", "b", "c"], ["c", "a", "b"]]);
  });

  it("drops changes that move nothing (no undo entry, not dirty)", () => {
    const s = storeWith(five());
    s.applyReorder([{ before: ["a", "b"], after: ["a", "b"] }]);
    expect(s.canUndo).toBe(false);
    expect(s.dirty).toBe(false);
  });

  it("a reorder queued behind an add reaches the backend after it", async () => {
    const { io, log } = recording();
    const s = storeWith(five(), "d1", io);
    s.create(mk("f"));
    s.applyReorder([{ before: ["e", "f"], after: ["f", "e"] }]);
    await s.flush();
    expect(log).toEqual(["add f", "reorder"]);
  });

  it("works against an ipc stub that predates reorder (the op is simply not mirrored)", async () => {
    const s = new MarkupStore("d", { add: async () => {}, update: async () => {}, remove: async () => {} });
    s.seed(five());
    s.applyReorder([{ before: ["a", "b"], after: ["b", "a"] }]);
    await expect(s.flush()).resolves.toBeUndefined();
  });

  it("adoptCountSets adds unknown sets once", () => {
    const s = storeWith([]);
    const before = s.countSets.length;
    const x = { id: "x", name: "X", color: "#000000", symbol: "Circle" as const };
    s.adoptCountSets([x, x]);
    s.adoptCountSets([x]);
    expect(s.countSets).toHaveLength(before + 1);
  });
});

describe("reorderMarkups invoke arguments (Tauri v2 camelCase)", () => {
  const mockInvoke = vi.mocked(invoke);
  beforeEach(() => { mockInvoke.mockReset(); mockInvoke.mockResolvedValue(undefined as never); });
  it("reorder_markups -> docId / markupIds", async () => {
    await ipc.reorderMarkups("d1", ["a", "b"]);
    expect(mockInvoke).toHaveBeenCalledWith("reorder_markups", { docId: "d1", markupIds: ["a", "b"] });
  });
});
