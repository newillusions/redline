// @vitest-environment jsdom
/**
 * SearchPanel tests (search-parity dispatch).
 *
 * Mounts the real SearchPanel.svelte with a REAL SearchStore (deps injected,
 * matching MarkupStore's/SearchStore's own test convention — no $lib/ipc
 * mocking needed since SearchStore already owns that boundary).
 *
 * Covers: scope tabs, debounced search-as-you-type, Enter-to-search-immediately,
 * grouped result rendering + collapse, click-to-jump, F3-equivalent next/prev
 * buttons, markup-kind labeling, folder-picker prompt when no folder chosen.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import SearchPanel from "./SearchPanel.svelte";
import { SearchStore, type SearchDeps, type UnifiedSearchHit, type SearchGroup } from "$lib/search-store.svelte";
import type { SearchHit, FolderSearchHit } from "$lib/ipc";

function hit(page: number, snippet = "hit"): SearchHit {
  return { page, rect: [0, 0, 1, 1], snippet };
}

function fakeDeps(overrides: Partial<SearchDeps> = {}): SearchDeps {
  return {
    searchDocument: vi.fn(async () => []),
    searchFolder: vi.fn(async () => []),
    searchPaths: vi.fn(async () => []),
    ...overrides,
  };
}

function mountPanel(store: SearchStore, extra: Partial<Parameters<typeof render>[1]["props"]> = {}) {
  const onSearch = vi.fn(() => {
    // Simulate App.svelte's real wiring: onSearch triggers the actual store.run()
    // against whatever context this test's onSearch override doesn't replace.
  });
  const onPickFolder = vi.fn();
  const onJump = vi.fn();
  const onHighlightChecked = vi.fn();
  const result = render(SearchPanel, {
    props: {
      store,
      folderPath: null,
      folderIndexStatus: null,
      onSearch,
      onPickFolder,
      onJump,
      onHighlightChecked,
      ...extra,
    },
  });
  return { ...result, onSearch, onPickFolder, onJump, onHighlightChecked };
}

beforeEach(() => {
  vi.useFakeTimers();
  try {
    localStorage.clear();
  } catch {
    // ignore
  }
});

afterEach(() => {
  vi.useRealTimers();
});

describe("SearchPanel — scope tabs", () => {
  it("renders all five scope tabs, Document active by default", () => {
    const store = new SearchStore(fakeDeps());
    const { getByTestId } = mountPanel(store);
    expect(getByTestId("scope-tab-document").getAttribute("aria-selected")).toBe("true");
    expect(getByTestId("scope-tab-page").getAttribute("aria-selected")).toBe("false");
    expect(getByTestId("scope-tab-open").getAttribute("aria-selected")).toBe("false");
    expect(getByTestId("scope-tab-recents").getAttribute("aria-selected")).toBe("false");
    expect(getByTestId("scope-tab-folder").getAttribute("aria-selected")).toBe("false");
  });

  it("clicking a scope tab switches store.scope", async () => {
    const store = new SearchStore(fakeDeps());
    const { getByTestId } = mountPanel(store, { folderPath: "/plans" });
    await fireEvent.click(getByTestId("scope-tab-open"));
    expect(store.scope).toBe("open");
  });

  it("clicking Folder with no folder chosen calls onPickFolder", async () => {
    const store = new SearchStore(fakeDeps());
    const { getByTestId, onPickFolder } = mountPanel(store, { folderPath: null });
    await fireEvent.click(getByTestId("scope-tab-folder"));
    expect(onPickFolder).toHaveBeenCalled();
  });

  it("re-runs the search immediately on scope switch when a query is already present", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "concrete";
    const { getByTestId, onSearch } = mountPanel(store, { folderPath: "/plans" });
    await fireEvent.click(getByTestId("scope-tab-open"));
    expect(onSearch).toHaveBeenCalledTimes(1);
  });
});

describe("SearchPanel — debounced search", () => {
  it("debounces search-as-you-type by 300ms", async () => {
    const store = new SearchStore(fakeDeps());
    const { getByTestId, onSearch } = mountPanel(store);
    const input = getByTestId("search-input") as HTMLInputElement;

    await fireEvent.input(input, { target: { value: "concrete" } });
    expect(onSearch).not.toHaveBeenCalled();

    vi.advanceTimersByTime(299);
    expect(onSearch).not.toHaveBeenCalled();
    vi.advanceTimersByTime(2);
    expect(onSearch).toHaveBeenCalledTimes(1);
  });

  it("Enter searches immediately, bypassing the debounce", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "concrete";
    const { getByTestId, onSearch } = mountPanel(store);
    const input = getByTestId("search-input") as HTMLInputElement;
    await fireEvent.keyDown(input, { key: "Enter" });
    expect(onSearch).toHaveBeenCalledTimes(1);
  });

  it("clearing the query to blank cancels the pending debounce and clears results", async () => {
    const store = new SearchStore(fakeDeps());
    const { getByTestId, onSearch } = mountPanel(store);
    const input = getByTestId("search-input") as HTMLInputElement;

    await fireEvent.input(input, { target: { value: "x" } });
    await fireEvent.input(input, { target: { value: "" } });
    vi.advanceTimersByTime(400);
    expect(onSearch).not.toHaveBeenCalled();
  });

  it("Escape clears the query and results", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "concrete";
    store.groups = [{ key: "d1", label: "a.pdf", hits: [] }];
    const { getByTestId } = mountPanel(store);
    const input = getByTestId("search-input") as HTMLInputElement;
    await fireEvent.keyDown(input, { key: "Escape" });
    expect(store.query).toBe("");
    expect(store.groups).toEqual([]);
  });
});

describe("SearchPanel — grouped result rendering", () => {
  function hitOf(kind: "text" | "markup", page: number, snippet: string): UnifiedSearchHit {
    return kind === "text"
      ? { kind, page, snippet, snippetHtml: false, docId: "d1", rect: [0, 0, 1, 1], checked: false }
      : { kind, page, snippet, snippetHtml: false, docId: "d1", markupId: "m1", checked: false };
  }

  it("document scope (single group) renders results WITHOUT a group header", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    store.groups = [{ key: "d1", label: "a.pdf", hits: [hitOf("text", 0, "hello world")] }];
    const { container } = mountPanel(store);
    await tick();
    expect(container.querySelector(".group-header")).toBeNull();
    expect(container.querySelectorAll(".search-result")).toHaveLength(1);
  });

  it("open/folder scope renders a group header per file with a match count", async () => {
    const store = new SearchStore(fakeDeps());
    store.setScope("open");
    store.query = "x";
    store.groups = [
      { key: "d1", label: "a.pdf", hits: [hitOf("text", 0, "hit1"), hitOf("text", 2, "hit2")] },
      { key: "d2", label: "b.pdf", hits: [hitOf("text", 1, "hit3")] },
    ];
    const { container } = mountPanel(store, { folderPath: null });
    await tick();
    // Group headers are now the shared Accordion component's toggle buttons
    // (data-testid="search-group-<key>", see SearchPanel.svelte) - the old
    // ".group-header" class lives only on Accordion's own scoped CSS now
    // (variant="nested"), so it can no longer be relied on as a cross-component
    // selector (Svelte's scoped-CSS hash doesn't reach here - see Accordion.svelte).
    const headers = container.querySelectorAll('[data-testid^="search-group-"]');
    expect(headers).toHaveLength(2);
    expect(headers[0].textContent).toContain("a.pdf");
    expect(headers[0].textContent).toContain("2");
    expect(headers[1].textContent).toContain("b.pdf");
    expect(headers[1].textContent).toContain("1");
  });

  it("clicking a group header collapses it, hiding its results", async () => {
    const store = new SearchStore(fakeDeps());
    store.setScope("open");
    store.query = "x";
    store.groups = [{ key: "d1", label: "a.pdf", hits: [hitOf("text", 0, "hello")] }];
    const { container } = mountPanel(store);
    await tick();
    expect(container.querySelectorAll(".search-result")).toHaveLength(1);

    await fireEvent.click(container.querySelector('[data-testid="search-group-d1"]')!);
    await tick();
    expect(container.querySelectorAll(".search-result")).toHaveLength(0);

    await fireEvent.click(container.querySelector('[data-testid="search-group-d1"]')!);
    await tick();
    expect(container.querySelectorAll(".search-result")).toHaveLength(1);
  });

  it("labels a markup-kind hit distinctly from a text hit", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    store.groups = [
      { key: "d1", label: "a.pdf", hits: [hitOf("text", 0, "doc text"), hitOf("markup", 1, "a comment")] },
    ];
    const { container } = mountPanel(store);
    await tick();
    const kindLabels = container.querySelectorAll(".search-result-kind");
    expect(kindLabels).toHaveLength(1);
    expect(kindLabels[0].textContent).toBe("markup");
  });

  it("clicking a result calls onJump with that hit and its group", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    const g: SearchGroup = { key: "d1", label: "a.pdf", hits: [hitOf("text", 3, "found it")] };
    store.groups = [g];
    const { container, onJump } = mountPanel(store);
    await tick();
    await fireEvent.click(container.querySelector(".search-result")!);
    expect(onJump).toHaveBeenCalledWith(g.hits[0], g);
  });

  it("renders an HTML snippet with {@html} only when snippetHtml is true", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    store.groups = [
      {
        key: "d1",
        label: "a.pdf",
        hits: [{ kind: "text", page: 0, snippet: "<b>bold</b>", snippetHtml: true, filePath: "/a.pdf", checked: false }],
      },
    ];
    const { container } = mountPanel(store, { folderPath: "/plans" });
    await tick();
    const snippetEl = container.querySelector(".search-result-snippet")!;
    expect(snippetEl.querySelector("b")).not.toBeNull();
  });
});

describe("SearchPanel — searching/no-results feedback (owner defect fix, 2026-09-07)", () => {
  /**
   * Owner report (v0.3.17 live use, image-only/scanned PDF): "i did a search, and
   * just got zero response... we need to see when the search is happening, and a
   * no results found response if that's the end result."
   *
   * Root cause (search-store.svelte.ts's old buildGroups): document/page scope
   * always returned one group object regardless of hit count, so
   * `store.groups.length` was never 0 even on a genuine zero-match search — none
   * of SearchPanel's four status branches (error/searching/no-results/summary)
   * ever matched. Fixed by filtering the zero-hit group out, matching how "open"
   * scope already behaved. These tests exercise the REAL SearchStore.run() path
   * (not a hand-set store.groups) so they'd have caught the original bug.
   */
  function doc(overrides: Partial<import("$lib/search-store.svelte").DocSearchInput> = {}) {
    return { docId: "d1", label: "scanned.pdf", path: "/mock/scanned.pdf", markups: [], ...overrides };
  }

  // These call store.run() directly (rather than clicking Find + waiting on the
  // debounce/onSearch wiring) so they aren't entangled with this file's
  // `beforeEach(vi.useFakeTimers())` — @testing-library's `waitFor` polls with
  // real timers and would hang under fake ones. mountPanel/SearchPanel still
  // render the REAL store reactively, so the assertions exercise the actual
  // fixed render path, not a hand-set store.groups shortcut.

  it("a zero-hit document-scope search shows a visible 'No results' message, not silence", async () => {
    // Mirrors the real Rust command's documented behaviour on an image-only PDF
    // (src-tauri/src/commands/text.rs::search_document): Ok(vec![]), not an error.
    const store = new SearchStore(fakeDeps({ searchDocument: vi.fn(async () => []) }));
    store.query = "elevation";
    const { container, getByTestId } = mountPanel(store);

    await store.run({ scope: "document", doc: doc() });
    await tick();

    expect(container.querySelector(".search-status, .search-error, .search-summary")).not.toBeNull();
    const status = getByTestId("search-status-no-results");
    expect(status.textContent).toContain('No results for "elevation"');
    // Document/page scope gets the scanned-PDF hint; folder/open scope (tested
    // below) deliberately does not, since "this document" phrasing wouldn't fit
    // a multi-file result set.
    expect(status.textContent).toContain("searchable text layer");
    expect(store.groups).toHaveLength(0);
  });

  it("a zero-hit page-scope search also shows 'No results' (same phantom-group class)", async () => {
    const store = new SearchStore(fakeDeps({ searchDocument: vi.fn(async () => []) }));
    store.query = "elevation";
    const { container, getByTestId } = mountPanel(store);

    await store.run({ scope: "page", doc: doc(), page: 0 });
    await tick();

    expect(getByTestId("search-status-no-results")).toBeTruthy();
    expect(store.groups).toHaveLength(0);
  });

  it("a zero-hit folder-scope search shows 'No results' without the single-document OCR hint", async () => {
    const store = new SearchStore(fakeDeps({ searchFolder: vi.fn(async () => []) }));
    store.setScope("folder");
    store.query = "elevation";
    const { container, getByTestId } = mountPanel(store);

    await store.run({ scope: "folder" });
    await tick();

    const status = getByTestId("search-status-no-results");
    expect(status.textContent).toContain('No results for "elevation"');
    expect(status.textContent).not.toContain("text layer");
  });

  it("shows an in-progress 'Searching…' status while a search is in flight", async () => {
    let resolveSearch: (v: never[]) => void = () => {};
    const pending = new Promise<never[]>((resolve) => { resolveSearch = resolve; });
    const store = new SearchStore(fakeDeps({ searchDocument: vi.fn(() => pending) }));
    store.query = "elevation";
    const { container, getByTestId } = mountPanel(store);

    const run = store.run({ scope: "document", doc: doc() });
    await tick();
    expect(store.searching).toBe(true);
    const status = getByTestId("search-status-searching");
    expect(status.textContent).toContain('Searching for "elevation"');

    resolveSearch([]);
    await run;
    await tick();
    expect(store.searching).toBe(false);
  });

  it("a search that finds hits shows the result summary, not the no-results message", async () => {
    const store = new SearchStore(
      fakeDeps({ searchDocument: vi.fn(async () => [{ page: 0, rect: [0, 0, 1, 1] as [number, number, number, number], snippet: "hit" }]) })
    );
    store.query = "elevation";
    const { container } = mountPanel(store);

    await store.run({ scope: "document", doc: doc() });
    await tick();

    expect(container.querySelector('[data-testid="search-status-no-results"]')).toBeNull();
    expect(container.querySelector(".search-summary")?.textContent).toContain("1 result");
  });
});

