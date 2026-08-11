/**
 * Pure interaction helpers: build markup geometry from pointer gestures (PDF user space)
 * and assemble a Markup envelope. No DOM, no Svelte, no clocks/UUIDs inside — the caller
 * passes `id` + `now` so this stays deterministic and unit-testable. Viewport.svelte does
 * the screen→PDF conversion (via the tested `screenToPdfUserSpace`) before calling these.
 */
import type {
  Markup,
  MarkupType,
  MarkupGeometry,
  Appearance,
  UserRef,
  PdfPoint,
  CountSet,
  StampAsset,
  DynamicField,
} from "./ipc";
import type { ToolKind } from "./markup-store.svelte";

/** The drag-draw tools — a subset of MarkupType (so no cast is needed at the call site). */
export type DrawTool = Extract<MarkupType, "Rectangle" | "Ellipse" | "Line" | "Arrow" | "Highlight">;

/** All drag-draw tools (press-drag-release gesture). */
export const DRAW_TOOLS: ReadonlySet<DrawTool> = new Set<DrawTool>(["Rectangle", "Ellipse", "Line", "Arrow", "Highlight"]);

/** Narrowing guard: true when the active tool is a drag-draw tool (narrows to DrawTool). */
export function isDrawTool(t: ToolKind): t is DrawTool {
  return (DRAW_TOOLS as ReadonlySet<string>).has(t);
}

/** Drag-draw tools whose geometry is an axis-aligned bounding Rect. */
export const RECT_TOOLS: ReadonlySet<ToolKind> = new Set<ToolKind>(["Rectangle", "Ellipse", "Highlight"]);

/**
 * Build geometry for a drag-draw tool from two PDF-space points (press + release).
 * When `opts.constrain` is true and the tool is a RECT_TOOL, the bounding rect is
 * constrained to a square: both axes use the larger of |dx|, |dy|, preserving sign
 * so the shape grows in the actual drag direction. Supports Shift-to-square/circle.
 */
export function dragDrawGeometry(
  tool: ToolKind,
  a: PdfPoint,
  b: PdfPoint,
  opts?: { constrain?: boolean },
): MarkupGeometry {
  if (RECT_TOOLS.has(tool)) {
    let bx = b.x, by = b.y;
    if (opts?.constrain) {
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const size = Math.max(Math.abs(dx), Math.abs(dy));
      bx = a.x + Math.sign(dx) * size;
      by = a.y + Math.sign(dy) * size;
    }
    return {
      Rect: {
        min: { x: Math.min(a.x, bx), y: Math.min(a.y, by) },
        max: { x: Math.max(a.x, bx), y: Math.max(a.y, by) },
      },
    };
  }
  return { Polyline: [a, b] }; // Line / Arrow
}

/** Multi-click polyline-family tools (click per vertex; closed for Polygon/Cloud). */
export type MultiClickTool = Extract<MarkupType, "Polyline" | "Polygon" | "Cloud">;
export const MULTI_CLICK_TOOLS: ReadonlySet<MultiClickTool> =
  new Set<MultiClickTool>(["Polyline", "Polygon", "Cloud"]);
export function isMultiClickTool(t: ToolKind): t is MultiClickTool {
  return (MULTI_CLICK_TOOLS as ReadonlySet<string>).has(t);
}
export function isInkTool(t: ToolKind): t is Extract<MarkupType, "Ink"> {
  return t === "Ink";
}

/** Minimum vertices before a multi-click shape can be committed. */
export function minVertices(tool: MultiClickTool): number {
  return tool === "Polyline" ? 2 : 3; // Polygon / Cloud are closed -> need 3
}
export function isMultiClickComplete(tool: MultiClickTool, verts: PdfPoint[]): boolean {
  return verts.length >= minVertices(tool);
}

/** Geometry builders (defensive copies — callers mutate their working arrays). */
export function polylineGeometry(verts: PdfPoint[]): MarkupGeometry {
  return { Polyline: verts.map((p) => ({ x: p.x, y: p.y })) };
}
export function inkGeometry(strokes: PdfPoint[][]): MarkupGeometry {
  return { Ink: strokes.map((s) => s.map((p) => ({ x: p.x, y: p.y }))) };
}

