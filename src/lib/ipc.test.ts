import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import * as ipc from "./ipc";
import type { Markup, TileRequest } from "./ipc";

/**
 * Guards the Tauri v2 invoke argument-naming convention: JS passes **camelCase**
 * keys; Tauri maps them to the Rust command's snake_case params. Passing raw
 * snake_case keys (e.g. `doc_id`) makes Tauri reject the call with
 * "command X missing required key …" — a class of bug the rest of the suite
 * can't see because it mocks `invoke` without asserting key casing. Regression
 * guard for the 2026-06-15 GUI incident (blank viewport + "Load markups failed").
 */
const mockInvoke = vi.mocked(invoke);

describe("ipc invoke argument keys (Tauri v2 camelCase)", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined as never);
  });

  it("get_page_size → docId / pageIndex", async () => {
    await ipc.getPageSize("d1", 3);
    expect(mockInvoke).toHaveBeenCalledWith("get_page_size", { docId: "d1", pageIndex: 3 });
  });

  it("get_page_count → docId", async () => {
    await ipc.getPageCount("d1");
    expect(mockInvoke).toHaveBeenCalledWith("get_page_count", { docId: "d1" });
  });

  it("load_markups → docId", async () => {
    mockInvoke.mockResolvedValue([] as never);
    await ipc.loadMarkups("d1");
    expect(mockInvoke).toHaveBeenCalledWith("load_markups", { docId: "d1" });
  });

  it("list_markups → docId", async () => {
    mockInvoke.mockResolvedValue([] as never);
    await ipc.listMarkups("d1");
    expect(mockInvoke).toHaveBeenCalledWith("list_markups", { docId: "d1" });
  });

  it("close_document / save_document → docId", async () => {
    await ipc.closeDocument("d1");
    expect(mockInvoke).toHaveBeenCalledWith("close_document", { docId: "d1" });
    await ipc.saveDocument("d1");
    expect(mockInvoke).toHaveBeenCalledWith("save_document", { docId: "d1" });
  });

  it("save_document_as → docId / newPath", async () => {
    await ipc.saveDocumentAs("d1", "/tmp/x.pdf");
    expect(mockInvoke).toHaveBeenCalledWith("save_document_as", { docId: "d1", newPath: "/tmp/x.pdf" });
  });

  it("delete_markup → docId / markupId", async () => {
    await ipc.deleteMarkup("d1", "m1");
    expect(mockInvoke).toHaveBeenCalledWith("delete_markup", { docId: "d1", markupId: "m1" });
  });

  it("add_markup / update_markup → docId + markup", async () => {
    const m = { id: "m1" } as Markup;
    await ipc.addMarkup("d1", m);
    expect(mockInvoke).toHaveBeenCalledWith("add_markup", { docId: "d1", markup: m });
    await ipc.updateMarkup("d1", m);
    expect(mockInvoke).toHaveBeenCalledWith("update_markup", { docId: "d1", markup: m });
  });

  it("open_document → path (single-word, unchanged)", async () => {
    await ipc.openDocument("/tmp/a.pdf");
    expect(mockInvoke).toHaveBeenCalledWith("open_document", { path: "/tmp/a.pdf" });
  });

  it("render_tile → req struct (inner fields stay snake_case via serde)", async () => {
    const req = { doc_id: "d1", page_index: 0 } as unknown as TileRequest;
    await ipc.renderTile(req);
    expect(mockInvoke).toHaveBeenCalledWith("render_tile", { req });
  });
});
