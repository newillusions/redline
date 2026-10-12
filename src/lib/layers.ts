/**
 * PDF layers (optional content groups): types mirroring `src-tauri/src/layers`, IPC wrappers,
 * and the pure rules the Layers panel and the viewer share. Behaviour spec: `docs/layers.md`.
 *
 * The VIEW is the set of group ids hidden (`off`). It is session state: nothing here writes
 * the file except `addLayer` and `saveLayersDefault`, which are explicit user actions.
 * Kept out of `ipc.ts` (hot file) on purpose.
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Types (mirror Rust `layers::*`)
// ---------------------------------------------------------------------------

export interface Layer {
  /** The group's object number: the key every other structure uses. */
  id: number;
  gen: number;
  name: string;
  /** Visible when the file is opened. */
  default_on: boolean;
  locked: boolean;
  /** False when the group's /Intent excludes View: it never affects what is drawn. */
  view_intent: boolean;
  usage_view_state: boolean | null;
}

export type LayerNode =
  | { kind: "Layer"; id: number; children: LayerNode[] }
  | { kind: "Group"; label: string; children: LayerNode[] };

export type OcExpr =
  | { Group: number }
  | { And: OcExpr[] }
  | { Or: OcExpr[] }
  | { Not: OcExpr };

export interface Ocmd {
  id: number;
  policy: string;
  ocgs: number[];
  ve: OcExpr | null;
}

export interface LayerModel {
  layers: Layer[];
  tree: LayerNode[];
  ocmds: Ocmd[];
  rb_groups: number[][];
  has_ocproperties: boolean;
  truncated: boolean;
  /** Groups past the read bound were not read; the view and Save as default are unavailable. */
  layers_truncated: boolean;
  unlisted: number;
  view_supported: boolean;
  view_unsupported_reason: string | null;
  encrypted: boolean;
}

export type LayersRead =
  | { status: "Deferred"; size: number }
  | { status: "Loaded"; model: LayerModel };

// ---------------------------------------------------------------------------
// IPC (camelCase keys: Tauri maps them to the Rust snake_case params)
// ---------------------------------------------------------------------------

export function readLayers(docId: string, force: boolean): Promise<LayersRead> {
  return invoke<LayersRead>("read_layers", { docId, force });
}

/** Show the document with `off` hidden; `null` = the file as it is. */
export function setLayerView(docId: string, off: number[] | null): Promise<void> {
  return invoke<void>("set_layer_view", { docId, off });
}

/** Add a blank layer. Writes the file (like a page operation). Returns the new group's id. */
export function addLayerToFile(docId: string, name: string, parent: number | null): Promise<number> {
  return invoke<number>("add_layer", { docId, name, parent });
}

