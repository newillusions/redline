// @vitest-environment jsdom
/**
 * MarkupsListPanel: rendering, sort/filter/column interaction, selection + jump, status
 * change (incl. Shift+N and the on_reply exclusion), export of the filtered view, hostile
 * text rendering and the 5,000-row render window.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/svelte";
import { tick } from "svelte";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import MarkupsListPanel from "./MarkupsListPanel.svelte";
import { MarkupStore } from "$lib/markup-store.svelte";
import { exportMarkupsView } from "$lib/ipc";
import { mk } from "$lib/markups-list.test-util";
import type { Markup } from "$lib/ipc";

vi.mock("$lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc")>()),
  exportMarkupsView: vi.fn(async () => 2),
  getUserIdentity: vi.fn(async () => ({ user_id: "me", display_name: "Me" })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(async () => "/tmp/out.csv") }));

function fakeIpc() {
  return { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
}

let ipc = fakeIpc();
function makeStore(markups: Markup[]) {
  ipc = fakeIpc();
  const store = new MarkupStore("doc1", ipc);
  store.seed(markups);
  return store;
}

const rowsInDom = () => document.querySelectorAll("[data-markup-id]");
const ids = () => [...rowsInDom()].map((r) => r.getAttribute("data-markup-id"));

function data(): Markup[] {
  return [
    mk("a", { page: 2, contents: "Check clearance", author: "Zed", subject: "Cloud" }),
    mk("b", { page: 0, contents: "Door swing", author: "Ann", workflow: { status: "Rejected", assignee: null, thread: [] } }),
    mk("c", { page: 9, contents: "Later", author: "Bob" }),
  ];
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  try { localStorage.clear(); } catch { /* ignore */ }
});

describe("MarkupsListPanel layer filter", () => {
  it("appears only when mark-ups sit on layers, and limits the rows to the ticked layer", async () => {
    render(MarkupsListPanel, { store: makeStore(data()), docId: "doc1" });
    expect(screen.queryByTestId("ml-layer-filter")).toBeNull();
    cleanup();
    const layered = [
      mk("a", { layer: "Lighting" }), mk("b", { layer: "Ceiling" }), mk("c"),
    ];
    render(MarkupsListPanel, { store: makeStore(layered), docId: "doc1" });
    const menu = screen.getByTestId("ml-layer-filter");
    expect(menu.textContent).toContain("Layer");
    const box = [...menu.querySelectorAll("label")].find((l) => l.textContent?.includes("Lighting"))!.querySelector("input")!;
    await fireEvent.click(box);
    await tick();
    expect(ids()).toEqual(["a"]);
    expect(screen.getByText("1 of 3")).toBeTruthy();
  });

  it("two groups with the same name are two entries, told apart by number, and filter separately", async () => {
    const on = (n: number) => ({ layer: "Lighting", optional_content: { Reference: [n, 0] as [number, number] } });
    render(MarkupsListPanel, { store: makeStore([mk("a", on(11)), mk("b", on(12))]), docId: "doc1" });
    const menu = screen.getByTestId("ml-layer-filter");
    const labels = [...menu.querySelectorAll("label")].map((l) => l.textContent?.trim());
    expect(labels).toEqual(["Lighting (#11)", "Lighting (#12)"]);
    await fireEvent.click(menu.querySelectorAll("label input")[1]);
    await tick();
    expect(ids()).toEqual(["b"]);
  });
});

