import { describe, it, expect } from "vitest";
import { mk } from "./markups-list.test-util";
import {
  applyRowClick, buildRows, cellText, customColumnNames, defaultColumnConfig, displayItems, emptyFilter,
  filterRows, isFilterActive, moveColumn, nextSort, normalizeColumnConfig, parsePageSpec, sortRows,
  statusForShortcut, toggleColumn, visibleColumns, windowRange, STATUS_ORDER, MAX_CELL_CHARS,
} from "./markups-list";
import type { Markup } from "./ipc";

const key = (code: string, shiftKey = true) => ({ code, shiftKey, ctrlKey: false, metaKey: false, altKey: false });

function sample(): Markup[] {
  return [
    mk("a", { page: 2, contents: "Check clearance", author: "Zed", created: "2026-03-01T10:00:00Z", subject: "Cloud" }),
    mk("b", { page: 0, contents: "Door swing", author: "Ann", created: "2026-01-01T10:00:00Z",
      workflow: { status: "Rejected", assignee: null, thread: [] } }),
    mk("c", { page: 9, contents: null, author: "Ann", created: "2026-02-01T10:00:00Z", markup_type: "Ellipse" }),
    mk("d", { page: 0, contents: "Page ten stuff", author: "bob", created: "2026-02-01T10:00:00Z",
      workflow: { status: "Accepted", assignee: null, thread: [
        { id: "r1", author: { user_id: "x", display_name: "Rae" }, at: "2026-02-02T08:00:00Z", contents: "lintel detail" },
        { id: "r2", author: { user_id: "y", display_name: "Sam" }, at: "2026-02-03T08:00:00Z", contents: "agreed", in_reply_to: "r1" },
      ] } }),
  ];
}

describe("rows and columns", () => {
  it("builds 1-based pages, subject fallback to type, and reply counts", () => {
    const rows = buildRows(sample());
    expect(rows.map((r) => r.page)).toEqual([3, 1, 10, 1]);
    expect(cellText(rows[1], "subject")).toBe("Rectangle");
    expect(cellText(rows[0], "subject")).toBe("Cloud");
    expect(rows[3].replies).toBe(2);
    expect(cellText(rows[3], "replies")).toBe("2");
    expect(cellText(rows[0], "replies")).toBe("");
  });

  it("status_by/at come from the latest markup-level Review change, not a reply state", () => {
    const m = mk("s", {
      workflow: {
        status: "Accepted", assignee: null, thread: [],
        status_history: [
          { id: "1", author: { user_id: "u", display_name: "Old" }, at: "2026-01-01T00:00:00Z", model: "Review", state: "Rejected" },
          { id: "2", author: { user_id: "v", display_name: "Rev" }, at: "2026-01-02T03:04:00Z", model: "Review", state: "Accepted" },
          { id: "3", author: { user_id: "w", display_name: "OnReply" }, at: "2026-01-03T00:00:00Z", model: "Review", state: "Cancelled", on_reply: "r9" },
        ],
      },
    });
    const [r] = buildRows([m]);
    expect(r.statusBy).toBe("Rev");
    expect(r.statusAt).toBe("2026-01-02 03:04");
    expect(r.status).toBe("Accepted");
  });

  it("measurement and custom columns", () => {
    const m = mk("m", {
      markup_type: "MeasurementLength",
      measurement: { scale_ref: null, raw_measure: 1, unit: "m", computed_quantity: 12.5, depth: null, count_value: null, custom_columns: { cost_code: "E-1" } },
    });
    const c = mk("n", {
      markup_type: "MeasurementCount",
      measurement: { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 4, custom_columns: {} },
    });
    const rows = buildRows([m, c]);
    expect(cellText(rows[0], "measurement")).toBe("12.50 m");
    expect(cellText(rows[1], "measurement")).toBe("4");
    expect(customColumnNames(rows)).toEqual(["cost_code"]);
    expect(cellText(rows[0], "custom:cost_code")).toBe("E-1");
  });

  it("does not offer hostile custom column names (too long, control characters, empty)", () => {
    const cc = (name: string) => mk("c" + name.length, { measurement: { scale_ref: null, raw_measure: 1, unit: "m", computed_quantity: 1, depth: null, count_value: null, custom_columns: { [name]: "v" } } });
    const rows = buildRows([cc("ok"), cc("x".repeat(65)), cc("bad\nname"), cc("=formula")]);
    expect(customColumnNames(rows)).toEqual(["=formula", "ok"]);
  });

  it("column visibility, toggle, reorder and stored-config repair", () => {
    let cfg = defaultColumnConfig();
    expect(visibleColumns(cfg)).toContain("status");
    expect(visibleColumns(cfg)).not.toContain("layer");
    cfg = toggleColumn(cfg, "layer");
    expect(visibleColumns(cfg)).toContain("layer");
    cfg = toggleColumn(cfg, "status");
    expect(visibleColumns(cfg)).not.toContain("status");
    const before = cfg.order.indexOf("author");
    cfg = moveColumn(cfg, "author", -1);
    expect(cfg.order.indexOf("author")).toBe(before - 1);
    expect(moveColumn(cfg, cfg.order[0], -1)).toBe(cfg);
    const fixed = normalizeColumnConfig({ order: ["bogus", "page"], visible: ["page", "bogus"] }, ["cost_code"]);
    expect(fixed.order[0]).toBe("page");
    expect(fixed.order).toContain("custom:cost_code");
    expect(fixed.order).not.toContain("bogus");
    expect(fixed.visible).toEqual(["page"]);
    expect(normalizeColumnConfig(null).visible).toEqual(defaultColumnConfig().visible);
  });
});

