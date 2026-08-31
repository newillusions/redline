/**
 * Unified search orchestrator (owner directive: match + improve on Bluebeam's
 * search — current document / all open documents / folder+subfolders scope,
 * results as a sequential list grouped by file with collapsible groups,
 * click-to-navigate, and a markup-text search layer Bluebeam gates behind
 * higher tiers).
 *
 * Deliberately IPC-agnostic: the three search primitives (searchDocument,
 * searchFolder, searchMarkupContents) are injected via SearchDeps so this
 * class is unit-testable without mocking $lib/ipc/Tauri (mirrors the
 * injected-ipc pattern MarkupStore already uses).
 *
 * App.svelte owns navigation (opening a file if needed, switching tabs,
 * driving Viewport's jump/highlight) — this store only produces the grouped
 * hit list + flat next/prev index + scope persistence.
 */
import type { Markup } from "./ipc";
import type { SearchHit, FolderSearchHit } from "./ipc";
import { searchMarkupContents, type MarkupSearchHit } from "./markup-search";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Matches Bluebeam's documented scope set (support.bluebeam.com/se/user-manual/
 * menus/window/search-panel.html) minus two that don't map onto redline's
 * simpler model: "Current Set" (Bluebeam's named/saved multi-file bundle —
 * redline has no such persisted grouping; "open" scope already covers "search
 * everything I have open right now" in spirit) and "Current Studio Project"
 * (cloud collaboration, an explicit v1 non-goal per the spec).
 */
export type SearchScope = "document" | "page" | "open" | "recents" | "folder";

/** One open document, as SearchStore needs to see it. */
export interface DocSearchInput {
  docId: string;
  /** Display label for the group header — usually the filename. */
  label: string;
  /** Absolute path, used to dedupe against folder-scope hits and for navigation. */
  path: string;
  markups: readonly Markup[];
}

export type SearchContext =
  | { scope: "document"; doc: DocSearchInput }
  | { scope: "page"; doc: DocSearchInput; page: number }
  | { scope: "open"; docs: DocSearchInput[] }
  | { scope: "recents"; paths: string[] }
  | { scope: "folder" };

export type UnifiedHitKind = "text" | "markup";

export interface UnifiedSearchHit {
  kind: UnifiedHitKind;
  /** Zero-based page index. */
  page: number;
  snippet: string;
  /** True when `snippet` is Tantivy-generated HTML (only <b> tags — safe to {@html}). */
  snippetHtml: boolean;
  /** Open-document navigation target (document/open scope). */
  docId?: string;
  /** Folder-scope navigation target — may or may not already be open. */
  filePath?: string;
  /** Set for kind==="markup" — the markup to select/highlight on arrival. */
  markupId?: string;
  /** Set for kind==="text" — PDF user-space rect to center + highlight on arrival. */
  rect?: [number, number, number, number];
  /** Bulk-action selection state (Bluebeam's Check All/Uncheck All + "Check
   *  Options" — Highlight/Underline/etc applied to every checked result at
   *  once). Defaults unchecked; the user opts in per-result or via checkAll(). */
  checked: boolean;
}

export interface SearchGroup {
  /** docId (document/open scope) or file_path (folder scope) — stable across re-runs. */
  key: string;
  label: string;
  hits: UnifiedSearchHit[];
}

/** A flattened reference used for F3/Shift-F3 next/prev traversal across all groups. */
export interface FlatHitRef {
  groupIndex: number;
  hitIndex: number;
  hit: UnifiedSearchHit;
}

export interface SearchDeps {
  searchDocument: (
    docId: string,
    query: string,
    caseSensitive: boolean,
    wholeWord: boolean
  ) => Promise<SearchHit[]>;
  searchFolder: (query: string, limit?: number) => Promise<FolderSearchHit[]>;
  searchPaths: (
    paths: string[],
    query: string,
    caseSensitive: boolean,
    wholeWord: boolean
  ) => Promise<FolderSearchHit[]>;
}

// ---------------------------------------------------------------------------
// Scope persistence (localStorage — per-viewer convenience, not critical state)
// ---------------------------------------------------------------------------

const SCOPE_STORAGE_KEY = "redline.search.scope";
const VALID_SCOPES: readonly SearchScope[] = ["document", "page", "open", "recents", "folder"];

function loadPersistedScope(): SearchScope {
  try {
    if (typeof localStorage === "undefined") return "document";
    const raw = localStorage.getItem(SCOPE_STORAGE_KEY);
    return (VALID_SCOPES as readonly string[]).includes(raw ?? "")
      ? (raw as SearchScope)
      : "document";
  } catch {
    return "document";
  }
}

function persistScope(scope: SearchScope): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(SCOPE_STORAGE_KEY, scope);
  } catch {
    // Best-effort only — a blocked/private-mode localStorage must not break search.
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function fromSearchHit(h: SearchHit): UnifiedSearchHit {
  return {
    kind: "text",
    page: h.page,
    snippet: h.snippet,
    snippetHtml: false,
    rect: h.rect,
    checked: false,
  };
}

function fromMarkupHit(h: MarkupSearchHit): UnifiedSearchHit {
  return {
    kind: "markup",
    page: h.page,
    snippet: h.snippet,
    snippetHtml: false,
    markupId: h.markupId,
    checked: false,
  };
}

function fromFolderHit(h: FolderSearchHit): UnifiedSearchHit {
  // Tantivy/ad-hoc-path pages are 1-based; the rest of the app (Markup.page,
  // SearchHit.page, Viewport.pageIndex) is 0-based — normalize at the boundary
  // so callers never have to remember which scope uses which convention.
  return {
    kind: "text",
    page: h.page_number - 1,
    snippet: h.snippet,
    snippetHtml: true,
    filePath: h.file_path,
    checked: false,
  };
}

// ---------------------------------------------------------------------------
// SearchStore
// ---------------------------------------------------------------------------

export class SearchStore {
  scope = $state<SearchScope>(loadPersistedScope());
  query = $state("");
  caseSensitive = $state(false);
  wholeWord = $state(false);
  searching = $state(false);
  error = $state<string | null>(null);
  groups = $state<SearchGroup[]>([]);
  /** Group keys the user has manually collapsed. Absence = expanded (the default). */
  collapsedKeys = $state<Set<string>>(new Set());
  /** Index into flatHits of the currently-focused result, or null. */
  activeFlatIndex = $state<number | null>(null);

  private deps: SearchDeps;
  /** Monotonic token — a stale in-flight run() completing after a newer one must not clobber it. */
  private runToken = 0;

  constructor(deps: SearchDeps) {
    this.deps = deps;
  }

  setScope(next: SearchScope): void {
    if (this.scope === next) return;
    this.scope = next;
    persistScope(next);
    this.clear();
  }

  clear(): void {
    this.groups = [];
    this.error = null;
    this.activeFlatIndex = null;
  }

  toggleGroupCollapsed(key: string): void {
    const next = new Set(this.collapsedKeys);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    this.collapsedKeys = next;
  }

  isGroupCollapsed(key: string): boolean {
    return this.collapsedKeys.has(key);
  }

  /** Collapse every group at once (the video-confirmed "minus sign" affordance). */
  collapseAll(): void {
    this.collapsedKeys = new Set(this.groups.map((g) => g.key));
  }

  /** Expand every group at once (the "plus sign" affordance). */
  expandAll(): void {
    this.collapsedKeys = new Set();
  }

  toggleChecked(groupIndex: number, hitIndex: number): void {
    const group = this.groups[groupIndex];
    const hit = group?.hits[hitIndex];
    if (!hit) return;
    hit.checked = !hit.checked;
  }

  /** Check every result across every group (Bluebeam's "Check All"). */
  checkAll(): void {
    for (const g of this.groups) for (const h of g.hits) h.checked = true;
  }

  /** Uncheck every result across every group (Bluebeam's "Uncheck All"). */
  uncheckAll(): void {
    for (const g of this.groups) for (const h of g.hits) h.checked = false;
  }

  /** All currently-checked hits, flattened — the input to any "Checked" bulk action. */
  get checkedHits(): FlatHitRef[] {
    return this.flatHits.filter((r) => r.hit.checked);
  }

  /** All hits across all groups, in group order — used for F3/Shift-F3 and the result count. */
  get flatHits(): FlatHitRef[] {
    const out: FlatHitRef[] = [];
    this.groups.forEach((g, groupIndex) => {
      g.hits.forEach((hit, hitIndex) => out.push({ groupIndex, hitIndex, hit }));
    });
    return out;
  }

  get totalHitCount(): number {
    return this.groups.reduce((n, g) => n + g.hits.length, 0);
  }

  get fileCount(): number {
    return this.groups.length;
  }

  /** Run a search for the current query against the given context. No-op on a blank query. */
  async run(ctx: SearchContext): Promise<void> {
    const q = this.query.trim();
    if (!q) {
      this.clear();
      return;
    }

    const token = ++this.runToken;
    this.searching = true;
    this.error = null;

    try {
      const groups = await this.buildGroups(ctx, q);
      if (token !== this.runToken) return; // superseded by a newer run
      this.groups = groups;
      this.activeFlatIndex = groups.some((g) => g.hits.length > 0) ? 0 : null;
    } catch (e) {
      if (token !== this.runToken) return;
      this.error = e instanceof Error ? e.message : String(e);
      this.groups = [];
      this.activeFlatIndex = null;
    } finally {
      if (token === this.runToken) this.searching = false;
    }
  }

  private async buildGroups(ctx: SearchContext, q: string): Promise<SearchGroup[]> {
    if (ctx.scope === "document") {
      return [await this.searchOneDoc(ctx.doc, q)];
    }
    if (ctx.scope === "page") {
      const group = await this.searchOneDoc(ctx.doc, q);
      return [{ ...group, hits: group.hits.filter((h) => h.page === ctx.page) }];
    }
    if (ctx.scope === "open") {
      const groups = await Promise.all(ctx.docs.map((d) => this.searchOneDoc(d, q)));
      return groups.filter((g) => g.hits.length > 0);
    }
    if (ctx.scope === "recents") {
      const hits = await this.deps.searchPaths(ctx.paths, q, this.caseSensitive, this.wholeWord);
      return this.groupFolderHits(hits);
    }
    // folder
    const folderHits = await this.deps.searchFolder(q, 200);
    return this.groupFolderHits(folderHits);
  }

  private groupFolderHits(hits: FolderSearchHit[]): SearchGroup[] {
    const byFile = new Map<string, SearchGroup>();
    for (const h of hits) {
      let group = byFile.get(h.file_path);
      if (!group) {
        group = { key: h.file_path, label: basename(h.file_path), hits: [] };
        byFile.set(h.file_path, group);
      }
      group.hits.push(fromFolderHit(h));
    }
    return [...byFile.values()];
  }

  private async searchOneDoc(doc: DocSearchInput, q: string): Promise<SearchGroup> {
    const [textHits, markupHits] = await Promise.all([
      this.deps.searchDocument(doc.docId, q, this.caseSensitive, this.wholeWord),
      Promise.resolve(searchMarkupContents(doc.markups, q, this.caseSensitive, this.wholeWord)),
    ]);
    const hits = [
      ...textHits.map((h) => ({ ...fromSearchHit(h), docId: doc.docId })),
      ...markupHits.map((h) => ({ ...fromMarkupHit(h), docId: doc.docId })),
    ].sort((a, b) => a.page - b.page);
    return { key: doc.docId, label: doc.label, hits };
  }

  /** Move focus to the next result (wraps). Auto-expands the target group if collapsed. */
  focusNext(): FlatHitRef | null {
    return this.stepFocus(1);
  }

  /** Move focus to the previous result (wraps). Auto-expands the target group if collapsed. */
  focusPrev(): FlatHitRef | null {
    return this.stepFocus(-1);
  }

  focusAt(flatIndex: number): FlatHitRef | null {
    const flat = this.flatHits;
    if (flatIndex < 0 || flatIndex >= flat.length) return null;
    this.activeFlatIndex = flatIndex;
    const ref = flat[flatIndex];
    this.ensureGroupExpanded(ref.groupIndex);
    return ref;
  }

  private stepFocus(delta: 1 | -1): FlatHitRef | null {
    const flat = this.flatHits;
    if (flat.length === 0) return null;
    const current = this.activeFlatIndex ?? -1;
    const next = (current + delta + flat.length) % flat.length;
    return this.focusAt(next);
  }

  private ensureGroupExpanded(groupIndex: number): void {
    const key = this.groups[groupIndex]?.key;
    if (key && this.collapsedKeys.has(key)) {
      this.toggleGroupCollapsed(key);
    }
  }
}

// ---------------------------------------------------------------------------
// Viewport highlight overlay (PR #86 review fix, 2026-08-31)
//
// Viewport.svelte already renders on-page highlight rects from a
// `searchHits`/`activeSearchHitIdx` prop pair (spec §4 M4 S3) — but nothing
// was ever computing them for the CURRENT tab from SearchStore's grouped
// results, so text-hit results only centered the viewport with no visible
// highlight box, contradicting the owner's directive ("highlights the
// relevant occurrence"). This is a pure function (not a SearchStore method)
// so App.svelte's `<Viewport>` wiring is unit-testable without mounting the
// whole app, mirroring keyboard-shortcuts.ts's own reason for existing as a
// separate module.
// ---------------------------------------------------------------------------

export interface ViewportSearchOverlay {
  /** Text-kind hits for the given tab, in group order — safe to pass straight
   *  through to Viewport's `searchHits` prop (structurally a superset of
   *  `SearchHit[]`, which only reads `page`/`rect`/`snippet`). */
  hits: (UnifiedSearchHit & { rect: [number, number, number, number] })[];
  /** Index into `hits` of the currently-focused result, or null. */
  activeIdx: number | null;
}

const EMPTY_OVERLAY: ViewportSearchOverlay = { hits: [], activeIdx: null };

/**
 * Compute the search-hit highlight overlay for one open tab.
 *
 * A tab's results may be grouped by `docId` (document/page/open/recents
 * scope, once the file has been opened and searched) OR by absolute file
 * path (folder/recents scope, before the file was ever opened — the group
 * key is the path Tantivy/search_paths returned). Match either so the
 * overlay appears regardless of which scope produced the currently-active
 * tab's results.
 */
export function computeViewportSearchOverlay(
  groups: readonly SearchGroup[],
  flatHits: readonly FlatHitRef[],
  activeFlatIndex: number | null,
  tabDocId: string | null,
  tabPath: string | null
): ViewportSearchOverlay {
  if (!tabDocId && !tabPath) return EMPTY_OVERLAY;

  const group = groups.find((g) => g.key === tabDocId || g.key === tabPath) ?? null;
  if (!group) return EMPTY_OVERLAY;

  const hits = group.hits.filter(
    (h): h is UnifiedSearchHit & { rect: [number, number, number, number] } =>
      h.kind === "text" && !!h.rect
  );

  let activeIdx: number | null = null;
  if (activeFlatIndex != null) {
    const ref = flatHits[activeFlatIndex];
    if (ref && groups[ref.groupIndex] === group && ref.hit.kind === "text") {
      const pos = hits.indexOf(ref.hit as (typeof hits)[number]);
      activeIdx = pos >= 0 ? pos : null;
    }
  }

  return { hits, activeIdx };
}
