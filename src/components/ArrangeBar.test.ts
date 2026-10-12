// @vitest-environment jsdom
/**
 * ArrangeBar + its dialogs: buttons enable by selection size, run the real commands against a
 * real MarkupStore, and the Multiply / Paste-to-pages dialogs keep open on a refusal.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import ArrangeBar from "./ArrangeBar.svelte";
import { markupClipboard } from "$lib/markup-clipboard.svelte";
import type { ArrangeCtx } from "$lib/arrange-actions";
import { BY, mk, seqIds, storeWith } from "$lib/arrange.test-util";

function mount(ms = [mk("a", { rect: [10, 100, 30, 120] }), mk("b", { rect: [50, 100, 70, 120] }), mk("c", { rect: [90, 100, 110, 120] })]) {
  const store = storeWith(ms);
  const mint = seqIds("ui");
  const getCtx = (): ArrangeCtx => ({
    store, identity: BY, scales: [], pageIndex: 0, pageCount: 4, pageWidthPts: 600, pageHeightPts: 400,
    cursor: null, viewCenter: { x: 300, y: 200 }, now: () => "2026-10-12T10:00:00Z", mintId: mint,
  });
  const r = render(ArrangeBar, { props: { store, getCtx } });
  return { store, ...r };
}
const btn = (name: RegExp | string) => screen.getByRole("button", { name }) as HTMLButtonElement;

beforeEach(() => markupClipboard.clear());

describe("ArrangeBar", () => {
  it("enables buttons by what is selected and on the clipboard", async () => {
    const { store } = mount();
    expect(btn("Copy").disabled).toBe(true);
    expect(btn("Left").disabled).toBe(true);
    expect(btn("Paste").disabled).toBe(true);
    store.selectedIds = new Set(["a"]);
    await Promise.resolve();
    expect(btn("Copy").disabled).toBe(false);
    expect(btn("Front").disabled).toBe(false);
    expect(btn("Left").disabled).toBe(true);
    expect(btn("Horizontally").disabled).toBe(true);
    store.selectedIds = new Set(["a", "b"]);
    await Promise.resolve();
    expect(btn("Left").disabled).toBe(false);
    expect(btn("Horizontally").disabled).toBe(true);
    store.selectedIds = new Set(["a", "b", "c"]);
    await Promise.resolve();
    expect(btn("Horizontally").disabled).toBe(false);
  });

  it("Copy then Paste puts a copy centred in the view and says so", async () => {
    const { store } = mount();
    store.selectedIds = new Set(["a"]);
    await fireEvent.click(btn("Copy"));
    expect(screen.getByRole("status").textContent).toBe("Copied 1 markup");
    await fireEvent.click(btn("Paste"));
    expect(store.markups).toHaveLength(4);
    expect(store.markups[3].geometry).toEqual({ Rect: { min: { x: 290, y: 190 }, max: { x: 310, y: 210 } } });
    expect(screen.getByRole("status").textContent).toBe("Pasted 1 markup");
  });

  it("Align left moves the others to the first selected", async () => {
    const { store } = mount();
    store.selectedIds = new Set(["b", "a", "c"]);
    await fireEvent.click(btn("Left"));
    const xs = ["a", "b", "c"].map((id) => (store.getById(id)!.geometry as { Rect: { min: { x: number } } }).Rect.min.x);
    expect(xs).toEqual([50, 50, 50]);
  });

  it("Multiply: a refusal keeps the dialog open with the reason; a good request closes it", async () => {
    const { store } = mount();
    store.selectedIds = new Set(["a"]);
    await fireEvent.click(btn("Multiply..."));
    const dlg = screen.getByRole("dialog", { name: "Multiply" });
    expect(dlg).toBeTruthy();
    // Far too big a step for the page: nothing fits, the dialog stays and says why.
    await fireEvent.input(screen.getByLabelText("Distance"), { target: { value: "5000" } });
    await fireEvent.click(btn(/^Multiply$/));
    expect(screen.getByRole("alert").textContent).toMatch(/No copy fits/);
    expect(store.markups).toHaveLength(3);
    await fireEvent.input(screen.getByLabelText("Distance"), { target: { value: "50" } });
    await fireEvent.input(screen.getByLabelText("Copies"), { target: { value: "2" } });
    await fireEvent.click(btn(/^Multiply$/));
    expect(store.markups).toHaveLength(5);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("status").textContent).toBe("Made 2 markups");
  });

  it("Paste to pages: parses a page list, refuses a bad one, pastes in place on each", async () => {
    const { store } = mount();
    store.selectedIds = new Set(["a"]);
    await fireEvent.click(btn("Copy"));
    await fireEvent.click(btn("Paste to pages..."));
    await fireEvent.input(screen.getByLabelText("Pages"), { target: { value: "abc" } });
    const dialogPaste = () => screen.getAllByRole("button", { name: "Paste" }).at(-1)!;
    await fireEvent.click(dialogPaste());
    expect(screen.getByRole("alert").textContent).toMatch(/between 1 and 4/);
    await fireEvent.input(screen.getByLabelText("Pages"), { target: { value: "2-3" } });
    await fireEvent.click(dialogPaste());
    expect(store.markups.slice(3).map((m) => m.page)).toEqual([1, 2]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
