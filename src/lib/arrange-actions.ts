/**
 * Copy / cut / paste / duplicate / Multiply / align / distribute / order as store commands
 * (docs/arrange.md). The pure planning lives in `markup-copy.ts` and `markup-arrange.ts`; this
 * module reads the selection, calls the planner and applies the result to the `MarkupStore` as
 * ONE undo step. Every command returns `{ ok, message }` so the Arrange bar and the keyboard
 * shortcuts report the same thing; nothing here touches the DOM.
 */
import { isMarkupContentsLocked, isMarkupLocked, type Markup, type PdfPoint, type ScaleRecord, type UserRef } from "./ipc";
import type { MarkupStore } from "./markup-store.svelte";
import { markupClipboard } from "./markup-clipboard.svelte";
import {
  CASCADE_OFFSET_PT, MAX_COPY_ITEMS, copySet, countSetsOf, isUsableMarkup, joinCounts, newCounts, planPaste, unionBounds,
} from "./markup-copy";
import {
  alignMoves, applyMoves, buildUnits, distributeMoves, multiplyOffsets, orderChanges,
  type AlignMode, type DistributeAxis, type MultiplySpec, type OrderOp,
} from "./markup-arrange";
import { countGroups } from "./count-groups";
import { bumpAudit } from "./markup-tools";
import { effectiveScale } from "./polylength";

export interface ArrangeCtx {
  store: MarkupStore;
  identity: UserRef | null;
  /** Scales of the document being edited (page scale first, else document default). */
  scales: readonly ScaleRecord[];
  pageIndex: number;
  pageCount: number;
  pageWidthPts: number;
  pageHeightPts: number;
  /** Pointer position on the viewed page in PDF pt, or null when it is not over the page. */
  cursor: PdfPoint | null;
  /** Centre of what is on screen in PDF pt, or null. */
  viewCenter: PdfPoint | null;
  now?: () => string;
  mintId?: () => string;
}

export interface ActionResult {
  ok: boolean;
  message: string;
}

const done = (message: string): ActionResult => ({ ok: true, message });
const refuse = (message: string): ActionResult => ({ ok: false, message });
const noun = (n: number) => `${n} markup${n === 1 ? "" : "s"}`;
const isLocked = (m: Markup) => isMarkupLocked(m) || isMarkupContentsLocked(m);

const clock = (c: ArrangeCtx) => (c.now ?? (() => new Date().toISOString()))();
const mint = (c: ArrangeCtx) => c.mintId ?? (() => crypto.randomUUID());

/** The selected markups in the order they were selected (the first is the primary). */
function selectionInOrder(store: MarkupStore): Markup[] {
  const out: Markup[] = [];
  for (const id of store.selectedIds) {
    const m = store.getById(id);
    if (m) out.push(m);
  }
  return out;
}

/** The same markups in document order (z-order), bottom first. */
function inDocumentOrder(store: MarkupStore, ms: readonly Markup[]): Markup[] {
  const ids = new Set(ms.map((m) => m.id));
  return store.markups.filter((m) => ids.has(m.id));
}

function select(store: MarkupStore, ms: readonly Markup[]): void {
  store.selectedIds = new Set(ms.map((m) => m.id));
}

// --- copy / cut ------------------------------------------------------------------------

export function copySelection(c: ArrangeCtx): ActionResult {
  const sel = inDocumentOrder(c.store, selectionInOrder(c.store));
  if (sel.length === 0) return refuse("Select a markup to copy.");
  markupClipboard.set(sel, c.store.docId);
  return done(`Copied ${noun(sel.length)}`);
}

/** Cut = copy + delete, one undo step. Locked markups are neither copied nor deleted (spec C15). */
export function cutSelection(c: ArrangeCtx): ActionResult {
  const all = selectionInOrder(c.store);
  if (all.length === 0) return refuse("Select a markup to cut.");
  const movable = all.filter((m) => !isLocked(m));
  if (movable.length === 0) return refuse("The selected markups are locked, so they cannot be cut.");
  markupClipboard.set(inDocumentOrder(c.store, movable), c.store.docId);
  select(c.store, movable);
  c.store.deleteSelected();
  const skipped = all.length - movable.length;
  return done(`Cut ${noun(movable.length)}${skipped > 0 ? `; ${skipped} locked left in place` : ""}`);
}

// --- paste -----------------------------------------------------------------------------

export type PasteMode = "cursor" | "center" | "inplace";

function commitCreates(c: ArrangeCtx, creates: Markup[]): void {
  c.store.adoptCountSets(countSetsOf(creates));
  c.store.createMany(creates);
  select(c.store, creates);
}

