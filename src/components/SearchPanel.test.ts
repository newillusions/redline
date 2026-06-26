// @vitest-environment jsdom
/**
 * SearchPanel tests (M4 S3 + M4 S4).
 *
 * - Mounts the real SearchPanel.svelte with controlled props.
 * - Mocks $lib/ipc so search calls are captured, not executed.
 * - S3 covers: initial render, search submission, result list, clear, options,
 *   result click invokes onJump, Escape clears, no-results state.
 * - S4 covers: folder mode tab, folder search call, cross-file results,
 *   folder result click invokes onFolderJump.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import SearchPanel from "./SearchPanel.svelte";
import type { SearchHit, FolderSearchHit } from "$lib/ipc";

// ---------------------------------------------------------------------------
// Mock $lib/ipc
// ---------------------------------------------------------------------------

const FIXTURE_HITS: SearchHit[] = [
  { page: 0, rect: [10, 20, 80, 35], snippet: "hello world" },
  { page: 1, rect: [5, 40, 60, 55], snippet: "hello again" },
];

const FIXTURE_FOLDER_HITS: FolderSearchHit[] = [
  { file_path: "/docs/plan.pdf", page_number: 3, snippet: "concrete <b>foundation</b>", source: "lopdf" },
  { file_path: "/docs/spec.pdf", page_number: 7, snippet: "steel <b>foundation</b> detail", source: "lopdf" },
];

const mockSearchDocument = vi.fn<
  (docId: string, query: string, caseSensitive: boolean, wholeWord: boolean) => Promise<SearchHit[]>
>();

const mockSearchFolder = vi.fn<
  (query: string, limit?: number) => Promise<FolderSearchHit[]>
>();

vi.mock("$lib/ipc", () => ({
  searchDocument: (
    docId: string,
    query: string,
    caseSensitive: boolean,
    wholeWord: boolean
  ) => mockSearchDocument(docId, query, caseSensitive, wholeWord),
  searchFolder: (query: string, limit?: number) =>
    mockSearchFolder(query, limit),
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

function mountPanelWithFolder({
  docId = "doc-1",
  pageCount = 5,
  folderPath = "/docs",
  onHits = vi.fn(),
  onJump = vi.fn(),
  onFolderHits = vi.fn(),
  onFolderJump = vi.fn(),
} = {}) {
  return render(SearchPanel, {
    props: { docId, pageCount, folderPath, onHits, onJump, onFolderHits, onFolderJump },
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
    mockSearchFolder.mockResolvedValue(FIXTURE_FOLDER_HITS);
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

// ---------------------------------------------------------------------------
// Folder search mode (M4 S4)
// ---------------------------------------------------------------------------

describe("SearchPanel folder search mode", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSearchDocument.mockResolvedValue(FIXTURE_HITS);
    mockSearchFolder.mockResolvedValue(FIXTURE_FOLDER_HITS);
  });

  it("does not show mode tabs when folderPath is not provided", () => {
    mountPanel();
    expect(screen.queryByRole("tab")).toBeNull();
  });

  it("shows Doc and Folder tabs when folderPath is provided", () => {
    mountPanelWithFolder();
    expect(screen.getByRole("tab", { name: /doc/i })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /folder/i })).toBeTruthy();
  });

  it("clicking Folder tab switches to folder mode (placeholder shows 'folder')", async () => {
    mountPanelWithFolder();
    await fireEvent.click(screen.getByRole("tab", { name: /folder/i }));
    await tick();
    const input = screen.getByRole("searchbox") as HTMLInputElement;
    expect(input.placeholder.toLowerCase()).toContain("folder");
  });

  it("folder mode calls searchFolder (not searchDocument)", async () => {
    const onFolderHits = vi.fn();
    mountPanelWithFolder({ onFolderHits });

    await fireEvent.click(screen.getByRole("tab", { name: /folder/i }));
    await tick();

    const input = screen.getByRole("searchbox");
    await typeQuery(input, "foundation");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    expect(mockSearchFolder).toHaveBeenCalledOnce();
    expect(mockSearchDocument).not.toHaveBeenCalled();
    expect(onFolderHits).toHaveBeenCalledWith(FIXTURE_FOLDER_HITS);
  });

  it("folder results show file name and page number", async () => {
    mountPanelWithFolder();

    await fireEvent.click(screen.getByRole("tab", { name: /folder/i }));
    await tick();

    const input = screen.getByRole("searchbox");
    await typeQuery(input, "foundation");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // File names (basename of path)
    expect(screen.getByText("plan.pdf")).toBeTruthy();
    expect(screen.getByText("spec.pdf")).toBeTruthy();
    // 1-based page numbers from FIXTURE_FOLDER_HITS
    expect(screen.getByText("p.3")).toBeTruthy();
    expect(screen.getByText("p.7")).toBeTruthy();
  });

  it("clicking folder result calls onFolderJump with filePath and pageNumber", async () => {
    const onFolderJump = vi.fn();
    mountPanelWithFolder({ onFolderJump });

    await fireEvent.click(screen.getByRole("tab", { name: /folder/i }));
    await tick();

    const input = screen.getByRole("searchbox");
    await typeQuery(input, "foundation");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // Click the first result row (plan.pdf, page 3)
    const firstResult = screen.getByText("plan.pdf").closest("li")!;
    await fireEvent.click(firstResult);
    await tick();

    expect(onFolderJump).toHaveBeenCalledWith("/docs/plan.pdf", 3);
  });

  it("folder mode shows cross-file summary", async () => {
    mountPanelWithFolder();

    await fireEvent.click(screen.getByRole("tab", { name: /folder/i }));
    await tick();

    const input = screen.getByRole("searchbox");
    await typeQuery(input, "foundation");
    await fireEvent.click(screen.getByRole("button", { name: /find/i }));
    await tick();
    await tick();

    // FIXTURE_FOLDER_HITS has 2 results across 2 files
    expect(screen.getByText(/2 result/i)).toBeTruthy();
    expect(screen.getByText(/2 file/i)).toBeTruthy();
  });
});
