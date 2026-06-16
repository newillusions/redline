/**
 * Tests for markup-properties.ts: property-patch helpers and indeterminate-value
 * logic for the PropertiesPanel. All pure TS - no DOM, no Svelte.
 */
import { describe, it, expect } from "vitest";
import {
  FONT_FAMILIES,
  FONT_SIZES,
  patchAppearance,
  patchFields,
  patchGroup,
  commonValue,
} from "./markup-properties";
import type { Appearance, UserRef, Markup } from "./ipc";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------
const AP: Appearance = {
  color: "#e02424",
  line_weight: 2,
  opacity: 1,
  fill: null,
  line_style: "Solid",
  font: null,
};

const USER: UserRef = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "Tester" };
const OTHER: UserRef = { user_id: "22222222-2222-2222-2222-222222222222", display_name: "Other" };
const NOW = "2026-06-16T12:00:00Z";
const LATER = "2026-06-16T13:00:00Z";

const BASE_AUDIT = {
  created_by: USER,
  created_at: "2026-06-16T00:00:00Z",
  modified_by: USER,
  modified_at: "2026-06-16T00:00:00Z",
  revision: 3,
  origin: "Desktop" as const,
};

function mkMarkup(id: string, overrides: Partial<Markup> = {}): Markup {
  return {
    id,
    markup_type: "Rectangle",
    page: 1,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 100, y: 80 } } },
    appearance: AP,
    subject: null,
    layer: null,
    contents: null,
    group_id: null,
    audit: { ...BASE_AUDIT },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// FONT_FAMILIES / FONT_SIZES constants
// ---------------------------------------------------------------------------
describe("FONT_FAMILIES", () => {
  it("includes Helvetica", () => {
    expect(FONT_FAMILIES).toContain("Helvetica");
  });
  it("includes Times", () => {
    expect(FONT_FAMILIES).toContain("Times");
  });
  it("includes Courier", () => {
    expect(FONT_FAMILIES).toContain("Courier");
  });
  it("has exactly 3 entries", () => {
    expect(FONT_FAMILIES.length).toBe(3);
  });
});

describe("FONT_SIZES", () => {
  it("includes 12", () => {
    expect(FONT_SIZES).toContain(12);
  });
  it("includes 8", () => {
    expect(FONT_SIZES).toContain(8);
  });
  it("includes 36", () => {
    expect(FONT_SIZES).toContain(36);
  });
  it("has 9 entries", () => {
    expect(FONT_SIZES.length).toBe(9);
  });
});

// ---------------------------------------------------------------------------
// patchAppearance
// ---------------------------------------------------------------------------
describe("patchAppearance", () => {
  const m = mkMarkup("m1");

  it("patches color, leaves other appearance fields intact", () => {
    const result = patchAppearance(m, { color: "#0000ff" }, OTHER, LATER);
    expect(result.appearance.color).toBe("#0000ff");
    expect(result.appearance.line_weight).toBe(AP.line_weight);
    expect(result.appearance.opacity).toBe(AP.opacity);
    expect(result.appearance.fill).toBe(AP.fill);
    expect(result.appearance.line_style).toBe(AP.line_style);
    expect(result.appearance.font).toBe(AP.font);
  });

  it("patches line_weight, leaves other fields intact", () => {
    const result = patchAppearance(m, { line_weight: 4 }, OTHER, LATER);
    expect(result.appearance.line_weight).toBe(4);
    expect(result.appearance.color).toBe(AP.color);
  });

  it("patches opacity, leaves other fields intact", () => {
    const result = patchAppearance(m, { opacity: 0.5 }, OTHER, LATER);
    expect(result.appearance.opacity).toBe(0.5);
    expect(result.appearance.color).toBe(AP.color);
  });

  it("patches fill (string)", () => {
    const result = patchAppearance(m, { fill: "#aabbcc" }, OTHER, LATER);
    expect(result.appearance.fill).toBe("#aabbcc");
  });

  it("patches fill to null", () => {
    const withFill = mkMarkup("m2", { appearance: { ...AP, fill: "#aabbcc" } });
    const result = patchAppearance(withFill, { fill: null }, OTHER, LATER);
    expect(result.appearance.fill).toBeNull();
  });

  it("patches line_style", () => {
    const result = patchAppearance(m, { line_style: "Dashed" }, OTHER, LATER);
    expect(result.appearance.line_style).toBe("Dashed");
    expect(result.appearance.color).toBe(AP.color);
  });

  it("patches font (object)", () => {
    const result = patchAppearance(m, { font: { family: "Times", size_pt: 14 } }, OTHER, LATER);
    expect(result.appearance.font).toEqual({ family: "Times", size_pt: 14 });
    expect(result.appearance.color).toBe(AP.color);
  });

  it("patches font to null", () => {
    const withFont = mkMarkup("m3", { appearance: { ...AP, font: { family: "Helvetica", size_pt: 12 } } });
    const result = patchAppearance(withFont, { font: null }, OTHER, LATER);
    expect(result.appearance.font).toBeNull();
  });

  it("bumps audit: revision +1, modified_by = passed user, modified_at = now", () => {
    const result = patchAppearance(m, { color: "#ffffff" }, OTHER, LATER);
    expect(result.audit.revision).toBe(BASE_AUDIT.revision + 1);
    expect(result.audit.modified_by).toEqual(OTHER);
    expect(result.audit.modified_at).toBe(LATER);
  });

  it("preserves created_by and created_at", () => {
    const result = patchAppearance(m, { color: "#ffffff" }, OTHER, LATER);
    expect(result.audit.created_by).toEqual(USER);
    expect(result.audit.created_at).toBe(BASE_AUDIT.created_at);
  });

  it("does not mutate the input markup", () => {
    const before = JSON.parse(JSON.stringify(m)) as Markup;
    patchAppearance(m, { color: "#ffffff", line_weight: 99 }, OTHER, LATER);
    expect(m.appearance.color).toBe(before.appearance.color);
    expect(m.appearance.line_weight).toBe(before.appearance.line_weight);
    expect(m.audit.revision).toBe(before.audit.revision);
  });

  it("returns a new object reference", () => {
    const result = patchAppearance(m, { color: "#ffffff" }, OTHER, LATER);
    expect(result).not.toBe(m);
    expect(result.appearance).not.toBe(m.appearance);
  });
});

