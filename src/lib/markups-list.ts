/**
 * Markups List model: rows, columns, sort, filter, selection, status shortcuts and the
 * virtual-row window. Pure (no DOM, no stores); the panel in `MarkupsListPanel.svelte`
 * is a thin view over this. Behaviour spec: docs/markups-list.md.
 *
 * Every string here comes from PDF mark-up data and is UNTRUSTED: it is only ever used as
 * text (never HTML) and all work is bounded (see the caps below).
 */
import type { Markup, MarkupStatus } from "./ipc";
import { latestReviewChange, threadRows } from "./markup-review";

/** Revu's default Review-model order (c16Kk8Cffcc@184-192); drives the status menu and Shift+N. */
export const STATUS_ORDER: readonly MarkupStatus[] = [
  "Accepted", "Rejected", "Cancelled", "Completed", "None",
];

/** Caps on user/PDF-controlled work. */
export const MAX_SEARCH_CHARS = 200;
export const MAX_PAGE_SPEC_TOKENS = 50;
export const MAX_CUSTOM_COLUMNS = 32;
/** Mirrors Rust `MAX_CUSTOM_NAME_CHARS`: longer names, or names with control characters, are not offered. */
export const MAX_CUSTOM_NAME_CHARS = 64;
export const MAX_CELL_CHARS = 2000;

export type ColumnKind = "text" | "number";

export interface ColumnDef {
  key: string;
  label: string;
  kind: ColumnKind;
  defaultVisible: boolean;
  width: number;
}

/** Built-in columns. Keys are shared with the Rust export (`commands/markup_list.rs`). */
export const COLUMN_DEFS: readonly ColumnDef[] = [
  { key: "number", label: "#", kind: "number", defaultVisible: false, width: 48 },
  { key: "subject", label: "Subject", kind: "text", defaultVisible: true, width: 130 },
  { key: "type", label: "Type", kind: "text", defaultVisible: false, width: 110 },
  { key: "page", label: "Page", kind: "number", defaultVisible: true, width: 56 },
  { key: "comment", label: "Comments", kind: "text", defaultVisible: true, width: 240 },
  { key: "author", label: "Author", kind: "text", defaultVisible: true, width: 110 },
  { key: "date", label: "Date", kind: "text", defaultVisible: true, width: 135 },
  { key: "modified", label: "Modified", kind: "text", defaultVisible: false, width: 120 },
  { key: "status", label: "Status", kind: "text", defaultVisible: true, width: 90 },
  { key: "replies", label: "Replies", kind: "number", defaultVisible: true, width: 64 },
  { key: "colour", label: "Colour", kind: "text", defaultVisible: false, width: 80 },
  { key: "layer", label: "Layer", kind: "text", defaultVisible: false, width: 90 },
  { key: "measurement", label: "Measurement", kind: "number", defaultVisible: false, width: 110 },
  { key: "status_by", label: "Status set by", kind: "text", defaultVisible: false, width: 110 },
  { key: "status_at", label: "Status set at", kind: "text", defaultVisible: false, width: 120 },
];

export const CUSTOM_PREFIX = "custom:";

export function customKey(name: string): string {
  return CUSTOM_PREFIX + name;
}

export interface ListRow {
  id: string;
  /** 1-based position in document order. */
  index: number;
  markup: Markup;
  typeLabel: string;
  subject: string;
  /** 1-based page number. */
  page: number;
  comment: string;
  author: string;
  date: string;
  modified: string;
  status: MarkupStatus;
  replies: number;
  colour: string;
  layer: string;
  measurement: string;
  measurementValue: number | null;
  statusBy: string;
  statusAt: string;
  custom: Record<string, string>;
  /** Lower-cased concatenation used by the text filter (replies included). */
  haystack: string;
}

function clip(s: string | null | undefined): string {
  const t = s ?? "";
  return t.length > MAX_CELL_CHARS ? t.slice(0, MAX_CELL_CHARS) : t;
}

/** `2026-10-12T09:30:00Z` -> `2026-10-12 09:30` (UTC; same text the export writes). */
export function formatStamp(iso: string | null | undefined): string {
  if (!iso) return "";
  const m = /^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2})/.exec(iso);
  return m ? `${m[1]} ${m[2]}` : clip(iso).slice(0, 16);
}

