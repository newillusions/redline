/**
 * Command-pattern undo/redo for markup edits (spec §15 — in-session editing, distinct
 * from the durable audit trail). Pure: operates on a MarkupSink abstraction and returns
 * the backend MirrorOp each command implies, so the store can mirror it asynchronously.
 *
 * The undo/redo stacks are frame-based: each frame is a Command[] so that a multi-command
 * batch (e.g. multi-select move/delete) occupies exactly ONE undo entry while still
 * generating the full set of granular IPC ops. A single push() records a 1-command frame
 * and behaves identically to the old single-command API from the caller's perspective.
 */
import type { Markup } from "./ipc";

/** The mutable markup collection a command acts on (the reactive store implements this). */
export interface MarkupSink {
  insert(m: Markup): void;
  replace(m: Markup): void;
  removeById(id: string): void;
  getById(id: string): Markup | undefined;
}

/** A backend mirror operation implied by a command (1:1 with the granular IPC ops). */
export type MirrorOp =
  | { kind: "add"; markup: Markup }
  | { kind: "update"; markup: Markup }
  | { kind: "delete"; id: string };

export interface Command {
  apply(sink: MarkupSink): MirrorOp;
  invert(sink: MarkupSink): MirrorOp;
}

export class CreateCmd implements Command {
  constructor(private readonly markup: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.insert(this.markup); return { kind: "add", markup: this.markup }; }
  invert(sink: MarkupSink): MirrorOp { sink.removeById(this.markup.id); return { kind: "delete", id: this.markup.id }; }
}

export class UpdateCmd implements Command {
  constructor(private readonly before: Markup, private readonly after: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.replace(this.after); return { kind: "update", markup: this.after }; }
  invert(sink: MarkupSink): MirrorOp { sink.replace(this.before); return { kind: "update", markup: this.before }; }
}

export class DeleteCmd implements Command {
  constructor(private readonly markup: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.removeById(this.markup.id); return { kind: "delete", id: this.markup.id }; }
  invert(sink: MarkupSink): MirrorOp { sink.insert(this.markup); return { kind: "add", markup: this.markup }; }
}

/** Frame-based undo/redo history. Each frame is a Command[]; a single push() wraps the
 * command in a 1-element frame so the per-command API is backward-compatible. */
export class History {
  /** Each element is one undo frame (may contain 1..N commands). */
  private undoStack: Command[][] = [];
  /** Mirror of undoStack for redos. */
  private redoStack: Command[][] = [];

  constructor(private readonly sink: MarkupSink) {}

  get canUndo(): boolean { return this.undoStack.length > 0; }
  get canRedo(): boolean { return this.redoStack.length > 0; }

  /**
   * Apply a single command, record it as a 1-command frame, clear the redo stack.
   * Returns the single MirrorOp (unchanged from the original API).
   */
  push(cmd: Command): MirrorOp {
    const op = cmd.apply(this.sink);
    this.undoStack.push([cmd]);
    this.redoStack = [];
    return op;
  }

  /**
   * Apply an array of commands in forward order, record the whole array as ONE undo frame,
   * clear the redo stack. Returns the ops in forward order.
   * An empty array returns [] and records no frame.
   */
  pushBatch(cmds: Command[]): MirrorOp[] {
    if (cmds.length === 0) return [];
    const ops = cmds.map((c) => c.apply(this.sink));
    this.undoStack.push([...cmds]);
    this.redoStack = [];
    return ops;
  }

  /**
   * Pop the last undo frame, invert its commands in REVERSE order, push the frame onto the
   * redo stack. Returns the ops array (reverse-command order). null if nothing to undo.
   */
  undo(): MirrorOp[] | null {
    const frame = this.undoStack.pop();
    if (!frame) return null;
    const ops: MirrorOp[] = [];
    for (let i = frame.length - 1; i >= 0; i--) {
      ops.push(frame[i].invert(this.sink));
    }
    this.redoStack.push(frame);
    return ops;
  }

  /**
   * Pop the last redo frame, apply its commands in FORWARD order, push onto undo stack.
   * Returns the ops array (forward order). null if nothing to redo.
   */
  redo(): MirrorOp[] | null {
    const frame = this.redoStack.pop();
    if (!frame) return null;
    const ops = frame.map((c) => c.apply(this.sink));
    this.undoStack.push(frame);
    return ops;
  }
}
