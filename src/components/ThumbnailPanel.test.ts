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
import { render, screen, fireEvent } from "@testing-library/svelte";
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

vi.mock("$lib/ipc", () => ({
  reorderPages: (args: unknown) => mockReorderPages(args),
  deletePage: (args: unknown) => mockDeletePage(args),
  rotatePage: (args: unknown) => mockRotatePage(args),
  // other ipc functions (required by setup mock)
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
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
});
