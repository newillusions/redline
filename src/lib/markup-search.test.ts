import { describe, it, expect } from "vitest";
import { searchMarkupContents } from "./markup-search";
import { buildMarkup } from "./markup-tools";
import type { Markup } from "./ipc";

const BASE_APPEARANCE = {
  color: "#ff0000",
  line_weight: 2,
  opacity: 1,
  fill: null,
  line_style: "Solid" as const,
  font: { family: "Helvetica", size_pt: 12 },
};

const IDENTITY = { user_id: "aaaaaaaa-0000-0000-0000-000000000001", display_name: "Tester" };

function markup(id: string, page: number, overrides: Partial<Markup> = {}): Markup {
  const m = buildMarkup({
    markupType: "Text",
    page,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 100, y: 100 } } },
    appearance: { ...BASE_APPEARANCE },
    identity: IDENTITY,
    now: "2026-01-01T00:00:00.000Z",
    id,
  });
  return { ...m, ...overrides };
}

describe("searchMarkupContents", () => {
  it("returns no hits for an empty or blank query", () => {
    const markups = [markup("m1", 0, { contents: "verify concrete strength" })];
    expect(searchMarkupContents(markups, "")).toEqual([]);
    expect(searchMarkupContents(markups, "   ")).toEqual([]);
  });

  it("matches contents (comment text) and reports the field", () => {
    const markups = [markup("m1", 2, { contents: "verify concrete strength before pour" })];
    const hits = searchMarkupContents(markups, "concrete");
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ markupId: "m1", page: 2, field: "contents" });
    expect(hits[0].snippet).toContain("concrete");
  });

  it("matches subject (tool label) and reports the field", () => {
    const markups = [markup("m1", 0, { subject: "RFI - concrete mix" })];
    const hits = searchMarkupContents(markups, "RFI");
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ markupId: "m1", field: "subject" });
  });

  it("is case-insensitive by default, case-sensitive when requested", () => {
    const markups = [markup("m1", 0, { contents: "Concrete Slab" })];
    expect(searchMarkupContents(markups, "concrete")).toHaveLength(1);
    expect(searchMarkupContents(markups, "concrete", true)).toHaveLength(0);
    expect(searchMarkupContents(markups, "Concrete", true)).toHaveLength(1);
  });

  it("honors whole-word matching", () => {
    const markups = [markup("m1", 0, { contents: "reinforced concrete works" })];
    // "concrete" is a whole word here — should match under wholeWord.
    expect(searchMarkupContents(markups, "concrete", false, true)).toHaveLength(1);
    // "crete" is only a substring of "concrete" — must NOT match under wholeWord.
    expect(searchMarkupContents(markups, "crete", false, true)).toHaveLength(0);
    // But it does match without wholeWord.
    expect(searchMarkupContents(markups, "crete", false, false)).toHaveLength(1);
  });

  it("skips markups with no contents or subject", () => {
    const markups = [markup("m1", 0, { contents: null, subject: null })];
    expect(searchMarkupContents(markups, "anything")).toEqual([]);
  });

  it("sorts hits by page ascending across multiple markups", () => {
    const markups = [
      markup("late", 5, { contents: "steel rebar spacing" }),
      markup("early", 1, { contents: "steel column base plate" }),
    ];
    const hits = searchMarkupContents(markups, "steel");
    expect(hits.map((h) => h.markupId)).toEqual(["early", "late"]);
  });

  it("can match both contents and subject on the same markup as two hits", () => {
    const markups = [markup("m1", 0, { subject: "steel note", contents: "steel spec here" })];
    const hits = searchMarkupContents(markups, "steel");
    expect(hits).toHaveLength(2);
    const fields = hits.map((h) => h.field).sort();
    expect(fields).toEqual(["contents", "subject"]);
  });
});