/**
 * Paste the clipboard onto the viewed page. `cursor`: the lower-left of the clipboard's bounding
 * box lands on the pointer (Revu, spec C3); `center`: centred in the view (the Paste button);
 * `inplace`: the same coordinates. With no usable position, a keyboard paste falls back to the
 * source position nudged by `CASCADE_OFFSET_PT` per repeat so it never hides under its source
 * (spec C10).
 */
export function pasteClipboard(c: ArrangeCtx, mode: PasteMode): ActionResult {
  const clip = markupClipboard.payload;
  if (!clip) return refuse("Nothing to paste. Copy or cut a markup first.");
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const usable = clip.items.filter(isUsableMarkup);
  const box = unionBounds(usable);
  if (!box) return refuse("The clipboard content is not usable.");

  let dx = 0;
  let dy = 0;
  if (mode === "cursor" && c.cursor) {
    dx = c.cursor.x - box.minX;
    dy = c.cursor.y - box.minY;
  } else if (mode === "center" && c.viewCenter) {
    dx = c.viewCenter.x - (box.minX + box.maxX) / 2;
    dy = c.viewCenter.y - (box.minY + box.maxY) / 2;
  } else if (mode !== "inplace") {
    const overSource = clip.sourceDocId === c.store.docId && usable.every((m) => m.page === c.pageIndex);
    const n = clip.pastes + (overSource ? 1 : 0);
    dx = CASCADE_OFFSET_PT * n;
    dy = -CASCADE_OFFSET_PT * n;
  }
  return pasteInto(c, [c.pageIndex], dx, dy);
}

/** Paste the clipboard in place onto each of `pages` (0-based) as one undo step (spec C9). */
export function pasteToPages(c: ArrangeCtx, pages: readonly number[]): ActionResult {
  if (!markupClipboard.payload) return refuse("Nothing to paste. Copy or cut a markup first.");
  return pasteInto(c, [...new Set(pages)].sort((a, b) => a - b), 0, 0);
}

function pasteInto(c: ArrangeCtx, pages: readonly number[], dx: number, dy: number): ActionResult {
  const clip = markupClipboard.payload;
  if (!clip) return refuse("Nothing to paste. Copy or cut a markup first.");
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const plan = planPaste({
    identity: c.identity, now: clock(c), mintId: mint(c),
    items: clip.items, sourceDocId: clip.sourceDocId, targetDocId: c.store.docId,
    pages, pageCount: c.pageCount, scales: c.scales, dx, dy,
  });
  if (plan.error) return refuse(plan.error);
  commitCreates(c, plan.creates);
  markupClipboard.notePasted();
  const where = pages.length > 1 ? ` onto ${pages.length} pages` : "";
  const skipped = plan.skipped > 0 ? `; ${plan.skipped} unusable item${plan.skipped === 1 ? "" : "s"} skipped` : "";
  return done(`Pasted ${noun(plan.creates.length)}${where}${skipped}`);
}

// --- duplicate -------------------------------------------------------------------------

/** Copy the selection 12 pt right and 12 pt down on its own page(s); the clipboard is untouched (spec C11). */
export function duplicateSelection(c: ArrangeCtx): ActionResult {
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const sel = inDocumentOrder(c.store, selectionInOrder(c.store)).filter(isUsableMarkup);
  if (sel.length === 0) return refuse("Select a markup to duplicate.");
  if (sel.length > MAX_COPY_ITEMS) return refuse(`At most ${MAX_COPY_ITEMS} markups at a time.`);
  const countGroupOf = newCounts(mint(c));
  const creates: Markup[] = [];
  for (const page of [...new Set(sel.map((m) => m.page))]) {
    creates.push(
      ...copySet(sel.filter((m) => m.page === page), {
        identity: c.identity, now: clock(c), mintId: mint(c), page,
        dx: CASCADE_OFFSET_PT, dy: -CASCADE_OFFSET_PT, countGroupOf,
      }),
    );
  }
  commitCreates(c, creates);
  return done(`Duplicated ${noun(creates.length)}`);
}

// --- multiply --------------------------------------------------------------------------

/**
 * Repeat the selection per `spec` (spec section 2). The selection must sit on one page; copies
 * that would leave the page are not made. One undo step. Count items join the source's count.
 */
