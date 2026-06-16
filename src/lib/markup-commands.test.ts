import { describe, it, expect } from "vitest";
import { History, CreateCmd, UpdateCmd, DeleteCmd, type MarkupSink, type MirrorOp } from "./markup-commands";
import type { Markup } from "./ipc";

// Minimal in-memory sink standing in for the reactive store.
class ArraySink implements MarkupSink {
  list: Markup[] = [];
  insert(m: Markup) { this.list.push(m); }
  replace(m: Markup) { const i = this.list.findIndex((x) => x.id === m.id); if (i >= 0) this.list[i] = m; }
  removeById(id: string) { this.list = this.list.filter((x) => x.id !== id); }
  getById(id: string) { return this.list.find((x) => x.id === id); }
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

describe("History undo/redo", () => {
  it("CreateCmd applies, undo removes, redo re-adds — and emits add/delete ops", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    const m = mk("a");

    const ops: MirrorOp[] = [];
    ops.push(h.push(new CreateCmd(m)));
    expect(sink.list.length).toBe(1);
    expect(ops[0]).toEqual({ kind: "add", markup: m });

    const [undoOp] = h.undo()!;
    expect(sink.list.length).toBe(0);
    expect(undoOp).toEqual({ kind: "delete", id: "a" });

    const [redoOp] = h.redo()!;
    expect(sink.list.length).toBe(1);
    expect(redoOp).toEqual({ kind: "add", markup: m });
  });

  it("UpdateCmd swaps before<->after on undo/redo with update ops", () => {
    const sink = new ArraySink();
    const before = mk("a", "old");
    sink.insert(before);
    const h = new History(sink);
    const after = mk("a", "new");

    const op = h.push(new UpdateCmd(before, after));
    expect(sink.getById("a")!.contents).toBe("new");
    expect(op).toEqual({ kind: "update", markup: after });

    const [undoOp] = h.undo()!;
    expect(sink.getById("a")!.contents).toBe("old");
    expect(undoOp).toEqual({ kind: "update", markup: before });
  });

  it("DeleteCmd removes, undo restores — delete/add ops", () => {
    const sink = new ArraySink();
    const m = mk("a");
    sink.insert(m);
    const h = new History(sink);

    const op = h.push(new DeleteCmd(m));
    expect(sink.list.length).toBe(0);
    expect(op).toEqual({ kind: "delete", id: "a" });

    const [undoOp] = h.undo()!;
    expect(sink.list.length).toBe(1);
    expect(undoOp).toEqual({ kind: "add", markup: m });
  });

  it("a fresh push clears the redo stack", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    h.push(new CreateCmd(mk("a")));
    h.undo();
    expect(h.canRedo).toBe(true);
    h.push(new CreateCmd(mk("b")));
    expect(h.canRedo).toBe(false);
  });

  it("undo/redo at the ends are no-ops returning null", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    expect(h.undo()).toBeNull();
    expect(h.redo()).toBeNull();
  });
});

describe("History pushBatch", () => {
  it("pushBatch of 2 commands applies both and records ONE undo frame", () => {
    const sink = new ArraySink();
    const a = mk("a", "before-a");
    const b = mk("b", "before-b");
    sink.insert(a);
    sink.insert(b);
    const h = new History(sink);

    const ops = h.pushBatch([
      new UpdateCmd(a, mk("a", "after-a")),
      new UpdateCmd(b, mk("b", "after-b")),
    ]);

    expect(ops).toHaveLength(2);
    expect(ops[0]).toEqual({ kind: "update", markup: mk("a", "after-a") });
    expect(ops[1]).toEqual({ kind: "update", markup: mk("b", "after-b") });
    expect(sink.getById("a")!.contents).toBe("after-a");
    expect(sink.getById("b")!.contents).toBe("after-b");
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
  });

  it("undo() after pushBatch reverts both commands in reverse order and returns both ops", () => {
    const sink = new ArraySink();
    const a = mk("a", "before-a");
    const b = mk("b", "before-b");
    sink.insert(a);
    sink.insert(b);
    const h = new History(sink);

    h.pushBatch([
      new UpdateCmd(a, mk("a", "after-a")),
      new UpdateCmd(b, mk("b", "after-b")),
    ]);

    const undoOps = h.undo()!;
    expect(undoOps).toHaveLength(2);
    // reverse order: b first, then a
    expect(undoOps[0]).toEqual({ kind: "update", markup: b });
    expect(undoOps[1]).toEqual({ kind: "update", markup: a });
    expect(sink.getById("a")!.contents).toBe("before-a");
    expect(sink.getById("b")!.contents).toBe("before-b");
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(true);
  });

  it("redo() after undo re-applies both commands in forward order", () => {
    const sink = new ArraySink();
    const a = mk("a", "before-a");
    const b = mk("b", "before-b");
    sink.insert(a);
    sink.insert(b);
    const h = new History(sink);

    h.pushBatch([
      new UpdateCmd(a, mk("a", "after-a")),
      new UpdateCmd(b, mk("b", "after-b")),
    ]);
    h.undo();

    const redoOps = h.redo()!;
    expect(redoOps).toHaveLength(2);
    expect(redoOps[0]).toEqual({ kind: "update", markup: mk("a", "after-a") });
    expect(redoOps[1]).toEqual({ kind: "update", markup: mk("b", "after-b") });
    expect(sink.getById("a")!.contents).toBe("after-a");
    expect(sink.getById("b")!.contents).toBe("after-b");
  });

  it("empty pushBatch records no frame and canUndo is unchanged", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    const ops = h.pushBatch([]);
    expect(ops).toEqual([]);
    expect(h.canUndo).toBe(false);
  });

  it("pushBatch clears the redo stack", () => {
    const sink = new ArraySink();
    const a = mk("a", "v1");
    sink.insert(a);
    const h = new History(sink);
    h.push(new UpdateCmd(a, mk("a", "v2")));
    h.undo();
    expect(h.canRedo).toBe(true);

    const b = mk("b", "x");
    sink.insert(b);
    h.pushBatch([new UpdateCmd(a, mk("a", "v3")), new UpdateCmd(b, mk("b", "y"))]);
    expect(h.canRedo).toBe(false);
  });
});
