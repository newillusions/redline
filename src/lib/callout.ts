/**
 * Pure Callout / Cloud+ helpers: the text box, leaders (tip, knee, attach), extra leaders,
 * auto-size, and cloud construction. PDF user space (y-up), no DOM. Behaviour spec:
 * `docs/callout-cloud.md`. Mirrors Rust `markup::callout`.
 *
 * Leader convention: a leader is `[tip, (knee,) attach]` - the arrowhead is at the tip, the
 * last point lies on the text box. The primary leader is the markup's `Polyline` geometry;
 * extra leaders live in `callout.extra_leaders`.
 */
import type { CalloutProps, Markup, MarkupGeometry, PdfPoint } from "./ipc";
import type { Bounds } from "./markup-select";

/** Text box of a callout with no `callout` props, PDF pt (mirrors Rust `LEGACY_BOX`). */
export const CALLOUT_LEGACY_BOX = { width: 144, height: 18 } as const;
/** Caps mirrored from Rust `markup::callout`. */
export const MAX_EXTRA_LEADERS = 16;
export const MAX_LEADER_POINTS = 3;
export const MAX_BOX_SIDE = 20_000;
/** Length of the last leader segment on a default knee, PDF pt. */
export const KNEE_RUN_PT = 18;
/** Where a new extra leader's tip lands relative to the primary tip, PDF pt. */
export const NEW_LEADER_OFFSET_PT = 36;
/** Smallest text box side a resize may produce, PDF pt. */
export const MIN_BOX_PT = 12;

const BOX_PAD_X = 4;
const BOX_PAD_Y = 2;
const AVG_GLYPH_EM = 0.55;
const LINE_HEIGHT_EM = 1.2;

const finite = (p: PdfPoint): boolean => Number.isFinite(p.x) && Number.isFinite(p.y);
const copy = (p: PdfPoint): PdfPoint => ({ x: p.x, y: p.y });

/** A leader reduced to at most MAX_LEADER_POINTS finite points (first, middle, last) -
 *  mirrors Rust `callout::compress`, so the preview matches what is saved. */
function compress(pts: PdfPoint[]): PdfPoint[] {
  const p = pts.filter(finite);
  return p.length <= MAX_LEADER_POINTS ? p : [p[0], p[Math.floor(p.length / 2)], p[p.length - 1]];
}

/** The primary leader (the Polyline geometry), finite points only, capped at 3 points. */
export function primaryLeader(m: Markup): PdfPoint[] {
  return "Polyline" in m.geometry ? compress(m.geometry.Polyline) : [];
}

/** Extra leaders (valid ones only: >= 2 finite points, capped). */
export function extraLeaders(m: Markup): PdfPoint[][] {
  return (m.callout?.extra_leaders ?? [])
    .map((l) => l.filter(finite))
    .filter((l) => l.length >= 2)
    .map(compress)
    .slice(0, MAX_EXTRA_LEADERS);
}

/** Every leader: primary first. */
export function allLeaders(m: Markup): PdfPoint[][] {
  const p = primaryLeader(m);
  return [...(p.length ? [p] : []), ...extraLeaders(m)];
}

function propsBox(p: CalloutProps): Bounds | null {
  const { box_min: a, box_max: b } = p;
  if (!finite(a) || !finite(b)) return null;
  const box = {
    minX: Math.min(a.x, b.x), minY: Math.min(a.y, b.y),
    maxX: Math.max(a.x, b.x), maxY: Math.max(a.y, b.y),
  };
  const w = box.maxX - box.minX, h = box.maxY - box.minY;
  return w > 0 && h > 0 && w <= MAX_BOX_SIDE && h <= MAX_BOX_SIDE ? box : null;
}

/**
 * The callout's text box in PDF space. With props: their box. Without: the legacy preview
 * box - the leader's last point is the box's top-left corner and it extends right and down.
 * `null` for a non-Callout or one with no usable leader.
 */
