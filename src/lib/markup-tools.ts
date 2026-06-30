/**
 * Pure interaction helpers: build markup geometry from pointer gestures (PDF user space)
 * and assemble a Markup envelope. No DOM, no Svelte, no clocks/UUIDs inside — the caller
 * passes `id` + `now` so this stays deterministic and unit-testable. Viewport.svelte does
 * the screen→PDF conversion (via the tested `screenToPdfUserSpace`) before calling these.
 */
import type { Markup, MarkupType, MarkupGeometry, Appearance, UserRef, PdfPoint, CountSet } from "./ipc";
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
    group_id: null,
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
  };
}