export function measurementText(m: Markup): { text: string; value: number | null } {
  const meas = m.measurement;
  if (!meas) return { text: "", value: null };
  if (m.markup_type === "MeasurementCount") {
    const n = meas.count_value ?? 0;
    return { text: n.toFixed(0), value: n };
  }
  const q = meas.computed_quantity;
  return { text: `${q.toFixed(2)} ${meas.unit ?? ""}`.trimEnd(), value: q };
}

export function buildRow(m: Markup, index: number): ListRow {
  const change = latestReviewChange(m.workflow.status_history);
  const { text, value } = measurementText(m);
  const custom: Record<string, string> = {};
  const cc = m.measurement?.custom_columns;
  if (cc) for (const k of Object.keys(cc)) custom[k] = clip(cc[k]);
  const subject = clip(m.subject) || m.markup_type;
  const comment = clip(m.contents);
  const author = clip(m.audit.created_by.display_name);
  const layer = clip(m.layer);
  const replyText = m.workflow.thread.map((r) => r.contents).join(" ");
  return {
    id: m.id,
    index,
    markup: m,
    typeLabel: m.markup_type,
    subject,
    page: m.page + 1,
    comment,
    author,
    date: formatStamp(m.audit.created_at),
    modified: formatStamp(m.audit.modified_at),
    status: m.workflow.status,
    replies: m.workflow.thread.length,
    colour: m.appearance.color,
    layer,
    measurement: text,
    measurementValue: value,
    statusBy: clip(change?.author.display_name),
    statusAt: formatStamp(change?.at),
    custom,
    haystack: `${subject} ${comment} ${author} ${m.markup_type} ${layer} ${clip(replyText)}`.toLowerCase(),
  };
}

export function buildRows(markups: readonly Markup[]): ListRow[] {
  return markups.map((m, i) => buildRow(m, i + 1));
}

/** Custom column names present in the document (sorted, capped). */
export function customColumnNames(rows: readonly ListRow[]): string[] {
  const names = new Set<string>();
  for (const r of rows) {
    for (const k of Object.keys(r.custom)) {
      // The export refuses these names (untrusted PDF data), so do not offer them as columns.
      // eslint-disable-next-line no-control-regex
      if (k === "" || [...k].length > MAX_CUSTOM_NAME_CHARS || /[\u0000-\u001f\u007f-\u009f]/.test(k)) continue;
      names.add(k);
      if (names.size >= MAX_CUSTOM_COLUMNS) return [...names].sort();
    }
  }
  return [...names].sort();
}

export function cellText(r: ListRow, key: string): string {
  switch (key) {
    case "number": return String(r.index);
    case "subject": return r.subject;
    case "type": return r.typeLabel;
    case "page": return String(r.page);
    case "comment": return r.comment;
    case "author": return r.author;
    case "date": return r.date;
    case "modified": return r.modified;
    case "status": return r.status;
    case "replies": return r.replies > 0 ? String(r.replies) : "";
    case "colour": return r.colour;
    case "layer": return r.layer;
    case "measurement": return r.measurement;
    case "status_by": return r.statusBy;
    case "status_at": return r.statusAt;
    default:
      return key.startsWith(CUSTOM_PREFIX) ? (r.custom[key.slice(CUSTOM_PREFIX.length)] ?? "") : "";
  }
}

export function columnLabel(key: string): string {
  const def = COLUMN_DEFS.find((c) => c.key === key);
  return def ? def.label : key.startsWith(CUSTOM_PREFIX) ? key.slice(CUSTOM_PREFIX.length) : key;
}

export function columnKind(key: string): ColumnKind {
  return COLUMN_DEFS.find((c) => c.key === key)?.kind ?? "text";
}

// ---------------------------------------------------------------------------
// Column configuration (visibility + order)
// ---------------------------------------------------------------------------

export interface ColumnConfig {
  /** Every known column key in display order. */
  order: string[];
  visible: string[];
}

export function defaultColumnConfig(): ColumnConfig {
  return {
    order: COLUMN_DEFS.map((c) => c.key),
    visible: COLUMN_DEFS.filter((c) => c.defaultVisible).map((c) => c.key),
  };
}