export function calloutBox(m: Markup): Bounds | null {
  if (m.markup_type !== "Callout") return null;
  const fromProps = m.callout ? propsBox(m.callout) : null;
  if (fromProps) return fromProps;
  const anchor = primaryLeader(m).at(-1);
  if (!anchor) return null;
  return {
    minX: anchor.x, minY: anchor.y - CALLOUT_LEGACY_BOX.height,
    maxX: anchor.x + CALLOUT_LEGACY_BOX.width, maxY: anchor.y,
  };
}

// --- text box sizing ----------------------------------------------------------------

/** Box size that fits `text` at `fontPt` (estimate: Helvetica-like average glyph width,
 *  no wrapping - the appearance draws one line per `\n`). */
export function fitBoxSize(text: string | null, fontPt: number): { width: number; height: number } {
  const lines = (text ?? "").split("\n");
  const longest = lines.reduce((n, l) => Math.max(n, l.length), 0);
  const width = longest * fontPt * AVG_GLYPH_EM + 2 * BOX_PAD_X;
  const height = lines.length * fontPt * LINE_HEIGHT_EM + 2 * BOX_PAD_Y;
  return {
    width: Math.min(MAX_BOX_SIDE, Math.max(MIN_BOX_PT * 2, width)),
    height: Math.min(MAX_BOX_SIDE, Math.max(MIN_BOX_PT, height)),
  };
}

// --- attach point and knee ------------------------------------------------------------

/** A point on the box edge for a leader coming from `target`: the middle of the side that
 *  faces the target (left/right when the target is beside the box, else top/bottom). */
export function attachPoint(box: Bounds, target: PdfPoint): PdfPoint {
  const midX = (box.minX + box.maxX) / 2, midY = (box.minY + box.maxY) / 2;
  if (target.x < box.minX) return { x: box.minX, y: midY };
  if (target.x > box.maxX) return { x: box.maxX, y: midY };
  return { x: midX, y: target.y >= midY ? box.maxY : box.minY };
}

/** Default knee: a short run straight out of the box side, so the leader bends once. */
export function defaultKnee(target: PdfPoint, attach: PdfPoint, box: Bounds): PdfPoint {
  const side = attach.x <= box.minX ? "l" : attach.x >= box.maxX ? "r" : attach.y >= box.maxY ? "t" : "b";
  const run = (d: number) => Math.min(KNEE_RUN_PT, Math.abs(d) / 2);
  switch (side) {
    case "l": return { x: attach.x - run(attach.x - target.x), y: attach.y };
    case "r": return { x: attach.x + run(target.x - attach.x), y: attach.y };
    case "t": return { x: attach.x, y: attach.y + run(target.y - attach.y) };
    default: return { x: attach.x, y: attach.y - run(attach.y - target.y) };
  }
}

/** Pull `p` onto the nearest edge of `box` (a point outside is clamped to it first). */
export function snapToBoxEdge(box: Bounds, p: PdfPoint): PdfPoint {
  const cx = Math.min(box.maxX, Math.max(box.minX, p.x));
  const cy = Math.min(box.maxY, Math.max(box.minY, p.y));
  const dl = cx - box.minX, dr = box.maxX - cx, db = cy - box.minY, dt = box.maxY - cy;
  const m = Math.min(dl, dr, db, dt);
  if (m > 0) {
    if (m === dl) return { x: box.minX, y: cy };
    if (m === dr) return { x: box.maxX, y: cy };
    if (m === db) return { x: cx, y: box.minY };
    return { x: cx, y: box.maxY };
  }
  return { x: cx, y: cy };
}

// --- construction -----------------------------------------------------------------------

/** Geometry + props for a new callout: `target` is the arrow tip, `topLeft` the box's
 *  top-left corner (the second click), `size` the box size (see {@link fitBoxSize}). */
export function newCallout(
  target: PdfPoint,
  topLeft: PdfPoint,
  size: { width: number; height: number },
): { geometry: MarkupGeometry; callout: CalloutProps } {
  const box: Bounds = {
    minX: topLeft.x, maxX: topLeft.x + size.width,
    maxY: topLeft.y, minY: topLeft.y - size.height,
  };
  const attach = attachPoint(box, target);
  const knee = defaultKnee(target, attach, box);
  return {
    geometry: { Polyline: [copy(target), knee, attach] },
    callout: {
      box_min: { x: box.minX, y: box.minY }, box_max: { x: box.maxX, y: box.maxY },
      extra_leaders: [], autosize: true,
    },
  };
}

