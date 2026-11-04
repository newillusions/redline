/**
 * Sidebar / panel layout model - pure, no Svelte or Tauri dependency.
 *
 * Terminology (owner-set, 2026-09-08): the left/right columns are SIDEBARS; the
 * collapsible modules stacked inside one (Search, Tool Chest, Properties, ...) are PANELS.
 *
 * Owner defect (2026-11-04): panels could not be resized or moved, and an open panel often
 * clipped its content with no way to scroll. This module holds everything about the layout
 * that is worth testing without a DOM: which sidebar each panel sits in and in what order,
 * each panel's share of its sidebar's height (a flex weight), collapsed state, sidebar
 * widths, sidebar/bottom-panel visibility and the bottom panel's height.
 *
 * Persisted as `AppSettings.panel_layout` (opaque JSON on the Rust side). Anything read
 * back goes through `normalizeLayout`, which never throws: an unknown panel id is dropped,
 * a missing one is put back in its default sidebar, and out-of-range sizes are clamped -
 * so an old, hand-edited or future layout value can never leave a panel unreachable.
 */

export type PanelId =
  | "search"
  | "recent"
  | "toolchest"
  | "bookmarks"
  | "links"
  | "navigator"
  | "layers"
  | "history"
  | "properties";

export type Side = "left" | "right";

export const PANEL_IDS: readonly PanelId[] = [
  "search",
  "recent",
  "toolchest",
  "bookmarks",
  "links",
  "navigator",
  "layers",
  "history",
  "properties",
];

export const PANEL_TITLES: Record<PanelId, string> = {
  search: "Search",
  recent: "Recent Documents",
  toolchest: "Tool Chest",
  bookmarks: "Bookmarks",
  links: "Links",
  navigator: "Navigator",
  layers: "Layers",
  history: "History",
  properties: "Properties",
};

export interface SidebarLayout {
  /** Sidebar width in CSS px. */
  width: number;
  /** Whole sidebar hidden (toolbar toggle). */
  hidden: boolean;
  /** Panels in this sidebar, top to bottom. */
  panels: PanelId[];
}

export interface PanelLayout {
  version: 1;
  left: SidebarLayout;
  right: SidebarLayout;
  bottom: { height: number; hidden: boolean };
  /** Flex weight of each expanded panel: its share of the sidebar height relative to the
   *  other expanded panels in the same sidebar. */
  weights: Record<PanelId, number>;
  collapsed: Record<PanelId, boolean>;
}

export const SIDEBAR_MIN_WIDTH = 180;
export const SIDEBAR_MAX_WIDTH = 640;
export const DEFAULT_SIDEBAR_WIDTH = 260;
/** A sidebar never takes more than this share of the window, so the page stays usable. */
export const SIDEBAR_MAX_WINDOW_SHARE = 0.4;

export const BOTTOM_MIN_HEIGHT = 120;
export const BOTTOM_MAX_HEIGHT = 1200;
export const DEFAULT_BOTTOM_HEIGHT = 280;
/** The bottom panel never takes more than this share of the window height. */
export const BOTTOM_MAX_WINDOW_SHARE = 0.7;

/** Smallest height (px, header included) an EXPANDED panel can be dragged down to. */
export const PANEL_MIN_HEIGHT = 96;
export const WEIGHT_MIN = 0.05;
export const WEIGHT_MAX = 50;

/** Keyboard step for resize handles (px); Shift multiplies it. */
export const RESIZE_KEY_STEP = 16;
export const RESIZE_KEY_STEP_LARGE = 64;

const DEFAULT_LEFT: PanelId[] = [
  "search",
  "recent",
  "toolchest",
  "bookmarks",
  "links",
  "navigator",
  "layers",
  "history",
];
const DEFAULT_RIGHT: PanelId[] = ["properties"];