/** Save `off` as the file's default configuration. Writes the file. */
export function saveLayersDefault(docId: string, off: number[]): Promise<void> {
  return invoke<void>("save_layers_default", { docId, off });
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

/** Groups that affect the view (View intent) and are off in the file's default. */
export function defaultOff(model: LayerModel): Set<number> {
  return new Set(model.layers.filter((l) => l.view_intent && !l.default_on).map((l) => l.id));
}

export function sameSet(a: ReadonlySet<number>, b: ReadonlySet<number>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

function layerById(model: LayerModel): Map<number, Layer> {
  return new Map(model.layers.map((l) => [l.id, l]));
}

/** A group's state in a view; a group without View intent is always on. */
function groupOn(byId: Map<number, Layer>, off: ReadonlySet<number>, id: number): boolean {
  const l = byId.get(id);
  if (!l) return true;
  return l.view_intent ? !off.has(id) : true;
}

const MAX_EVAL_DEPTH = 16;

function evalExpr(byId: Map<number, Layer>, off: ReadonlySet<number>, e: OcExpr, depth: number): boolean {
  if (depth > MAX_EVAL_DEPTH) return true;
  if ("Group" in e) return groupOn(byId, off, e.Group);
  if ("And" in e) return e.And.every((x) => evalExpr(byId, off, x, depth + 1));
  if ("Or" in e) return e.Or.some((x) => evalExpr(byId, off, x, depth + 1));
  return !evalExpr(byId, off, e.Not, depth + 1);
}

/**
 * Whether content that carries `/OC` object `ref` is shown in the view (ISO 32000-1 8.11.2):
 * a group by its state; a membership dictionary by its /VE, else its /P policy over the
 * groups that exist. Anything unknown (a deleted object, a number that is not an optional
 * content object) is shown.
 */
export function isOcShown(model: LayerModel, off: ReadonlySet<number>, ref: number): boolean {
  const byId = layerById(model);
  return ocShown(model, byId, off, ref);
}

function ocShown(model: LayerModel, byId: Map<number, Layer>, off: ReadonlySet<number>, ref: number): boolean {
  if (byId.has(ref)) return groupOn(byId, off, ref);
  const m = model.ocmds.find((o) => o.id === ref);
  if (!m) return true;
  if (m.ve) return evalExpr(byId, off, m.ve, 0);
  const states = m.ocgs.filter((g) => byId.has(g)).map((g) => groupOn(byId, off, g));
  if (states.length === 0) return true; // no effect on visibility
  switch (m.policy) {
    case "AllOn": return states.every((s) => s);
    case "AnyOff": return states.some((s) => !s);
    case "AllOff": return states.every((s) => !s);
    default: return states.some((s) => s); // AnyOn
  }
}

/** The `/OC` object number a mark-up carries, if it is a reference. */
export function markupOcRef(m: { optional_content?: unknown }): number | null {
  const oc = m.optional_content as { Reference?: [number, number] } | null | undefined;
  return oc && Array.isArray(oc.Reference) && Number.isInteger(oc.Reference[0]) ? oc.Reference[0] : null;
}

/** Whether a mark-up is drawn in the view. */
export function isMarkupShown(
  model: LayerModel | null,
  off: ReadonlySet<number>,
  m: { optional_content?: unknown },
): boolean {
  if (!model || off.size === 0 && model.ocmds.length === 0) return true;
  const ref = markupOcRef(m);
  return ref === null ? true : isOcShown(model, off, ref);
}

// ---------------------------------------------------------------------------
// Changing the view
// ---------------------------------------------------------------------------

/** Ids of a node and everything under it. */
export function subtreeIds(n: LayerNode): number[] {
  const out: number[] = [];
  const walk = (x: LayerNode) => {
    if (x.kind === "Layer") out.push(x.id);
    x.children.forEach(walk);
  };
  walk(n);
  return out;
}

export function findNode(tree: LayerNode[], id: number): LayerNode | null {
  for (const n of tree) {
    if (n.kind === "Layer" && n.id === id) return n;
    const r = findNode(n.children, id);
    if (r) return r;
  }
  return null;
}

function changeable(model: LayerModel): Set<number> {
  return new Set(model.layers.filter((l) => !l.locked && l.view_intent).map((l) => l.id));
}

/**
 * Turn `ids` on or off. Locked groups and groups without View intent do not change. Turning
 * a group ON turns the other members of its radio-button group OFF (ISO 32000-1 8.11.4.3).
 */
export function setGroups(model: LayerModel, off: ReadonlySet<number>, ids: number[], on: boolean): Set<number> {
  const ok = changeable(model);
  const next = new Set(off);
  for (const id of ids) {
    if (!ok.has(id)) continue;
    if (on) {
      next.delete(id);
      for (const grp of model.rb_groups) {
        if (!grp.includes(id)) continue;
        for (const other of grp) if (other !== id && ok.has(other)) next.add(other);
      }
    } else {
      next.add(id);
    }
  }
  return next;
}

/** Toggle one layer; with `withChildren`, its sub-layers follow it. */
export function toggleLayer(model: LayerModel, off: ReadonlySet<number>, id: number, withChildren: boolean): Set<number> {
  const node = findNode(model.tree, id);
  const ids = withChildren && node ? subtreeIds(node) : [id];
  const turnOn = off.has(id);
  return setGroups(model, off, ids, turnOn);
}

/** Show only `id` and its sub-layers; hide every other changeable layer ("hide all except"). */
export function isolate(model: LayerModel, off: ReadonlySet<number>, id: number): Set<number> {
  const node = findNode(model.tree, id);
  const keep = new Set(node ? subtreeIds(node) : [id]);
  const ok = changeable(model);
  const next = new Set<number>();
  for (const l of model.layers) {
    if (!ok.has(l.id)) {
      if (off.has(l.id)) next.add(l.id); // locked / no View intent: unchanged
      continue;
    }
    if (!keep.has(l.id)) next.add(l.id);
  }
  return next;
}

export function showAll(model: LayerModel, off: ReadonlySet<number>): Set<number> {
  const ok = changeable(model);
  return new Set([...off].filter((id) => !ok.has(id)));
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

export interface LayerRow {
  depth: number;
  kind: "layer" | "group";
  id: number | null;
  label: string;
  layer: Layer | null;
}

/** The tree flattened to rows in display order, skipping the children of collapsed nodes. */
export function flattenRows(
  model: LayerModel,
  collapsed: ReadonlySet<number | string>,
): LayerRow[] {
  const byId = layerById(model);
  const rows: LayerRow[] = [];
  const walk = (nodes: LayerNode[], depth: number, path: string) => {
    nodes.forEach((n, i) => {
      if (n.kind === "Layer") {
        const l = byId.get(n.id);
        if (!l) return;
        rows.push({ depth, kind: "layer", id: n.id, label: l.name, layer: l });
        if (!collapsed.has(n.id)) walk(n.children, depth + 1, `${path}/${n.id}`);
      } else {
        const key = `g:${path}/${i}:${n.label}`;
        rows.push({ depth, kind: "group", id: null, label: n.label, layer: null });
        if (!collapsed.has(key)) walk(n.children, depth + 1, `${path}/${i}`);
      }
    });
  };
  walk(model.tree, 0, "");
  return rows;
}

/** Layers in the tree that have sub-layers, for the expand/collapse control. */
export function hasChildren(model: LayerModel, id: number): boolean {
  const n = findNode(model.tree, id);
  return !!n && n.children.length > 0;
}
