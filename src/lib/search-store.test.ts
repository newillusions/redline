// @vitest-environment jsdom
/**
 * Unit tests for SearchStore — scope switching, grouping, flat next/prev
 * navigation, and scope persistence. IPC (searchDocument/searchFolder) is
 * injected as a fake, matching MarkupStore's injected-ipc test pattern.
 * jsdom environment is required for the localStorage-backed scope persistence.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  SearchStore,
  computeViewportSearchOverlay,
  type SearchDeps,
  type DocSearchInput,
  type SearchGroup,
} from "./search-store.svelte";
import { buildMarkup } from "./markup-tools";
import type { Markup, SearchHit, FolderSearchHit } from "./ipc";

const BASE_APPEARANCE = {
  color: "#ff0000",
  line_weight: 2,
  opacity: 1,
  fill: null,
  line_style: "Solid" as const,
  font: { family: "Helvetica", size_pt: 12 },
};
const IDENTITY = { user_id: "aaaaaaaa-0000-0000-0000-000000000001", display_name: "Tester" };

function markup(id: string, page: number, overrides: Partial<Markup> = {}): Markup {
  const m = buildMarkup({
    markupType: "Text",
    page,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 100, y: 100 } } },
    appearance: { ...BASE_APPEARANCE },
    identity: IDENTITY,
    now: "2026-01-01T00:00:00.000Z",
    id,
  });
  return { ...m, ...overrides };
}

function fakeDeps(overrides: Partial<SearchDeps> = {}): SearchDeps {
  return {
    searchDocument: vi.fn(async () => []),
    searchFolder: vi.fn(async () => []),
    searchPaths: vi.fn(async () => []),
    ...overrides,
  };
}

function docInput(docId: string, path: string, markups: Markup[] = []): DocSearchInput {
  return { docId, label: path.split("/").pop()!, path, markups };
}

/** A minimal SearchHit fixture — typed explicitly so `rect` widens to the tuple, not number[]. */
function hit(page: number, snippet = "hit"): SearchHit {
  return { page, rect: [0, 0, 1, 1], snippet };
}

beforeEach(() => {
  // Fresh localStorage between tests (persisted-scope tests rely on this).
  try {
    localStorage.clear();
  } catch {
    // node/vitest environment without localStorage — fine, store falls back to "document".
  }
});

describe("SearchStore — scope + persistence", () => {
  it("defaults to document scope when nothing persisted", () => {
    const store = new SearchStore(fakeDeps());
    expect(store.scope).toBe("document");
  });

  it("persists scope changes across store instances", () => {
    const store = new SearchStore(fakeDeps());
    store.setScope("folder");
    const reloaded = new SearchStore(fakeDeps());
    expect(reloaded.scope).toBe("folder");
  });

  it("clears results when scope changes", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => [hit(0, "hit")]),
    });
    const store = new SearchStore(deps);
    store.query = "hit";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    expect(store.totalHitCount).toBe(1);

    store.setScope("open");
    expect(store.totalHitCount).toBe(0);
    expect(store.groups).toEqual([]);
  });
});

describe("SearchStore — document scope", () => {
  it("merges text hits and markup hits, sorted by page", async () => {
    const textHits: SearchHit[] = [hit(3, "text on p4")];
    const markups = [markup("m1", 0, { contents: "note on p1" })];
    const deps = fakeDeps({ searchDocument: vi.fn(async () => textHits) });
    const store = new SearchStore(deps);
    store.query = "text on p4".includes("note") ? "note" : "on"; // matches both fixtures via "on"
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf", markups) });

    expect(store.groups).toHaveLength(1);
    const hits = store.groups[0].hits;
    expect(hits.map((h) => h.kind)).toEqual(["markup", "text"]); // page 0 markup before page 3 text
    expect(hits[0].page).toBe(0);
    expect(hits[1].page).toBe(3);
  });

  it("does not render a group header requirement for a single-doc scope (still returns one group)", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => [hit(0, "x")]),
    });
    const store = new SearchStore(deps);
    store.query = "x";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    expect(store.groups).toHaveLength(1);
    expect(store.groups[0].key).toBe("d1");
  });

  it("no-ops (clears) on a blank query", async () => {
    const deps = fakeDeps();
    const store = new SearchStore(deps);
    store.query = "   ";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    expect(store.groups).toEqual([]);
    expect(deps.searchDocument).not.toHaveBeenCalled();
  });

  it("surfaces a rejected searchDocument as store.error", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => {
        throw new Error("boom");
      }),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    expect(store.error).toBe("boom");
    expect(store.groups).toEqual([]);
  });
});

