// @vitest-environment jsdom
/**
 * ExtractPagesDialog tests — confirms the selection, drives the native Save-As dialog,
 * calls extractPages IPC with the normalized (sorted/deduped) page list.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import ExtractPagesDialog from "./ExtractPagesDialog.svelte";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockExtractPages = vi.fn(async (_args: any) => {});
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockSaveDialog = vi.fn(async (_opts: any) => null as string | null);

vi.mock("$lib/ipc", () => ({
  extractPages: (args: unknown) => mockExtractPages(args),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: (opts: unknown) => mockSaveDialog(opts),
}));

function mountDialog(pageIndices: number[], overrides: Partial<{ onClose: () => void; onExtracted: (dest: string) => void }> = {}) {
  const onClose = overrides.onClose ?? vi.fn();
  const onExtracted = overrides.onExtracted ?? vi.fn();
  const props = { docId: "doc-1", docPath: "/plans/sheet-set.pdf", pageIndices, onClose, onExtracted };
  const result = render(ExtractPagesDialog, { props });
  return { ...result, onClose, onExtracted };
}

describe("ExtractPagesDialog", () => {
  beforeEach(() => {
    mockExtractPages.mockReset().mockResolvedValue(undefined);
    mockSaveDialog.mockReset().mockResolvedValue(null);
  });

  it("shows the page count and formatted range", () => {
    mountDialog([0, 1, 2, 4]);
    expect(screen.getByText("Extract 4 pages into a new PDF?")).toBeTruthy();
    expect(screen.getByText("Pages: 1-3, 5")).toBeTruthy();
  });

  it("uses singular wording for one page", () => {
    mountDialog([0]);
    expect(screen.getByText("Extract 1 page into a new PDF?")).toBeTruthy();
  });

  it("opens the native save dialog with a suggested filename derived from the source path", async () => {
    mountDialog([0]);
    await fireEvent.click(screen.getByRole("button", { name: /extract to new pdf/i }));

    expect(mockSaveDialog).toHaveBeenCalledWith(
      expect.objectContaining({ defaultPath: "/plans/sheet-set-extracted.pdf" }),
    );
  });

  it("does nothing further if the user cancels the native save dialog", async () => {
    mockSaveDialog.mockResolvedValue(null);
    mountDialog([0]);
    await fireEvent.click(screen.getByRole("button", { name: /extract to new pdf/i }));

    expect(mockExtractPages).not.toHaveBeenCalled();
  });

  it("calls extractPages with the sorted/deduped page list and chosen destination", async () => {
    mockSaveDialog.mockResolvedValue("/plans/out.pdf");
    const { onClose } = mountDialog([4, 0, 2, 2]); // out-of-order + duplicate, as a ctrl-click selection could produce
    await fireEvent.click(screen.getByRole("button", { name: /extract to new pdf/i }));

    await waitFor(() => {
      expect(mockExtractPages).toHaveBeenCalledWith({
        doc_id: "doc-1",
        page_indices: [0, 2, 4],
        dest_path: "/plans/out.pdf",
      });
    });
    expect(onClose).toHaveBeenCalled();
  });

  it("calls onExtracted with the destination path on success", async () => {
    mockSaveDialog.mockResolvedValue("/plans/out.pdf");
    const { onExtracted } = mountDialog([0]);
    await fireEvent.click(screen.getByRole("button", { name: /extract to new pdf/i }));

    await waitFor(() => expect(onExtracted).toHaveBeenCalledWith("/plans/out.pdf"));
  });

  it("shows an error and does not close on IPC failure", async () => {
    mockSaveDialog.mockResolvedValue("/plans/out.pdf");
    mockExtractPages.mockRejectedValue(new Error("disk full"));
    const { onClose } = mountDialog([0]);
    await fireEvent.click(screen.getByRole("button", { name: /extract to new pdf/i }));

    await waitFor(() => expect(screen.getByText(/extraction failed/i)).toBeTruthy());
    expect(onClose).not.toHaveBeenCalled();
  });

  it("Cancel button calls onClose without touching IPC", async () => {
    const { onClose } = mountDialog([0]);
    await fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    expect(onClose).toHaveBeenCalled();
    expect(mockExtractPages).not.toHaveBeenCalled();
  });

  it("Escape key calls onClose", async () => {
    const { onClose, container } = mountDialog([0]);
    await fireEvent.keyDown(container.querySelector("dialog")!, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });
});