export function multiplySelection(c: ArrangeCtx, spec: MultiplySpec): ActionResult {
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const sel = inDocumentOrder(c.store, selectionInOrder(c.store)).filter(isUsableMarkup);
  if (sel.length === 0) return refuse("Select a markup to multiply.");
  const page = sel[0].page;
  if (sel.some((m) => m.page !== page)) return refuse("Multiply works on markups from one page at a time.");
  const box = unionBounds(sel)!;
  const r = multiplyOffsets(spec, box, { w: c.pageWidthPts, h: c.pageHeightPts });
  if (r.error) return refuse(r.error);
  if (r.offsets.length === 0) return refuse("No copy fits on the page at that distance.");
  if (r.offsets.length * sel.length > MAX_COPY_ITEMS) {
    return refuse(`That would create ${r.offsets.length * sel.length} markups; the limit is ${MAX_COPY_ITEMS}.`);
  }
  // An ungrouped count is "a count of one" whose key is its own id. The copies join that key, so
  // the source must carry it too (as Resume does): otherwise the frontend sees one count of N+1
  // while the Rust totals, exports and Markups List see 1 + N (review round 1, F1). A locked
  // source cannot take the stamp, so the whole Multiply is refused.
  const stamps = sel.filter((m) => m.markup_type === "MeasurementCount" && !m.count_group);
  if (stamps.some(isLocked)) return refuse("A selected count is locked, so its count cannot be extended. Unlock it first.");
  const now = clock(c);
  const updates = stamps.map((m) => ({ before: m, after: bumpAudit({ ...m, count_group: m.id }, c.identity!, now) }));
  const sizes = new Map(countGroups(c.store.markups).map((g) => [g.key, g.ids.length]));
  const countGroupOf = joinCounts(sizes, mint(c));
  const creates: Markup[] = [];
  for (const o of r.offsets) {
    creates.push(
      ...copySet(sel, { identity: c.identity, now, mintId: mint(c), page, dx: o.dx, dy: o.dy, countGroupOf }),
    );
  }
  c.store.adoptCountSets(countSetsOf(creates));
  c.store.applyEdits({ updates, creates }); // stamps and copies: ONE undo step
  select(c.store, creates);
  const clipped = r.clipped > 0 ? `; ${r.clipped} more would not fit on the page` : "";
  return done(`Made ${noun(creates.length)}${clipped}`);
}

/** The scale that applies to the viewed page, for the Multiply dialog's unit list. */
export function pageScale(c: Pick<ArrangeCtx, "scales" | "pageIndex">): ScaleRecord | null {
  return effectiveScale([...c.scales], c.pageIndex);
}

// --- align / distribute / order --------------------------------------------------------

function movedMessage(verb: string, units: number, lockedSkipped: number): string {
  return `${verb} ${units} item${units === 1 ? "" : "s"}${lockedSkipped > 0 ? `; ${lockedSkipped} locked left in place` : ""}`;
}

export function alignSelection(c: ArrangeCtx, mode: AlignMode): ActionResult {
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const units = buildUnits(selectionInOrder(c.store), c.store.markups);
  if (units.length < 2) return refuse("Select two or more markups to align.");
  const moves = alignMoves(units, mode);
  if (moves.length === 0) return done("Already aligned");
  const { pairs, lockedSkipped } = applyMoves(moves, c.identity, clock(c));
  if (pairs.length === 0) return refuse("The markups that would move are locked.");
  c.store.applyBatch(pairs);
  return done(movedMessage("Aligned", moves.length - lockedSkipped, lockedSkipped));
}

export function distributeSelection(c: ArrangeCtx, axis: DistributeAxis): ActionResult {
  if (!c.identity) return refuse("The user identity is not loaded yet.");
  const units = buildUnits(selectionInOrder(c.store), c.store.markups);
  if (units.length < 3) return refuse("Select three or more markups to distribute.");
  const moves = distributeMoves(units, axis);
  if (moves.length === 0) return done("Already evenly spaced");
  const { pairs, lockedSkipped } = applyMoves(moves, c.identity, clock(c));
  if (pairs.length === 0) return refuse("The markups that would move are locked.");
  c.store.applyBatch(pairs);
  return done(movedMessage("Distributed", moves.length - lockedSkipped, lockedSkipped));
}

export function orderSelection(c: ArrangeCtx, op: OrderOp): ActionResult {
  if (c.store.selectedIds.size === 0) return refuse("Select a markup to re-order.");
  const { changes, lockedSkipped } = orderChanges(c.store.markups, c.store.selectedIds, op);
  if (changes.length === 0) {
    return lockedSkipped > 0 ? refuse("The selected markups are locked.") : done("Already there");
  }
  c.store.applyReorder(changes.map(({ before, after }) => ({ before, after })));
  return done(lockedSkipped > 0 ? `Re-ordered; ${lockedSkipped} locked left in place` : "Re-ordered");
}

/** Page numbers (0-based) named by a typed spec such as `1-3, 5` (1-based, clamped to the document). */
export function pagesFromRanges(ranges: readonly [number, number][], pageCount: number): number[] {
  const out = new Set<number>();
  for (const [a, b] of ranges) {
    for (let p = a; p <= Math.min(b, pageCount) && out.size <= MAX_COPY_ITEMS; p++) out.add(p - 1);
  }
  return [...out].sort((x, y) => x - y);
}
