import { describe, it, expect, vi } from "vitest";
import { MarkupStore } from "./markup-store.svelte";
import { mk } from "./arrange.test-util";
import { markupMenuItems } from "./context-menu";
import { runMarkupCommand } from "./context-actions";
import type { ArrangeCtx } from "./arrange-actions";

function ipc() {
  return {
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
    setLocked: vi.fn(async (_d: string, _id: string, _l: boolean) => {}),
  };
}
const LOCKED = 0x80;

describe("MarkupStore.setLocked", () => {
  it("sets only the Locked bit, mirrors via setLocked (not update), and is one undo step", async () => {
    const i = ipc();
    const s = new MarkupStore("d", i);
    s.seed([mk("a", { annot_flags: 4 }), mk("b", { annot_flags: 4 })]);
    expect(s.setLocked(["a", "b"], true)).toBe(2);
    expect(s.getById("a")!.annot_flags).toBe(4 | LOCKED);
    await s.flush();
    expect(i.setLocked).toHaveBeenCalledWith("d", "a", true);
    expect(i.setLocked).toHaveBeenCalledWith("d", "b", true);
    expect(i.update).not.toHaveBeenCalled();
    s.undo();
    await s.flush();
    expect(s.getById("a")!.annot_flags).toBe(4);
    expect(s.getById("b")!.annot_flags).toBe(4);
    expect(i.setLocked).toHaveBeenLastCalledWith("d", "a", false);
  });

  it("unlocks a locked markup (the update guard would refuse it) and keeps other flag bits", async () => {
    const i = ipc();
    const s = new MarkupStore("d", i);
    s.seed([mk("a", { annot_flags: 4 | LOCKED | 0x200 })]);
    expect(s.setLocked(["a"], false)).toBe(1);
    expect(s.getById("a")!.annot_flags).toBe(4 | 0x200);
    await s.flush();
    expect(i.setLocked).toHaveBeenCalledWith("d", "a", false);
  });

  it("a locked markup refuses move/edit/delete until unlocked", () => {
    const s = new MarkupStore("d", ipc());
    const m = mk("a", { annot_flags: 4 | LOCKED });
    s.seed([m]);
    s.update(m, { ...m, contents: "x" });
    expect(s.getById("a")!.contents).toBeNull();
    expect(s.mirrorError).toMatch(/locked/);
    s.delete("a");
    expect(s.markups).toHaveLength(1);
    s.setLocked(["a"], false);
    const cur = s.getById("a")!;
    s.update(cur, { ...cur, contents: "x" });
    expect(s.getById("a")!.contents).toBe("x");
  });

  it("skips markups already in the target state and marks the doc dirty only on change", () => {
    const s = new MarkupStore("d", ipc());
    s.seed([mk("a", { annot_flags: 4 | LOCKED })]);
    expect(s.setLocked(["a"], true)).toBe(0);
    expect(s.dirty).toBe(false);
    expect(s.canUndo).toBe(false);
    expect(s.setLocked(["a"], false)).toBe(1);
    expect(s.dirty).toBe(true);
  });
});

describe("Lock menu item", () => {
  const ids = (allLocked: boolean) =>
    markupMenuItems({ count: 1, anyLocked: allLocked, allLocked }).flatMap((i) => (i.kind === "item" ? [i.id] : []));
  it("offers Lock on an unlocked selection and Unlock on a locked one", () => {
    expect(ids(false)).toContain("lock");
    expect(ids(false)).not.toContain("unlock");
    expect(ids(true)).toContain("unlock");
    expect(ids(true)).not.toContain("lock");
  });

  it("runMarkupCommand lock / unlock act on the selection", () => {
    const s = new MarkupStore("d", ipc());
    s.seed([mk("a", { annot_flags: 4 })]);
    s.selectedIds = new Set(["a"]);
    const c = { store: s, identity: { user_id: "u", display_name: "U" } } as unknown as ArrangeCtx;
    expect(runMarkupCommand("lock", c)).toEqual({ ok: true, message: "Locked 1 markup" });
    expect(s.getById("a")!.annot_flags).toBe(4 | LOCKED);
    expect(runMarkupCommand("unlock", c)).toEqual({ ok: true, message: "Unlocked 1 markup" });
    expect(s.getById("a")!.annot_flags).toBe(4);
  });
});
