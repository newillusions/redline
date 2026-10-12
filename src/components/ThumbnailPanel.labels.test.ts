// @vitest-environment jsdom
/** ThumbnailPanel page labels (docs/navigation.md L1, L2, RL3): shown, renamed, Number pages. */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import ThumbnailPanel from "./ThumbnailPanel.svelte";

vi.mock("$lib/ipc", () => ({
  reorderPages: vi.fn(async () => {}),
  deletePage: vi.fn(async () => {}),
  rotatePage: vi.fn(async () => {}),
  getPageSize: vi.fn(async (docId: string, pageIndex: number) => ({
    doc_id: docId, page_index: pageIndex, width_pts: 200, height_pts: 260,
  })),
  renderTile: vi.fn(async (req: { doc_id: string; page_index: number }) => ({
    doc_id: req.doc_id, page_index: req.page_index, tile_x: 0, tile_y: 0, width_px: 1, height_px: 1,
    zoom: 1, dpr: 1, png_base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", render_ms: 1,
  })),
}));

const labels = ["", "E-101", "E-102"];

function setup(extra: Record<string, unknown> = {}) {
  const onrenamelabel = vi.fn();
  const onnumber = vi.fn();
  const r = render(ThumbnailPanel, {
    docId: "d1", pageCount: 3, currentPage: 0, labels, onrenamelabel, onnumber, ...extra,
  });
  return { onrenamelabel, onnumber, ...r };
}

describe("ThumbnailPanel labels", () => {
  beforeEach(() => vi.clearAllMocks());

  it("shows the label under a labelled thumbnail and nothing under an unlabelled one", () => {
    setup();
    expect(screen.getByText("E-101")).toBeTruthy();
    expect(screen.getByText("E-102")).toBeTruthy();
    expect(document.querySelectorAll(".thumbnail-label")).toHaveLength(2);
  });

  it("renders a hostile label as text", () => {
    const evil = '<img src=x onerror="window.__pwned=1">';
    const { container } = setup({ labels: [evil, "", ""] });
    expect(container.querySelector("img[src='x']")).toBeNull();
    expect(screen.getByText(evil)).toBeTruthy();
  });

  it("double-clicking a label renames it; Enter commits", async () => {
    const { onrenamelabel } = setup();
    await fireEvent.dblClick(screen.getByText("E-101"));
    const input = screen.getByLabelText("Label for page 2") as HTMLInputElement;
    expect(input.value).toBe("E-101");
    await fireEvent.input(input, { target: { value: "E-101A" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(onrenamelabel).toHaveBeenCalledWith(1, "E-101A");
  });

  it("Escape and an unchanged label write nothing", async () => {
    const { onrenamelabel } = setup();
    await fireEvent.dblClick(screen.getByText("E-101"));
    await fireEvent.input(screen.getByLabelText("Label for page 2"), { target: { value: "zzz" } });
    await fireEvent.keyDown(screen.getByLabelText("Label for page 2"), { key: "Escape" });
    expect(onrenamelabel).not.toHaveBeenCalled();
    await fireEvent.dblClick(screen.getByText("E-101"));
    await fireEvent.keyDown(screen.getByLabelText("Label for page 2"), { key: "Enter" });
    expect(onrenamelabel).not.toHaveBeenCalled();
  });

  it("F2 renames the selected page's label, even when it has none", async () => {
    const { onrenamelabel } = setup();
    const thumbs = document.querySelectorAll(".thumbnail");
    await fireEvent.click(thumbs[0]); // selects page 1 (unlabelled)
    await fireEvent.keyDown(screen.getByLabelText("Page thumbnails"), { key: "F2" });
    const input = screen.getByLabelText("Label for page 1") as HTMLInputElement;
    expect(input.value).toBe("");
    await fireEvent.input(input, { target: { value: "Cover" } });
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(onrenamelabel).toHaveBeenCalledWith(0, "Cover");
  });

  it("the selection bar offers Number pages... with the selected pages", async () => {
    const { onnumber } = setup();
    const thumbs = document.querySelectorAll(".thumbnail");
    await fireEvent.click(thumbs[1], { ctrlKey: true });
    await fireEvent.click(thumbs[2], { ctrlKey: true });
    await fireEvent.click(screen.getByText("Number pages…"));
    expect(onnumber).toHaveBeenCalledWith([1, 2]);
  });

  it("without the callbacks the panel is unchanged (no rename, no Number pages)", async () => {
    render(ThumbnailPanel, { docId: "d1", pageCount: 2, currentPage: 0 });
    const thumbs = document.querySelectorAll(".thumbnail");
    await fireEvent.click(thumbs[1], { ctrlKey: true });
    expect(screen.queryByText("Number pages…")).toBeNull();
    expect(screen.getByText("Extract…")).toBeTruthy();
  });
});
