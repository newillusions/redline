/**
 * Legends (docs/legends.md): a table of the document's mark-up quantities placed on a page and
 * recomputed from the live mark-ups. Pure (no DOM, no stores, no clocks beyond the injected
 * `now`); `MarkupStore.syncLegends` and the Properties panel are thin views over this.
 *
 * This is the live-view half of one rule that Rust implements for the saved appearance
 * (`src-tauri/src/markup/legend.rs`). Both run the same fixture
 * (`src-tauri/tests/fixtures/legend-agreement.json`), so the numbers cannot drift apart.
 *
 * Every string and number in a legend definition is UNTRUSTED (PDF data, MCP input): it goes
 * through `sanitizeLegend`, is only ever used as text, and all work is bounded by the caps below.
 */
import type { Appearance, CountSet, CountSymbol, Markup, MarkupType, UserRef } from "./ipc";
import { bumpAudit, buildMarkup } from "./markup-tools";
import { markupOcRef } from "./layers";

export const MAX_ENTRIES = 256;
export const MAX_ROWS = 512;
export const MAX_PAGE_RANGES = 50;
export const MAX_TEXT_CHARS = 500;
export const MAX_TITLE_CHARS = 200;
export const MAX_PAGES_CELL_CHARS = 80;
export const MIN_FONT_PT = 4;
export const MAX_FONT_PT = 72;
export const DEFAULT_FONT_PT = 9;
export const MIN_WIDTH = 80;
export const MAX_WIDTH = 20_000;
export const DEFAULT_WIDTH = 300;
export const ROW_H_EM = 1.8;
export const TITLE_H_EM = 2.2;
export const GLYPH_EM = 0.52;

export type LegendColumn = "Symbol" | "Subject" | "Description" | "Quantity" | "Unit" | "Pages";
export const LEGEND_COLUMNS: readonly LegendColumn[] = [
  "Symbol", "Subject", "Description", "Quantity", "Unit", "Pages",
];
export const COLUMN_HEADER: Record<LegendColumn, string> = {
  Symbol: "", Subject: "Subject", Description: "Description", Quantity: "Quantity", Unit: "Unit", Pages: "Pages",
};
const COLUMN_WEIGHT: Record<LegendColumn, number> = {
  Symbol: 0, Subject: 3, Description: 4, Quantity: 1.5, Unit: 1, Pages: 2,
};

export type LegendScope = "CurrentPage" | "AllPages" | { Pages: [number, number][] };

export interface LegendKey {
  markup_type: MarkupType;
  count_set?: string | null;
  color: string;
  fill?: string | null;
}

export interface LegendEntry {
  key: LegendKey;
  label?: string | null;
  description?: string | null;
  hidden?: boolean;
  symbol?: CountSymbol | null;
  /** Name of the count set the row was made for; shown while no mark-up matches. */
  set_name?: string | null;
}

export interface LegendProps {
  title: string;
  show_title: boolean;
  show_headers: boolean;
  scope: LegendScope;
  /** Count only mark-ups on this layer (the group's object number); null/absent = every layer.
   *  Layer visibility never changes a legend; this filter does. */
  layer?: number | null;
  columns: LegendColumn[];
  entries: LegendEntry[];
  auto_count_sets: boolean;
  hide_empty: boolean;
  font_size_pt: number;
}

export interface LegendRow {
  /** Index of the explicit entry; null for an automatic (count set) row. */
  entry: number | null;
  key: LegendKey;
  subject: string;
  description: string;
  quantity: number;
  quantity_text: string;
  unit: string;
  pages: string;
  matched: number;
  color: string;
  fill: string | null;
  symbol: CountSymbol | null;
}

export function defaultLegendProps(): LegendProps {
  return {
    title: "Legend",
    show_title: true,
    show_headers: true,
    scope: "CurrentPage",
    layer: null,
    columns: ["Symbol", "Subject", "Quantity", "Unit"],
    entries: [],
    auto_count_sets: false,
    hide_empty: false,
    font_size_pt: DEFAULT_FONT_PT,
  };
}

