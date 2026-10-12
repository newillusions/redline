/** Shared builders for the copy / arrange tests. */
import type { Markup, MarkupGeometry, MarkupType, ScaleRecord, UserRef } from "./ipc";
import { MarkupStore, type MarkupIpc } from "./markup-store.svelte";

export const BY: UserRef = { user_id: "u1", display_name: "Tester" };
export const NOW = "2026-10-12T10:00:00Z";
export const SET_A = { id: "setA", name: "Type A", color: "#e02424", symbol: "Circle" as const };

export function mk(id: string, o: Partial<Markup> & { rect?: [number, number, number, number] } = {}): Markup {
  const { rect, ...rest } = o;
  const geometry: MarkupGeometry = rect
    ? { Rect: { min: { x: rect[0], y: rect[1] }, max: { x: rect[2], y: rect[3] } } }
    : { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } };
  return {
    id, markup_type: "Rectangle" as MarkupType, page: 0, geometry,
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: { user_id: "other", display_name: "Other" }, created_at: "2020-01-01T00:00:00Z", modified_by: { user_id: "other", display_name: "Other" }, modified_at: "2020-01-01T00:00:00Z", revision: 7, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
    ...rest,
  } as Markup;
}

export function mkCount(id: string, x: number, y: number, group: string | null, o: Partial<Markup> = {}): Markup {
  return mk(id, {
    markup_type: "MeasurementCount", geometry: { Point: { x, y } }, count_group: group, count_set: SET_A,
    measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 1, custom_columns: {} },
    ...o,
  });
}

export function scale(id: string, ratio: number, page: number | null = null, unit = "m"): ScaleRecord {
  return {
    id, applies_to: page === null ? { kind: "DocumentDefault" } : { kind: "Page", page },
    method: "TwoPoint", ratio, unit, label: id, precision: 2,
  } as ScaleRecord;
}

export const NOOP_IPC: MarkupIpc = {
  add: async () => {}, update: async () => {}, remove: async () => {}, reorder: async () => {},
};

/** A store holding `ms`, loaded the way a document open does (no undo entries, not dirty). */
export function storeWith(ms: Markup[], docId = "doc1", ipc: MarkupIpc = NOOP_IPC): MarkupStore {
  const s = new MarkupStore(docId, ipc);
  s.seed(ms);
  return s;
}

export function seqIds(prefix = "n"): () => string {
  let n = 0;
  return () => `${prefix}-${++n}`;
}