describe("SearchStore — open-documents scope", () => {
  it("produces one group per open document with hits, skipping docs with zero hits", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async (docId: string) =>
        docId === "d1" ? [hit(0, "found")] : []
      ),
    });
    const store = new SearchStore(deps);
    store.query = "found";
    await store.run({
      scope: "open",
      docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")],
    });

    expect(store.groups).toHaveLength(1);
    expect(store.groups[0].key).toBe("d1");
    expect(store.fileCount).toBe(1);
    expect(store.totalHitCount).toBe(1);
  });

  it("searches all open documents in parallel (does not serialize)", async () => {
    const order: string[] = [];
    const deps = fakeDeps({
      searchDocument: vi.fn(async (docId: string) => {
        order.push(`start:${docId}`);
        await new Promise((r) => setTimeout(r, 0));
        order.push(`end:${docId}`);
        return [];
      }),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });
    // Both starts happen before either end — proves Promise.all, not sequential awaits.
    expect(order.slice(0, 2).sort()).toEqual(["start:d1", "start:d2"]);
  });
});

describe("SearchStore — folder scope", () => {
  it("groups folder hits by file_path and normalizes 1-based page_number to 0-based page", async () => {
    const folderHits: FolderSearchHit[] = [
      { file_path: "/plans/a.pdf", page_number: 1, snippet: "<b>x</b>", source: "lopdf" },
      { file_path: "/plans/a.pdf", page_number: 3, snippet: "<b>x</b>", source: "lopdf" },
      { file_path: "/plans/b.pdf", page_number: 2, snippet: "<b>x</b>", source: "lopdf" },
    ];
    const deps = fakeDeps({ searchFolder: vi.fn(async () => folderHits) });
    const store = new SearchStore(deps);
    store.query = "x";
    await store.run({ scope: "folder" });

    expect(store.fileCount).toBe(2);
    expect(store.totalHitCount).toBe(3);
    const groupA = store.groups.find((g) => g.key === "/plans/a.pdf")!;
    expect(groupA.label).toBe("a.pdf");
    expect(groupA.hits.map((h) => h.page)).toEqual([0, 2]); // 1-based -> 0-based
    expect(groupA.hits[0].snippetHtml).toBe(true);
    // filePath must be set — it's the only navigation target folder-scope hits carry
    // (they have no docId; the file may not even be open yet).
    expect(groupA.hits[0].filePath).toBe("/plans/a.pdf");
  });

  it("folder-scope hits never include markup kind (markup search is doc/open-scope only)", async () => {
    const deps = fakeDeps({
      searchFolder: vi.fn(async () => [
        { file_path: "/a.pdf", page_number: 1, snippet: "x", source: "lopdf" },
      ]),
    });
    const store = new SearchStore(deps);
    store.query = "x";
    await store.run({ scope: "folder" });
    expect(store.groups[0].hits.every((h) => h.kind === "text")).toBe(true);
  });
});

describe("SearchStore — flat navigation (F3/Shift-F3)", () => {
  function twoGroupStore(): SearchStore {
    const deps = fakeDeps({
      searchDocument: vi.fn(async (docId: string) => [
        hit(0, docId),
        hit(1, docId),
      ]),
    });
    return new SearchStore(deps);
  }

  it("focusNext walks every hit across groups in order and wraps", async () => {
    const store = twoGroupStore();
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });
    expect(store.flatHits).toHaveLength(4);

    expect(store.activeFlatIndex).toBe(0);
    store.focusNext();
    expect(store.activeFlatIndex).toBe(1);
    store.focusNext();
    store.focusNext();
    expect(store.activeFlatIndex).toBe(3);
    store.focusNext(); // wraps
    expect(store.activeFlatIndex).toBe(0);
  });

  it("focusPrev wraps backward from the first result to the last", async () => {
    const store = twoGroupStore();
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });
    expect(store.activeFlatIndex).toBe(0);
    store.focusPrev();
    expect(store.activeFlatIndex).toBe(3);
  });

  it("focusNext auto-expands a collapsed target group", async () => {
    const store = twoGroupStore();
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });
    const secondGroupKey = store.groups[1].key;
    store.toggleGroupCollapsed(secondGroupKey);
    expect(store.isGroupCollapsed(secondGroupKey)).toBe(true);

    store.focusAt(2); // first hit of the second (collapsed) group
    expect(store.isGroupCollapsed(secondGroupKey)).toBe(false);
  });

  it("returns null from focusNext/focusPrev when there are no results", () => {
    const store = new SearchStore(fakeDeps());
    expect(store.focusNext()).toBeNull();
    expect(store.focusPrev()).toBeNull();
  });
});