export function defaultLayout(): PanelLayout {
  const weights = Object.fromEntries(PANEL_IDS.map((id) => [id, 1])) as Record<PanelId, number>;
  const collapsed = Object.fromEntries(PANEL_IDS.map((id) => [id, false])) as Record<PanelId, boolean>;
  collapsed.links = true;
  // Search is opened on purpose to read results - give it a bigger share than the
  // reference panels it stacks above (the old stack capped it at 45% of the sidebar).
  weights.search = 3;
  return {
    version: 1,
    left: { width: DEFAULT_SIDEBAR_WIDTH, hidden: false, panels: [...DEFAULT_LEFT] },
    right: { width: DEFAULT_SIDEBAR_WIDTH, hidden: false, panels: [...DEFAULT_RIGHT] },
    bottom: { height: DEFAULT_BOTTOM_HEIGHT, hidden: true },
    weights,
    collapsed,
  };
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

function finiteOr(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function isPanelId(v: unknown): v is PanelId {
  return typeof v === "string" && (PANEL_IDS as readonly string[]).includes(v);
}

export function clampSidebarWidth(width: number, windowWidth?: number): number {
  const max = windowWidth
    ? Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, Math.floor(windowWidth * SIDEBAR_MAX_WINDOW_SHARE)))
    : SIDEBAR_MAX_WIDTH;
  return Math.round(clamp(width, SIDEBAR_MIN_WIDTH, max));
}

export function clampBottomHeight(height: number, windowHeight?: number): number {
  const max = windowHeight
    ? Math.max(BOTTOM_MIN_HEIGHT, Math.min(BOTTOM_MAX_HEIGHT, Math.floor(windowHeight * BOTTOM_MAX_WINDOW_SHARE)))
    : BOTTOM_MAX_HEIGHT;
  return Math.round(clamp(height, BOTTOM_MIN_HEIGHT, max));
}

/** Read any value (persisted JSON, possibly from an older or newer build) as a valid layout. */
/**
 * Why a stored version-1 layout is damaged, or null when its shape is sound. Missing keys are
 * fine (older builds, new panels); a key that is present with the wrong type is not.
 */
export function layoutProblem(raw: unknown): string | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return "not an object";
  const r = raw as Record<string, unknown>;
  const isObj = (v: unknown) => !!v && typeof v === "object" && !Array.isArray(v);
  for (const key of ["left", "right", "bottom", "weights", "collapsed"]) {
    if (r[key] !== undefined && !isObj(r[key])) return `"${key}" is not an object`;
  }
  for (const side of ["left", "right"] as const) {
    const s = r[side] as Record<string, unknown> | undefined;
    if (s && s.panels !== undefined && !Array.isArray(s.panels)) return `"${side}.panels" is not a list`;
  }
  return null;
}

export function normalizeLayout(raw: unknown): PanelLayout {
  const base = defaultLayout();
  if (!raw || typeof raw !== "object") return base;
  const r = raw as Record<string, unknown>;

  const seen = new Set<PanelId>();
  const readSide = (side: Side): SidebarLayout => {
    const s = (r[side] && typeof r[side] === "object" ? r[side] : {}) as Record<string, unknown>;
    const panels: PanelId[] = [];
    if (Array.isArray(s.panels)) {
      for (const id of s.panels) {
        if (isPanelId(id) && !seen.has(id)) {
          seen.add(id);
          panels.push(id);
        }
      }
    }
    return {
      width: clampSidebarWidth(finiteOr(s.width, base[side].width)),
      hidden: typeof s.hidden === "boolean" ? s.hidden : base[side].hidden,
      panels,
    };
  };
  const left = readSide("left");
  const right = readSide("right");
  // Any panel missing from both sidebars goes back where the default layout puts it.
  for (const id of PANEL_IDS) {
    if (seen.has(id)) continue;
    (DEFAULT_RIGHT.includes(id) ? right : left).panels.push(id);
  }

  const b = (r.bottom && typeof r.bottom === "object" ? r.bottom : {}) as Record<string, unknown>;
  const bottom = {
    height: clampBottomHeight(finiteOr(b.height, base.bottom.height)),
    hidden: typeof b.hidden === "boolean" ? b.hidden : base.bottom.hidden,
  };

  const w = (r.weights && typeof r.weights === "object" ? r.weights : {}) as Record<string, unknown>;
  const c = (r.collapsed && typeof r.collapsed === "object" ? r.collapsed : {}) as Record<string, unknown>;
  const weights = { ...base.weights };
  const collapsed = { ...base.collapsed };
  for (const id of PANEL_IDS) {
    weights[id] = clamp(finiteOr(w[id], base.weights[id]), WEIGHT_MIN, WEIGHT_MAX);
    if (typeof c[id] === "boolean") collapsed[id] = c[id] as boolean;
  }

  return { version: 1, left, right, bottom, weights, collapsed };
}

