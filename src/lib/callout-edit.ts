/**
 * Callout editing gestures as pure functions (handle hit-testing and applying a drag), so
 * Viewport.svelte only routes pointer events. PDF user space. Spec: `docs/callout-cloud.md`.
 */
import type { Markup, PdfPoint } from "./ipc";
import {
  type Bounds, type HandleId, HANDLE_IDS, handleAnchors, resizeBounds, translateGeometry,
} from "./markup-select";
import {
  MIN_BOX_PT, calloutBox, leaderHandles, moveBox, resizeBox, setLeaderPoint, translateCallout,
} from "./callout";

/** What a callout drag changes. */
export type CalloutEdit =
  | { kind: "point"; leader: number; index: number }
  | { kind: "box"; start: PdfPoint }
  | { kind: "resize"; handle: HandleId; box: Bounds };

/** What a pointer-down at a point lands on, in priority order. */
export type CalloutHit =
  | { kind: "point"; leader: number; index: number; handle: "tip" | "knee" }
  | { kind: "resize"; handle: HandleId }
  | { kind: "box" };

const near = (a: PdfPoint, b: PdfPoint, tol: number): boolean =>
  Math.abs(a.x - b.x) <= tol && Math.abs(a.y - b.y) <= tol;

/** Hit-test the handles of a (selected) callout: leader tips and knees first, then the box's
 *  eight resize handles, then the box body. `tol` is the grab radius in PDF pt. */
export function hitCallout(m: Markup, p: PdfPoint, tol: number): CalloutHit | null {
  if (m.markup_type !== "Callout") return null;
  for (const h of leaderHandles(m)) {
    if (near(h.point, p, tol)) return { kind: "point", leader: h.leader, index: h.index, handle: h.kind };
  }
  const box = calloutBox(m);
  if (!box) return null;
  const anchors = handleAnchors(box);
  for (const id of HANDLE_IDS) {
    if (near(anchors[id], p, tol)) return { kind: "resize", handle: id };
  }
  if (p.x >= box.minX - tol && p.x <= box.maxX + tol && p.y >= box.minY - tol && p.y <= box.maxY + tol) {
    return { kind: "box" };
  }
  return null;
}

/** The callout as it looks with `edit` dragged to `p` (`before` is the committed markup). */
export function applyCalloutEdit(before: Markup, edit: CalloutEdit, p: PdfPoint): Markup {
  switch (edit.kind) {
    case "point":
      return setLeaderPoint(before, edit.leader, edit.index, p);
    case "box":
      return moveBox(before, p.x - edit.start.x, p.y - edit.start.y);
    case "resize":
      return resizeBox(before, resizeBounds(edit.box, edit.handle, p, MIN_BOX_PT));
  }
}

/** `m` shifted by (dx, dy) - the "move" gesture for any markup (a callout also carries its
 *  box and extra leaders). */
export function translateMarkup(m: Markup, dx: number, dy: number): Markup {
  return m.markup_type === "Callout"
    ? translateCallout(m, dx, dy)
    : { ...m, geometry: translateGeometry(m.geometry, dx, dy) };
}
