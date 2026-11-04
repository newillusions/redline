import { describe, it, expect, beforeEach } from "vitest";
import type { Markup } from "./ipc";
import type { ArrangeCtx } from "./arrange-actions";
import { copyFormat, pasteFormat, useAsDefault, formatClipboard } from "./format-painter";
import { markupMenuItems } from "./context-menu";
import { runMarkupCommand } from "./context-actions";
import { BY, NOW, mk, storeWith } from "./arrange.test-util";

function ctxFor(store: ReturnType<typeof storeWith>): ArrangeCtx {
  return {
    store, identity: BY, scales: [], pageIndex: 0, pageCount: 1, pageWidthPts: 600, pageHeightPts: 400,
    cursor: null, viewCenter: null, now: () => NOW,
  };
}
const select = (s: ReturnType<typeof storeWith>, ...ids: string[]) => { s.selectedIds = new Set(ids); };
const styled = (id: string): Markup => {
  const m = mk(id);
  m.appearance = { ...m.appearance, color: "#00aa00", line_weight: 5, opacity: 0.5, fill: "#ffff00", line_style: "Dashed" };
  return m;
};

beforeEach(() => { formatClipboard.format = null; });

describe("format painter", () => {
  it("copy needs exactly one selected markup", () => {
    const s = storeWith([styled("a"), mk("b")]);
    expect(copyFormat(ctxFor(s)).ok).toBe(false);
    select(s, "a", "b");
    expect(copyFormat(ctxFor(s)).ok).toBe(false);
    expect(formatClipboard.format).toBeNull();
  });

  it("paste before any copy says what to do and changes nothing", () => {
    const s = storeWith([mk("b")]);
    select(s, "b");
    const r = pasteFormat(ctxFor(s));
    expect(r.ok).toBe(false);
    expect(r.message).toContain("Copy a format first");
    expect(s.markups[0].appearance.color).toBe("#e02424");
  });

  it("paste applies the copied look to every selected markup as one undo step, geometry untouched", () => {
    const s = storeWith([styled("a"), mk("b"), mk("c")]);
    select(s, "a");
    expect(copyFormat(ctxFor(s)).ok).toBe(true);
    select(s, "b", "c");
    const before = JSON.stringify(s.markups.find((m) => m.id === "b")!.geometry);
    const r = pasteFormat(ctxFor(s));
    expect(r.ok).toBe(true);
    for (const id of ["b", "c"]) {
      const m = s.markups.find((x) => x.id === id)!;
      expect(m.appearance).toMatchObject({ color: "#00aa00", line_weight: 5, opacity: 0.5, fill: "#ffff00", line_style: "Dashed" });
    }
    expect(JSON.stringify(s.markups.find((m) => m.id === "b")!.geometry)).toBe(before);
    s.undo();
    expect(s.markups.find((m) => m.id === "b")!.appearance.color).toBe("#e02424");
    expect(s.markups.find((m) => m.id === "c")!.appearance.color).toBe("#e02424");
  });

  it("the font goes only to markups that already have one", () => {
    const text = mk("t", { markup_type: "Text" });
    text.appearance = { ...text.appearance, font: { family: "Courier", size_pt: 18 } };
    const rect = mk("r");
    const other = mk("o", { markup_type: "Text" });
    other.appearance = { ...other.appearance, font: { family: "Helvetica", size_pt: 10 } };
    const s = storeWith([text, rect, other]);
    select(s, "t");
    copyFormat(ctxFor(s));
    select(s, "r", "o");
    pasteFormat(ctxFor(s));
    expect(s.markups.find((m) => m.id === "r")!.appearance.font).toBeNull();
    expect(s.markups.find((m) => m.id === "o")!.appearance.font).toEqual({ family: "Courier", size_pt: 18 });
  });

  it("locked markups are skipped and counted", () => {
    const locked = mk("l");
    locked.annot_flags = 0x80;
    const s = storeWith([styled("a"), locked, mk("b")]);
    select(s, "a");
    copyFormat(ctxFor(s));
    select(s, "l", "b");
    const r = pasteFormat(ctxFor(s));
    expect(r.message).toContain("1 locked skipped");
    expect(s.markups.find((m) => m.id === "l")!.appearance.color).toBe("#e02424");
    expect(s.markups.find((m) => m.id === "b")!.appearance.color).toBe("#00aa00");
  });

  it("use as default sets the look of new markups from the one selected", () => {
    const s = storeWith([styled("a"), mk("b")]);
    select(s, "a");
    expect(useAsDefault(ctxFor(s)).ok).toBe(true);
    expect(s.draftAppearance.color).toBe("#00aa00");
    select(s, "a", "b");
    expect(useAsDefault(ctxFor(s)).ok).toBe(false);
  });

  it("is offered on the markup menu and routed by runMarkupCommand", () => {
    const ids = (canPaste: boolean) =>
      markupMenuItems({ count: 1, anyLocked: false, canPasteFormat: canPaste }).flatMap((i) => (i.kind === "item" ? [i] : []));
    expect(ids(false).find((i) => i.id === "paste-format")!.disabled).toBe(true);
    expect(ids(true).find((i) => i.id === "paste-format")!.disabled).toBe(false);
    expect(ids(true).map((i) => i.id)).toEqual(expect.arrayContaining(["copy-format", "use-as-default"]));
    const s = storeWith([styled("a")]);
    select(s, "a");
    expect(runMarkupCommand("copy-format", ctxFor(s))!.ok).toBe(true);
    expect(formatClipboard.format!.color).toBe("#00aa00");
  });
});
