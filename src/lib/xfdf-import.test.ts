// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, fireEvent, cleanup } from "@testing-library/svelte";
import { tick } from "svelte";
import { invoke } from "@tauri-apps/api/core";
import { importMarkupsXfdf, importSummary, type XfdfImportResult } from "./xfdf-import";
import { MarkupStore } from "./markup-store.svelte";
import { mk } from "./markups-list.test-util";
import MarkupsListPanel from "../components/MarkupsListPanel.svelte";

const mockInvoke = vi.mocked(invoke);
const pickXfdfFile = vi.hoisted(() => vi.fn(async () => "/tmp/in.xfdf" as string | null));

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(async () => null) }));
vi.mock("./xfdf-import", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./xfdf-import")>()),
  pickXfdfFile,
}));
vi.mock("$lib/xfdf-import", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./xfdf-import")>()),
  pickXfdfFile,
}));
vi.mock("$lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc")>()),
  getUserIdentity: vi.fn(async () => ({ user_id: "me", display_name: "Me" })),
  getPageCount: vi.fn(async () => 5),
}));

const result = (over: Partial<XfdfImportResult> = {}): XfdfImportResult => ({
  markups: [],
  skippedExisting: 0,
  ignored: 0,
  simplified: 0,
  ...over,
});

describe("importMarkupsXfdf", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(result() as never);
  });

  it("passes camelCase keys (docId, pageCount) to import_markups_xfdf", async () => {
    await importMarkupsXfdf("d1", "/tmp/a.xfdf", 7);
    expect(mockInvoke).toHaveBeenCalledWith("import_markups_xfdf", { docId: "d1", path: "/tmp/a.xfdf", pageCount: 7 });
  });
});

describe("importSummary", () => {
  it("says what came in and only mentions what happened", () => {
    expect(importSummary(result({ markups: [mk("a")] }))).toBe("Imported 1 mark-up");
    expect(importSummary(result({ markups: [mk("a"), mk("b")], skippedExisting: 3, ignored: 1, simplified: 2 }))).toBe(
      "Imported 2 mark-ups; 3 already in the document; 2 as plain shapes; 1 not imported",
    );
  });
});

describe("Markups List Import XFDF", () => {
  const make = () => {
    const ipc = { add: vi.fn(async () => {}), update: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
    const s = new MarkupStore("doc1", ipc);
    s.seed([mk("a", { contents: "one" })]);
    return { s, ipc };
  };
  beforeEach(() => {
    cleanup();
    mockInvoke.mockReset();
    pickXfdfFile.mockResolvedValue("/tmp/in.xfdf");
  });
  afterEach(() => cleanup());

  it("adds the file's markups as ONE undo step and reports the summary", async () => {
    const { s, ipc } = make();
    mockInvoke.mockResolvedValue(
      result({ markups: [mk("b", { contents: "two" }), mk("c", { contents: "three" })], skippedExisting: 1 }) as never,
    );
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    await fireEvent.click(document.querySelector("[data-action='import-xfdf']")!);
    await vi.waitFor(() => expect(s.markups).toHaveLength(3));
    expect(mockInvoke).toHaveBeenCalledWith("import_markups_xfdf", { docId: "doc1", path: "/tmp/in.xfdf", pageCount: 5 });
    await s.flush();
    expect(ipc.add).toHaveBeenCalledTimes(2);
    expect(document.body.textContent).toContain("Imported 2 mark-ups; 1 already in the document");
    // One undo removes both; nothing further to undo.
    s.undo();
    await tick();
    expect(s.markups.map((m) => m.id)).toEqual(["a"]);
    expect(s.canUndo).toBe(false);
  });

  it("does nothing when the picker is cancelled", async () => {
    const { s } = make();
    pickXfdfFile.mockResolvedValue(null);
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    await fireEvent.click(document.querySelector("[data-action='import-xfdf']")!);
    await tick();
    expect(mockInvoke).not.toHaveBeenCalledWith("import_markups_xfdf", expect.anything());
    expect(s.markups).toHaveLength(1);
  });

  it("shows the backend error and adds nothing", async () => {
    const { s } = make();
    mockInvoke.mockRejectedValue("not valid XFDF: bad");
    render(MarkupsListPanel, { store: s, docId: "doc1" });
    await fireEvent.click(document.querySelector("[data-action='import-xfdf']")!);
    await vi.waitFor(() => expect(document.body.textContent).toContain("Import failed"));
    expect(s.markups).toHaveLength(1);
  });
});
