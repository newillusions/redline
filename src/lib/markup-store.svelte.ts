/**
 * In-session source of truth for markups (spec §6/§15). Owns the reactive markup array,
 * selection + active tool, and a command-pattern History. Each committed command's
 * MirrorOp is drained through an ordered FIFO to the Rust store (the save buffer) via the
 * injected IPC. flush() awaits a full drain — App.svelte calls it before save_document.
 */
import type { Markup, Appearance, CountSet, CountSymbol } from "./ipc";
import { History, CreateCmd, UpdateCmd, DeleteCmd, type MarkupSink, type MirrorOp } from "./markup-commands";

/** Default colour rotation for newly-created count sets (distinct, legible hues). */
export const COUNT_SET_PALETTE: readonly string[] = [
  "#e02424", "#1d70b8", "#00875a", "#b8860b", "#7b2ff7", "#d63384", "#0aa2c0",
];

/**
 * Reconstruct the document's count-set definitions from loaded markups: the unique set
 * (by id) embedded on each MeasurementCount markup. The annotation is the source of truth,
 * so re-opening a saved document restores exactly the sets it was saved with.
 */
export function reconstructCountSets(markups: Markup[]): CountSet[] {
  const byId = new Map<string, CountSet>();
  for (const m of markups) {
    const cs = m.count_set;
    if (cs && !byId.has(cs.id)) byId.set(cs.id, cs);
  }
  return [...byId.values()];
}

/** The IPC surface the store mirrors to (injected for testability). */
export interface MarkupIpc {
  add(doc_id: string, m: Markup): Promise<void>;
  update(doc_id: string, m: Markup): Promise<void>;
  remove(doc_id: string, id: string): Promise<void>;
}

export type ToolKind =
  | "hand" | "select" | "Rectangle" | "Ellipse" | "Line" | "Arrow" | "Highlight"
  | "Polyline" | "Polygon" | "Cloud" | "Ink" | "Text" | "Callout"
  // I-beam text-selection tool: drag to select real PDF text (not a drawn shape).
  // Enter (with an active selection) creates a text-anchored Highlight; Ctrl/Cmd+C
  // copies the selected text. See Viewport.svelte onOverlayPointerDown/Move/Up.
  | "selectText"
  // M3 measurement tools:
  | "calibrate"
  | "MeasurementLength"
  | "MeasurementArea"
  | "MeasurementCount";

const DEFAULT_APPEARANCE: Appearance = {
  color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null,
  outline_color: null, fill_opacity: null,
};

export class MarkupStore implements MarkupSink {
  markups = $state<Markup[]>([]);
  selectedIds = $state<Set<string>>(new Set());
  activeTool = $state<ToolKind>("hand");
  draftAppearance = $state<Appearance>({ ...DEFAULT_APPEARANCE });
  mirrorError = $state<string | null>(null);
  /** True when markups have changed since the last save (or since the doc was opened). */
  dirty = $state(false);

  // --- Count sets (spec §7): document-scoped category definitions for the Count tool. ---
  countSets = $state<CountSet[]>([]);
  /** Id of the set new count markers are assigned to (null → counts go unassigned). */
  activeCountSetId = $state<string | null>(null);

  private history = new History(this);
  private queue: MirrorOp[] = [];
  private drainPromise: Promise<void> | null = null;

  constructor(private readonly docId: string, private readonly ipc: MarkupIpc) {
    // Seed one default set so the Count tool tallies into a named bucket out of the box.
    const def = this.makeCountSet("Count 1", "Circle");
    this.countSets = [def];
    this.activeCountSetId = def.id;
  }

  // --- Count sets ---

  /** The set new count markers are assigned to (or null when none is active). */
  get activeCountSet(): CountSet | null {
    return this.countSets.find((s) => s.id === this.activeCountSetId) ?? null;
  }

  /** Build a CountSet (id assigned here). Colour defaults to the next palette hue. */
  makeCountSet(name: string, symbol: CountSymbol, color?: string): CountSet {
    const hue = color ?? COUNT_SET_PALETTE[this.countSets.length % COUNT_SET_PALETTE.length];
    return { id: crypto.randomUUID(), name, color: hue, symbol };
  }

  /** Create a new count set and make it active. Returns the created set. */
  addCountSet(name: string, symbol: CountSymbol, color?: string): CountSet {
    const set = this.makeCountSet(name, symbol, color);
    this.countSets = [...this.countSets, set];
    this.activeCountSetId = set.id;
    return set;
  }

  /** Select the active count set by id (no-op if the id is unknown). */
  setActiveCountSet(id: string): void {
    if (this.countSets.some((s) => s.id === id)) this.activeCountSetId = id;
  }

