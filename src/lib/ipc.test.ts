import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import * as ipc from "./ipc";
import type { Markup, TileRequest } from "./ipc";
import type { RotatePageArgs, DeletePageArgs, ReorderPagesArgs, InsertBlankPageArgs } from "./ipc";

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

// ---------------------------------------------------------------------------
// Page operation IPC wrappers (M4 S1)
// ---------------------------------------------------------------------------

describe("page operation ipc wrappers (Tauri v2 camelCase keys)", () => {
  const mockInvoke = vi.mocked(invoke);

  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(undefined as never);
  });

  it("rotatePage → docId / pageIdx / degrees", async () => {
    const args: RotatePageArgs = { doc_id: "d1", page_idx: 2, degrees: 90 };
    await ipc.rotatePage(args);
    expect(mockInvoke).toHaveBeenCalledWith("rotate_page", {
      docId: "d1",
      pageIdx: 2,
      degrees: 90,
    });
  });

  it("deletePage → docId / pageIdx", async () => {
    const args: DeletePageArgs = { doc_id: "d1", page_idx: 3 };
    await ipc.deletePage(args);
    expect(mockInvoke).toHaveBeenCalledWith("delete_page", {
      docId: "d1",
      pageIdx: 3,
    });
  });

  it("reorderPages → docId / newOrder", async () => {
    const args: ReorderPagesArgs = { doc_id: "d1", new_order: [2, 0, 1] };
    await ipc.reorderPages(args);
    expect(mockInvoke).toHaveBeenCalledWith("reorder_pages", {
      docId: "d1",
      newOrder: [2, 0, 1],
    });
  });

  it("insertBlankPage → docId / at / width / height", async () => {
    const args: InsertBlankPageArgs = { doc_id: "d1", at: 1, width: 612, height: 792 };
    await ipc.insertBlankPage(args);
    expect(mockInvoke).toHaveBeenCalledWith("insert_blank_page", {
      docId: "d1",
      at: 1,
      width: 612,
      height: 792,
    });
  });

  it("rotatePage returns void on success", async () => {
    const args: RotatePageArgs = { doc_id: "d1", page_idx: 0, degrees: 180 };
    const result = await ipc.rotatePage(args);
    expect(result).toBeUndefined();
  });

  it("deletePage returns void on success", async () => {
    const args: DeletePageArgs = { doc_id: "d1", page_idx: 0 };
    const result = await ipc.deletePage(args);
    expect(result).toBeUndefined();
  });

  it("reorderPages returns void on success", async () => {
    const args: ReorderPagesArgs = { doc_id: "d1", new_order: [1, 0] };
    const result = await ipc.reorderPages(args);
    expect(result).toBeUndefined();
  });

  it("insertBlankPage returns void on success", async () => {
    const args: InsertBlankPageArgs = { doc_id: "d1", at: 0, width: 595, height: 842 };
    const result = await ipc.insertBlankPage(args);
    expect(result).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// Text search IPC wrappers (M4 S3)
// ---------------------------------------------------------------------------

describe("text search ipc wrappers (Tauri v2 camelCase keys)", () => {
  const mockInvoke = vi.mocked(invoke);

  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue([] as never);
  });

  it("searchDocument → docId / query / caseSensitive / wholeWord (defaults false)", async () => {
    await ipc.searchDocument("d1", "hello");
    expect(mockInvoke).toHaveBeenCalledWith("search_document", {
      docId: "d1",
      query: "hello",
      caseSensitive: false,
      wholeWord: false,
    });
  });

  it("searchDocument passes caseSensitive=true when set", async () => {
    await ipc.searchDocument("d1", "Hello", true);
    expect(mockInvoke).toHaveBeenCalledWith("search_document", {
      docId: "d1",
      query: "Hello",
      caseSensitive: true,
      wholeWord: false,
    });
  });

  it("searchDocument passes wholeWord=true when set", async () => {
    await ipc.searchDocument("d1", "the", false, true);
    expect(mockInvoke).toHaveBeenCalledWith("search_document", {
      docId: "d1",
      query: "the",
      caseSensitive: false,
      wholeWord: true,
    });
  });

  it("searchDocument returns the hit array from invoke", async () => {
    const hits: ipc.SearchHit[] = [{ page: 2, rect: [1, 2, 3, 4], snippet: "foo" }];
    mockInvoke.mockResolvedValue(hits as never);
    const result = await ipc.searchDocument("d1", "foo");
    expect(result).toEqual(hits);
  });
});
