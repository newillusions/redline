import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import * as ipc from "./ipc";
import type { Markup, TileRequest } from "./ipc";
import type { RotatePageArgs, DeletePageArgs, ReorderPagesArgs, InsertBlankPageArgs } from "./ipc";
import type { FolderSearchHit, IndexStatus } from "./ipc";

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

  it("open_document → path, password defaults to null when omitted", async () => {
    await ipc.openDocument("/tmp/a.pdf");
    expect(mockInvoke).toHaveBeenCalledWith("open_document", {
      path: "/tmp/a.pdf",
      password: null,
    });
  });

  it("open_document → password passed through when given", async () => {
    await ipc.openDocument("/tmp/a.pdf", "secret");
    expect(mockInvoke).toHaveBeenCalledWith("open_document", {
      path: "/tmp/a.pdf",
      password: "secret",
    });
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

// ---------------------------------------------------------------------------
// Versioning IPC wrappers (M4 S2)
// ---------------------------------------------------------------------------

describe("versioning ipc wrappers (Tauri v2 camelCase keys)", () => {
  const mockInvokeVer = vi.mocked(invoke);

  beforeEach(() => {
    mockInvokeVer.mockReset();
    mockInvokeVer.mockResolvedValue(undefined as never);
  });

  it("snapshotVersion → docId / label (with label)", async () => {
    mockInvokeVer.mockResolvedValue({
      id: "s1",
      created_at: "2026-06-25T12:00:00Z",
      label: "pre-issue",
      filename: "0000001__2026-06-25T12-00-00Z__s1.pdf",
    } as never);
    await ipc.snapshotVersion("d1", "pre-issue");
    expect(mockInvokeVer).toHaveBeenCalledWith("snapshot_version", {
      docId: "d1",
      label: "pre-issue",
    });
  });

  it("snapshotVersion → docId / label (null label)", async () => {
    mockInvokeVer.mockResolvedValue({
      id: "s2",
      created_at: "2026-06-25T12:00:00Z",
      label: null,
      filename: "0000002__2026-06-25T12-00-00Z__s2.pdf",
    } as never);
    await ipc.snapshotVersion("d1", null);
    expect(mockInvokeVer).toHaveBeenCalledWith("snapshot_version", {
      docId: "d1",
      label: null,
    });
  });

  it("listDocumentVersions → docId", async () => {
    mockInvokeVer.mockResolvedValue([] as never);
    await ipc.listDocumentVersions("d1");
    expect(mockInvokeVer).toHaveBeenCalledWith("list_document_versions", { docId: "d1" });
  });

  it("restoreDocumentVersion → docId / versionId", async () => {
    await ipc.restoreDocumentVersion("d1", "ver1");
    expect(mockInvokeVer).toHaveBeenCalledWith("restore_document_version", {
      docId: "d1",
      versionId: "ver1",
    });
  });

  it("listDocumentVersions returns array of VersionRecord", async () => {
    const records: ipc.VersionRecord[] = [
      {
        id: "v1",
        created_at: "2026-06-25T12:00:00Z",
        label: "snapshot A",
        filename: "0000001__2026-06-25T12-00-00Z__v1.pdf",
      },
    ];
    mockInvokeVer.mockResolvedValue(records as never);
    const result = await ipc.listDocumentVersions("d1");
    expect(result).toEqual(records);
  });
});

// ---------------------------------------------------------------------------
// Folder full-text search IPC wrappers (M4 S4)
// ---------------------------------------------------------------------------

describe("folder search ipc wrappers (Tauri v2 camelCase keys)", () => {
  const mockInvokeFolderSearch = vi.mocked(invoke);

  const IDLE_STATUS: IndexStatus = {
    folder_path: "/docs",
    indexed_files: 0,
    indexed_pages: 0,
    state: { kind: "Idle" },
  };

  beforeEach(() => {
    mockInvokeFolderSearch.mockReset();
    mockInvokeFolderSearch.mockResolvedValue(undefined as never);
  });

  it("openFolderIndex → folderPath (camelCase)", async () => {
    mockInvokeFolderSearch.mockResolvedValue(IDLE_STATUS as never);
    await ipc.openFolderIndex("/docs");
    expect(mockInvokeFolderSearch).toHaveBeenCalledWith("open_folder_index", {
      folderPath: "/docs",
    });
  });

  it("openFolderIndex returns IndexStatus", async () => {
    mockInvokeFolderSearch.mockResolvedValue(IDLE_STATUS as never);
    const result = await ipc.openFolderIndex("/docs");
    expect(result).toEqual(IDLE_STATUS);
  });

  it("searchFolder → query / limit (explicit)", async () => {
    mockInvokeFolderSearch.mockResolvedValue([] as never);
    await ipc.searchFolder("concrete", 10);
    expect(mockInvokeFolderSearch).toHaveBeenCalledWith("search_folder", {
      query: "concrete",
      limit: 10,
    });
  });

  it("searchFolder uses default limit 50 when omitted", async () => {
    mockInvokeFolderSearch.mockResolvedValue([] as never);
    await ipc.searchFolder("concrete");
    expect(mockInvokeFolderSearch).toHaveBeenCalledWith("search_folder", {
      query: "concrete",
      limit: 50,
    });
  });

  it("searchFolder returns FolderSearchHit array", async () => {
    const hits: FolderSearchHit[] = [
      {
        file_path: "a.pdf",
        page_number: 1,
        snippet: "foo <b>bar</b> baz",
        source: "lopdf",
      },
    ];
    mockInvokeFolderSearch.mockResolvedValue(hits as never);
    const result = await ipc.searchFolder("bar");
    expect(result).toEqual(hits);
  });

  it("getFolderIndexStatus → no args", async () => {
    mockInvokeFolderSearch.mockResolvedValue(IDLE_STATUS as never);
    const result = await ipc.getFolderIndexStatus();
    expect(mockInvokeFolderSearch).toHaveBeenCalledWith("folder_index_status");
    expect(result).toEqual(IDLE_STATUS);
  });
});

// ---------------------------------------------------------------------------
// Compare IPC wrappers (M6 Phase 1.1)
// ---------------------------------------------------------------------------

describe("compare ipc wrappers (Tauri v2 camelCase keys)", () => {
  const mockInvokeCompare = vi.mocked(invoke);

  const SAMPLE_RESULT: import("./ipc").PageDiffResult = {
    text_char_match: true,
    text_delta_count: 0,
    text_rms_delta_pts: 0.0,
    pixel_passed: false,
    changed_pct: 3.14,
    max_pixel_delta: 42,
    diff_png_b64: "iVBORw0KGgo=",
    render_dpi: 150.0,
  };

  beforeEach(() => {
    mockInvokeCompare.mockReset();
    mockInvokeCompare.mockResolvedValue(SAMPLE_RESULT as never);
  });

  it("comparePages → pathA / pathB / pageA / pageB (camelCase, required args)", async () => {
    await ipc.comparePages("/a.pdf", "/b.pdf", 0, 1);
    expect(mockInvokeCompare).toHaveBeenCalledWith("compare_pages", {
      pathA: "/a.pdf",
      pathB: "/b.pdf",
      pageA: 0,
      pageB: 1,
      dpi: undefined,
      pixelTolerance: undefined,
    });
  });

  it("comparePages → optional dpi and pixelTolerance forwarded as-is", async () => {
    await ipc.comparePages("/a.pdf", "/b.pdf", 2, 3, 300.0, 10);
    expect(mockInvokeCompare).toHaveBeenCalledWith("compare_pages", {
      pathA: "/a.pdf",
      pathB: "/b.pdf",
      pageA: 2,
      pageB: 3,
      dpi: 300.0,
      pixelTolerance: 10,
    });
  });

  it("comparePages returns PageDiffResult", async () => {
    const result = await ipc.comparePages("/a.pdf", "/b.pdf", 0, 0);
    expect(result).toEqual(SAMPLE_RESULT);
  });
});
