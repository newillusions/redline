/**
 * Callout / Cloud+ pure helpers (callout.ts, callout-edit.ts) and the bounded cloud path.
 * Behaviour spec: docs/callout-cloud.md. PDF user space, y-up.
 */
import { describe, it, expect } from "vitest";
import type { Markup, PdfPoint } from "./ipc";
import {
  MAX_EXTRA_LEADERS, addLeader, allLeaders, attachPoint, calloutBox, defaultKnee, fitBoxSize,
  leaderHandles, moveBox, nearestOnPolygon, newCallout, rectCloudVertices, refitBox,
  removeLeader, resizeBox, setAutosize, setLeaderPoint, snapToBoxEdge, translateCallout,
} from "./callout";
import { applyCalloutEdit, hitCallout, translateMarkup } from "./callout-edit";
import { MAX_CLOUD_BUMPS, cloudPath } from "./markup-render";
import { boundsOf, hitTest } from "./markup-select";

const USER = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "T" };
function callout(over: Partial<Markup> = {}): Markup {
  const made = newCallout({ x: 10, y: 100 }, { x: 100, y: 80 }, { width: 60, height: 20 });
  return {
    id: "c1", markup_type: "Callout", page: 0, geometry: made.geometry, callout: made.callout,
    appearance: { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: { family: "Helvetica", size_pt: 12 } },
    subject: null, layer: null, contents: "hello", group_id: null,
    audit: { created_by: USER, created_at: "t", modified_by: USER, modified_at: "t", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] }, measurement: null,
    ...over,
  };
}
const poly = (m: Markup): PdfPoint[] => ("Polyline" in m.geometry ? m.geometry.Polyline : []);

describe("newCallout", () => {
  it("builds [tip, knee, attach] with the attach on the box side facing the tip", () => {
    const m = callout();
    const [tip, knee, attach] = poly(m);
    expect(tip).toEqual({ x: 10, y: 100 });
    expect(attach).toEqual({ x: 100, y: 70 }); // left edge, mid-height of box x:[100,160] y:[60,80]
    expect(knee).toEqual({ x: 82, y: 70 });    // 18pt run straight out of the box side
    expect(calloutBox(m)).toEqual({ minX: 100, minY: 60, maxX: 160, maxY: 80 });
    expect(m.callout!.autosize).toBe(true);
  });
  it("attaches to the top or bottom when the tip is above or below the box", () => {
    const box = { minX: 0, minY: 0, maxX: 100, maxY: 20 };
    expect(attachPoint(box, { x: 50, y: 90 })).toEqual({ x: 50, y: 20 });
    expect(attachPoint(box, { x: 50, y: -40 })).toEqual({ x: 50, y: 0 });
    expect(defaultKnee({ x: 50, y: 90 }, { x: 50, y: 20 }, box)).toEqual({ x: 50, y: 38 });
  });
});

describe("legacy callouts (no props)", () => {
  it("keep the fixed box at the anchor, top-left, until a box edit materialises props", () => {
    const m = callout({ callout: null, geometry: { Polyline: [{ x: 0, y: 0 }, { x: 50, y: 60 }] } });
    expect(calloutBox(m)).toEqual({ minX: 50, minY: 42, maxX: 194, maxY: 60 });
    const tip = setLeaderPoint(m, 0, 0, { x: 5, y: 5 });
    expect(tip.callout).toBeNull(); // a tip drag alone does not add props
    const moved = moveBox(m, 10, 0);
    expect(moved.callout!.box_min).toEqual({ x: 60, y: 42 });
    expect(poly(moved)[1]).toEqual({ x: 60, y: 60 });
  });
});

describe("knee and tip editing", () => {
  it("moves the knee and the tip independently; the attach point stays on the box edge", () => {
    const m = callout();
    const knee = setLeaderPoint(m, 0, 1, { x: 60, y: 40 });
    expect(poly(knee)[1]).toEqual({ x: 60, y: 40 });
    const attach = setLeaderPoint(m, 0, 2, { x: 130, y: 75 }); // inside the box: pulled to an edge
    const a = poly(attach)[2];
    expect(a.y === 80 || a.y === 60 || a.x === 100 || a.x === 160).toBe(true);
    expect(poly(setLeaderPoint(m, 0, 0, { x: -5, y: 7 }))[0]).toEqual({ x: -5, y: 7 });
  });
  it("ignores non-finite points and out-of-range indices", () => {
    const m = callout();
    expect(setLeaderPoint(m, 0, 1, { x: NaN, y: 0 })).toBe(m);
    expect(setLeaderPoint(m, 0, 9, { x: 0, y: 0 })).toBe(m);
    expect(setLeaderPoint(m, 4, 0, { x: 0, y: 0 })).toBe(m);
  });
  it("exposes tip and knee handles per leader", () => {
    const m = addLeader(callout())!;
    expect(leaderHandles(m).map((h) => `${h.leader}:${h.kind}`)).toEqual(["0:tip", "0:knee", "1:tip", "1:knee"]);
  });
});

