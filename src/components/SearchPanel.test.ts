// @vitest-environment jsdom
/**
 * SearchPanel tests (M4 S3).
 *
 * - Mounts the real SearchPanel.svelte with controlled props.
 * - Mocks $lib/ipc so searchDocument calls are captured, not executed.
 * - Covers: initial render, search submission, result list, clear, options,
 *   result click invokes onJump, Escape clears, no-results state.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import SearchPanel from "./SearchPanel.svelte";
import type { SearchHit } from "$lib/ipc";

// ---------------------------------------------------------------------------
// Mock $lib/ipc
// ---------------------------------------------------------------------------

const FIXTURE_HITS: SearchHit[] = [
  { page: 0, rect: [10, 20, 80, 35], snippet: "hello world" },
  { page: 1, rect: [5, 40, 60, 55], snippet: "hello again" },
];

const mockSearchDocument = vi.fn<
  (docId: string, query: string, caseSensitive: boolean, wholeWord: boolean) => Promise<SearchHit[]>
>();

vi.mock("$lib/ipc", () => ({
  searchDocument: (
    docId: string,
    query: string,
    caseSensitive: boolean,
    wholeWord: boolean
  ) => mockSearchDocument(docId, query, caseSensitive, wholeWord),
  // other ipc stubs required by potential transitive imports
  getPageSize: vi.fn(),
  renderTile: vi.fn(),
  processRssMb: vi.fn(),
  getUserIdentity: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mountPanel({
  docId = "doc-1",
  pageCount = 5,
  onHits = vi.fn(),
  onJump = vi.fn(),
} = {}) {
  return render(SearchPanel, {
    props: { docId, pageCount, onHits, onJump },
  });
}

async function typeQuery(input: HTMLElement, value: string) {
  await fireEvent.input(input, { target: { value } });
  // Svelte bind:value uses the change event in some environments; cover both.
  await fireEvent.change(input, { target: { value } });
  await tick();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("SearchPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSearchDocument.mockResolvedValue(FIXTURE_HITS);
  });

  it("renders the query input and Find button", () => {
    mountPanel();
    expect(screen.getByRole("searchbox")).toBeTruthy();
    expect(screen.getByRole("button", { name: /find/i })).toBeTruthy();
  });

  it("Find button is disabled when query is empty", () => {
    mountPanel();
    const btn = screen.getByRole("button", { name: /find/i }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("calls searchDocument with correct args and shows results", async () => {
    const onHits = vi.fn();
    mountPanel({ onHits });

    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    const btn = screen.getByRole("button", { name: /find/i }) as HTMLButtonElement;
    expect(btn.disabled).toBe(false);

    await fireEvent.click(btn);
    await tick();
    await tick(); // allow promise to resolve

    expect(mockSearchDocument).toHaveBeenCalledWith("doc-1", "hello", false, false);
    expect(onHits).toHaveBeenCalledWith(FIXTURE_HITS);

    // Results should appear
    expect(screen.getByText("hello world")).toBeTruthy();
    expect(screen.getByText("hello again")).toBeTruthy();
  });

  it("shows correct hit count summary", async () => {
    mountPanel();
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // 2 results on 2 pages
    const summary = screen.getByText(/2 result/i);
    expect(summary).toBeTruthy();
  });

  it("shows 'No results' when search returns empty", async () => {
    mockSearchDocument.mockResolvedValue([]);
    mountPanel();
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "xyz_notfound");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    expect(screen.getByText(/no results/i)).toBeTruthy();
  });

  it("Enter key triggers search", async () => {
    const onHits = vi.fn();
    mountPanel({ onHits });
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    await tick();

    expect(mockSearchDocument).toHaveBeenCalledOnce();
    expect(onHits).toHaveBeenCalledWith(FIXTURE_HITS);
  });

  it("Escape key clears query and results", async () => {
    const onHits = vi.fn();
    mountPanel({ onHits });
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // Results are present; now Escape
    await fireEvent.keyDown(input, { key: "Escape" });
    await tick();

    expect(onHits).toHaveBeenLastCalledWith([]);
  });

  it("clicking a result row calls onJump with the correct page and index", async () => {
    const onJump = vi.fn();
    mountPanel({ onJump });
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    const firstResult = screen.getByText("hello world").closest("li")!;
    await fireEvent.click(firstResult);
    await tick();

    // idx=0, page=0 (FIXTURE_HITS[0].page)
    expect(onJump).toHaveBeenCalledWith(0, 0);
  });

  it("second result click calls onJump with correct page", async () => {
    const onJump = vi.fn();
    mountPanel({ onJump });
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    const secondResult = screen.getByText("hello again").closest("li")!;
    await fireEvent.click(secondResult);
    await tick();

    // idx=1, page=1 (FIXTURE_HITS[1].page)
    expect(onJump).toHaveBeenCalledWith(1, 1);
  });

  it("case-sensitive checkbox passes caseSensitive=true", async () => {
    mountPanel();
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "Hello");

    // Toggle case-sensitive option
    const checkbox = screen.getByRole("checkbox", { hidden: true, name: /aa/i });
    if (checkbox) {
      await fireEvent.click(checkbox);
      await tick();
    }

    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // caseSensitive arg should be true
    const call = mockSearchDocument.mock.calls[0];
    expect(call[2]).toBe(true);
  });

  it("shows error message when searchDocument rejects", async () => {
    mockSearchDocument.mockRejectedValue(new Error("PDFium failed"));
    const onHits = vi.fn();
    mountPanel({ onHits });
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "crash");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    expect(screen.getByRole("alert")).toBeTruthy();
    expect(onHits).toHaveBeenCalledWith([]);
  });

  it("page numbers show 1-based in result list", async () => {
    mountPanel();
    const input = screen.getByRole("searchbox");
    await typeQuery(input, "hello");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // Page 0 → "p.1", page 1 → "p.2"
    expect(screen.getByText("p.1")).toBeTruthy();
    expect(screen.getByText("p.2")).toBeTruthy();
  });
});