// --- sanitising -----------------------------------------------------------------------

// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;

/** Printable text: control characters become spaces, ends trimmed, at most `max` code points. */
export function cleanText(s: unknown, max: number): string {
  if (typeof s !== "string") return "";
  return [...s.replace(CONTROL, " ").trim()].slice(0, max).join("");
}

function cleanColor(c: unknown): string {
  return typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c) ? c.toLowerCase() : "";
}

function sanitizeKey(k: LegendKey): LegendKey {
  const bySet = typeof k.count_set === "string" && k.count_set !== "";
  const fill = bySet ? "" : cleanColor(k.fill);
  return {
    markup_type: k.markup_type,
    count_set: bySet ? k.count_set : null,
    color: cleanColor(k.color),
    fill: fill === "" ? null : fill,
  };
}

interface Norm {
  ty: MarkupType;
  set: string | null;
  color: string;
  fill: string | null;
}

function normOf(k: LegendKey): Norm {
  if (k.count_set) return { ty: k.markup_type, set: k.count_set, color: "", fill: null };
  return { ty: k.markup_type, set: null, color: k.color.toLowerCase(), fill: k.fill ? k.fill.toLowerCase() : null };
}

function sameNorm(a: Norm, b: Norm): boolean {
  return a.ty === b.ty && a.set === b.set && a.color === b.color && a.fill === b.fill;
}

/** The definition as safe to draw and write (mirrors Rust `LegendProps::sanitized`). */
export function sanitizeLegend(p: Partial<LegendProps> | null | undefined): LegendProps {
  const d = defaultLegendProps();
  const src = p ?? {};
  const columns: LegendColumn[] = [];
  for (const c of Array.isArray(src.columns) ? src.columns : []) {
    if (LEGEND_COLUMNS.includes(c) && !columns.includes(c)) columns.push(c);
  }
  let scope: LegendScope = "CurrentPage";
  if (src.scope === "AllPages" || src.scope === "CurrentPage") scope = src.scope;
  else if (src.scope && typeof src.scope === "object" && Array.isArray(src.scope.Pages)) {
    const ranges = src.scope.Pages
      .filter((r) => Array.isArray(r) && Number.isInteger(r[0]) && Number.isInteger(r[1]) && r[0] >= 1 && r[0] <= r[1])
      .slice(0, MAX_PAGE_RANGES)
      .map((r): [number, number] => [r[0], r[1]]);
    if (ranges.length > 0) scope = { Pages: ranges };
  }
  const font = typeof src.font_size_pt === "number" && Number.isFinite(src.font_size_pt)
    ? Math.min(MAX_FONT_PT, Math.max(MIN_FONT_PT, src.font_size_pt))
    : DEFAULT_FONT_PT;
  const seen: Norm[] = [];
  const entries: LegendEntry[] = [];
  for (const e of Array.isArray(src.entries) ? src.entries : []) {
    if (!e || !e.key || e.key.markup_type === "Legend") continue;
    const key = sanitizeKey(e.key);
    const n = normOf(key);
    if (seen.some((s) => sameNorm(s, n))) continue;
    seen.push(n);
    entries.push({
      key,
      label: e.label == null ? null : cleanText(e.label, MAX_TEXT_CHARS),
      description: e.description == null ? null : cleanText(e.description, MAX_TEXT_CHARS),
      hidden: !!e.hidden,
      symbol: e.symbol ?? null,
      set_name: e.set_name ? cleanText(e.set_name, MAX_TEXT_CHARS) || null : null,
    });
    if (entries.length >= MAX_ENTRIES) break;
  }
  return {
    title: cleanText(src.title ?? d.title, MAX_TITLE_CHARS),
    show_title: src.show_title ?? d.show_title,
    show_headers: src.show_headers ?? d.show_headers,
    scope,
    layer: Number.isInteger(src.layer) && (src.layer as number) > 0 ? (src.layer as number) : null,
    columns: columns.length > 0 ? columns : d.columns,
    entries,
    auto_count_sets: src.auto_count_sets ?? d.auto_count_sets,
    hide_empty: src.hide_empty ?? d.hide_empty,
    font_size_pt: font,
  };
}