describe("sorting", () => {
  const rows = buildRows(sample());
  it("cycles asc, desc, off", () => {
    const s1 = nextSort(null, "page");
    expect(s1).toEqual({ key: "page", dir: "asc" });
    const s2 = nextSort(s1, "page");
    expect(s2).toEqual({ key: "page", dir: "desc" });
    expect(nextSort(s2, "page")).toBeNull();
    expect(nextSort(s2, "author")).toEqual({ key: "author", dir: "asc" });
  });
  it("sorts pages numerically (10 after 3) and is stable on ties", () => {
    expect(sortRows(rows, { key: "page", dir: "asc" }).map((r) => r.id)).toEqual(["b", "d", "a", "c"]);
    expect(sortRows(rows, { key: "page", dir: "desc" }).map((r) => r.id)).toEqual(["c", "a", "b", "d"]);
  });
  it("sorts text case-insensitively and keeps empty cells last in both directions", () => {
    expect(sortRows(rows, { key: "author", dir: "asc" }).map((r) => r.author)).toEqual(["Ann", "Ann", "bob", "Zed"]);
    const asc = sortRows(rows, { key: "comment", dir: "asc" }).map((r) => r.id);
    const desc = sortRows(rows, { key: "comment", dir: "desc" }).map((r) => r.id);
    expect(asc[asc.length - 1]).toBe("c");
    expect(desc[desc.length - 1]).toBe("c");
  });
  it("no sort = document order, input untouched", () => {
    const copy = sortRows(rows, null);
    expect(copy.map((r) => r.id)).toEqual(["a", "b", "c", "d"]);
    expect(copy).not.toBe(rows);
  });
  it("sorts by replies numerically", () => {
    expect(sortRows(rows, { key: "replies", dir: "desc" })[0].id).toBe("d");
  });
});