describe("multiple leaders", () => {
  it("adds a leader from the knee, removes any but the last, and promotes an extra to primary", () => {
    const m = callout();
    const two = addLeader(m, { x: 30, y: 130 })!;
    expect(allLeaders(two)).toHaveLength(2);
    expect(allLeaders(two)[1]).toEqual([{ x: 30, y: 130 }, { x: 82, y: 70 }, { x: 100, y: 70 }]);
    const back = removeLeader(two, 1);
    expect(allLeaders(back)).toHaveLength(1);
    const promoted = removeLeader(two, 0);
    expect(poly(promoted)[0]).toEqual({ x: 30, y: 130 });
    expect(promoted.callout!.extra_leaders).toHaveLength(0);
    expect(removeLeader(m, 0)).toBe(m); // the last leader stays
  });
  it("caps extra leaders", () => {
    let m = callout();
    for (let i = 0; i < MAX_EXTRA_LEADERS + 5; i++) m = addLeader(m, { x: i, y: i }) ?? m;
    expect(m.callout!.extra_leaders).toHaveLength(MAX_EXTRA_LEADERS);
    expect(addLeader(m)).toBeNull();
  });
  it("hit-tests and bounds include extra leaders", () => {
    const m = addLeader(callout(), { x: -200, y: 300 })!;
    expect(hitTest([m], { x: -59, y: 185 }, 3)).toBe("c1"); // midpoint of the extra leader only
    expect(boundsOf(m).minX).toBeLessThanOrEqual(-200);
    expect(boundsOf(m).maxY).toBeGreaterThanOrEqual(300);
  });
});

describe("text box", () => {
  it("fits the text, one line per newline", () => {
    const one = fitBoxSize("abcd", 10);
    const two = fitBoxSize("abcd\nab", 10);
    expect(two.height).toBeGreaterThan(one.height);
    expect(two.width).toBe(one.width);
    expect(fitBoxSize("", 10).width).toBeGreaterThan(0);
    expect(fitBoxSize("x".repeat(1e6), 10).width).toBeLessThanOrEqual(20_000);
  });
  it("auto-size refits keeping the top-left corner; a manual resize turns it off", () => {
    const m = callout({ contents: "a much longer line of text here" });
    const fit = refitBox(m, 12);
    const b = calloutBox(fit)!;
    expect(b.minX).toBe(100);
    expect(b.maxY).toBe(80);
    expect(b.maxX - b.minX).toBeGreaterThan(60);
    expect(fit.callout!.autosize).toBe(true);
    const resized = resizeBox(fit, { minX: 100, maxX: 300, minY: 40, maxY: 80 });
    expect(resized.callout!.autosize).toBe(false);
    expect(refitBox(resized, 12)).toBe(resized); // off: no refit
    expect(setAutosize(resized, true, 12).callout!.autosize).toBe(true);
  });
  it("resizing keeps every leader's attach point on the new box edge", () => {
    const m = addLeader(callout(), { x: 30, y: 130 })!;
    const r = resizeBox(m, { minX: 120, maxX: 200, minY: 60, maxY: 80 });
    for (const l of allLeaders(r)) {
      const a = l[l.length - 1];
      expect(a.x === 120 || a.x === 200 || a.y === 60 || a.y === 80).toBe(true);
    }
  });
  it("snapToBoxEdge pulls inside and outside points onto the box outline", () => {
    const box = { minX: 0, minY: 0, maxX: 10, maxY: 10 };
    expect(snapToBoxEdge(box, { x: 1, y: 5 })).toEqual({ x: 0, y: 5 });
    expect(snapToBoxEdge(box, { x: 50, y: 5 })).toEqual({ x: 10, y: 5 });
  });
});

describe("moving", () => {
  it("translateCallout shifts leaders, box and extras together", () => {
    const m = addLeader(callout(), { x: 30, y: 130 })!;
    const t = translateCallout(m, 5, -5);
    expect(poly(t)[0]).toEqual({ x: 15, y: 95 });
    expect(t.callout!.box_min).toEqual({ x: 105, y: 55 });
    expect(t.callout!.extra_leaders[0][0]).toEqual({ x: 35, y: 125 });
    expect(translateMarkup(m, 5, -5)).toEqual(t);
  });
  it("moveBox keeps tips and knees, moves the box and attach points", () => {
    const m = callout();
    const b = moveBox(m, 20, 10);
    expect(poly(b)[0]).toEqual(poly(m)[0]);
    expect(poly(b)[1]).toEqual(poly(m)[1]);
    expect(poly(b)[2]).toEqual({ x: 120, y: 80 });
    expect(b.callout!.box_min).toEqual({ x: 120, y: 70 });
  });
});