// --- matching and rows ----------------------------------------------------------------

/** The key a mark-up would be listed under. */
export function keyOf(m: Markup): LegendKey {
  return sanitizeKey({
    markup_type: m.markup_type,
    count_set: m.count_set?.id ?? null,
    color: m.count_set ? m.count_set.color : m.appearance.color,
    fill: m.count_set ? null : m.appearance.fill,
  });
}

function matches(n: Norm, m: Markup): boolean {
  if (m.markup_type !== n.ty) return false;
  if ((m.count_set?.id ?? null) !== n.set) return false;
  if (n.set !== null) return true;
  const fill = m.appearance.fill ? m.appearance.fill.toLowerCase() : null;
  return m.appearance.color.toLowerCase() === n.color && fill === n.fill;
}

function inScope(scope: LegendScope, legendPage: number, page: number): boolean {
  if (scope === "CurrentPage") return page === legendPage;
  if (scope === "AllPages") return true;
  return scope.Pages.some(([a, b]) => page + 1 >= a && page + 1 <= b);
}

/** Types whose symbol cell shows a line sample rather than a box. */
export function isLineType(t: MarkupType): boolean {
  return (
    t === "Line" || t === "Arrow" || t === "Polyline" || t === "Ink" || t === "Underline" ||
    t === "StrikeOut" || t === "Squiggly" || t === "MeasurementLength" || t === "MeasurementAngle" ||
    t === "MeasurementRadius"
  );
}

function isSummed(t: MarkupType): boolean {
  return t === "MeasurementLength" || t === "MeasurementPerimeter" || t === "MeasurementArea" || t === "MeasurementVolume";
}

/** `toFixed(2)` with -0 normalised (matches Rust `fixed2`). */
export function fixed2(q: number): string {
  return (q === 0 ? 0 : q).toFixed(2);
}

/** Pages as ranges (`1-3, 5`), clipped to `MAX_PAGES_CELL_CHARS`. `pages` are 0-based.
 *  A document with page labels lists them instead (`E-101, E-102`, an unlabelled page by its
 *  number); mirrors Rust `pages_text_labelled`. */
export function pagesText(pages: Iterable<number>, labels: readonly string[] = []): string {
  const sortedPages = [...new Set(pages)].sort((a, b) => a - b);
  const labelOf = (p: number): string => {
    const l = labels[p];
    return typeof l === "string" && l !== "" ? l : "";
  };
  const clip = (out: string): string =>
    [...out].length > MAX_PAGES_CELL_CHARS ? [...out].slice(0, MAX_PAGES_CELL_CHARS - 3).join("") + "..." : out;
  if (sortedPages.some((p) => labelOf(p) !== "")) {
    let out = "";
    for (const p of sortedPages) {
      if (out !== "") out += ", ";
      out += labelOf(p) !== "" ? labelOf(p) : String(p + 1);
      if ([...out].length > MAX_PAGES_CELL_CHARS) break;
    }
    return clip(out);
  }
  const sorted = sortedPages.map((p) => p + 1);
  let out = "";
  for (let i = 0; i < sorted.length; ) {
    const start = sorted[i];
    let end = start;
    i++;
    while (i < sorted.length && sorted[i] === end + 1) { end = sorted[i]; i++; }
    if (out !== "") out += ", ";
    out += end === start ? String(start) : `${start}-${end}`;
    if ([...out].length > MAX_PAGES_CELL_CHARS) break;
  }
  return clip(out);
}

/** The first candidate that is a valid `#rrggbb`, else black (colours are PDF data). */
function safeColor(candidates: (string | null | undefined)[]): string {
  for (const c of candidates) {
    const ok = cleanColor(c);
    if (ok !== "") return ok;
  }
  return "#000000";
}