/** Text-entry tools (inline textarea commits contents + font). */
export type TextTool = Extract<MarkupType, "Text" | "Callout">;
export const TEXT_TOOLS: ReadonlySet<TextTool> = new Set<TextTool>(["Text", "Callout"]);
export function isTextTool(t: ToolKind): t is TextTool {
  return (TEXT_TOOLS as ReadonlySet<string>).has(t);
}

/** Default font for new text/callout markups (G7 adds the picker). */
export const DEFAULT_TEXT_FONT = { family: "Helvetica", size_pt: 12 } as const;

/** Default text-box size in PDF points (≈2in × ~1 line @12pt). */
export const DEFAULT_TEXT_BOX = { width: 144, height: 18 } as const;

/** Build a Text-box Rect from a top-left anchor (PDF user space, y-up). */
export function textBoxGeometry(anchor: PdfPoint, box: { width: number; height: number } = DEFAULT_TEXT_BOX): MarkupGeometry {
  return {
    Rect: {
      min: { x: anchor.x, y: anchor.y - box.height },
      max: { x: anchor.x + box.width, y: anchor.y },
    },
  };
}

/** Build a Callout leader Polyline from the target point to the text anchor (anchor last). */
export function calloutGeometry(target: PdfPoint, anchor: PdfPoint): MarkupGeometry {
  return { Polyline: [{ x: target.x, y: target.y }, { x: anchor.x, y: anchor.y }] };
}

/**
 * Translate a Drawing-mode Tool's fixed geometry template so a copy lands at `clickPoint`
 * (Tool Chest "drop an exact copy" placement mode). The anchor used per geometry variant:
 *  - `Point`: the point itself (translated copy == clickPoint).
 *  - `Rect`: the bounding box's min (bottom-left) corner - size/shape preserved.
 *  - `Polyline` / `Ink`: the bounding box min corner across all vertices/strokes.
 *  - `Quads`: NAMED simplification - returned untouched (text-anchored-highlight geometry
 *    is not a realistic Drawing-mode tool template; translating it correctly needs a
 *    different anchor convention this MVP does not need to solve).
 */
export function translateToolGeometry(template: MarkupGeometry, clickPoint: PdfPoint): MarkupGeometry {
  const delta = toolPlacementDelta(template, clickPoint);
  if (!delta) return template; // Quads - see doc comment.
  return shiftGeometry(template, delta.dx, delta.dy);
}

/**
 * The anchor point `translateToolGeometry` uses for `template`'s geometry variant (see
 * that function's doc comment for the per-variant convention), or `null` for Quads
 * (no anchor convention defined - see doc comment).
 */
function toolTemplateAnchor(template: MarkupGeometry): PdfPoint | null {
  if ("Point" in template) return template.Point;
  if ("Rect" in template) return template.Rect.min;
  if ("Polyline" in template) return bboxMin(template.Polyline);
  if ("Ink" in template) return bboxMin(template.Ink.flat());
  return null; // Quads
}

/**
 * The `(dx, dy)` translation that `translateToolGeometry(template, clickPoint)` would
 * apply, exposed separately so a GROUPED tool's placement (design doc
 * `docs/design/2026-08-11-grouped-markups.md` §4) can compute it ONCE from the parent
 * tool's own template and apply the SAME delta uniformly to every child's geometry too
 * (via `shiftGeometry`, this module) - never each child's own independent anchor, which
 * would collapse every member on top of the click point instead of preserving the
 * group's relative layout. `null` for Quads (no anchor convention).
 */
export function toolPlacementDelta(template: MarkupGeometry, clickPoint: PdfPoint): { dx: number; dy: number } | null {
  const anchor = toolTemplateAnchor(template);
  if (!anchor) return null;
  return { dx: clickPoint.x - anchor.x, dy: clickPoint.y - anchor.y };
}

/**
 * Shift every coordinate in `g` by a fixed `(dx, dy)` - the uniform-translation
 * primitive `translateToolGeometry`/`toolPlacementDelta` build on. Deliberately NOT
 * imported from `markup-select.ts`'s equivalent `translateGeometry` (which already
 * exists there) to avoid a circular module dependency - `markup-select.ts` already
 * imports `DEFAULT_TEXT_BOX` from this file. Same Quads behaviour as the rest of this
 * module: passed through untouched (no defined anchor/shift convention for it here).
 * Exported so grouped-tool placement (`Viewport.svelte`) can apply the SAME delta to
 * every child's geometry, not just the parent's.
 */
