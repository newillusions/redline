import { describe, expect, it } from "vitest";
import { buildRows, cellText, COLUMN_DEFS } from "./markups-list";
import type { Markup } from "./ipc";

function mk(id: string, page: number): Markup {
  return {
    id, page, markup_type: "Rectangle", subject: "S", contents: "", layer: null,
    audit: { created_by: { user_id: "u", display_name: "A" }, created_at: "2026-10-01T00:00:00Z", modified_at: "2026-10-01T00:00:00Z" },
    workflow: { status: "None", status_history: [], thread: [] },
    appearance: { color: "#ff0000" },
    measurement: null,
  } as unknown as Markup;
}

describe("Markups List page labels (docs/navigation.md RL1)", () => {
  const labels = ["", "E-101", "=SUM(A1)"];

  it("the Page column shows the label when the page has one, else the number", () => {
    const rows = buildRows([mk("a", 0), mk("b", 1)], labels);
    expect(cellText(rows[0], "page")).toBe("1");
    expect(cellText(rows[1], "page")).toBe("E-101");
    expect(rows[1].page).toBe(2); // sorting and the page filter still use the number
  });

  it("the Page label column falls back to the number too", () => {
    const rows = buildRows([mk("a", 0), mk("b", 1)], labels);
    expect(cellText(rows[0], "page_label")).toBe("1");
    expect(cellText(rows[1], "page_label")).toBe("E-101");
    const def = COLUMN_DEFS.find((c) => c.key === "page_label");
    expect(def?.defaultVisible).toBe(false);
  });

  it("without labels nothing changes", () => {
    const rows = buildRows([mk("a", 4)]);
    expect(cellText(rows[0], "page")).toBe("5");
    expect(rows[0].pageLabel).toBe("");
  });

  it("a label is kept as text and bounded", () => {
    const rows = buildRows([mk("a", 2)], labels);
    expect(cellText(rows[0], "page")).toBe("=SUM(A1)");
    const long = buildRows([mk("a", 0)], ["x".repeat(5000)]);
    expect(long[0].pageLabel.length).toBeLessThanOrEqual(2000); // the list's own text cap
    expect(long[0].pageLabel.length).toBeLessThan(5000);
  });
});