describe("SearchPanel — next/prev navigation controls", () => {
  it("shows a position indicator and Next/Prev buttons only when there are results", async () => {
    const store = new SearchStore(fakeDeps());
    const { container, rerender } = mountPanel(store);
    await tick();
    expect(container.querySelector(".search-nav")).toBeNull();

    store.query = "x";
    store.groups = [{ key: "d1", label: "a.pdf", hits: [{ kind: "text", page: 0, snippet: "a", snippetHtml: false, checked: false }] }];
    store.activeFlatIndex = 0;
    await rerender({});
    await tick();
    expect(container.querySelector(".search-nav")).not.toBeNull();
    expect(container.querySelector(".nav-pos")?.textContent).toBe("1 / 1");
  });

  it("Next button calls onJump for the next flat hit", async () => {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    store.groups = [
      { key: "d1", label: "a.pdf", hits: [
        { kind: "text", page: 0, snippet: "a", snippetHtml: false, checked: false },
        { kind: "text", page: 1, snippet: "b", snippetHtml: false, checked: false },
      ] },
    ];
    store.activeFlatIndex = 0;
    const { container, onJump } = mountPanel(store);
    await tick();
    await fireEvent.click(container.querySelector(".nav-btn:last-child")!);
    expect(onJump).toHaveBeenCalled();
    expect(store.activeFlatIndex).toBe(1);
  });
});