function firstUnit(items: readonly Markup[]): string | null {
  for (const m of items) {
    const u = m.measurement?.unit?.trim();
    if (u) return cleanText(u, 16);
  }
  return null;
}

/** (quantity, text, unit) for the mark-ups one row claimed: the rule the Quantities panel,
 *  the Markups List and the exports use (docs/legends.md R9). */
function quantityOf(ty: MarkupType, items: readonly Markup[]): { q: number; text: string; unit: string } {
  if (ty === "MeasurementCount") {
    let n = 0;
    for (const m of items) n += m.measurement?.count_value ?? 1;
    return { q: n, text: String(n), unit: firstUnit(items) ?? "ea" };
  }
  if (isSummed(ty)) {
    let q = 0;
    for (const m of items) {
      const v = m.measurement?.computed_quantity;
      if (typeof v === "number" && Number.isFinite(v)) q += v;
    }
    return { q, text: fixed2(q), unit: firstUnit(items) ?? "" };
  }
  return { q: items.length, text: String(items.length), unit: "ea" };
}

function firstText(items: readonly Markup[], f: (m: Markup) => string | null | undefined): string | null {
  for (const m of items) {
    const t = cleanText(f(m), MAX_TEXT_CHARS);
    if (t !== "") return t;
  }
  return null;
}

function buildRow(entryIdx: number | null, entry: LegendEntry | null, key: LegendKey, items: readonly Markup[], labels: readonly string[]): LegendRow {
  const { q, text, unit } = quantityOf(key.markup_type, items);
  const set = items.find((m) => m.count_set)?.count_set ?? null;
  const labelRaw = entry?.label != null ? cleanText(entry.label, MAX_TEXT_CHARS) : "";
  const setName = set ? cleanText(set.name, MAX_TEXT_CHARS) : "";
  const subject = labelRaw !== "" ? labelRaw
    : firstText(items, (m) => m.subject) ?? (setName !== "" ? setName : entry?.set_name || key.markup_type);
  const descRaw = entry?.description != null ? cleanText(entry.description, MAX_TEXT_CHARS) : "";
  const description = descRaw !== "" ? descRaw : firstText(items, (m) => m.contents) ?? "";
  const first = items[0];
  return {
    entry: entryIdx,
    key,
    subject,
    description,
    quantity: q,
    quantity_text: text,
    unit,
    pages: pagesText(items.map((m) => m.page), labels),
    matched: items.length,
    color: safeColor([set?.color, first?.appearance.color, key.color]),
    fill: first
      ? (set ? null : cleanColor(first.appearance.fill) || null)
      : key.fill ?? null,
    symbol: key.markup_type === "MeasurementCount" ? set?.symbol ?? entry?.symbol ?? "Circle" : null,
  };
}

/** The mark-ups one entry claimed, split by measurement unit (first-seen order) for the summed
 *  measurement types, so 5.0 m and 7.5 ft are two rows, not "12.50 m". Everything else, and an
 *  entry that claimed nothing, is one group (mirrors Rust `split_by_unit`). */
function splitByUnit(ty: MarkupType, items: Markup[]): Markup[][] {
  if (!isSummed(ty) || items.length === 0) return [items];
  const groups = new Map<string, Markup[]>();
  for (const m of items) {
    const u = m.measurement?.unit?.trim() ?? "";
    const g = groups.get(u);
    if (g) g.push(m);
    else groups.set(u, [m]);
  }
  return [...groups.values()];
}

/** The visible rows of a legend sitting on `legendPage`. `props` must be sanitised. `labels` is
 *  the document's page-label text per page (`""` = none), for the Pages column. */