export function sideOf(layout: PanelLayout, id: PanelId): Side {
  return layout.right.panels.includes(id) ? "right" : "left";
}

/**
 * Move `id` to `side`, inserted at `index` in that sidebar's list as it reads AFTER the
 * panel has been taken out of its old place (so "index 0" is always "top of the sidebar").
 * The index is clamped; the input layout is not mutated.
 */
export function movePanel(layout: PanelLayout, id: PanelId, side: Side, index: number): PanelLayout {
  const left = layout.left.panels.filter((p) => p !== id);
  const right = layout.right.panels.filter((p) => p !== id);
  const target = side === "left" ? left : right;
  target.splice(clamp(Math.round(index), 0, target.length), 0, id);
  return {
    ...layout,
    left: { ...layout.left, panels: left },
    right: { ...layout.right, panels: right },
    // A panel moved into a hidden sidebar would vanish - show the sidebar it lands in.
    [side]: { ...layout[side], hidden: false, panels: target },
  } as PanelLayout;
}

/** Move `id` up (-1) or down (+1) within its own sidebar. No-op at either end. */
export function movePanelBy(layout: PanelLayout, id: PanelId, delta: -1 | 1): PanelLayout {
  const side = sideOf(layout, id);
  const list = layout[side].panels;
  const from = list.indexOf(id);
  const to = from + delta;
  if (from === -1 || to < 0 || to >= list.length) return layout;
  return movePanel(layout, id, side, to);
}

export function setCollapsed(layout: PanelLayout, id: PanelId, collapsed: boolean): PanelLayout {
  return { ...layout, collapsed: { ...layout.collapsed, [id]: collapsed } };
}

export function setSidebarWidth(layout: PanelLayout, side: Side, width: number, windowWidth?: number): PanelLayout {
  return { ...layout, [side]: { ...layout[side], width: clampSidebarWidth(width, windowWidth) } } as PanelLayout;
}

export function setSidebarHidden(layout: PanelLayout, side: Side, hidden: boolean): PanelLayout {
  return { ...layout, [side]: { ...layout[side], hidden } } as PanelLayout;
}

export function setBottomHeight(layout: PanelLayout, height: number, windowHeight?: number): PanelLayout {
  return { ...layout, bottom: { ...layout.bottom, height: clampBottomHeight(height, windowHeight) } };
}

export function setBottomHidden(layout: PanelLayout, hidden: boolean): PanelLayout {
  return { ...layout, bottom: { ...layout.bottom, hidden } };
}

/**
 * Resize the split between two adjacent expanded panels A (above) and B (below).
 *
 * `heightA`/`heightB` are their current rendered heights; `delta` is how far the split
 * moved (positive = down, A grows). A and B keep their combined height and combined
 * weight - only the share between them changes - so no other panel in the sidebar moves.
 * Neither panel goes below `minHeight` (if they cannot both fit, nothing changes).
 */
export function splitWeights(
  weightA: number,
  weightB: number,
  heightA: number,
  heightB: number,
  delta: number,
  minHeight = PANEL_MIN_HEIGHT,
): [number, number] {
  const total = heightA + heightB;
  if (!(total > 0) || total < 2 * minHeight) return [weightA, weightB];
  const nextA = clamp(heightA + delta, minHeight, total - minHeight);
  const sum = weightA + weightB;
  const a = clamp((sum * nextA) / total, WEIGHT_MIN, WEIGHT_MAX);
  const b = clamp(sum - a, WEIGHT_MIN, WEIGHT_MAX);
  return [a, b];
}

export function setWeights(layout: PanelLayout, a: PanelId, wa: number, b: PanelId, wb: number): PanelLayout {
  return { ...layout, weights: { ...layout.weights, [a]: wa, [b]: wb } };
}

/**
 * Where a dragged panel lands. `frames` are the panels currently shown in the sidebar the
 * pointer is over, top to bottom, with their vertical midpoints; the dragged panel itself
 * may be among them. Returns the insertion index in that sidebar's list with the dragged
 * panel removed (the form `movePanel` takes).
 */
export function dropIndex(frames: { id: PanelId; mid: number }[], dragged: PanelId, pointerY: number): number {
  const others = frames.filter((f) => f.id !== dragged);
  const i = others.findIndex((f) => pointerY < f.mid);
  return i === -1 ? others.length : i;
}