describe("SearchStore — group collapse", () => {
  it("toggleGroupCollapsed flips independently per group key", async () => {
    const store = new SearchStore(fakeDeps());
    expect(store.isGroupCollapsed("k1")).toBe(false);
    store.toggleGroupCollapsed("k1");
    expect(store.isGroupCollapsed("k1")).toBe(true);
    expect(store.isGroupCollapsed("k2")).toBe(false);
    store.toggleGroupCollapsed("k1");
    expect(store.isGroupCollapsed("k1")).toBe(false);
  });

  it("collapseAll collapses every current group; expandAll clears all", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async (docId: string) => [hit(0, docId)]),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });

    store.collapseAll();
    expect(store.groups.every((g) => store.isGroupCollapsed(g.key))).toBe(true);

    store.expandAll();
    expect(store.groups.every((g) => !store.isGroupCollapsed(g.key))).toBe(true);
  });
});

describe("SearchStore — checkbox selection (Bluebeam Check Options parity)", () => {
  function twoHitStore(): SearchStore {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => [hit(0, "a"), hit(1, "b")]),
    });
    return new SearchStore(deps);
  }

  it("every new hit starts unchecked", async () => {
    const store = twoHitStore();
    store.query = "q";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    expect(store.groups[0].hits.every((h) => h.checked === false)).toBe(true);
    expect(store.checkedHits).toEqual([]);
  });

  it("toggleChecked flips one hit's checked state without affecting others", async () => {
    const store = twoHitStore();
    store.query = "q";
    await store.run({ scope: "document", doc: docInput("d1", "/a.pdf") });
    store.toggleChecked(0, 0);
    expect(store.groups[0].hits[0].checked).toBe(true);
    expect(store.groups[0].hits[1].checked).toBe(false);
    expect(store.checkedHits).toHaveLength(1);
  });

  it("checkAll checks every hit across every group; uncheckAll clears them", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async (docId: string) => [hit(0, docId)]),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "open", docs: [docInput("d1", "/a.pdf"), docInput("d2", "/b.pdf")] });

    store.checkAll();
    expect(store.checkedHits).toHaveLength(2);

    store.uncheckAll();
    expect(store.checkedHits).toHaveLength(0);
  });

  it("toggleChecked on an out-of-range index is a no-op, not a throw", () => {
    const store = new SearchStore(fakeDeps());
    expect(() => store.toggleChecked(5, 5)).not.toThrow();
  });
});

describe("SearchStore — page scope", () => {
  it("filters the document's hits down to the current page only", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => [hit(0, "on page 0"), hit(2, "on page 2")]),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "page", doc: docInput("d1", "/a.pdf"), page: 2 });
    expect(store.totalHitCount).toBe(1);
    expect(store.groups[0].hits[0].page).toBe(2);
  });

  it("returns zero hits when nothing on the current page matches", async () => {
    const deps = fakeDeps({
      searchDocument: vi.fn(async () => [hit(0, "on page 0")]),
    });
    const store = new SearchStore(deps);
    store.query = "q";
    await store.run({ scope: "page", doc: docInput("d1", "/a.pdf"), page: 5 });
    expect(store.totalHitCount).toBe(0);
  });
});

describe("SearchStore — recents scope", () => {
  it("calls searchPaths with the given file list and groups results by file", async () => {
    const folderHits: FolderSearchHit[] = [
      { file_path: "/recent/a.pdf", page_number: 1, snippet: "x", source: "lopdf" },
      { file_path: "/recent/b.pdf", page_number: 4, snippet: "x", source: "lopdf" },
    ];
    const deps = fakeDeps({ searchPaths: vi.fn(async () => folderHits) });
    const store = new SearchStore(deps);
    store.query = "x";
    await store.run({ scope: "recents", paths: ["/recent/a.pdf", "/recent/b.pdf"] });

    expect(deps.searchPaths).toHaveBeenCalledWith(
      ["/recent/a.pdf", "/recent/b.pdf"],
      "x",
      false,
      false
    );
    expect(store.fileCount).toBe(2);
    expect(store.groups.find((g) => g.key === "/recent/a.pdf")?.hits[0].page).toBe(0); // 1-based -> 0-based
  });
});

