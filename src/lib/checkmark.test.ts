// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/svelte";
import { tick } from "svelte";
import { patchMarked } from "./markup-properties";
import { isMarked, latestMarkedChange } from "./markup-review";
import { buildRow, cellText } from "./markups-list";
import { MarkupStore } from "./markup-store.svelte";
import { mk } from "./markups-list.test-util";
import MarkupsListPanel from "../components/MarkupsListPanel.svelte";

vi.mock("$lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc")>()),
  getUserIdentity: vi.fn(async () => ({ user_id: "me", display_name: "Me" })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(async () => null) }));

const BY = { user_id: "u1", display_name: "Rev" };
const T0 = "2026-11-14T10:00:00.000Z";

describe("patchMarked", () => {
  it("appends a standard Marked-model entry, leaves the Review status alone, and is pure", () => {
    const m = mk("a");
    const t = patchMarked(m, true, BY, T0, "e1");
    expect(m.workflow.status_history ?? []).toHaveLength(0);
    expect(t.workflow.status_history).toEqual([{ id: "e1", author: BY, at: T0, model: "Marked", state: "Marked" }]);
    expect(t.workflow.status).toBe(m.workflow.status);
    expect(isMarked(t)).toBe(true);
    expect(isMarked(m)).toBe(false);
  });

  it("untick appends Unmarked and keeps the Marked entry in the history", () => {
    const t = patchMarked(patchMarked(mk("a"), true, BY, T0, "e1"), false, BY, "2026-11-14T10:05:00.000Z", "e2");
    expect(t.workflow.status_history!.map((c) => c.state)).toEqual(["Marked", "Unmarked"]);
    expect(isMarked(t)).toBe(false);
  });

  it("returns the same object when the checkmark already has that value", () => {
    const m = mk("a");
    expect(patchMarked(m, false, BY, T0)).toBe(m);
    const t = patchMarked(m, true, BY, T0);
    expect(patchMarked(t, true, BY, T0)).toBe(t);
  });

  it("dates a quick untick at least one second after the tick so the saved order survives a reload", () => {
    const t = patchMarked(patchMarked(mk("a"), true, BY, T0, "e1"), false, BY, T0, "e2");
    const h = t.workflow.status_history!;
    expect(Date.parse(h[1].at) - Date.parse(h[0].at)).toBeGreaterThanOrEqual(1000);
  });

  it("ignores Marked entries on a reply and entries of other models", () => {
    const m = mk("a");
    m.workflow.status_history = [
      { id: "x", author: BY, at: T0, model: "Marked", state: "Marked", on_reply: "r1" },
      { id: "y", author: BY, at: T0, model: "Review", state: "Accepted" },
    ];
    expect(isMarked(m)).toBe(false);
    expect(latestMarkedChange(m.workflow.status_history)).toBeNull();
  });
});

describe("list row", () => {
  it("shows Yes in the Checkmark column only for a ticked markup", () => {
    expect(cellText(buildRow(patchMarked(mk("a"), true, BY, T0), 1), "checkmark")).toBe("Yes");
    expect(cellText(buildRow(mk("b"), 2), "checkmark")).toBe("");
  });
});

describe("Markups List checkmark", () => {
  beforeEach(() => { cleanup(); try { localStorage.clear(); } catch { /* ignore */ } });
  afterEach(() => cleanup());
  const make = () => {
    const ipc = { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
    const s = new MarkupStore("doc1", ipc);
    s.seed([mk("a", { contents: "one" }), mk("b", { contents: "two" }), mk("c", { contents: "three" })]);
    return { s, ipc };
  };

  it("a Checkmark column is shown and a checkbox ticks one markup, mirroring the Marked entry", async () => {
    const { s, ipc } = make();
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    const boxes = screen.getAllByTestId("ml-checkmark") as HTMLInputElement[];
    expect(boxes).toHaveLength(3);
    expect(boxes.every((b) => !b.checked)).toBe(true);
    await fireEvent.click(boxes[0]);
    await tick();
    expect(isMarked(s.getById("a")!)).toBe(true);
    expect(isMarked(s.getById("b")!)).toBe(false);
    await s.flush();
    const sent = ipc.update.mock.calls.map((c) => (c as unknown as [string, { id: string; workflow: { status_history: { model: string; state: string }[] } }])[1]);
    expect(sent.map((m) => m.id)).toEqual(["a"]);
    expect(sent[0].workflow.status_history.at(-1)).toMatchObject({ model: "Marked", state: "Marked" });
    expect(s.dirty).toBe(true);
  });

  it("Check all ticks every markup and Clear all resets every mark, each as ONE undo step", async () => {
    const { s } = make();
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    await fireEvent.click(document.querySelector("[data-action='check-all']")!);
    await tick();
    expect(s.markups.every((m) => isMarked(m))).toBe(true);
    await fireEvent.click(document.querySelector("[data-action='clear-all']")!);
    await tick();
    expect(s.markups.some((m) => isMarked(m))).toBe(false);
    // One undo brings every mark back; a second removes them again (two frames, not six).
    s.undo();
    await tick();
    expect(s.markups.every((m) => isMarked(m))).toBe(true);
    s.undo();
    await tick();
    expect(s.markups.some((m) => isMarked(m))).toBe(false);
    expect(s.canUndo).toBe(false);
  });

  it("Clear all is disabled with nothing ticked and Check all with everything ticked", async () => {
    const { s } = make();
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    expect((document.querySelector("[data-action='clear-all']") as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(document.querySelector("[data-action='check-all']")!);
    await tick();
    expect((document.querySelector("[data-action='check-all']") as HTMLButtonElement).disabled).toBe(true);
    expect((document.querySelector("[data-action='clear-all']") as HTMLButtonElement).disabled).toBe(false);
  });
});