  // --- MarkupSink (used by History; never enqueues — the History caller does) ---
  insert(m: Markup) { this.markups.push(m); }
  replace(m: Markup) { const i = this.markups.findIndex((x) => x.id === m.id); if (i >= 0) this.markups[i] = m; }
  removeById(id: string) {
    this.markups = this.markups.filter((x) => x.id !== id);
    const next = new Set(this.selectedIds);
    next.delete(id);
    this.selectedIds = next;
  }
  getById(id: string) { return this.markups.find((x) => x.id === id); }

  /** Mark the document as clean (call after a successful save). */
  clearDirty(): void { this.dirty = false; }

  // --- Loading (no undo entry, no mirror — the PDF already has these) ---
  seed(markups: Markup[]) {
    this.markups = markups;
    this.history = new History(this);
    this.queue = [];
    this.drainPromise = null;
    this.dirty = false;
    // Restore the count sets the document was saved with (annotation = source of truth).
    // Keep the default set available so new counts always have a bucket; prefer a restored
    // set as the active one when the document already has counts.
    const restored = reconstructCountSets(markups);
    if (restored.length > 0) {
      const ids = new Set(restored.map((s) => s.id));
      const extras = this.countSets.filter((s) => !ids.has(s.id));
      this.countSets = [...restored, ...extras];
      this.activeCountSetId = restored[0].id;
    }
  }

  // --- Mutations (undoable + mirrored) ---
  create(m: Markup) { this.dirty = true; this.enqueue(this.history.push(new CreateCmd(m))); }
  update(before: Markup, after: Markup) { this.dirty = true; this.enqueue(this.history.push(new UpdateCmd(before, after))); }
  delete(id: string) { const m = this.getById(id); if (m) { this.dirty = true; this.enqueue(this.history.push(new DeleteCmd(m))); } }

  /** Undo the last frame (which may be 1 or N commands). Each op is enqueued in order. */
  undo() { const ops = this.history.undo(); if (ops) { this.dirty = true; ops.forEach((op) => this.enqueue(op)); } }
  /** Redo the last undone frame. Each op is enqueued in order. */
  redo() { const ops = this.history.redo(); if (ops) { this.dirty = true; ops.forEach((op) => this.enqueue(op)); } }

  get canUndo() { return this.history.canUndo; }
  get canRedo() { return this.history.canRedo; }

  /** The markups that are currently selected. */
  get selectedMarkups(): Markup[] {
    return this.markups.filter((m) => this.selectedIds.has(m.id));
  }

  /**
   * Apply a batch of before/after update pairs as ONE undo frame.
   * Enqueues each update op in forward order. Empty pairs array is a no-op.
   */
  applyBatch(pairs: { before: Markup; after: Markup }[]): void {
    if (pairs.length === 0) return;
    this.dirty = true;
    const cmds = pairs.map(({ before, after }) => new UpdateCmd(before, after));
    const ops = this.history.pushBatch(cmds);
    ops.forEach((op) => this.enqueue(op));
  }

  /**
   * Delete all currently-selected markups as ONE undo frame. Clears selectedIds.
   * If nothing is selected, this is a no-op.
   */
  deleteSelected(): void {
    const targets = this.selectedMarkups;
    if (targets.length === 0) return;
    this.dirty = true;
    const cmds = targets.map((m) => new DeleteCmd(m));
    const ops = this.history.pushBatch(cmds);
    ops.forEach((op) => this.enqueue(op));
    this.selectedIds = new Set();
  }

  // --- Ordered async mirror ---
  private enqueue(op: MirrorOp) { this.queue.push(op); this.startDrain(); }

  private startDrain() {
    if (this.drainPromise) return;
    this.drainPromise = this.runDrain().finally(() => { this.drainPromise = null; });
  }

  private async runDrain(): Promise<void> {
    while (this.queue.length > 0) {
      const op = this.queue[0];
      try {
        if (op.kind === "add") await this.ipc.add(this.docId, op.markup);
        else if (op.kind === "update") await this.ipc.update(this.docId, op.markup);
        else await this.ipc.remove(this.docId, op.id);
      } catch (e) {
        this.mirrorError = `Sync failed: ${e instanceof Error ? e.message : String(e)}`;
        return; // halt; queue head stays; startDrain() inside flush() or the next enqueue will retry
      }
      this.queue.shift();
    }
    this.mirrorError = null;
  }

  /** Await a full drain of pending mirror ops (call before save). Throws if the queue
   * could not fully drain (mirror failure) so the caller refuses to save stale state. */
  async flush(): Promise<void> {
    this.startDrain();
    await this.drainPromise;
    if (this.queue.length > 0) throw new Error(this.mirrorError ?? "mirror queue not drained");
  }
}
