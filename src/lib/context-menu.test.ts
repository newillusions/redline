import { describe, it, expect } from "vitest";
import {
  markupMenuItems, canvasMenuItems, thumbnailMenuItems, nextEnabled, firstEnabled, lastEnabled,
  clampMenuPosition, statusFromId, type MenuItem, type ActionItem,
} from "./context-menu";

const actions = (items: MenuItem[]) => items.filter((i): i is ActionItem => i.kind === "item");
const byId = (items: MenuItem[], id: string) => actions(items).find((i) => i.id === id);

describe("markupMenuItems", () => {
  it("offers status, properties, copy/duplicate, order, save as tool and delete", () => {
    const ids = actions(markupMenuItems({ count: 1, anyLocked: false })).map((i) => i.id);
    expect(ids).toEqual([
      "status:Accepted", "status:Rejected", "status:Cancelled", "status:Completed", "status:None",
      "properties", "save-as-tool", "copy", "cut", "duplicate", "bring-to-front", "send-to-back", "delete",
    ]);
  });
  it("greys out edits on a locked markup but leaves copy and properties", () => {
    const m = markupMenuItems({ count: 1, anyLocked: true });
    for (const id of ["status:Accepted", "cut", "bring-to-front", "send-to-back", "delete"]) {
      expect(byId(m, id)?.disabled).toBe(true);
    }
    expect(byId(m, "copy")?.disabled).toBeFalsy();
    expect(byId(m, "properties")?.disabled).toBeFalsy();
  });
  it("disables Save as Tool for a multi-selection and says how many it affects", () => {
    const m = markupMenuItems({ count: 3, anyLocked: false });
    expect(byId(m, "save-as-tool")?.disabled).toBe(true);
    expect(m.find((i) => i.kind === "heading")).toMatchObject({ label: "Status (3 markups)" });
  });
  it("marks the shared current status", () => {
    const m = markupMenuItems({ count: 2, anyLocked: false, commonStatus: "Rejected" });
    expect(byId(m, "status:Rejected")?.label).toBe("Rejected (current)");
    expect(byId(m, "status:Accepted")?.label).toBe("Accepted");
  });
});

describe("statusFromId", () => {
  it("round-trips status ids and rejects others", () => {
    expect(statusFromId("status:Completed")).toBe("Completed");
    expect(statusFromId("status:Bogus")).toBeNull();
    expect(statusFromId("copy")).toBeNull();
  });
});

describe("canvasMenuItems", () => {
  it("disables Paste with an empty clipboard and views with no history", () => {
    const m = canvasMenuItems({ canPaste: false, hasMarkups: false, canBack: false, canForward: true });
    expect(byId(m, "paste")?.disabled).toBe(true);
    expect(byId(m, "select-all")?.disabled).toBe(true);
    expect(byId(m, "prev-view")?.disabled).toBe(true);
    expect(byId(m, "next-view")?.disabled).toBe(false);
    expect(byId(m, "fit-width")?.disabled).toBeFalsy();
  });

  it("offers Rotate Page Clockwise / Counterclockwise", () => {
    const m = canvasMenuItems({ canPaste: true, hasMarkups: true, canBack: true, canForward: true });
    expect(byId(m, "rotate-page-cw")?.label).toBe("Rotate Page Clockwise");
    expect(byId(m, "rotate-page-ccw")?.label).toBe("Rotate Page Counterclockwise");
  });
});

describe("thumbnailMenuItems", () => {
  it("names the page, pluralises Extract and respects capabilities", () => {
    const m = thumbnailMenuItems({ page: 4, targetCount: 3, canDelete: false, canRename: true, canExtract: true, canNumber: false });
    expect(byId(m, "go-to-page")?.label).toBe("Go to Page 5");
    expect(byId(m, "extract")?.label).toBe("Extract 3 Pages...");
    expect(byId(m, "delete-page")?.disabled).toBe(true);
    expect(byId(m, "number-pages")?.disabled).toBe(true);
    expect(byId(m, "rename-label")?.disabled).toBe(false);
  });
});

describe("keyboard navigation helpers", () => {
  const items: MenuItem[] = [
    { kind: "heading", label: "h" },
    { kind: "item", id: "a", label: "A" },
    { kind: "separator" },
    { kind: "item", id: "b", label: "B", disabled: true },
    { kind: "item", id: "c", label: "C" },
  ];
  it("skips headings, separators and disabled items, and wraps", () => {
    expect(firstEnabled(items)).toBe(1);
    expect(lastEnabled(items)).toBe(4);
    expect(nextEnabled(items, 1, 1)).toBe(4);
    expect(nextEnabled(items, 4, 1)).toBe(1);
    expect(nextEnabled(items, 1, -1)).toBe(4);
  });
  it("returns -1 when nothing is enabled", () => {
    expect(firstEnabled([{ kind: "item", id: "x", label: "X", disabled: true }])).toBe(-1);
    expect(firstEnabled([])).toBe(-1);
  });
});

describe("clampMenuPosition", () => {
  it("keeps a menu that fits where it was opened", () => {
    expect(clampMenuPosition(100, 100, 200, 300, 1000, 800)).toEqual({ left: 100, top: 100 });
  });
  it("flips left and up near the bottom-right corner", () => {
    expect(clampMenuPosition(950, 780, 200, 300, 1000, 800)).toEqual({ left: 750, top: 480 });
  });
  it("clamps inside the window when it fits on neither side", () => {
    const p = clampMenuPosition(10, 10, 600, 900, 500, 700);
    expect(p.left).toBeGreaterThanOrEqual(0);
    expect(p.top).toBeGreaterThanOrEqual(0);
  });
});
