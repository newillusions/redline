// @vitest-environment jsdom
/**
 * ComparePanel tests (M6 Phase 1.1, spec §10).
 *
 * - Mounts real ComparePanel.svelte with controlled props.
 * - Mocks $lib/ipc so IPC calls are captured, not executed.
 * - Covers: initial render, compare button calls comparePages, result display,
 *   changed_pct shown, diff image rendered, error on IPC failure.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ComparePanel from "./ComparePanel.svelte";
import type { PageDiffResult } from "$lib/ipc";

// ---------------------------------------------------------------------------
// Mock $lib/ipc
// ---------------------------------------------------------------------------

const SAMPLE_RESULT: PageDiffResult = {
  text_char_match: false,
  text_delta_count: 12,
  text_rms_delta_pts: 0.0,
  pixel_passed: false,
  changed_pct: 8.25,
  max_pixel_delta: 200,
  diff_png_b64: "iVBORw0KGgo=",
  render_dpi: 150.0,
};

const mockComparePages = vi.fn<
  (pathA: string, pathB: string, pageA: number, pageB: number, dpi?: number, pixelTolerance?: number) => Promise<PageDiffResult>
>();

vi.mock("$lib/ipc", () => ({
  comparePages: (
    pathA: string,
    pathB: string,
    pageA: number,
    pageB: number,
    dpi?: number,
    pixelTolerance?: number,
  ) => mockComparePages(pathA, pathB, pageA, pageB, dpi, pixelTolerance),
  // other ipc stubs
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("ComparePanel (M6 compare UI)", () => {
  beforeEach(() => {
    mockComparePages.mockReset();
    mockComparePages.mockResolvedValue(SAMPLE_RESULT);
  });

  it("renders file path inputs and page inputs", () => {
    render(ComparePanel, {});
    // File A and File B path displays (or inputs)
    expect(screen.getByText(/File A/i)).toBeTruthy();
    expect(screen.getByText(/File B/i)).toBeTruthy();
    // Compare button
    expect(screen.getByRole("button", { name: /compare/i })).toBeTruthy();
  });

  it("compare button is disabled when paths are empty", () => {
    render(ComparePanel, {});
    const btn = screen.getByRole("button", { name: /compare/i });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
  });

  it("calls comparePages with correct camelCase args on submit", async () => {
    const { getByTestId } = render(ComparePanel, {
      pathA: "/docs/rev1.pdf",
      pathB: "/docs/rev2.pdf",
    });

    // Find page number inputs by test id
    const pageAInput = getByTestId("page-a-input") as HTMLInputElement;
    const pageBInput = getByTestId("page-b-input") as HTMLInputElement;

    fireEvent.input(pageAInput, { target: { value: "2" } });
    fireEvent.input(pageBInput, { target: { value: "3" } });
    await tick();

    const btn = screen.getByRole("button", { name: /compare/i });
    fireEvent.click(btn);
    await tick();

    expect(mockComparePages).toHaveBeenCalledWith(
      "/docs/rev1.pdf",
      "/docs/rev2.pdf",
      2,
      3,
      undefined,
      undefined,
    );
  });

  it("shows changed_pct and max_pixel_delta after compare", async () => {
    render(ComparePanel, {
      pathA: "/docs/rev1.pdf",
      pathB: "/docs/rev2.pdf",
    });

    const btn = screen.getByRole("button", { name: /compare/i });
    fireEvent.click(btn);
    await tick();
    await tick(); // wait for async mock resolution

    // changed_pct value visible
    expect(screen.getByText(/8\.25/)).toBeTruthy();
  });

  it("renders diff image after compare", async () => {
    render(ComparePanel, {
      pathA: "/docs/rev1.pdf",
      pathB: "/docs/rev2.pdf",
    });

    const btn = screen.getByRole("button", { name: /compare/i });
    fireEvent.click(btn);
    await tick();
    await tick();

    const img = screen.getByRole("img") as HTMLImageElement;
    expect(img.src).toContain("data:image/png;base64,iVBORw0KGgo=");
  });

  it("shows error message when comparePages rejects", async () => {
    mockComparePages.mockRejectedValue(new Error("PDFium open failed"));

    render(ComparePanel, {
      pathA: "/docs/rev1.pdf",
      pathB: "/docs/rev2.pdf",
    });

    const btn = screen.getByRole("button", { name: /compare/i });
    fireEvent.click(btn);
    await tick();
    await tick();

    expect(screen.getByText(/PDFium open failed/i)).toBeTruthy();
  });

  it("disables compare button while comparison is in progress", async () => {
    let resolve!: (v: PageDiffResult) => void;
    mockComparePages.mockReturnValue(new Promise((r) => { resolve = r; }));

    render(ComparePanel, {
      pathA: "/docs/rev1.pdf",
      pathB: "/docs/rev2.pdf",
    });

    const btn = screen.getByRole("button", { name: /compare/i });
    fireEvent.click(btn);
    await tick();

    expect((btn as HTMLButtonElement).disabled).toBe(true);

    resolve(SAMPLE_RESULT);
    await tick();
    await tick();
  });
});
