/**
 * Pure helpers for the Navigator (bookmarks, page labels, links): labels for display, the
 * bookmark tree, move arithmetic, targets, link hit-testing, page-range choices.
 * No Tauri, no DOM: unit-tested in navigation.test.ts. Behaviour spec: docs/navigation.md
 * "Document structure".
 *
 * Every string shown here comes from a PDF and is untrusted: it is only ever rendered as
 * text (`{...}` interpolation), never as HTML.
 */
import type {
  BookmarkView,
  LabelStyle,
  LinkView,
  NavTarget,
  PageView,
  Place,
  TargetInput,
} from "./navigation-ipc";
import { parsePageSpec } from "./legend";
import type { Markup } from "./ipc";
import { boundsOf } from "./markup-select";

// ---------------------------------------------------------------------------
// Page labels
// ---------------------------------------------------------------------------

/** The label of page `i` (0-based), or "" when it has none. */
export function labelOf(labels: readonly string[], i: number): string {
  const l = labels[i];
  return typeof l === "string" ? l : "";
}

/** What to call a page: its label, else its 1-based number. */
export function pageText(labels: readonly string[], i: number): string {
  const l = labelOf(labels, i).trim();
  return l !== "" ? l : String(i + 1);
}

/** True when the document has at least one page label. */
export function hasLabels(labels: readonly string[]): boolean {
  return labels.some((l) => l.trim() !== "");
}

/**
 * Resolve what the user typed in a page box: an exact page label (case-insensitive, first
 * match) wins; otherwise a 1-based page number. So in a set labelled `E-101`... typing
 * `E-101` or `1` both reach the first sheet, and in a set whose labels are plain numbers the
 * label, not the position, is what the number means.
 */