/** The markup with props materialised (a legacy callout gets its current box as props). */
function withProps(m: Markup): Markup & { callout: CalloutProps } {
  if (m.callout) return m as Markup & { callout: CalloutProps };
  const box = calloutBox(m) ?? { minX: 0, minY: 0, maxX: CALLOUT_LEGACY_BOX.width, maxY: CALLOUT_LEGACY_BOX.height };
  return {
    ...m,
    callout: {
      box_min: { x: box.minX, y: box.minY }, box_max: { x: box.maxX, y: box.maxY },
      extra_leaders: [], autosize: false,
    },
  };
}

function setLeaders(m: Markup, leaders: PdfPoint[][]): Markup {
  const [primary, ...rest] = leaders;
  const geometry: MarkupGeometry = { Polyline: primary ?? primaryLeader(m) };
  // Editing only the primary leader of a callout with no props leaves it without props.
  if (rest.length === 0 && !m.callout) return { ...m, geometry };
  const base = withProps(m);
  return { ...base, geometry, callout: { ...base.callout, extra_leaders: rest } };
}

function mapAttach(leaders: PdfPoint[][], fn: (p: PdfPoint) => PdfPoint): PdfPoint[][] {
  return leaders.map((l) => l.map((p, i) => (i === l.length - 1 ? fn(p) : p)));
}

// --- editing -----------------------------------------------------------------------------

/** Shift the whole callout (all leaders and the box) - the "move" gesture. */
export function translateCallout(m: Markup, dx: number, dy: number): Markup {
  const sh = (p: PdfPoint): PdfPoint => ({ x: p.x + dx, y: p.y + dy });
  const out: Markup = { ...m };
  if ("Polyline" in m.geometry) out.geometry = { Polyline: m.geometry.Polyline.map(sh) };
  if (m.callout) {
    out.callout = {
      ...m.callout,
      box_min: sh(m.callout.box_min), box_max: sh(m.callout.box_max),
      extra_leaders: m.callout.extra_leaders.map((l) => l.map(sh)),
    };
  }
  return out;
}

/** Move one leader point (`leader` 0 = primary). The last point stays on the box edge. */
export function setLeaderPoint(m: Markup, leader: number, index: number, p: PdfPoint): Markup {
  const leaders = allLeaders(m).map((l) => l.map(copy));
  const target = leaders[leader];
  if (!target || index < 0 || index >= target.length || !finite(p)) return m;
  const box = calloutBox(m);
  target[index] = index === target.length - 1 && box ? snapToBoxEdge(box, p) : copy(p);
  return setLeaders(m, leaders);
}

/** Drag the text box by (dx, dy): the box and every leader's attach point move; tips and
 *  knees stay. */
export function moveBox(m: Markup, dx: number, dy: number): Markup {
  const base = withProps(m);
  const sh = (p: PdfPoint): PdfPoint => ({ x: p.x + dx, y: p.y + dy });
  const moved = {
    ...base,
    callout: { ...base.callout, box_min: sh(base.callout.box_min), box_max: sh(base.callout.box_max) },
  };
  return setLeaders(moved, mapAttach(allLeaders(m).map((l) => l.map(copy)), sh));
}

/** Resize the text box to `box`: auto-size turns off, attach points are pulled onto the new
 *  edges. */
export function resizeBox(m: Markup, box: Bounds): Markup {
  const base = withProps(m);
  const resized: Markup = {
    ...base,
    callout: {
      ...base.callout,
      box_min: { x: box.minX, y: box.minY }, box_max: { x: box.maxX, y: box.maxY },
      autosize: false,
    },
  };
  return setLeaders(resized, mapAttach(allLeaders(m).map((l) => l.map(copy)), (p) => snapToBoxEdge(box, p)));
}