export function computeRows(
  legendPage: number,
  props: LegendProps,
  markups: readonly Markup[],
  labels: readonly string[] = [],
): LegendRow[] {
  const norms = props.entries.map((e) => normOf(e.key));
  const claimed: Markup[][] = props.entries.map(() => []);
  const implicitOrder: string[] = [];
  const implicit = new Map<string, Markup[]>();
  for (const m of markups) {
    if (m.markup_type === "Legend" || !inScope(props.scope, legendPage, m.page)) continue;
    if (props.layer != null && markupOcRef(m) !== props.layer) continue;
    let hit = -1;
    for (let i = 0; i < norms.length; i++) {
      if (matches(norms[i], m)) { hit = i; break; }
    }
    if (hit >= 0) { claimed[hit].push(m); continue; }
    if (props.auto_count_sets && m.markup_type === "MeasurementCount" && m.count_set) {
      const id = m.count_set.id;
      let list = implicit.get(id);
      if (!list) {
        if (implicitOrder.length >= MAX_ENTRIES) continue;
        implicitOrder.push(id);
        list = [];
        implicit.set(id, list);
      }
      list.push(m);
    }
  }
  const rows: LegendRow[] = [];
  for (let i = 0; i < props.entries.length; i++) {
    const e = props.entries[i];
    if (e.hidden || (props.hide_empty && claimed[i].length === 0)) continue;
    // Measured quantities in different units are never added together: one row per unit.
    for (const group of splitByUnit(e.key.markup_type, claimed[i])) {
      rows.push(buildRow(i, e, e.key, group, labels));
      if (rows.length >= MAX_ROWS) return rows;
    }
  }
  for (const id of implicitOrder) {
    const key: LegendKey = { markup_type: "MeasurementCount", count_set: id, color: "", fill: null };
    rows.push(buildRow(null, null, key, implicit.get(id) ?? [], labels));
    if (rows.length >= MAX_ROWS) break;
  }
  return rows;
}

// --- layout ---------------------------------------------------------------------------

export interface ColumnBox { column: LegendColumn; x: number; w: number }
export interface LegendLayout {
  width: number;
  height: number;
  font: number;
  rowH: number;
  titleH: number;
  headerH: number;
  columns: ColumnBox[];
}

/** Layout for a legend `width` wide showing `rows` rows. `props` must be sanitised. */
export function layout(props: LegendProps, width: number, rows: number): LegendLayout {
  const font = props.font_size_pt;
  const rowH = font * ROW_H_EM;
  const titleH = props.show_title ? font * TITLE_H_EM : 0;
  const headerH = props.show_headers ? rowH : 0;
  const w = Number.isFinite(width) ? Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width)) : DEFAULT_WIDTH;
  const hasSymbol = props.columns.includes("Symbol");
  const flexible = props.columns.reduce((s, c) => s + COLUMN_WEIGHT[c], 0);
  const free = Math.max(0, w - (hasSymbol ? rowH : 0));
  let x = 0;
  const columns: ColumnBox[] = props.columns.map((c) => {
    const cw = c === "Symbol" ? (flexible === 0 ? w : rowH) : (free * COLUMN_WEIGHT[c]) / flexible;
    const box = { column: c, x, w: cw };
    x += cw;
    return box;
  });
  return { width: w, height: titleH + headerH + rowH * Math.max(rows, 1), font, rowH, titleH, headerH, columns };
}

/** Text clipped to a column: `...` replaces the tail that does not fit. */
export function clipCell(s: string, colW: number, font: number): string {
  const max = Math.floor((colW - font) / (GLYPH_EM * font));
  if (!(max >= 1)) return "";
  const chars = [...s];
  if (chars.length <= max) return s;
  if (max <= 3) return chars.slice(0, max).join("");
  return chars.slice(0, max - 3).join("") + "...";
}

export function cellText(row: LegendRow, column: LegendColumn): string {
  switch (column) {
    case "Symbol": return "";
    case "Subject": return row.subject;
    case "Description": return row.description;
    case "Quantity": return row.quantity_text;
    case "Unit": return row.unit;
    case "Pages": return row.pages;
  }
}

export interface LegendRect { x0: number; y0: number; x1: number; y1: number }

/** The rectangle a legend occupies when it shows `rows` rows: the stored left, right and top
 *  edges are the user's, the bottom edge follows the rows. `null` for a non-rect legend. */
