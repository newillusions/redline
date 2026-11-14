/**
 * Symbol Search client: find every copy of a boxed luminaire symbol on a page or sheet set,
 * review the hits, and place the accepted ones as count markers.
 *
 * The matching runs in Rust (`src-tauri/src/symbol_search`, command `symbol_search`) off the
 * render thread; this module only builds the request, keeps the review state, and turns
 * accepted hits into `MeasurementCount` markups in the active count set. Pure functions, no
 * DOM and no Svelte, so the review rules are unit-testable.
 */
import { invoke } from "@tauri-apps/api/core";
import type { CountSet, Markup, MeasurementPayload, PdfPoint, UserRef } from "./ipc";
import { buildMarkup } from "./markup-tools";
import { MAX_COUNT_GROUP_ITEMS } from "./count-groups";
import type { Appearance } from "./ipc";

/** Axis-aligned box in PDF page space (`x`, `y` is the lower-left corner). */
export interface SearchRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One found copy, as returned by Rust (`symbol_search::SymbolMatch`). */
export interface SymbolMatch {
  center: [number, number];
  rect: SearchRect;
  /** Turn from the template in degrees (0 with rotations off). */
  angle: number;
  /** Vector: share of the template's lines found; raster: correlation. */
  score: number;
  /** Size relative to the template (1 unless scaled copies are searched for). */
  scale: number;
}

export type SearchMode = "auto" | "vector" | "raster";

export interface SymbolSearchOptions {
  mode?: SearchMode;
  rotations?: boolean;
  scales?: boolean;
  /** Share of the template's lines a vector match needs, 0-1. Rust default 0.9. */
  minScore?: number;
  /** Correlation a raster match needs, 0-1. Rust default 0.75. */
  rasterThreshold?: number;
}

export interface SymbolSearchResponse {
  method: "vector" | "raster";
  template_segments: number;
  pages: { page: number; matches: SymbolMatch[]; truncated: boolean }[];
  skipped: { page: number; reason: string }[];
  /** A result, time or size limit cut the search short: the list may be incomplete. */
  truncated: boolean;
  elapsed_ms: number;
}

/** Run the search. `pages: null` searches the template page only. */
export function symbolSearch(
  docId: string,
  templatePage: number,
  rect: SearchRect,
  pages: number[] | null,
  opts: SymbolSearchOptions = {},
): Promise<SymbolSearchResponse> {
  // Tauri passes the command's single struct argument under its parameter name; the
  // struct's own fields are snake_case (serde), unlike top-level argument names.
  const request: Record<string, unknown> = {
    doc_id: docId,
    template_page: templatePage,
    rect,
    pages,
    mode: opts.mode ?? "auto",
    rotations: opts.rotations ?? false,
    scales: opts.scales ?? false,
  };
  if (opts.minScore !== undefined) request.min_score = opts.minScore;
  if (opts.rasterThreshold !== undefined) request.raster_threshold = opts.rasterThreshold;
  return invoke<SymbolSearchResponse>("symbol_search", { request });
}

/** Every page of a document, 0-based. */
export function allPages(pageCount: number): number[] {
  return Array.from({ length: Math.max(0, pageCount) }, (_, i) => i);
}

/** The template box from a drawn Rectangle markup (what "Get Rectangle" is in Bluebeam). */
export function templateRectFromMarkup(m: Markup | undefined | null): SearchRect | null {
  if (!m || m.markup_type !== "Rectangle" || !("Rect" in m.geometry)) return null;
  const { min, max } = m.geometry.Rect;
  const w = Math.abs(max.x - min.x);
  const h = Math.abs(max.y - min.y);
  if (!(w > 0 && h > 0)) return null;
  return { x: Math.min(min.x, max.x), y: Math.min(min.y, max.y), w, h };
}

/** One row in the review list. */
export interface Hit {
  /** Stable within one search: `page:index`. */
  key: string;
  page: number;
  match: SymbolMatch;
  /** The boxed original itself (counted, but flagged so the reviewer can tell). */
  isTemplate: boolean;
  accepted: boolean;
}

/** Flatten a response into review rows, all accepted, in page then reading order. */
export function hitsFromResponse(
  resp: SymbolSearchResponse,
  templatePage: number,
  templateRect: SearchRect,
): Hit[] {
  const out: Hit[] = [];
  for (const p of [...resp.pages].sort((a, b) => a.page - b.page)) {
    p.matches.forEach((match, i) => {
      const [cx, cy] = match.center;
      out.push({
        key: `${p.page}:${i}`,
        page: p.page,
        match,
        isTemplate:
          p.page === templatePage &&
          cx >= templateRect.x &&
          cx <= templateRect.x + templateRect.w &&
          cy >= templateRect.y &&
          cy <= templateRect.y + templateRect.h,
        accepted: true,
      });
    });
  }
  return out;
}

/** Flip one hit. Returns a new array; the input is untouched. */
export function toggleHit(hits: Hit[], key: string): Hit[] {
  return hits.map((h) => (h.key === key ? { ...h, accepted: !h.accepted } : h));
}

export function setAll(hits: Hit[], accepted: boolean, pred: (h: Hit) => boolean = () => true): Hit[] {
  return hits.map((h) => (pred(h) ? { ...h, accepted } : h));
}

/** Accepted hits per page, for the review header and the commit button. */
export function acceptedByPage(hits: Hit[]): Map<number, number> {
  const out = new Map<number, number>();
  for (const h of hits) if (h.accepted) out.set(h.page, (out.get(h.page) ?? 0) + 1);
  return out;
}

export interface CommitOptions {
  identity: UserRef;
  now: string;
  mintId: () => string;
  /** Count set the markers go into; `null` uses the draft colour only. */
  set: CountSet | null;
  appearance: Appearance;
  /** Label for the new count (shown in the quantity table). */
  label?: string | null;
}

/**
 * Accepted hits as `MeasurementCount` items, ONE count group per page-and-set run: a count in
 * this app is a run of items sharing `count_group` on one page (docs/count-polylength.md), so a
 * multi-page search yields one count per page, each carrying the same label. Large pages split
 * at the group size cap.
 */
export function buildCountMarkups(hits: Hit[], o: CommitOptions): Markup[] {
  const accepted = hits.filter((h) => h.accepted);
  const byPage = new Map<number, Hit[]>();
  for (const h of accepted) byPage.set(h.page, [...(byPage.get(h.page) ?? []), h]);
  const meas: MeasurementPayload = {
    scale_ref: null,
    raw_measure: 1,
    unit: "ea",
    computed_quantity: 1,
    depth: null,
    count_value: 1,
    custom_columns: {},
  };
  const out: Markup[] = [];
  for (const [page, rows] of [...byPage].sort((a, b) => a[0] - b[0])) {
    let group = o.mintId();
    let inGroup = 0;
    for (const h of rows) {
      if (inGroup >= MAX_COUNT_GROUP_ITEMS) {
        group = o.mintId();
        inGroup = 0;
      }
      const p: PdfPoint = { x: h.match.center[0], y: h.match.center[1] };
      const m = buildMarkup({
        markupType: "MeasurementCount",
        page,
        geometry: { Point: p },
        appearance: o.set ? { ...o.appearance, color: o.set.color } : o.appearance,
        identity: o.identity,
        now: o.now,
        id: o.mintId(),
        countSet: o.set,
      });
      m.count_group = group;
      if (o.label) m.subject = o.label;
      m.measurement = { ...meas };
      out.push(m);
      inGroup++;
    }
  }
  return out;
}