describe("hit-testing and applying edits", () => {
  it("prefers leader handles, then box resize handles, then the box body", () => {
    const m = callout();
    expect(hitCallout(m, { x: 10.5, y: 100 }, 4)).toMatchObject({ kind: "point", leader: 0, index: 0, handle: "tip" });
    expect(hitCallout(m, { x: 82, y: 70.5 }, 4)).toMatchObject({ kind: "point", index: 1, handle: "knee" });
    expect(hitCallout(m, { x: 160, y: 80 }, 4)).toEqual({ kind: "resize", handle: "ne" });
    expect(hitCallout(m, { x: 130, y: 70 }, 4)).toEqual({ kind: "box" });
    expect(hitCallout(m, { x: 500, y: 500 }, 4)).toBeNull();
  });
  it("applies point, box and resize drags", () => {
    const m = callout();
    expect(poly(applyCalloutEdit(m, { kind: "point", leader: 0, index: 0 }, { x: 1, y: 2 }))[0]).toEqual({ x: 1, y: 2 });
    expect(applyCalloutEdit(m, { kind: "box", start: { x: 130, y: 70 } }, { x: 140, y: 70 }).callout!.box_min.x).toBe(110);
    const r = applyCalloutEdit(m, { kind: "resize", handle: "se", box: calloutBox(m)! }, { x: 200, y: 40 });
    expect(calloutBox(r)).toEqual({ minX: 100, minY: 40, maxX: 200, maxY: 80 });
  });
  it("a non-callout has no handles", () => {
    expect(hitCallout({ ...callout(), markup_type: "Polyline" }, { x: 10, y: 100 }, 4)).toBeNull();
  });
});

describe("cloud helpers", () => {
  it("builds a rectangle cloud from a drag, in any drag direction, and rejects tiny drags", () => {
    const want = [{ x: 10, y: 20 }, { x: 60, y: 20 }, { x: 60, y: 50 }, { x: 10, y: 50 }];
    expect(rectCloudVertices({ x: 10, y: 20 }, { x: 60, y: 50 })).toEqual(want);
    expect(rectCloudVertices({ x: 60, y: 50 }, { x: 10, y: 20 })).toEqual(want);
    expect(rectCloudVertices({ x: 0, y: 0 }, { x: 2, y: 80 })).toBeNull();
    expect(rectCloudVertices({ x: NaN, y: 0 }, { x: 20, y: 20 })).toBeNull();
  });
  it("finds the nearest point on a polygon outline", () => {
    const sq = [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }, { x: 0, y: 10 }];
    expect(nearestOnPolygon(sq, { x: 30, y: 5 })).toEqual({ x: 10, y: 5 });
    expect(nearestOnPolygon([], { x: 0, y: 0 })).toBeNull();
  });
  it("bounds the scallop count of a huge cloud and ignores non-finite vertices", () => {
    const huge = [{ x: 0, y: 0 }, { x: 1e7, y: 0 }, { x: 1e7, y: 1e7 }, { x: 0, y: 1e7 }];
    const d = cloudPath(huge, 4);
    expect((d.match(/ A /g) ?? []).length).toBeLessThanOrEqual(MAX_CLOUD_BUMPS + 8);
    expect(cloudPath([{ x: NaN, y: 0 }, { x: 1, y: 1 }], 4)).toBe("");
    expect(cloudPath(huge, NaN)).toBe("");
    expect(cloudPath(huge, 0)).toBe("");
    const many = Array.from({ length: 50_000 }, (_, i) => ({ x: i, y: i % 7 }));
    expect((cloudPath(many, 4).match(/ A /g) ?? []).length).toBeLessThan(10_000);
  });
});

import { patchAppearance, patchCalloutAutosize, patchFields } from "./markup-properties";

describe("Properties edits refit an auto-size callout", () => {
  const by = USER;
  it("changing the text or font size refits; auto-size off leaves the box alone", () => {
    const m = callout({ contents: "short" });
    const longer = patchFields(m, { contents: "a considerably longer callout text" }, by, "t2");
    expect(calloutBox(longer)!.maxX).toBeGreaterThan(calloutBox(m)!.maxX);
    const bigger = patchAppearance(m, { font: { family: "Helvetica", size_pt: 24 } }, by, "t2");
    expect(calloutBox(bigger)!.maxY - calloutBox(bigger)!.minY).toBeGreaterThan(calloutBox(m)!.maxY - calloutBox(m)!.minY);
    const manual = patchCalloutAutosize(m, false, by, "t2");
    expect(manual.callout!.autosize).toBe(false);
    const same = patchFields(manual, { contents: "a considerably longer callout text" }, by, "t3");
    expect(calloutBox(same)).toEqual(calloutBox(manual));
    expect(patchCalloutAutosize(manual, true, by, "t4").callout!.autosize).toBe(true);
  });
  it("leaves other markup types and legacy callouts untouched", () => {
    const legacy = callout({ callout: null, geometry: { Polyline: [{ x: 0, y: 0 }, { x: 5, y: 5 }] } });
    expect(patchFields(legacy, { contents: "x".repeat(100) }, by, "t").callout).toBeNull();
    const rect = { ...callout(), markup_type: "Polyline" as const };
    expect(patchCalloutAutosize(rect, true, by, "t")).toBe(rect);
  });
});