/** Repair a stored config: drop unknown built-ins, append new ones, keep custom keys. */
export function normalizeColumnConfig(raw: unknown, customNames: readonly string[] = []): ColumnConfig {
  const known = new Set<string>([...COLUMN_DEFS.map((c) => c.key), ...customNames.map(customKey)]);
  const base = defaultColumnConfig();
  const r = raw as Partial<ColumnConfig> | null;
  const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  const order = strs(r?.order).filter((k) => known.has(k));
  for (const k of known) if (!order.includes(k)) order.push(k);
  const visible = r && Array.isArray(r.visible) ? strs(r.visible).filter((k) => known.has(k)) : base.visible;
  return { order, visible: [...new Set(visible)] };
}

export function visibleColumns(cfg: ColumnConfig): string[] {
  const vis = new Set(cfg.visible);
  return cfg.order.filter((k) => vis.has(k));
}

export function toggleColumn(cfg: ColumnConfig, key: string): ColumnConfig {
  const on = cfg.visible.includes(key);
  return { ...cfg, visible: on ? cfg.visible.filter((k) => k !== key) : [...cfg.visible, key] };
}

export function moveColumn(cfg: ColumnConfig, key: string, dir: -1 | 1): ColumnConfig {
  const i = cfg.order.indexOf(key);
  const j = i + dir;
  if (i < 0 || j < 0 || j >= cfg.order.length) return cfg;
  const order = [...cfg.order];
  [order[i], order[j]] = [order[j], order[i]];
  return { ...cfg, order };
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

export interface ListFilter {
  text: string;
  statuses: ReadonlySet<MarkupStatus>;
  authors: ReadonlySet<string>;
  types: ReadonlySet<string>;
  /** Page spec such as `1-3, 7` (1-based). Empty = all pages. */
  pages: string;
}

export function emptyFilter(): ListFilter {
  return { text: "", statuses: new Set(), authors: new Set(), types: new Set(), pages: "" };
}

export function isFilterActive(f: ListFilter): boolean {
  return f.text.trim() !== "" || f.statuses.size > 0 || f.authors.size > 0 || f.types.size > 0 || f.pages.trim() !== "";
}

/** Parse `1-3, 7` into inclusive ranges; invalid tokens are ignored, at most 50 tokens read. */
export function parsePageSpec(spec: string): [number, number][] {
  const out: [number, number][] = [];
  for (const tok of spec.split(/[,\s]+/).slice(0, MAX_PAGE_SPEC_TOKENS)) {
    const m = /^(\d{1,7})(?:-(\d{1,7}))?$/.exec(tok);
    if (!m) continue;
    const a = Number(m[1]);
    const b = m[2] === undefined ? a : Number(m[2]);
    out.push(a <= b ? [a, b] : [b, a]);
  }
  return out;
}

export function filterRows(rows: readonly ListRow[], f: ListFilter): ListRow[] {
  const needle = f.text.trim().slice(0, MAX_SEARCH_CHARS).toLowerCase();
  const ranges = parsePageSpec(f.pages);
  const pageActive = f.pages.trim() !== "";
  return rows.filter((r) => {
    if (f.statuses.size > 0 && !f.statuses.has(r.status)) return false;
    if (f.authors.size > 0 && !f.authors.has(r.author)) return false;
    if (f.types.size > 0 && !f.types.has(r.typeLabel)) return false;
    // A page spec with no valid token matches nothing (rather than silently everything).
    if (pageActive && !ranges.some(([a, b]) => r.page >= a && r.page <= b)) return false;
    if (needle && !r.haystack.includes(needle)) return false;
    return true;
  });
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

export type SortDir = "asc" | "desc";
export interface SortState { key: string; dir: SortDir }

/** Header click cycle: unsorted -> asc -> desc -> unsorted (document order). */
export function nextSort(cur: SortState | null, key: string): SortState | null {
  if (!cur || cur.key !== key) return { key, dir: "asc" };
  return cur.dir === "asc" ? { key, dir: "desc" } : null;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function numericValue(r: ListRow, key: string): number | null {
  switch (key) {
    case "number": return r.index;
    case "page": return r.page;
    case "replies": return r.replies;
    case "measurement": return r.measurementValue;
    default: return null;
  }
}

export function sortRows(rows: readonly ListRow[], sort: SortState | null): ListRow[] {
  const out = [...rows];
  if (!sort) return out;
  const sign = sort.dir === "asc" ? 1 : -1;
  const numeric = columnKind(sort.key) === "number";
  out.sort((a, b) => {
    let c: number;
    if (numeric) {
      const x = numericValue(a, sort.key);
      const y = numericValue(b, sort.key);
      // Rows without a value sort last in either direction.
      if (x === null || y === null) {
        if (x === y) return a.index - b.index;
        return x === null ? 1 : -1;
      }
      c = x - y;
    } else {
      const x = cellText(a, sort.key);
      const y = cellText(b, sort.key);
      // Empty cells sort last in either direction.
      if (x === "" || y === "") {
        if (x === y) return a.index - b.index;
        return x === "" ? 1 : -1;
      }
      c = collator.compare(x, y);
    }
    return c !== 0 ? c * sign : a.index - b.index;
  });
  return out;
}

// ---------------------------------------------------------------------------
// Display items (rows + expanded reply rows) and the virtual window
// ---------------------------------------------------------------------------

export type DisplayItem =
  | { kind: "markup"; row: ListRow }
  | { kind: "reply"; row: ListRow; replyId: string; author: string; at: string; text: string; depth: number };

export function displayItems(rows: readonly ListRow[], expanded: ReadonlySet<string>): DisplayItem[] {
  const out: DisplayItem[] = [];
  for (const row of rows) {
    out.push({ kind: "markup", row });
    if (row.replies > 0 && expanded.has(row.id)) {
      for (const { reply, depth } of threadRows(row.markup.workflow.thread)) {
        out.push({
          kind: "reply",
          row,
          replyId: reply.id,
          author: clip(reply.author.display_name),
          at: formatStamp(reply.at),
          text: clip(reply.contents),
          depth,
        });
      }
    }
  }
  return out;
}

export interface WindowRange { start: number; end: number }

/** Row indices [start, end) to render for a scrolled fixed-row-height list. */
export function windowRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan = 8,
): WindowRange {
  if (total <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const count = Math.ceil(Math.max(0, viewportHeight) / rowHeight) + 1;
  const start = Math.min(Math.max(0, first - overscan), total);
  const end = Math.min(total, first + count + overscan);
  return { start, end };
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

export interface ClickMods { ctrl: boolean; shift: boolean }

/**
 * Row click. Plain = select only that row; Ctrl/Cmd = toggle it; Shift = range from the
 * anchor in current view order. (Modifier behaviour is inferred - Revu's is unknown.)
 */
export function applyRowClick(
  selected: ReadonlySet<string>,
  anchor: string | null,
  order: readonly string[],
  clicked: string,
  mods: ClickMods,
): { ids: Set<string>; anchor: string } {
  if (mods.shift && anchor !== null) {
    const a = order.indexOf(anchor);
    const b = order.indexOf(clicked);
    if (a >= 0 && b >= 0) {
      const [lo, hi] = a <= b ? [a, b] : [b, a];
      const ids = mods.ctrl ? new Set(selected) : new Set<string>();
      for (let i = lo; i <= hi; i++) ids.add(order[i]);
      return { ids, anchor };
    }
  }
  if (mods.ctrl) {
    const ids = new Set(selected);
    if (ids.has(clicked)) ids.delete(clicked);
    else ids.add(clicked);
    return { ids, anchor: clicked };
  }
  return { ids: new Set([clicked]), anchor: clicked };
}

// ---------------------------------------------------------------------------
// Status shortcuts
// ---------------------------------------------------------------------------

/** Shift+1..9 -> the Nth status in `STATUS_ORDER` (Revu: "in the order statuses are displayed"). */
export function statusForShortcut(e: { code: string; shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }): MarkupStatus | null {
  if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return null;
  const m = /^Digit([1-9])$/.exec(e.code);
  if (!m) return null;
  return STATUS_ORDER[Number(m[1]) - 1] ?? null;
}

// ---------------------------------------------------------------------------
// Jump target
// ---------------------------------------------------------------------------

/** Zero-based page + `[left, bottom, right, top]` rect for the viewport jump request. */
export function jumpTarget(
  m: Markup,
  bounds: { minX: number; minY: number; maxX: number; maxY: number },
): { page: number; rect: [number, number, number, number] } {
  return { page: m.page, rect: [bounds.minX, bounds.minY, bounds.maxX, bounds.maxY] };
}