describe("SearchPanel — Check Options (Bluebeam bulk-action parity)", () => {
  function seeded(): SearchStore {
    const store = new SearchStore(fakeDeps());
    store.query = "x";
    store.groups = [
      {
        key: "d1",
        label: "a.pdf",
        hits: [
          { kind: "text", page: 0, snippet: "a", snippetHtml: false, checked: false },
          { kind: "text", page: 1, snippet: "b", snippetHtml: false, checked: false },
        ],
      },
    ];
    return store;
  }

  it("clicking a result checkbox toggles it WITHOUT triggering navigation", async () => {
    const store = seeded();
    const { container, onJump } = mountPanel(store);
    await tick();
    const checkbox = container.querySelector(".search-result-check") as HTMLInputElement;
    await fireEvent.click(checkbox);
    expect(store.groups[0].hits[0].checked).toBe(true);
    expect(onJump).not.toHaveBeenCalled();
  });

  it("Check All / Uncheck All buttons operate on every result", async () => {
    const store = seeded();
    const { container } = mountPanel(store);
    await tick();
    const [checkAllBtn, uncheckAllBtn] = container.querySelectorAll(".check-opt-btn");
    await fireEvent.click(checkAllBtn);
    expect(store.checkedHits).toHaveLength(2);
    await fireEvent.click(uncheckAllBtn);
    expect(store.checkedHits).toHaveLength(0);
  });

  it("Collapse All / Expand All buttons operate on every group", async () => {
    const store = seeded();
    const { container } = mountPanel(store);
    await tick();
    const buttons = container.querySelectorAll(".check-opt-btn");
    const collapseBtn = Array.from(buttons).find((b) => b.textContent?.includes("Collapse"))!;
    const expandBtn = Array.from(buttons).find((b) => b.textContent?.includes("Expand"))!;
    await fireEvent.click(collapseBtn);
    expect(store.isGroupCollapsed("d1")).toBe(true);
    await fireEvent.click(expandBtn);
    expect(store.isGroupCollapsed("d1")).toBe(false);
  });

  it("Highlight Checked is disabled with zero checked results, enabled once one is checked", async () => {
    const store = seeded();
    const { container } = mountPanel(store);
    await tick();
    const highlightBtn = container.querySelector(".check-opt-btn--primary") as HTMLButtonElement;
    expect(highlightBtn.disabled).toBe(true);

    store.toggleChecked(0, 0);
    await tick();
    expect(highlightBtn.disabled).toBe(false);
    expect(highlightBtn.textContent).toContain("(1)");
  });

  it("Highlight Checked button click calls onHighlightChecked", async () => {
    const store = seeded();
    store.toggleChecked(0, 0);
    const { container, onHighlightChecked } = mountPanel(store);
    await tick();
    const highlightBtn = container.querySelector(".check-opt-btn--primary") as HTMLButtonElement;
    await fireEvent.click(highlightBtn);
    expect(onHighlightChecked).toHaveBeenCalled();
  });
});