/** Refit the box to the contents when auto-size is on, keeping its top-left corner. */
export function refitBox(m: Markup, fontPt: number): Markup {
  if (!m.callout?.autosize) return m;
  const box = calloutBox(m);
  if (!box) return m;
  const size = fitBoxSize(m.contents, fontPt);
  const next: Bounds = { minX: box.minX, maxY: box.maxY, maxX: box.minX + size.width, minY: box.maxY - size.height };
  const refit = resizeBox(m, next);
  return { ...refit, callout: { ...refit.callout!, autosize: true } };
}

/** Turn auto-size on (refitting) or off. */
export function setAutosize(m: Markup, on: boolean, fontPt: number): Markup {
  const base = withProps(m);
  const flagged: Markup = { ...base, callout: { ...base.callout, autosize: on } };
  return on ? refitBox(flagged, fontPt) : flagged;
}

/** Add a leader starting at the primary leader's knee (or attach point when it has none),
 *  with its tip offset from the primary tip. `null` at the leader cap. */
export function addLeader(m: Markup, tip?: PdfPoint): Markup | null {
  const leaders = allLeaders(m);
  const primary = leaders[0];
  if (!primary || leaders.length - 1 >= MAX_EXTRA_LEADERS) return null;
  const from = primary[0];
  const t = tip ?? { x: from.x + NEW_LEADER_OFFSET_PT, y: from.y + NEW_LEADER_OFFSET_PT };
  const shared = primary.slice(1); // knee (if any) and attach
  return setLeaders(m, [...leaders.map((l) => l.map(copy)), [copy(t), ...shared.map(copy)]]);
}

/** Remove leader `leader` (0 = primary: the first extra becomes primary). The last
 *  remaining leader cannot be removed. */
export function removeLeader(m: Markup, leader: number): Markup {
  const leaders = allLeaders(m);
  if (leaders.length < 2 || leader < 0 || leader >= leaders.length) return m;
  return setLeaders(m, leaders.filter((_, i) => i !== leader).map((l) => l.map(copy)));
}

/** A draggable point of a leader: its tip, or its knee (the middle of a 3-point leader). */
export interface LeaderHandle { leader: number; index: number; kind: "tip" | "knee"; point: PdfPoint }

export function leaderHandles(m: Markup): LeaderHandle[] {
  const out: LeaderHandle[] = [];
  allLeaders(m).forEach((l, leader) => {
    out.push({ leader, index: 0, kind: "tip", point: l[0] });
    if (l.length === 3) out.push({ leader, index: 1, kind: "knee", point: l[1] });
  });
  return out;
}

// --- cloud ------------------------------------------------------------------------------

/** Smallest side of a drag-rectangle cloud, PDF pt. */
export const MIN_RECT_CLOUD_PT = 4;

/** Cloud polygon for a drag from `a` to `b` (4 corners, counter-clockwise in PDF space);
 *  `null` when the drag is too small to be a rectangle. */
export function rectCloudVertices(a: PdfPoint, b: PdfPoint): PdfPoint[] | null {
  if (!finite(a) || !finite(b)) return null;
  const minX = Math.min(a.x, b.x), maxX = Math.max(a.x, b.x);
  const minY = Math.min(a.y, b.y), maxY = Math.max(a.y, b.y);
  if (maxX - minX < MIN_RECT_CLOUD_PT || maxY - minY < MIN_RECT_CLOUD_PT) return null;
  return [{ x: minX, y: minY }, { x: maxX, y: minY }, { x: maxX, y: maxY }, { x: minX, y: maxY }];
}

/** Closest point on the closed polygon `poly` to `p`. */
export function nearestOnPolygon(poly: PdfPoint[], p: PdfPoint): PdfPoint | null {
  let best: PdfPoint | null = null;
  let bestD = Infinity;
  for (let i = 0; i < poly.length; i++) {
    const a = poly[i], b = poly[(i + 1) % poly.length];
    const dx = b.x - a.x, dy = b.y - a.y;
    const len2 = dx * dx + dy * dy;
    const t = len2 === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2));
    const q = { x: a.x + t * dx, y: a.y + t * dy };
    const d = Math.hypot(p.x - q.x, p.y - q.y);
    if (d < bestD) { bestD = d; best = q; }
  }
  return best;
}