export function shiftGeometry(g: MarkupGeometry, dx: number, dy: number): MarkupGeometry {
  const apply = (p: PdfPoint): PdfPoint => ({ x: p.x + dx, y: p.y + dy });
  if ("Point" in g) return { Point: apply(g.Point) };
  if ("Rect" in g) return { Rect: { min: apply(g.Rect.min), max: apply(g.Rect.max) } };
  if ("Polyline" in g) return { Polyline: g.Polyline.map(apply) };
  if ("Ink" in g) return { Ink: g.Ink.map((s) => s.map(apply)) };
  return g; // Quads
}

/**
 * Extract the `label`s of every `PromptedText` field in `fields`, in order (spec "Stamps" -
 * a dynamic stamp's placement-time prompt UI collects one value per `PromptedText` field,
 * in the same order `compose_stamp_text`'s `prompted` array expects them back). Pure/no-DOM
 * so the placement-time prompt-or-skip decision is unit-testable without a rendered dialog.
 */
export function extractPromptedLabels(fields: DynamicField[]): string[] {
  return fields
    .filter((f): f is { PromptedText: { label: string } } => typeof f === "object" && "PromptedText" in f)
    .map((f) => f.PromptedText.label);
}

function bboxMin(pts: PdfPoint[]): PdfPoint {
  if (pts.length === 0) return { x: 0, y: 0 };
  return { x: Math.min(...pts.map((p) => p.x)), y: Math.min(...pts.map((p) => p.y)) };
}

/**
 * Return a clone of `m` with the audit trail advanced for an edit: `modified_by`/
 * `modified_at` refreshed and `revision` incremented. `created_by`/`created_at` are
 * preserved. Used on every edit commit (move/resize now; G7 properties). No mutation.
 */
export function bumpAudit(m: Markup, by: UserRef, now: string): Markup {
  return {
    ...m,
    audit: { ...m.audit, modified_by: by, modified_at: now, revision: m.audit.revision + 1 },
  };
}

/** Assemble a fresh markup envelope. `id` (UUID) and `now` (ISO-8601) are injected. */
export function buildMarkup(opts: {
  markupType: MarkupType;
  page: number;
  geometry: MarkupGeometry;
  appearance: Appearance;
  identity: UserRef;
  now: string;
  id: string;
  contents?: string | null;
  /** Count set assignment (MeasurementCount only). Embedded so it round-trips via the PDF. */
  countSet?: CountSet | null;
  /** Stamp's backing visual asset snapshot (Stamp/StampDynamic placement only). */
  stampAsset?: StampAsset | null;
  /** Group id (G8, `/RLGroup` + real `/IRT`+`/RT /Group` on save) - set when this
   *  markup is one member of a grouped-tool placement (design doc
   *  `docs/design/2026-08-11-grouped-markups.md` §4). `null`/omitted for an
   *  ordinary ungrouped markup, matching every prior call site. */
  groupId?: string | null;
}): Markup {
  return {
    id: opts.id,
    markup_type: opts.markupType,
    page: opts.page,
    geometry: opts.geometry,
    // Deep-clone appearance so each markup owns its own object. Without this,
    // all markups created from the same draftAppearance share one reference and
    // any in-place mutation (e.g. Object.assign on the store's draft) silently
    // changes every existing markup's appearance.
    appearance: {
      ...opts.appearance,
      font: opts.appearance.font ? { ...opts.appearance.font } : opts.appearance.font,
    },
    subject: null,
    layer: null,
    contents: opts.contents ?? null,
    group_id: opts.groupId ?? null,
    audit: {
      created_by: opts.identity,
      created_at: opts.now,
      modified_by: opts.identity,
      modified_at: opts.now,
      revision: 0,
      origin: "Desktop",
    },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
    count_set: opts.countSet ? { ...opts.countSet } : null,
    stamp_asset: opts.stampAsset ?? null,
  };
}