// ---------------------------------------------------------------------------
// patchFields
// ---------------------------------------------------------------------------
describe("patchFields", () => {
  const m = mkMarkup("f1", { contents: "hello", subject: "S1", layer: "A" });

  it("patches contents only when only contents is in patch", () => {
    const result = patchFields(m, { contents: "world" }, OTHER, LATER);
    expect(result.contents).toBe("world");
    expect(result.subject).toBe("S1");
    expect(result.layer).toBe("A");
  });

  it("patches subject only when only subject is in patch", () => {
    const result = patchFields(m, { subject: "S2" }, OTHER, LATER);
    expect(result.subject).toBe("S2");
    expect(result.contents).toBe("hello");
    expect(result.layer).toBe("A");
  });

  it("patches layer only when only layer is in patch", () => {
    const result = patchFields(m, { layer: "B" }, OTHER, LATER);
    expect(result.layer).toBe("B");
    expect(result.contents).toBe("hello");
    expect(result.subject).toBe("S1");
  });

  it("explicit null in patch clears a field", () => {
    const result = patchFields(m, { contents: null }, OTHER, LATER);
    expect(result.contents).toBeNull();
    expect(result.subject).toBe("S1");
  });

  it("explicit null on layer clears it", () => {
    const result = patchFields(m, { layer: null }, OTHER, LATER);
    expect(result.layer).toBeNull();
  });

  it("absent key in patch leaves original value unchanged", () => {
    // patch has no 'subject' key - subject must remain "S1"
    const result = patchFields(m, { layer: "C" }, OTHER, LATER);
    expect(result.subject).toBe("S1");
  });

  it("leaves appearance untouched", () => {
    const result = patchFields(m, { contents: "new" }, OTHER, LATER);
    expect(result.appearance).toEqual(m.appearance);
  });

  it("bumps audit: revision +1, modified_by/at updated", () => {
    const result = patchFields(m, { subject: "X" }, OTHER, LATER);
    expect(result.audit.revision).toBe(BASE_AUDIT.revision + 1);
    expect(result.audit.modified_by).toEqual(OTHER);
    expect(result.audit.modified_at).toBe(LATER);
  });

  it("preserves created_by and created_at", () => {
    const result = patchFields(m, { contents: "y" }, OTHER, LATER);
    expect(result.audit.created_by).toEqual(USER);
    expect(result.audit.created_at).toBe(BASE_AUDIT.created_at);
  });

  it("does not mutate the input markup", () => {
    const snapshot = JSON.parse(JSON.stringify(m)) as Markup;
    patchFields(m, { contents: "mutated?", subject: "mutated?", layer: "X" }, OTHER, LATER);
    expect(m.contents).toBe(snapshot.contents);
    expect(m.subject).toBe(snapshot.subject);
    expect(m.layer).toBe(snapshot.layer);
    expect(m.audit.revision).toBe(snapshot.audit.revision);
  });

  it("returns a new object reference", () => {
    const result = patchFields(m, { contents: "y" }, OTHER, LATER);
    expect(result).not.toBe(m);
  });
});