export function resolvePageInput(
  raw: string,
  labels: readonly string[],
  pageCount: number,
): number | null {
  const text = raw.trim();
  if (text === "" || pageCount <= 0) return null;
  const lower = text.toLowerCase();
  const byLabel = labels.findIndex((l, i) => i < pageCount && l.trim().toLowerCase() === lower);
  if (byLabel >= 0) return byLabel;
  if (!/^\d+$/.test(text)) return null;
  const n = Number(text);
  if (!Number.isFinite(n) || n < 1 || n > pageCount) return null;
  return n - 1;
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

/** Short text for a target, for lists and tooltips. */
export function targetText(t: NavTarget, labels: readonly string[] = []): string {
  switch (t.kind) {
    case "none": return "No action";
    case "page": return `Page ${pageText(labels, t.page)}`;
    case "uri": return t.uri;
    case "blocked": return `Blocked address (${t.reason})`;
    case "broken": return "Broken: the page no longer exists";
    case "unsupported": return `Not supported (${t.action})`;
  }
}

/** True when redline can act on the target (go to the page / ask to open the address). */
export function isFollowable(t: NavTarget): boolean {
  return t.kind === "page" || t.kind === "uri";
}

export interface JumpTarget {
  page: number;
  /** Centre the viewport on this rectangle (page space `[l, b, r, t]`). */
  rect?: [number, number, number, number];
  /** Place this point at the top-left of the view, at `zoom` when given. */
  view?: { left: number; top: number; zoom: number | null };
}

/** The viewport request for a page target. A zoom-less position just centres on the point. */
export function jumpFor(t: NavTarget): JumpTarget | null {
  if (t.kind !== "page") return null;
  const v = t.view;
  if (v.kind === "xyz" && v.left !== null && v.top !== null) {
    return { page: t.page, view: { left: v.left, top: v.top, zoom: v.zoom } };
  }
  if (v.kind === "fit_r") {
    return { page: t.page, rect: [v.left, v.bottom, v.right, v.top] };
  }
  return { page: t.page };
}

/** The view choices offered when creating a destination. */
export type ViewChoice = "inherit" | "fit_page" | "fit_width" | "actual_size";

export const VIEW_CHOICES: readonly { value: ViewChoice; label: string }[] = [
  { value: "inherit", label: "Inherit zoom" },
  { value: "fit_page", label: "Fit page" },
  { value: "fit_width", label: "Fit width" },
  { value: "actual_size", label: "Actual size" },
];

export function viewFromChoice(c: ViewChoice): PageView {
  switch (c) {
    case "inherit": return { kind: "xyz", left: null, top: null, zoom: null };
    case "fit_page": return { kind: "fit" };
    case "fit_width": return { kind: "fit_h", top: null };
    case "actual_size": return { kind: "xyz", left: null, top: null, zoom: 1 };
  }
}

/** The choice closest to an existing view, for pre-filling a dialog. */
export function choiceOf(v: PageView): ViewChoice {
  if (v.kind === "fit" || v.kind === "fit_b") return "fit_page";
  if (v.kind === "fit_h" || v.kind === "fit_b_h") return "fit_width";
  if (v.kind === "xyz" && v.zoom === 1) return "actual_size";
  return "inherit";
}

/** The current view as a destination: the top-left of what is on screen, at this zoom. */
export function currentViewDest(
  page: number,
  zoom: number,
  scrollX: number,
  scrollY: number,
  pageHeightPts: number,
): TargetInput {
  const z = zoom > 0 && Number.isFinite(zoom) ? zoom : 1;
  return {
    kind: "page",
    page,
    view: {
      kind: "xyz",
      left: round2(Math.max(0, scrollX) / z),
      top: round2(pageHeightPts - Math.max(0, scrollY) / z),
      zoom: round2(z * 1000) / 1000,
    },
  };
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

// ---------------------------------------------------------------------------
// Bookmark tree
// ---------------------------------------------------------------------------

export function findBookmark(nodes: readonly BookmarkView[], id: string): BookmarkView | null {
  for (const n of nodes) {
    if (n.id === id) return n;
    if (id.startsWith(n.id + ".")) {
      const hit = findBookmark(n.children, id);
      if (hit) return hit;
    }
  }
  return null;
}

/** The parent's id, or null for a top-level bookmark. */
export function parentId(id: string): string | null {
  const i = id.lastIndexOf(".");
  return i < 0 ? null : id.slice(0, i);
}

/** Sibling list that holds `id`. */
export function siblingsOf(nodes: readonly BookmarkView[], id: string): readonly BookmarkView[] {
  const p = parentId(id);
  if (p === null) return nodes;
  return findBookmark(nodes, p)?.children ?? [];
}

function indexIn(id: string): number {
  return Number(id.slice(id.lastIndexOf(".") + 1));
}

export type MoveOp = "up" | "down" | "indent" | "outdent";

export interface MoveSpec {
  anchor: BookmarkView;
  place: Place;
}

/**
 * Where a node goes for a Move Up / Down / Indent / Outdent, as an anchor and a place, or
 * null when the move is impossible (first/last sibling, a top-level outdent).
 * Indent makes it the last child of its previous sibling; outdent a sibling after its parent.
 */
export function moveSpec(nodes: readonly BookmarkView[], id: string, op: MoveOp): MoveSpec | null {
  const sibs = siblingsOf(nodes, id);
  const i = indexIn(id);
  switch (op) {
    case "up":
      return i > 0 ? { anchor: sibs[i - 1], place: "before" } : null;
    case "down":
      return i + 1 < sibs.length ? { anchor: sibs[i + 1], place: "after" } : null;
    case "indent":
      return i > 0 ? { anchor: sibs[i - 1], place: "child" } : null;
    case "outdent": {
      const p = parentId(id);
      if (p === null) return null;
      const parent = findBookmark(nodes, p);
      return parent ? { anchor: parent, place: "after" } : null;
    }
  }
}

export interface DropSpec extends MoveSpec {}

/** Drag-and-drop: the upper quarter of a row drops before it, the lower quarter after, the
 *  middle makes the dragged node its child. A drop onto itself or its own subtree is null. */
export function dropSpec(
  nodes: readonly BookmarkView[],
  dragId: string,
  overId: string,
  fraction: number,
): DropSpec | null {
  if (overId === dragId || overId.startsWith(dragId + ".")) return null;
  const over = findBookmark(nodes, overId);
  if (!over) return null;
  const place: Place = fraction < 0.25 ? "before" : fraction > 0.75 ? "after" : "child";
  return { anchor: over, place };
}

/** Ids of every bookmark that has children and is open in the file. */
export function openIds(nodes: readonly BookmarkView[]): Set<string> {
  const out = new Set<string>();
  const walk = (ns: readonly BookmarkView[]) => {
    for (const n of ns) {
      if (n.children.length > 0 && n.open) out.add(n.id);
      walk(n.children);
    }
  };
  walk(nodes);
  return out;
}

/** Visible rows for the panel: a preorder walk skipping the children of collapsed nodes. */
export interface BookmarkRow {
  node: BookmarkView;
  depth: number;
  expanded: boolean;
}

export function visibleRows(nodes: readonly BookmarkView[], expanded: ReadonlySet<string>): BookmarkRow[] {
  const out: BookmarkRow[] = [];
  const walk = (ns: readonly BookmarkView[], depth: number) => {
    for (const n of ns) {
      const isOpen = n.children.length > 0 && expanded.has(n.id);
      out.push({ node: n, depth, expanded: isOpen });
      if (isOpen) walk(n.children, depth + 1);
    }
  };
  walk(nodes, 0);
  return out;
}

/**
 * Expansion state after the tree was replaced by an edit: keep what the user opened, drop
 * ids that no longer have children, and open nodes the file now says are open that were not
 * before (an added child opens its parent).
 */
export function reconcileExpanded(
  prev: readonly BookmarkView[] | null,
  next: readonly BookmarkView[],
  expanded: ReadonlySet<string>,
): Set<string> {
  const out = new Set<string>();
  const prevOpen = prev ? openIds(prev) : new Set<string>();
  const walk = (ns: readonly BookmarkView[]) => {
    for (const n of ns) {
      if (n.children.length > 0) {
        if (expanded.has(n.id) || (n.open && !prevOpen.has(n.id))) out.add(n.id);
      }
      walk(n.children);
    }
  };
  walk(next);
  return out;
}

/** Ids of the bookmarks whose target is `page`. */
export function bookmarksOnPage(nodes: readonly BookmarkView[], page: number): Set<string> {
  const out = new Set<string>();
  const walk = (ns: readonly BookmarkView[]) => {
    for (const n of ns) {
      if (n.target.kind === "page" && n.target.page === page) out.add(n.id);
      walk(n.children);
    }
  };
  walk(nodes);
  return out;
}

/** The id the new bookmark will have after `bookmark_add` with this anchor and place. */
export function predictNewId(
  nodes: readonly BookmarkView[],
  anchorId: string | null,
  place: Place,
): string {
  if (anchorId === null) return place === "before" ? "0" : String(nodes.length);
  const join = (parent: string | null, i: number) => (parent === null ? String(i) : `${parent}.${i}`);
  if (place === "child") {
    return join(anchorId, findBookmark(nodes, anchorId)?.children.length ?? 0);
  }
  const i = indexIn(anchorId);
  return join(parentId(anchorId), place === "before" ? i : i + 1);
}

/** Ancestors of `id`, outermost first. */
export function ancestorIds(id: string): string[] {
  const parts = id.split(".");
  const out: string[] = [];
  for (let i = 1; i < parts.length; i++) out.push(parts.slice(0, i).join("."));
  return out;
}

/** Default title for a bookmark made for `page`: its label, else `Page N`. */
export function defaultBookmarkTitle(labels: readonly string[], page: number): string {
  const l = labelOf(labels, page).trim();
  return l !== "" ? l : `Page ${page + 1}`;
}

/** A bookmark for a mark-up: titled with its subject (else its comment, else its type),
 *  pointing at the page and a box around it (never smaller than `MIN_BOOKMARK_BOX` points, so
 *  a point mark-up does not make another viewer zoom in absurdly). */
export const MIN_BOOKMARK_BOX = 120;

export function bookmarkForMarkup(m: Markup): { title: string; target: TargetInput } {
  const clip = (t: string | null | undefined) => (t ?? "").replace(/\s+/g, " ").trim().slice(0, 80);
  const title = clip(m.subject) || clip(m.contents) || m.markup_type;
  const b = boundsOf(m);
  const grow = (lo: number, hi: number): [number, number] => {
    const pad = Math.max(12, (MIN_BOOKMARK_BOX - (hi - lo)) / 2);
    return [lo - pad, hi + pad];
  };
  const [left, right] = grow(b.minX, b.maxX);
  const [bottom, top] = grow(b.minY, b.maxY);
  return {
    title,
    target: { kind: "page", page: m.page, view: { kind: "fit_r", left, bottom, right, top } },
  };
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

export function linksOnPage(links: readonly LinkView[], page: number): LinkView[] {
  return links.filter((l) => l.page === page);
}

/** The topmost (last-listed) link containing the page-space point, or null. */
export function linkAt(links: readonly LinkView[], x: number, y: number): LinkView | null {
  for (let i = links.length - 1; i >= 0; i--) {
    const [l, b, r, t] = links[i].rect;
    if (x >= l && x <= r && y >= b && y <= t) return links[i];
  }
  return null;
}

/** A normalised `[left, bottom, right, top]` from two page-space corners. */
export function rectFromCorners(
  a: { x: number; y: number },
  b: { x: number; y: number },
): [number, number, number, number] {
  return [Math.min(a.x, b.x), Math.min(a.y, b.y), Math.max(a.x, b.x), Math.max(a.y, b.y)];
}

/** Smallest side, in page units, a drawn link area must have. */
export const MIN_LINK_SIDE = 2;

export function rectIsUsable(r: readonly [number, number, number, number]): boolean {
  return r.every(Number.isFinite) && r[2] - r[0] >= MIN_LINK_SIDE && r[3] - r[1] >= MIN_LINK_SIDE;
}

/** Links filtered by a text query over the page text and target text (case-insensitive). */
export function filterLinks(
  links: readonly LinkView[],
  query: string,
  labels: readonly string[],
): LinkView[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [...links];
  return links.filter((l) =>
    `${pageText(labels, l.page)} ${targetText(l.target, labels)}`.toLowerCase().includes(q),
  );
}

/** Centre of a link rectangle, for jumping to it. */
export function linkJump(l: LinkView): JumpTarget {
  return { page: l.page, rect: l.rect };
}

// ---------------------------------------------------------------------------
// Page ranges (Number Pages, Create bookmarks / labels)
// ---------------------------------------------------------------------------

export type RangeMode = "all" | "current" | "selected" | "custom";

/**
 * Pages (0-based, ascending, unique) for a range choice. `custom` takes `1-3, 5, 9` in page
 * numbers; null when it cannot be read or reaches outside the document.
 */
export function pagesForRange(
  mode: RangeMode,
  pageCount: number,
  current: number | null,
  selected: readonly number[],
  custom: string,
): number[] | null {
  if (pageCount <= 0) return null;
  switch (mode) {
    case "all":
      return Array.from({ length: pageCount }, (_, i) => i);
    case "current":
      return current !== null && current >= 0 && current < pageCount ? [current] : null;
    case "selected": {
      const s = [...new Set(selected)].filter((p) => p >= 0 && p < pageCount).sort((a, b) => a - b);
      return s.length > 0 ? s : null;
    }
    case "custom": {
      const ranges = parsePageSpec(custom);
      if (!ranges) return null;
      const set = new Set<number>();
      for (const [a, b] of ranges) {
        if (b > pageCount) return null;
        for (let p = a; p <= b; p++) set.add(p - 1);
      }
      return [...set].sort((x, y) => x - y);
    }
  }
}

export const LABEL_STYLES: readonly { value: LabelStyle | "none"; label: string }[] = [
  { value: "none", label: "None (text only)" },
  { value: "D", label: "1, 2, 3" },
  { value: "R", label: "I, II, III" },
  { value: "r", label: "i, ii, iii" },
  { value: "A", label: "A, B, C" },
  { value: "a", label: "a, b, c" },
];

/** A preview of the labels Number Pages would give, for the first few pages of a range. */
export function previewLabels(
  style: LabelStyle | null,
  prefix: string,
  start: number,
  count: number,
): string[] {
  const out: string[] = [];
  for (let k = 0; k < count; k++) out.push(prefix + (style ? formatLabelNumber(style, start + k) : ""));
  return out;
}

export function formatLabelNumber(style: LabelStyle, n: number): string {
  const v = Math.max(1, Math.floor(n));
  const roman = (x: number, upper: boolean): string | null => {
    if (x > 3999) return null;
    const table: [number, string][] = [
      [1000, "M"], [900, "CM"], [500, "D"], [400, "CD"], [100, "C"], [90, "XC"],
      [50, "L"], [40, "XL"], [10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"],
    ];
    let s = "";
    let r = x;
    for (const [val, sym] of table) {
      while (r >= val) { s += sym; r -= val; }
    }
    return upper ? s : s.toLowerCase();
  };
  const alpha = (x: number, upper: boolean): string | null => {
    const repeat = Math.floor((x - 1) / 26) + 1;
    if (repeat > 8) return null;
    const ch = String.fromCharCode((upper ? 65 : 97) + ((x - 1) % 26));
    return ch.repeat(repeat);
  };
  switch (style) {
    case "D": return String(v);
    case "R": return roman(v, true) ?? String(v);
    case "r": return roman(v, false) ?? String(v);
    case "A": return alpha(v, true) ?? String(v);
    case "a": return alpha(v, false) ?? String(v);
  }
}