describe("computeViewportSearchOverlay (PR #86 review fix: Viewport highlight wiring)", () => {
  function textHit(page: number, rect: [number, number, number, number] = [0, 0, 1, 1]) {
    return { kind: "text" as const, page, snippet: "x", snippetHtml: false, rect, checked: false };
  }
  function markupHit(page: number, markupId = "m1") {
    return { kind: "markup" as const, page, snippet: "x", snippetHtml: false, markupId, checked: false };
  }
  function group(key: string, hits: (ReturnType<typeof textHit> | ReturnType<typeof markupHit>)[]): SearchGroup {
    return { key, label: key, hits };
  }
  /** Mirrors SearchStore.flatHits' own flattening logic, built directly from fixture groups. */
  function flatten(groups: SearchGroup[]) {
    const out: { groupIndex: number; hitIndex: number; hit: SearchGroup["hits"][number] }[] = [];
    groups.forEach((g, groupIndex) => g.hits.forEach((hit, hitIndex) => out.push({ groupIndex, hitIndex, hit })));
    return out;
  }

  it("returns an empty overlay when neither docId nor path is given", () => {
    const groups = [group("d1", [textHit(0)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), 0, null, null);
    expect(overlay).toEqual({ hits: [], activeIdx: null });
  });

  it("returns an empty overlay when no group matches the tab", () => {
    const groups = [group("d1", [textHit(0)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), 0, "d-other", "/other.pdf");
    expect(overlay.hits).toEqual([]);
  });

  it("matches by docId (document/page/open/recents-once-opened scope)", () => {
    const groups = [group("d1", [textHit(0), textHit(2)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), null, "d1", "/unrelated.pdf");
    expect(overlay.hits).toHaveLength(2);
  });

  it("matches by absolute path when docId doesn't match (folder/recents scope, file not opened yet)", () => {
    const groups = [group("/plans/a.pdf", [textHit(0)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), null, "some-doc-id", "/plans/a.pdf");
    expect(overlay.hits).toHaveLength(1);
  });

  it("excludes markup-kind hits — only text hits with a rect are highlightable", () => {
    const groups = [group("d1", [textHit(0), markupHit(1)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), null, "d1", null);
    expect(overlay.hits).toHaveLength(1);
    expect(overlay.hits[0].kind).toBe("text");
  });

  it("computes activeIdx when the globally-focused flat hit belongs to this tab's group", () => {
    const groups = [group("d1", [textHit(0), textHit(2)])];
    const flat = flatten(groups);
    const overlay = computeViewportSearchOverlay(groups, flat, 1, "d1", null); // flat[1] = groups[0].hits[1]
    expect(overlay.activeIdx).toBe(1);
  });

  it("activeIdx is null when the globally-focused flat hit belongs to a DIFFERENT tab's group", () => {
    const groups = [group("d1", [textHit(0)]), group("d2", [textHit(0)])];
    const flat = flatten(groups);
    // Focus is on groups[1] (d2)'s hit, but we're asking about d1's overlay.
    const overlay = computeViewportSearchOverlay(groups, flat, 1, "d1", null);
    expect(overlay.activeIdx).toBeNull();
    expect(overlay.hits).toHaveLength(1); // d1's own hit is still returned, just not "active"
  });

  it("activeIdx is null when the focused flat hit is a markup hit, even within this tab's group", () => {
    const groups = [group("d1", [textHit(0), markupHit(1)])];
    const flat = flatten(groups);
    const overlay = computeViewportSearchOverlay(groups, flat, 1, "d1", null); // flat[1] = the markup hit
    expect(overlay.activeIdx).toBeNull();
  });

  it("activeIdx is null when activeFlatIndex is null", () => {
    const groups = [group("d1", [textHit(0)])];
    const overlay = computeViewportSearchOverlay(groups, flatten(groups), null, "d1", null);
    expect(overlay.activeIdx).toBeNull();
  });
});