// ---------------------------------------------------------------------------
// patchGroup
// ---------------------------------------------------------------------------
describe("patchGroup", () => {
  const m = mkMarkup("g1");

  it("sets group_id to the provided value and bumps audit", () => {
    const gid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const result = patchGroup(m, gid, OTHER, LATER);
    expect(result.group_id).toBe(gid);
    expect(result.audit.revision).toBe(BASE_AUDIT.revision + 1);
    expect(result.audit.modified_by).toEqual(OTHER);
    expect(result.audit.modified_at).toBe(LATER);
  });

  it("clears group_id to null when passed null", () => {
    const grouped = mkMarkup("g2", { group_id: "some-group-id" });
    const result = patchGroup(grouped, null, OTHER, LATER);
    expect(result.group_id).toBeNull();
    expect(result.audit.revision).toBe(BASE_AUDIT.revision + 1);
  });

  it("does not mutate the input markup", () => {
    const snapshot = JSON.parse(JSON.stringify(m)) as Markup;
    patchGroup(m, "new-group", OTHER, LATER);
    expect(m.group_id).toBe(snapshot.group_id);
    expect(m.audit.revision).toBe(snapshot.audit.revision);
  });
});

// ---------------------------------------------------------------------------
// commonValue
// ---------------------------------------------------------------------------
describe("commonValue", () => {
  const m1 = mkMarkup("c1", { appearance: { ...AP, color: "#ff0000" } });
  const m2 = mkMarkup("c2", { appearance: { ...AP, color: "#ff0000" } });
  const m3 = mkMarkup("c3", { appearance: { ...AP, color: "#0000ff" } });

  it("returns the shared value when all markups project the same value", () => {
    expect(commonValue([m1, m2], (m) => m.appearance.color)).toBe("#ff0000");
  });

  it("returns undefined when markups project different values", () => {
    expect(commonValue([m1, m3], (m) => m.appearance.color)).toBeUndefined();
  });

  it("returns undefined for an empty list", () => {
    expect(commonValue([], (m) => m.appearance.color)).toBeUndefined();
  });

  it("returns the single element's value for a one-element list", () => {
    expect(commonValue([m1], (m) => m.appearance.color)).toBe("#ff0000");
  });

  it("works on numeric projections", () => {
    const a = mkMarkup("n1", { appearance: { ...AP, line_weight: 3 } });
    const b = mkMarkup("n2", { appearance: { ...AP, line_weight: 3 } });
    const c = mkMarkup("n3", { appearance: { ...AP, line_weight: 5 } });
    expect(commonValue([a, b], (m) => m.appearance.line_weight)).toBe(3);
    expect(commonValue([a, c], (m) => m.appearance.line_weight)).toBeUndefined();
  });

  it("works on a nested getter: font family", () => {
    const withHelvetica = mkMarkup("h1", { appearance: { ...AP, font: { family: "Helvetica", size_pt: 12 } } });
    const withHelvetica2 = mkMarkup("h2", { appearance: { ...AP, font: { family: "Helvetica", size_pt: 12 } } });
    const withTimes = mkMarkup("t1", { appearance: { ...AP, font: { family: "Times", size_pt: 12 } } });
    const noFont = mkMarkup("n1");

    expect(commonValue([withHelvetica, withHelvetica2], (m) => m.appearance.font?.family)).toBe("Helvetica");
    expect(commonValue([withHelvetica, withTimes], (m) => m.appearance.font?.family)).toBeUndefined();
    expect(commonValue([withHelvetica, noFont], (m) => m.appearance.font?.family)).toBeUndefined();
  });

  it("uses strict equality (===): two different objects with same shape are NOT equal", () => {
    // Two markups with font objects - even if same shape, different references -> undefined
    const a = mkMarkup("fa", { appearance: { ...AP, font: { family: "Helvetica", size_pt: 12 } } });
    const b = mkMarkup("fb", { appearance: { ...AP, font: { family: "Helvetica", size_pt: 12 } } });
    // Projecting the font object itself (reference equality): different objects, so undefined
    expect(commonValue([a, b], (m) => m.appearance.font)).toBeUndefined();
  });

  it("returns undefined for three markups with mixed values", () => {
    expect(commonValue([m1, m2, m3], (m) => m.appearance.color)).toBeUndefined();
  });

  it("returns shared value for three identical projections", () => {
    const m4 = mkMarkup("c4", { appearance: { ...AP, color: "#ff0000" } });
    expect(commonValue([m1, m2, m4], (m) => m.appearance.color)).toBe("#ff0000");
  });
});
