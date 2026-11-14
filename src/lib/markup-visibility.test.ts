// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/svelte";
import { tick } from "svelte";
import { hideMarkups, showMarkups, isMarkupHidden, hiddenCount, withoutHidden } from "./markup-visibility";
import { dropLayerStore, layerStoreFor } from "./layers-store.svelte";
import { MarkupStore } from "./markup-store.svelte";
import { canvasMenuItems, markupMenuItems, type ActionItem, type MenuItem } from "./context-menu";
import { mk } from "./markups-list.test-util";
import MarkupsListPanel from "../components/MarkupsListPanel.svelte";

vi.mock("$lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc")>()),
  getUserIdentity: vi.fn(async () => ({ user_id: "me", display_name: "Me" })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(async () => null) }));

const acts = (items: MenuItem[]) => items.filter((i): i is ActionItem => i.kind === "item");

describe("pure helpers", () => {
  it("hide and show return new sets and leave the input alone", () => {
    const base = new Set(["a"]);
    const h = hideMarkups(base, ["b", "c"]);
    expect([...h].sort()).toEqual(["a", "b", "c"]);
    expect([...base]).toEqual(["a"]);
    expect([...showMarkups(h, ["a", "z"])].sort()).toEqual(["b", "c"]);
  });
  it("isMarkupHidden and hiddenCount ignore ids that no longer exist", () => {
    const h = new Set(["a", "gone"]);
    expect(isMarkupHidden(h, { id: "a" })).toBe(true);
    expect(isMarkupHidden(h, {})).toBe(false);
    expect(hiddenCount(h, ["a", "b"])).toBe(1);
  });
  it("withoutHidden drops hidden ids from a selection and keeps the rest", () => {
    expect([...withoutHidden(new Set(["a", "b", "c"]), new Set(["b"]))]).toEqual(["a", "c"]);
  });
});

describe("layer store view", () => {
  beforeEach(() => dropLayerStore("dv"));
  it("isShown is false for a hidden markup and true again after Show all", () => {
    const v = layerStoreFor("dv");
    expect(v.isShown({ id: "a" })).toBe(true);
    v.hideMarkupIds(["a"]);
    expect(v.isShown({ id: "a" })).toBe(false);
    expect(v.isShown({ id: "b" })).toBe(true);
    v.showAllMarkups();
    expect(v.isShown({ id: "a" })).toBe(true);
  });
  it("hiding is a view change only: no undo step, no dirty flag, no backend call", () => {
    const ipc = { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
    const s = new MarkupStore("dv", ipc);
    s.seed([mk("a"), mk("b")]);
    layerStoreFor("dv").hideMarkupIds(["a", "b"]);
    expect(s.dirty).toBe(false);
    expect(s.canUndo).toBe(false);
    expect(s.markups).toHaveLength(2);
    expect(ipc.add.mock.calls.length + ipc.update.mock.calls.length + ipc.remove.mock.calls.length).toBe(0);
  });
});

describe("menus", () => {
  it("markup menu offers Hide; canvas menu offers Hide All and Show All (enabled only when something is hidden)", () => {
    expect(acts(markupMenuItems({ count: 1, anyLocked: true })).find((i) => i.id === "hide")?.disabled).toBeFalsy();
    const none = acts(canvasMenuItems({ canPaste: false, hasMarkups: true, canBack: false, canForward: false }));
    expect(none.find((i) => i.id === "hide-all")?.disabled).toBe(false);
    expect(none.find((i) => i.id === "show-all")?.disabled).toBe(true);
    const some = acts(canvasMenuItems({ canPaste: false, hasMarkups: true, canBack: false, canForward: false, hasHidden: true }));
    expect(some.find((i) => i.id === "show-all")?.disabled).toBe(false);
  });
});

describe("Markups List", () => {
  beforeEach(() => { cleanup(); dropLayerStore("doc1"); try { localStorage.clear(); } catch { /* ignore */ } });
  afterEach(() => cleanup());
  const make = () => {
    const s = new MarkupStore("doc1", { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) });
    s.seed([mk("a", { contents: "one" }), mk("b", { contents: "two" })]);
    return s;
  };

  it("shows an icon on hidden rows, keeps them listed, and Hide / Show all act from the toolbar", async () => {
    const store = make();
    render(MarkupsListPanel, { store, docId: "doc1" });
    expect(screen.queryAllByTestId("ml-hidden-icon")).toHaveLength(0);

    store.selectedIds = new Set(["a"]);
    await tick();
    await fireEvent.click(document.querySelector("[data-action='hide-selected']")!);
    await tick();
    expect(screen.getAllByTestId("ml-hidden-icon")).toHaveLength(1);
    expect(document.querySelectorAll("[data-markup-id]")).toHaveLength(2); // still listed
    expect(layerStoreFor("doc1").isShown({ id: "a" })).toBe(false);

    await fireEvent.click(document.querySelector("[data-action='hide-all']")!);
    await tick();
    expect(screen.getAllByTestId("ml-hidden-icon")).toHaveLength(2);

    await fireEvent.click(document.querySelector("[data-action='show-all']")!);
    await tick();
    expect(screen.queryAllByTestId("ml-hidden-icon")).toHaveLength(0);
    expect(store.dirty).toBe(false);
  });

  it("Hide clears the hidden markups from the selection, so Delete removes nothing hidden", async () => {
    const store = make();
    render(MarkupsListPanel, { store, docId: "doc1" });
    store.selectedIds = new Set(["a"]);
    await tick();
    await fireEvent.click(document.querySelector("[data-action='hide-selected']")!);
    await tick();
    expect(store.selectedIds.size).toBe(0);
    store.deleteSelected();
    await tick();
    expect(store.markups.map((m) => m.id).sort()).toEqual(["a", "b"]);
    expect(store.dirty).toBe(false);
  });

  it("Hide keeps still-visible markups selected; Hide all empties the selection and Delete removes nothing", async () => {
    const store = make();
    render(MarkupsListPanel, { store, docId: "doc1" });
    store.selectedIds = new Set(["a", "b"]);
    await tick();
    await fireEvent.click(document.querySelector("[data-action='hide-all']")!);
    await tick();
    expect(store.selectedIds.size).toBe(0);
    store.deleteSelected();
    expect(store.markups).toHaveLength(2);
    // Hiding only some leaves the others selected.
    await fireEvent.click(document.querySelector("[data-action='show-all']")!);
    store.selectedIds = new Set(["a", "b"]);
    layerStoreFor("doc1").hideMarkupIds(["a"]);
    store.selectedIds = withoutHidden(store.selectedIds, layerStoreFor("doc1").hiddenMarkups);
    expect([...store.selectedIds]).toEqual(["b"]);
  });
});
