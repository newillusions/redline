// @vitest-environment jsdom
/**
 * ThumbnailPanel tests (M4 S1).
 *
 * - Mounts the real ThumbnailPanel.svelte with controlled props.
 * - Mocks $lib/ipc so page-op calls are captured, not executed.
 * - Covers: thumbnail render count, delete button triggers IPC,
 *   drag-to-reorder triggers IPC with correct permutation, rotate button triggers IPC.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import ThumbnailPanel from "./ThumbnailPanel.svelte";

// ---------------------------------------------------------------------------
// Mock $lib/ipc
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockReorderPages = vi.fn(async (_args: any) => {});
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockDeletePage = vi.fn(async (_args: any) => {});
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockRotatePage = vi.fn(async (_args: any) => {});
// Thumbnail rendering (PR-B, 2026-09-15) — default resolved values so every existing
// test (which never asserts on thumbnail content) mounts cleanly; thumbnail-specific
// tests below override these per-case.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockGetPageSize = vi.fn(async (docId: string, pageIndex: number) => ({
  doc_id: docId, page_index: pageIndex, width_pts: 200, height_pts: 260,
}));
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockRenderTile = vi.fn(async (req: any) => ({
  doc_id: req.doc_id, page_index: req.page_index, tile_x: req.tile_x, tile_y: req.tile_y,
  width_px: 100, height_px: 130, zoom: req.zoom, dpr: req.dpr,
  png_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  render_ms: 1,
}));

vi.mock("$lib/ipc", () => ({
  reorderPages: (args: unknown) => mockReorderPages(args),
  deletePage: (args: unknown) => mockDeletePage(args),
  rotatePage: (args: unknown) => mockRotatePage(args),
  getPageSize: (docId: string, pageIndex: number) => mockGetPageSize(docId, pageIndex),
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  renderTile: (req: any) => mockRenderTile(req),
  // other ipc functions (required by setup mock)
  processRssMb: vi.fn(),
  getUserIdentity: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
  addMarkup: vi.fn(),
  listMarkups: vi.fn(),
  loadMarkups: vi.fn(),
  saveDocument: vi.fn(),
  saveDocumentAs: vi.fn(),
  updateMarkup: vi.fn(),
  deleteMarkup: vi.fn(),
  insertBlankPage: vi.fn(),
  insertPage: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mountPanel(pageCount: number, onPageOp = vi.fn()) {
  return render(ThumbnailPanel, {
    props: { docId: "doc-1", pageCount, onPageOp },
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("ThumbnailPanel", () => {
  beforeEach(() => {
    mockReorderPages.mockReset();
    mockDeletePage.mockReset();
    mockRotatePage.mockReset();
    mockGetPageSize.mockClear();
    mockRenderTile.mockClear();
    // Stub window.confirm to auto-confirm for delete tests.
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  describe("rendering", () => {
    it("renders N thumbnails for pageCount N", () => {
      mountPanel(3);
      const items = screen.getAllByRole("listitem");
      expect(items).toHaveLength(3);
    });

    it("renders 1 thumbnail for pageCount 1", () => {
      mountPanel(1);
      expect(screen.getAllByRole("listitem")).toHaveLength(1);
    });

    it("numbers thumbnails 1..N", () => {
      mountPanel(4);
      for (let i = 1; i <= 4; i++) {
        expect(screen.getByLabelText(`Page ${i}`)).toBeTruthy();
      }
    });

    it("delete button is disabled when pageCount is 1", () => {
      mountPanel(1);
      const deleteBtn = screen.getByLabelText("Delete page 1");
      expect(deleteBtn).toBeDisabled();
    });

    it("delete button is enabled when pageCount > 1", () => {
      mountPanel(3);
      const deleteBtn = screen.getByLabelText("Delete page 2");
      expect(deleteBtn).not.toBeDisabled();
    });
  });

  describe("delete page", () => {
    it("clicking delete calls deletePage IPC with correct args", async () => {
      mountPanel(3);
      const deleteBtn = screen.getByLabelText("Delete page 2");
      await fireEvent.click(deleteBtn);
      await tick();
      expect(mockDeletePage).toHaveBeenCalledOnce();
      expect(mockDeletePage).toHaveBeenCalledWith({ doc_id: "doc-1", page_idx: 1 });
    });

    it("user cancelling confirm skips deletePage IPC", async () => {
      vi.spyOn(window, "confirm").mockReturnValue(false);
      mountPanel(3);
      const deleteBtn = screen.getByLabelText("Delete page 1");
      await fireEvent.click(deleteBtn);
      await tick();
      expect(mockDeletePage).not.toHaveBeenCalled();
    });

    it("delete calls onPageOp callback after IPC resolves", async () => {
      const onPageOp = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp } });
      const deleteBtn = screen.getByLabelText("Delete page 1");
      await fireEvent.click(deleteBtn);
      await tick();
      expect(onPageOp).toHaveBeenCalledOnce();
    });

    it("delete of page 1 in a 3-page doc passes page_idx 0", async () => {
      mountPanel(3);
      const deleteBtn = screen.getByLabelText("Delete page 1");
      await fireEvent.click(deleteBtn);
      await tick();
      expect(mockDeletePage).toHaveBeenCalledWith({ doc_id: "doc-1", page_idx: 0 });
    });
  });

  describe("rotate page", () => {
    it("clicking rotate calls rotatePage IPC with 90 degrees", async () => {
      mountPanel(2);
      const rotateBtn = screen.getByLabelText("Rotate page 1 90 degrees clockwise");
      await fireEvent.click(rotateBtn);
      await tick();
      expect(mockRotatePage).toHaveBeenCalledOnce();
      expect(mockRotatePage).toHaveBeenCalledWith({
        doc_id: "doc-1",
        page_idx: 0,
        degrees: 90,
      });
    });

    it("rotate on page 3 passes page_idx 2", async () => {
      mountPanel(3);
      const rotateBtn = screen.getByLabelText("Rotate page 3 90 degrees clockwise");
      await fireEvent.click(rotateBtn);
      await tick();
      expect(mockRotatePage).toHaveBeenCalledWith({
        doc_id: "doc-1",
        page_idx: 2,
        degrees: 90,
      });
    });

    it("rotate calls onPageOp callback after IPC resolves", async () => {
      const onPageOp = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 2, onPageOp } });
      const rotateBtn = screen.getByLabelText("Rotate page 1 90 degrees clockwise");
      await fireEvent.click(rotateBtn);
      await tick();
      expect(onPageOp).toHaveBeenCalledOnce();
    });
  });

  describe("drag-to-reorder", () => {
    it("drop from page 0 onto page 2 calls reorderPages with correct permutation", async () => {
      mountPanel(3);
      const thumbnails = screen.getAllByRole("listitem");

      // Drag page 0 (first) to page 2 (third).
      await fireEvent.dragStart(thumbnails[0], {
        dataTransfer: { setData: vi.fn(), effectAllowed: "" },
      });
      await fireEvent.dragOver(thumbnails[2], {
        dataTransfer: { dropEffect: "" },
      });
      await fireEvent.drop(thumbnails[2], {
        dataTransfer: { getData: () => "0" },
      });
      await tick();

      expect(mockReorderPages).toHaveBeenCalledOnce();
      // Moving page 0 to position 2: [1, 2, 0]
      expect(mockReorderPages).toHaveBeenCalledWith({
        doc_id: "doc-1",
        new_order: [1, 2, 0],
      });
    });

    it("drop from page 2 onto page 0 calls reorderPages with correct permutation", async () => {
      mountPanel(3);
      const thumbnails = screen.getAllByRole("listitem");

      await fireEvent.dragStart(thumbnails[2], {
        dataTransfer: { setData: vi.fn(), effectAllowed: "" },
      });
      await fireEvent.dragOver(thumbnails[0], {
        dataTransfer: { dropEffect: "" },
      });
      await fireEvent.drop(thumbnails[0], {
        dataTransfer: { getData: () => "2" },
      });
      await tick();

      expect(mockReorderPages).toHaveBeenCalledOnce();
      // Moving page 2 to position 0: [2, 0, 1]
      expect(mockReorderPages).toHaveBeenCalledWith({
        doc_id: "doc-1",
        new_order: [2, 0, 1],
      });
    });

    it("dropping onto the same page does not call reorderPages", async () => {
      mountPanel(3);
      const thumbnails = screen.getAllByRole("listitem");

      await fireEvent.dragStart(thumbnails[1], {
        dataTransfer: { setData: vi.fn(), effectAllowed: "" },
      });
      await fireEvent.dragOver(thumbnails[1], {
        dataTransfer: { dropEffect: "" },
      });
      await fireEvent.drop(thumbnails[1], {
        dataTransfer: { getData: () => "1" },
      });
      await tick();

      expect(mockReorderPages).not.toHaveBeenCalled();
    });

    it("reorderPages calls onPageOp callback after IPC resolves", async () => {
      const onPageOp = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 2, onPageOp } });
      const thumbnails = screen.getAllByRole("listitem");

      await fireEvent.dragStart(thumbnails[0], {
        dataTransfer: { setData: vi.fn(), effectAllowed: "" },
      });
      await fireEvent.drop(thumbnails[1], {
        dataTransfer: { getData: () => "0" },
      });
      await tick();

      expect(onPageOp).toHaveBeenCalledOnce();
    });
  });

  describe("thumbnail rendering (PR-B, 2026-09-15)", () => {
    it("fetches a page size and renders a tile for each page (jsdom has no IntersectionObserver, so this fails open to eager loading)", async () => {
      mountPanel(3);
      await tick();
      await tick(); // one for getPageSize, one for the follow-up renderTile

      expect(mockGetPageSize).toHaveBeenCalledTimes(3);
      expect(mockRenderTile).toHaveBeenCalledTimes(3);
      expect(mockRenderTile).toHaveBeenCalledWith(
        expect.objectContaining({ doc_id: "doc-1", page_index: 0, tile_x: 0, tile_y: 0 }),
      );
    });

    it("shows the rendered image once the raster resolves, replacing the placeholder", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 1, onPageOp: vi.fn() } });

      const img = await waitFor(() => {
        const found = screen.getByRole("listitem").querySelector("img.thumbnail-image");
        expect(found).toBeTruthy();
        return found;
      });
      expect(img?.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
      expect(screen.queryByText("p1")).toBeNull();
    });

    it("does not re-fetch a page whose thumbnail is already cached", async () => {
      const { rerender } = render(ThumbnailPanel, {
        props: { docId: "doc-1", pageCount: 2, onPageOp: vi.fn() },
      });
      await waitFor(() => expect(mockRenderTile).toHaveBeenCalledTimes(2));

      mockRenderTile.mockClear();
      mockGetPageSize.mockClear();
      await rerender({ docId: "doc-1", pageCount: 2, onPageOp: vi.fn(), currentPage: 1 });
      await tick();

      expect(mockRenderTile).not.toHaveBeenCalled();
    });

    it("invalidates every cached thumbnail after a rotate (aspect ratio may have changed)", async () => {
      mountPanel(2);
      await waitFor(() => expect(mockRenderTile).toHaveBeenCalledTimes(2));

      mockRenderTile.mockClear();
      mockGetPageSize.mockClear();
      const rotateBtn = screen.getByLabelText("Rotate page 1 90 degrees clockwise");
      await fireEvent.click(rotateBtn);

      // Both thumbnails re-fetched, not just the rotated one — cache was cleared wholesale.
      await waitFor(() => expect(mockRenderTile).toHaveBeenCalledTimes(2));
    });

    it("invalidates every cached thumbnail after a delete (indices shift)", async () => {
      mountPanel(3);
      await waitFor(() => expect(mockRenderTile).toHaveBeenCalledTimes(3));
      mockRenderTile.mockClear();

      const deleteBtn = screen.getByLabelText("Delete page 1");
      await fireEvent.click(deleteBtn);

      await waitFor(() => expect(mockRenderTile.mock.calls.length).toBeGreaterThan(0));
    });
  });

  describe("current-page highlight", () => {
    it("marks the current page's thumbnail with aria-current", () => {
      mountPanel(3);
      const page2 = screen.getByLabelText("Page 2");
      expect(page2.getAttribute("aria-current")).toBeNull();

      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, currentPage: 1, onPageOp: vi.fn() } });
      const highlighted = screen.getAllByLabelText("Page 2").find((el) => el.getAttribute("aria-current") === "page");
      expect(highlighted).toBeTruthy();
    });

    it("no page is marked current when currentPage is null", () => {
      mountPanel(3);
      for (const el of screen.getAllByRole("listitem")) {
        expect(el.getAttribute("aria-current")).toBeNull();
      }
    });
  });

  describe("click-to-jump", () => {
    it("clicking a thumbnail calls onjump with its 0-based index", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onjump, onPageOp: vi.fn() } });
      const page3 = screen.getByLabelText("Page 3");
      await fireEvent.click(page3);
      expect(onjump).toHaveBeenCalledWith(2);
    });

    it("clicking the rotate button does not also trigger onjump (stopPropagation)", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 2, onjump, onPageOp: vi.fn() } });
      const rotateBtn = screen.getByLabelText("Rotate page 1 90 degrees clockwise");
      await fireEvent.click(rotateBtn);
      expect(onjump).not.toHaveBeenCalled();
    });

    it("clicking the delete button does not also trigger onjump (stopPropagation)", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 2, onjump, onPageOp: vi.fn() } });
      const deleteBtn = screen.getByLabelText("Delete page 1");
      await fireEvent.click(deleteBtn);
      expect(onjump).not.toHaveBeenCalled();
    });

    it("does not throw when onjump is not provided", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 1, onPageOp: vi.fn() } });
      const page1 = screen.getByLabelText("Page 1");
      await expect(fireEvent.click(page1)).resolves.not.toThrow();
    });
  });

  describe("selection (PR-C, 2026-09-15)", () => {
    it("a plain click selects only that page and still navigates", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onjump, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 2"));

      expect(screen.getByLabelText("Page 2").getAttribute("aria-selected")).toBe("true");
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
      expect(onjump).toHaveBeenCalledWith(1);
    });

    it("ctrl-click toggles a page into the selection without navigating", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onjump, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });

      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("true");
      expect(onjump).not.toHaveBeenCalled();
    });

    it("cmd-click (metaKey) also toggles without navigating", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onjump, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { metaKey: true });
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("true");
      expect(onjump).not.toHaveBeenCalled();
    });

    it("ctrl-clicking an already-selected page deselects it", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("true");
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
    });

    it("ctrl-click accumulates multiple pages into the selection", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 5, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      await fireEvent.click(screen.getByLabelText("Page 3"), { ctrlKey: true });
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("true");
      expect(screen.getByLabelText("Page 3").getAttribute("aria-selected")).toBe("true");
      expect(screen.getByLabelText("Page 2").getAttribute("aria-selected")).toBe("false");
    });

    it("shift-click selects the contiguous range from the last plain click", async () => {
      const onjump = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 6, onjump, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 2")); // anchor = index 1
      onjump.mockClear();
      await fireEvent.click(screen.getByLabelText("Page 5"), { shiftKey: true }); // range 1..4

      for (const label of ["Page 2", "Page 3", "Page 4", "Page 5"]) {
        expect(screen.getByLabelText(label).getAttribute("aria-selected")).toBe("true");
      }
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
      expect(screen.getByLabelText("Page 6").getAttribute("aria-selected")).toBe("false");
      expect(onjump).not.toHaveBeenCalled(); // shift-click does not navigate
    });

    it("shift-click works in reverse (clicking backwards from the anchor)", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 6, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 5")); // anchor = index 4
      await fireEvent.click(screen.getByLabelText("Page 2"), { shiftKey: true }); // range 1..4

      for (const label of ["Page 2", "Page 3", "Page 4", "Page 5"]) {
        expect(screen.getByLabelText(label).getAttribute("aria-selected")).toBe("true");
      }
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
    });

    it("a second shift-click extends/shrinks from the SAME original anchor", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 6, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 2")); // anchor = index 1
      await fireEvent.click(screen.getByLabelText("Page 4"), { shiftKey: true }); // range 1..3
      await fireEvent.click(screen.getByLabelText("Page 3"), { shiftKey: true }); // range 1..2, NOT 2..3

      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
      expect(screen.getByLabelText("Page 2").getAttribute("aria-selected")).toBe("true");
      expect(screen.getByLabelText("Page 3").getAttribute("aria-selected")).toBe("true");
      expect(screen.getByLabelText("Page 4").getAttribute("aria-selected")).toBe("false");
    });

    it("shows the extract action bar with a count once anything is selected", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      expect(screen.queryByText(/selected/)).toBeNull();

      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      await fireEvent.click(screen.getByLabelText("Page 2"), { ctrlKey: true });

      expect(screen.getByText("2 pages selected")).toBeTruthy();
    });

    it("uses singular wording for a one-page selection", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"));
      expect(screen.getByText("1 page selected")).toBeTruthy();
    });

    it("Extract… button calls onextract with the selected indices", async () => {
      const onextract = vi.fn();
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 5, onextract, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      await fireEvent.click(screen.getByLabelText("Page 3"), { ctrlKey: true });
      await fireEvent.click(screen.getByRole("button", { name: /extract…/i }));

      expect(onextract).toHaveBeenCalledOnce();
      const [calledWith] = onextract.mock.calls[0];
      expect(new Set(calledWith)).toEqual(new Set([0, 2]));
    });

    it("the clear-selection button empties the selection and hides the action bar", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      expect(screen.getByText("1 page selected")).toBeTruthy();

      await fireEvent.click(screen.getByTitle("Clear selection"));

      expect(screen.queryByText(/selected/)).toBeNull();
      expect(screen.getByLabelText("Page 1").getAttribute("aria-selected")).toBe("false");
    });

    it("a delete clears the selection (indices have shifted)", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 2"), { ctrlKey: true });
      expect(screen.getByText("1 page selected")).toBeTruthy();

      await fireEvent.click(screen.getByLabelText("Delete page 1"));
      await tick();

      expect(screen.queryByText(/selected/)).toBeNull();
    });

    it("a rotate does NOT clear the selection (no index shift)", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 3, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 2"), { ctrlKey: true });
      expect(screen.getByText("1 page selected")).toBeTruthy();

      await fireEvent.click(screen.getByLabelText("Rotate page 1 90 degrees clockwise"));
      await tick();

      expect(screen.getByText("1 page selected")).toBeTruthy();
    });

    it("does not throw when onextract is not provided", async () => {
      render(ThumbnailPanel, { props: { docId: "doc-1", pageCount: 1, onPageOp: vi.fn() } });
      await fireEvent.click(screen.getByLabelText("Page 1"), { ctrlKey: true });
      await expect(
        fireEvent.click(screen.getByRole("button", { name: /extract…/i })),
      ).resolves.not.toThrow();
    });
  });
});