export function effectiveRect(m: Markup, rows: number): LegendRect | null {
  if (!m.legend || !("Rect" in m.geometry)) return null;
  const { min, max } = m.geometry.Rect;
  if (![min.x, min.y, max.x, max.y].every(Number.isFinite)) return null;
  const x0 = Math.min(min.x, max.x);
  const top = Math.max(min.y, max.y);
  const l = layout(sanitizeLegend(m.legend), Math.abs(max.x - min.x), rows);
  return { x0, y0: top - l.height, x1: x0 + l.width, y1: top };
}

/** `m` with its stored rectangle matching the rows it shows now (same object when it already
 *  does), so selection and resize handles sit on the visible table. */
export function fitLegendGeometry(m: Markup, markups: readonly Markup[]): Markup {
  if (m.markup_type !== "Legend" || !m.legend) return m;
  const props = sanitizeLegend(m.legend);
  const rows = computeRows(m.page, props, markups);
  const r = effectiveRect(m, rows.length);
  if (!r || !("Rect" in m.geometry)) return m;
  const { min, max } = m.geometry.Rect;
  const same = (a: number, b: number) => Math.abs(a - b) < 1e-6;
  if (same(Math.min(min.y, max.y), r.y0) && same(Math.max(min.y, max.y), r.y1)
      && same(Math.min(min.x, max.x), r.x0) && same(Math.max(min.x, max.x), r.x1)) return m;
  return { ...m, geometry: { Rect: { min: { x: r.x0, y: r.y0 }, max: { x: r.x1, y: r.y1 } } } };
}

// --- page ranges ----------------------------------------------------------------------

/** `1-3, 5, 9` -> ranges; `null` when there is no usable token (never "everything"). */
export function parsePageSpec(text: string): [number, number][] | null {
  const out: [number, number][] = [];
  const tokens = text.split(",").map((t) => t.trim()).filter((t) => t !== "").slice(0, MAX_PAGE_RANGES);
  for (const t of tokens) {
    const m = /^(\d{1,9})(?:\s*-\s*(\d{1,9}))?$/.exec(t);
    if (!m) return null;
    const a = Number(m[1]);
    const b = m[2] === undefined ? a : Number(m[2]);
    if (a < 1 || a > b) return null;
    out.push([a, b]);
  }
  return out.length > 0 ? out : null;
}

export function formatPageSpec(scope: LegendScope): string {
  if (typeof scope === "string") return "";
  return scope.Pages.map(([a, b]) => (a === b ? String(a) : `${a}-${b}`)).join(", ");
}

// --- creating and editing -------------------------------------------------------------

export interface LegendContext {
  page: number;
  /** Top-left corner of the table, PDF user space. */
  topLeft: { x: number; y: number };
  identity: UserRef;
  now: string;
  id: string;
  width?: number;
}

const LEGEND_APPEARANCE: Appearance = {
  color: "#000000", line_weight: 1, opacity: 1, fill: "#ffffff", line_style: "Solid", font: null,
  outline_color: null, fill_opacity: null,
};

/** A new Legend mark-up for `props` (rows are not stored; the rectangle is fitted by
 *  `fitLegendGeometry` once the mark-ups are known). */
export function newLegend(props: LegendProps, ctx: LegendContext): Markup {
  const p = sanitizeLegend(props);
  const l = layout(p, ctx.width ?? DEFAULT_WIDTH, Math.max(p.entries.length, 1));
  const m = buildMarkup({
    markupType: "Legend",
    page: ctx.page,
    geometry: {
      Rect: {
        min: { x: ctx.topLeft.x, y: ctx.topLeft.y - l.height },
        max: { x: ctx.topLeft.x + l.width, y: ctx.topLeft.y },
      },
    },
    appearance: LEGEND_APPEARANCE,
    identity: ctx.identity,
    now: ctx.now,
    id: ctx.id,
  });
  return { ...m, subject: "Legend", legend: p };
}