describe("filtering", () => {
  const rows = buildRows(sample());
  it("empty filter is inactive and keeps everything", () => {
    expect(isFilterActive(emptyFilter())).toBe(false);
    expect(filterRows(rows, emptyFilter())).toHaveLength(4);
  });
  it("by status, author, type (AND across facets, OR within)", () => {
    expect(filterRows(rows, { ...emptyFilter(), statuses: new Set(["Rejected", "Accepted"]) }).map((r) => r.id)).toEqual(["b", "d"]);
    expect(filterRows(rows, { ...emptyFilter(), authors: new Set(["Ann"]) }).map((r) => r.id)).toEqual(["b", "c"]);
    expect(filterRows(rows, { ...emptyFilter(), types: new Set(["Ellipse"]) }).map((r) => r.id)).toEqual(["c"]);
    expect(filterRows(rows, { ...emptyFilter(), authors: new Set(["Ann"]), statuses: new Set(["Rejected"]) }).map((r) => r.id)).toEqual(["b"]);
  });
  it("by page spec", () => {
    expect(filterRows(rows, { ...emptyFilter(), pages: "1" }).map((r) => r.id)).toEqual(["b", "d"]);
    expect(filterRows(rows, { ...emptyFilter(), pages: "2-3, 10" }).map((r) => r.id)).toEqual(["a", "c"]);
    expect(filterRows(rows, { ...emptyFilter(), pages: "nonsense" })).toHaveLength(0);
  });
  it("text search covers comment, subject, author and reply text, case-insensitively", () => {
    expect(filterRows(rows, { ...emptyFilter(), text: "DOOR" }).map((r) => r.id)).toEqual(["b"]);
    expect(filterRows(rows, { ...emptyFilter(), text: "cloud" }).map((r) => r.id)).toEqual(["a"]);
    expect(filterRows(rows, { ...emptyFilter(), text: "lintel" }).map((r) => r.id)).toEqual(["d"]);
    expect(filterRows(rows, { ...emptyFilter(), text: "zed" }).map((r) => r.id)).toEqual(["a"]);
  });
  it("parsePageSpec ignores junk, normalises reversed ranges and caps tokens", () => {
    expect(parsePageSpec("5-2, x, 7")).toEqual([[2, 5], [7, 7]]);
    const huge = Array.from({ length: 5000 }, (_, i) => String(i + 1)).join(",");
    expect(parsePageSpec(huge).length).toBeLessThanOrEqual(50);
    expect(parsePageSpec("1-99999999999999")).toEqual([]);
  });
});

describe("hostile text", () => {
  it("clips huge cells and leaves markup-looking text as plain strings", () => {
    const big = "x".repeat(MAX_CELL_CHARS * 5);
    const [r] = buildRows([mk("h", { contents: `<img src=x onerror=alert(1)>${big}`, subject: "<script>1</script>" })]);
    expect(r.comment.length).toBe(MAX_CELL_CHARS);
    expect(r.comment.startsWith("<img")).toBe(true);
    expect(r.subject).toBe("<script>1</script>");
  });
  it("cyclic or dangling reply parents cannot hang display items", () => {
    const m = mk("t", { workflow: { status: "None", assignee: null, thread: [
      { id: "1", author: { user_id: "a", display_name: "A" }, at: "2026-01-01T00:00:00Z", contents: "x", in_reply_to: "2" },
      { id: "2", author: { user_id: "a", display_name: "A" }, at: "2026-01-01T00:00:00Z", contents: "y", in_reply_to: "1" },
      { id: "3", author: { user_id: "a", display_name: "A" }, at: "2026-01-01T00:00:00Z", contents: "z", in_reply_to: "nope" },
    ] } });
    const rows = buildRows([m]);
    const items = displayItems(rows, new Set(["t"]));
    expect(items.length).toBeLessThanOrEqual(4);
  });
});

describe("display items", () => {
  it("expands replies under their parent with depth, only for expanded rows", () => {
    const rows = buildRows(sample());
    expect(displayItems(rows, new Set())).toHaveLength(4);
    const items = displayItems(rows, new Set(["d"]));
    expect(items).toHaveLength(6);
    const replies = items.filter((i) => i.kind === "reply");
    expect(replies.map((i) => i.kind === "reply" && i.depth)).toEqual([0, 1]);
    expect(items[4].kind).toBe("reply");
  });
});

describe("selection", () => {
  const order = ["a", "b", "c", "d", "e"];
  const none = { ctrl: false, shift: false };
  it("plain click selects only that row", () => {
    expect(applyRowClick(new Set(["a", "b"]), "a", order, "c", none)).toEqual({ ids: new Set(["c"]), anchor: "c" });
  });
  it("ctrl toggles", () => {
    const r1 = applyRowClick(new Set(["a"]), "a", order, "c", { ctrl: true, shift: false });
    expect([...r1.ids].sort()).toEqual(["a", "c"]);
    const r2 = applyRowClick(r1.ids, r1.anchor, order, "a", { ctrl: true, shift: false });
    expect([...r2.ids]).toEqual(["c"]);
  });
  it("shift selects a range in view order from the anchor, either direction", () => {
    expect([...applyRowClick(new Set(["b"]), "b", order, "d", { ctrl: false, shift: true }).ids]).toEqual(["b", "c", "d"]);
    expect([...applyRowClick(new Set(["d"]), "d", order, "b", { ctrl: false, shift: true }).ids].sort()).toEqual(["b", "c", "d"]);
  });
  it("shift without an anchor degrades to a plain click", () => {
    expect(applyRowClick(new Set(), null, order, "c", { ctrl: false, shift: true }).ids).toEqual(new Set(["c"]));
  });
});