describe("MarkupsListPanel", () => {
  it("lists every mark-up with the default columns", () => {
    render(MarkupsListPanel, { store: makeStore(data()), docId: "doc1" });
    expect(ids()).toEqual(["a", "b", "c"]);
    expect(screen.getByRole("columnheader", { name: /Comments/ })).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: /Status/ })).toBeTruthy();
    expect(screen.queryByRole("columnheader", { name: /Layer/ })).toBeNull();
    expect(screen.getByText("3 mark-ups")).toBeTruthy();
  });

  it("sorts by a column header click (asc then desc) and flags aria-sort", async () => {
    render(MarkupsListPanel, { store: makeStore(data()), docId: "doc1" });
    const head = screen.getByRole("columnheader", { name: /^Page/ });
    await fireEvent.click(head);
    expect(ids()).toEqual(["b", "a", "c"]);
    expect(head.getAttribute("aria-sort")).toBe("ascending");
    await fireEvent.click(head);
    expect(ids()).toEqual(["c", "a", "b"]);
    await fireEvent.click(head);
    expect(ids()).toEqual(["a", "b", "c"]);
  });

  it("filters by search text and by status, and shows the filtered count", async () => {
    render(MarkupsListPanel, { store: makeStore(data()), docId: "doc1" });
    await fireEvent.input(screen.getByLabelText("Search mark-ups"), { target: { value: "door" } });
    expect(ids()).toEqual(["b"]);
    expect(screen.getByText("1 of 3")).toBeTruthy();
    await fireEvent.click(screen.getByText("Clear"));
    expect(ids()).toHaveLength(3);
    const rejected = screen.getAllByLabelText("Rejected")[0] ?? screen.getByLabelText(/Rejected/);
    await fireEvent.click(rejected);
    expect(ids()).toEqual(["b"]);
  });

  it("column picker hides and shows columns", async () => {
    render(MarkupsListPanel, { store: makeStore(data()), docId: "doc1" });
    await fireEvent.click(screen.getByLabelText("Layer"));
    expect(screen.getByRole("columnheader", { name: /Layer/ })).toBeTruthy();
    await fireEvent.click(screen.getByLabelText("Author"));
    expect(screen.queryByRole("columnheader", { name: /Author/ })).toBeNull();
    // Persisted for the next session.
    expect(localStorage.getItem("redline.markupsList.columns.v1")).toContain("layer");
  });

  it("click selects the mark-up in the store and requests a jump; double-click too", async () => {
    const store = makeStore(data());
    const onjump = vi.fn();
    render(MarkupsListPanel, { store, docId: "doc1", onjump });
    const rowB = document.querySelector('[data-markup-id="b"]')!;
    await fireEvent.click(rowB);
    expect([...store.selectedIds]).toEqual(["b"]);
    expect(store.activeTool).toBe("select");
    expect(onjump).toHaveBeenCalledTimes(1);
    expect(onjump.mock.calls[0][0]).toMatchObject({ page: 0, markupId: "b", rect: [0, 0, 10, 10] });
    await fireEvent.dblClick(document.querySelector('[data-markup-id="c"]')!);
    expect([...store.selectedIds]).toEqual(["c"]);
    expect(onjump.mock.calls[1][0]).toMatchObject({ page: 9, markupId: "c" });
  });

  it("ctrl-click and shift-click multi-select without jumping", async () => {
    const store = makeStore(data());
    const onjump = vi.fn();
    render(MarkupsListPanel, { store, docId: "doc1", onjump });
    await fireEvent.click(document.querySelector('[data-markup-id="a"]')!);
    onjump.mockClear();
    await fireEvent.click(document.querySelector('[data-markup-id="c"]')!, { shiftKey: true });
    expect([...store.selectedIds].sort()).toEqual(["a", "b", "c"]);
    await fireEvent.click(document.querySelector('[data-markup-id="b"]')!, { ctrlKey: true });
    expect([...store.selectedIds].sort()).toEqual(["a", "c"]);
    expect(onjump).not.toHaveBeenCalled();
  });

  it("highlights rows selected on the canvas", async () => {
    const store = makeStore(data());
    render(MarkupsListPanel, { store, docId: "doc1" });
    store.selectedIds = new Set(["c"]);
    await tick();
    expect(document.querySelector('[data-markup-id="c"]')!.getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector('[data-markup-id="a"]')!.getAttribute("aria-selected")).toBe("false");
  });

  it("Shift+3 sets Cancelled on the selection as one undoable frame and records history", async () => {
    const store = makeStore(data());
    render(MarkupsListPanel, { store, docId: "doc1" });
    await fireEvent.click(document.querySelector('[data-markup-id="a"]')!);
    await fireEvent.click(document.querySelector('[data-markup-id="c"]')!, { ctrlKey: true });
    const grid = screen.getByRole("grid");
    await fireEvent.keyDown(grid, { code: "Digit3", key: "#", shiftKey: true });
    const a = store.getById("a")!;
    const c = store.getById("c")!;
    expect(a.workflow.status).toBe("Cancelled");
    expect(c.workflow.status).toBe("Cancelled");
    expect(store.getById("b")!.workflow.status).toBe("Rejected");
    const last = a.workflow.status_history!.at(-1)!;
    expect(last).toMatchObject({ model: "Review", state: "Cancelled" });
    expect(last.on_reply ?? null).toBeNull();
    await store.flush();
    expect(ipc.update).toHaveBeenCalledTimes(2);
    store.undo();
    expect(store.getById("a")!.workflow.status).toBe("None");
    expect(store.getById("c")!.workflow.status).toBe("None");
  });

  it("Shift+1..5 does nothing while focus is in a text input or the inline status select", async () => {
    const store = makeStore(data());
    render(MarkupsListPanel, { store, docId: "doc1" });
    await fireEvent.click(document.querySelector('[data-markup-id="a"]')!);
    // Search box: its keydown never reaches the grid, and typing must not change status.
    const search = screen.getByLabelText("Search mark-ups");
    await fireEvent.keyDown(search, { code: "Digit3", key: "#", shiftKey: true });
    expect(store.getById("a")!.workflow.status).toBe("None");
    // Inline status select lives inside the grid, so its keydown DOES bubble to the grid handler.
    await fireEvent.dblClick(document.querySelector('[data-markup-id="a"] [title="Double-click to change"]')!);
    const sel = document.querySelector('[data-markup-id="a"] select')!;
    expect(sel).toBeTruthy();
    await fireEvent.keyDown(sel, { code: "Digit3", key: "#", shiftKey: true });
    expect(store.getById("a")!.workflow.status).toBe("None");
    // Control: the same key on the grid itself still applies the status.
    await fireEvent.keyDown(screen.getByRole("grid"), { code: "Digit3", key: "#", shiftKey: true });
    expect(store.getById("a")!.workflow.status).toBe("Cancelled");
  });

  it("the Set status menu changes the selection; locked mark-ups are skipped", async () => {
    const locked = mk("l", { annot_flags: 0x80 });
    const store = makeStore([...data(), locked]);
    render(MarkupsListPanel, { store, docId: "doc1" });
    await fireEvent.click(document.querySelector('[data-markup-id="a"]')!);
    await fireEvent.click(document.querySelector('[data-markup-id="l"]')!, { ctrlKey: true });
    await fireEvent.change(screen.getByLabelText("Set status of selected mark-ups"), { target: { value: "Accepted" } });
    expect(store.getById("a")!.workflow.status).toBe("Accepted");
    expect(store.getById("l")!.workflow.status).toBe("None");
    expect(screen.getByRole("alert").textContent).toContain("1 locked mark-up skipped");
  });

  it("a status on a reply never changes the mark-up's shown status", () => {
    const m = mk("r", {
      workflow: {
        status: "None", assignee: null,
        thread: [{ id: "rep", author: { user_id: "x", display_name: "Rae" }, at: "2026-01-02T00:00:00Z", contents: "ok" }],
        status_history: [{ id: "s", author: { user_id: "x", display_name: "Rae" }, at: "2026-01-02T00:00:00Z", model: "Review", state: "Rejected", on_reply: "rep" }],
      },
    });
    render(MarkupsListPanel, { store: makeStore([m]), docId: "doc1" });
    const row = document.querySelector('[data-markup-id="r"]')!;
    expect(row.textContent).toContain("None");
    expect(row.textContent).not.toContain("Rejected");
    expect(row.textContent).toContain("1"); // reply count
  });

  it("expands replies under their parent", async () => {
    const m = mk("r", {
      workflow: { status: "None", assignee: null, thread: [
        { id: "1", author: { user_id: "x", display_name: "Rae" }, at: "2026-01-02T00:00:00Z", contents: "first reply" },
        { id: "2", author: { user_id: "y", display_name: "Sam" }, at: "2026-01-03T00:00:00Z", contents: "nested", in_reply_to: "1" },
      ] },
    });
    render(MarkupsListPanel, { store: makeStore([m]), docId: "doc1" });
    expect(document.querySelectorAll("[data-reply-of]")).toHaveLength(0);
    await fireEvent.click(screen.getByLabelText("Show replies"));
    expect(document.querySelectorAll("[data-reply-of]")).toHaveLength(2);
    expect(screen.getByText(/first reply/)).toBeTruthy();
  });

  it("exports the FILTERED view in sorted order with the visible columns", async () => {
    const store = makeStore(data());
    render(MarkupsListPanel, { store, docId: "doc1" });
    await fireEvent.input(screen.getByLabelText("Filter by page"), { target: { value: "1-3" } });
    await fireEvent.click(screen.getByRole("columnheader", { name: /^Page/ }));
    await fireEvent.click(screen.getByLabelText("Layer"));
    await fireEvent.click(screen.getByText("Export CSV"));
    await vi.waitFor(() => expect(exportMarkupsView).toHaveBeenCalled());
    const args = vi.mocked(exportMarkupsView).mock.calls[0];
    expect(saveDialog).toHaveBeenCalled();
    expect(args[0]).toBe("doc1");
    expect(args[1]).toBe("/tmp/out.csv");
    expect(args[2]).toBe("Csv");
    expect(args[3]).toEqual(["b", "a"]);
    expect(args[4]).toEqual(["subject", "page", "comment", "author", "date", "status", "replies", "layer"]);
  });

  it("renders hostile text as text, never as HTML", () => {
    const m = mk("h", { contents: '<img src=x onerror="window.__pwned=1"><b>bold</b>', subject: "<script>window.__pwned=1</script>", author: "<i>x</i>" });
    render(MarkupsListPanel, { store: makeStore([m]), docId: "doc1" });
    const row = document.querySelector('[data-markup-id="h"]')!;
    expect(row.querySelector("img, b, script, i")).toBeNull();
    expect(row.textContent).toContain("<img src=x");
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("with 5,000 mark-ups only a window of rows is in the DOM, and scrolling moves it", async () => {
    const many = Array.from({ length: 5000 }, (_, i) => mk("m" + i, { page: i % 50, contents: "c" + i }));
    const t0 = performance.now();
    render(MarkupsListPanel, { store: makeStore(many), docId: "doc1" });
    const renderMs = performance.now() - t0;
    expect(rowsInDom().length).toBeGreaterThan(0);
    expect(rowsInDom().length).toBeLessThan(60);
    expect(screen.getByText("5000 mark-ups")).toBeTruthy();
    const grid = screen.getByRole("grid");
    grid.scrollTop = 24 * 2500;
    await fireEvent.scroll(grid);
    await tick();
    expect(rowsInDom().length).toBeLessThan(60);
    expect(ids()).toContain("m2500");
    expect(ids()).not.toContain("m0");
    // Local: well under 500 ms; the bound is loose for loaded CI runners.
    expect(renderMs).toBeLessThan(3000);
  });
});