function entryFor(m: Markup): LegendEntry {
  return {
    key: keyOf(m),
    label: null,
    description: null,
    hidden: false,
    symbol: m.count_set?.symbol ?? null,
    set_name: m.count_set?.name ?? null,
  };
}

/** Entries for the distinct keys among `markups` that `have` does not already list. */
export function entriesFromMarkups(markups: readonly Markup[], have: readonly LegendEntry[] = []): LegendEntry[] {
  const norms = have.map((e) => normOf(e.key));
  const out: LegendEntry[] = [];
  for (const m of markups) {
    if (m.markup_type === "Legend") continue;
    const e = entryFor(m);
    const n = normOf(e.key);
    if (norms.some((x) => sameNorm(x, n))) continue;
    norms.push(n);
    out.push(e);
    if (have.length + out.length >= MAX_ENTRIES) break;
  }
  return out;
}

/** Ad-hoc legend: one row per distinct type/appearance among the selected mark-ups. */
export function legendFromSelection(selected: readonly Markup[], ctx: LegendContext): Markup | null {
  const entries = entriesFromMarkups(selected);
  if (entries.length === 0) return null;
  return newLegend({ ...defaultLegendProps(), scope: "AllPages", entries }, ctx);
}

/** Count-set legend (redline's tool-set analogue): one row per set, shown at 0 until its first
 *  marker; sets created later are picked up automatically. */
export function legendFromCountSets(sets: readonly CountSet[], ctx: LegendContext): Markup {
  const entries: LegendEntry[] = sets.slice(0, MAX_ENTRIES).map((s) => ({
    key: { markup_type: "MeasurementCount" as const, count_set: s.id, color: s.color, fill: null },
    label: null,
    description: null,
    hidden: false,
    symbol: s.symbol,
    set_name: s.name,
  }));
  return newLegend(
    { ...defaultLegendProps(), title: "Count legend", scope: "AllPages", entries, auto_count_sets: true },
    ctx,
  );
}

function edited(m: Markup, props: LegendProps, by: UserRef, now: string): Markup {
  return { ...bumpAudit(m, by, now), legend: sanitizeLegend(props) };
}

/** `m` with `patch` applied to its definition (one audit bump). */
export function patchLegend(m: Markup, patch: Partial<LegendProps>, by: UserRef, now: string): Markup {
  return edited(m, { ...sanitizeLegend(m.legend), ...patch }, by, now);
}

/** Add the type/appearance of `selected` to `m` (existing rows are left alone). */
export function addMarkupsToLegend(m: Markup, selected: readonly Markup[], by: UserRef, now: string): Markup {
  const p = sanitizeLegend(m.legend);
  const more = entriesFromMarkups(selected, p.entries);
  if (more.length === 0) return m;
  return edited(m, { ...p, entries: [...p.entries, ...more] }, by, now);
}

/** Edit one row. A row without an entry (automatic count set) is made explicit first. */
export function patchRow(
  m: Markup,
  row: LegendRow,
  patch: Pick<LegendEntry, "label" | "description" | "hidden">,
  by: UserRef,
  now: string,
): Markup {
  const p = sanitizeLegend(m.legend);
  const entries = [...p.entries];
  let idx = row.entry;
  if (idx === null) {
    if (entries.length >= MAX_ENTRIES) return m;
    entries.push({ key: row.key, label: null, description: null, hidden: false, symbol: row.symbol, set_name: null });
    idx = entries.length - 1;
  }
  entries[idx] = { ...entries[idx], ...patch };
  return edited(m, { ...p, entries }, by, now);
}

/** Remove one explicit row (a count set row would come back while `auto_count_sets` is on). */
export function removeRow(m: Markup, row: LegendRow, by: UserRef, now: string): Markup {
  if (row.entry === null) return m;
  const p = sanitizeLegend(m.legend);
  return edited(m, { ...p, entries: p.entries.filter((_, i) => i !== row.entry) }, by, now);
}