describe("status shortcuts", () => {
  it("Shift+1..5 map to the display order, 6..9 and non-shift do nothing", () => {
    expect(STATUS_ORDER).toEqual(["Accepted", "Rejected", "Cancelled", "Completed", "None"]);
    expect(statusForShortcut(key("Digit1"))).toBe("Accepted");
    expect(statusForShortcut(key("Digit3"))).toBe("Cancelled");
    expect(statusForShortcut(key("Digit5"))).toBe("None");
    expect(statusForShortcut(key("Digit6"))).toBeNull();
    expect(statusForShortcut(key("Digit1", false))).toBeNull();
    expect(statusForShortcut({ ...key("Digit1"), ctrlKey: true })).toBeNull();
    expect(statusForShortcut(key("KeyA"))).toBeNull();
  });
});

describe("virtual window", () => {
  it("renders a bounded slice for a long list", () => {
    const w = windowRange(0, 300, 24, 5000);
    expect(w.start).toBe(0);
    expect(w.end - w.start).toBeLessThan(40);
    const mid = windowRange(24 * 2500, 300, 24, 5000);
    expect(mid.start).toBe(2500 - 8);
    expect(mid.end).toBeLessThan(2500 + 40);
  });
  it("clamps at the end and handles empty", () => {
    expect(windowRange(1e9, 300, 24, 100)).toEqual({ start: 100, end: 100 });
    expect(windowRange(0, 300, 24, 0)).toEqual({ start: 0, end: 0 });
    expect(windowRange(0, 300, 24, 3)).toEqual({ start: 0, end: 3 });
  });
});

describe("5,000 mark-ups", () => {
  it("builds, filters and sorts well inside an interactive budget", () => {
    const authors = ["Ann", "Bob", "Cy", "Di", "Ed"];
    const statuses = ["None", "Accepted", "Rejected", "Completed", "Cancelled"] as const;
    const ms = Array.from({ length: 5000 }, (_, i) =>
      mk("m" + i, {
        page: i % 120,
        contents: `Comment number ${i} about ${i % 7 === 0 ? "clearance" : "detail"}`,
        author: authors[i % 5],
        created: `2026-0${1 + (i % 9)}-1${i % 9}T10:00:00Z`,
        workflow: { status: statuses[i % 5], assignee: null, thread: i % 10 === 0 ? [{ id: "r" + i, author: { user_id: "z", display_name: "Rae" }, at: "2026-01-01T00:00:00Z", contents: "reply" }] : [] },
      }));
    const t0 = performance.now();
    const rows = buildRows(ms);
    const filtered = filterRows(rows, { ...emptyFilter(), text: "clearance", statuses: new Set(["Accepted", "None"]) });
    const sorted = sortRows(filtered, { key: "comment", dir: "desc" });
    const all = sortRows(rows, { key: "date", dir: "asc" });
    const win = windowRange(24 * 2000, 400, 24, all.length);
    const ms1 = performance.now() - t0;
    expect(rows).toHaveLength(5000);
    expect(sorted.length).toBeGreaterThan(0);
    expect(win.end - win.start).toBeLessThan(50);
    // Measured ~30-60 ms locally; 1500 ms leaves headroom for a loaded CI runner.
    expect(ms1).toBeLessThan(1500);
  });
});

describe("count groups in the list (docs/count-polylength.md)", () => {
  const item = (id: string, group: string | null) => {
    const m = mk(id, { markup_type: "MeasurementCount" });
    m.count_group = group;
    m.measurement = { scale_ref: null, raw_measure: 1, unit: "ea", computed_quantity: 1, depth: null, count_value: 1, custom_columns: {} };
    return m;
  };
  it("one row per count carries the group total; ungrouped items stay separate", () => {
    const rows = buildRows([item("a", "G"), item("b", "G"), item("c", "G"), item("d", null)]);
    expect(rows.map((r) => [r.id, r.measurement, r.measurementValue])).toEqual([["a", "3", 3], ["d", "1", 1]]);
    expect(rows.map((r) => r.index)).toEqual([1, 2]);
  });
});
