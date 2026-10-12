<script lang="ts">
  /**
   * App root — 3-column dockable layout (spec §17).
   *
   * Layout:
   *   ┌──────────────────────────────────────────────┐
   *   │  Toolbar (top, full width)                   │
   *   ├──────────────────────────────────────────────┤
   *   │  Tab bar (multi-doc tabs, feat/tabbed-multi-file) │
   *   ├─────────────┬──────────────────┬─────────────┤
   *   │  Left panel │  Viewport (PDF)  │ Right panel │
   *   │  (collapsible)│               │ (collapsible)│
   *   ├─────────────┴──────────────────┴─────────────┤
   *   │  Bottom panel (Markups/Comments list)         │
   *   │  (collapsible)                                │
   *   └──────────────────────────────────────────────┘
   *
   * Multi-doc: each open PDF lives in a DocTab (MarkupStore + TakeoffStore +
   * ViewportSnapshot). Only one Viewport is mounted at a time — switching tabs
   * saves the current zoom/page/scroll into the tab's snapshot and restores it
   * via the new initialState prop when the Viewport remounts for the new tab.
   *
   * Svelte 5 runes: $state / $derived / $effect throughout.
   */
  import "$lib/styles.css";
  import { onMount, onDestroy, tick } from "svelte";
  import Viewport from "./components/Viewport.svelte";
  import ToolPalette from "./components/ToolPalette.svelte";
  import PropertiesPanel from "./components/PropertiesPanel.svelte";
  import MeasurementPanel from "./components/MeasurementPanel.svelte";
  import MarkupsListPanel from "./components/MarkupsListPanel.svelte";
  import ComparePanel from "./components/ComparePanel.svelte";
  import TabBar from "./components/TabBar.svelte";
  import SavePromptDialog from "./components/SavePromptDialog.svelte";
  import StaleFileDialog from "./components/StaleFileDialog.svelte";
  import PasswordPromptDialog from "./components/PasswordPromptDialog.svelte";
  import DocOpDialog from "./components/DocOpDialog.svelte";
  import VersionPanel from "./components/VersionPanel.svelte";
  import LayersPanel from "./components/LayersPanel.svelte";
  import { dropLayerStore, layerStoreFor } from "$lib/layers-store.svelte";
  import ToolbarMenu from "./components/ToolbarMenu.svelte";
  import type { ToolbarMenuItem } from "./components/ToolbarMenu.svelte";
  import { openDocument, closeDocument, setActiveDocument, loadMarkups, listMarkups, listScales, saveDocument, saveDocumentAs, saveDocumentAsRebuild, canRebuildFromOriginal, reloadKeepingMarkups, isDiskChangedError, saveUnprotectedCopy, rememberPassword, addMarkup, updateMarkup, deleteMarkup, flattenDocument, optimizeDocument, redactDocument, snapshotVersion, exportMarkupList, documentNeedsOcr, runOcrDocument, ERR_PASSWORD_REQUIRED, ERR_WRONG_PASSWORD, searchDocument, searchFolder, searchPaths, openFolderIndex, getFolderIndexStatus, getUserIdentity, getPageCount, type ScaleRecord } from "$lib/ipc";
  import { normaliseMeasurements } from "$lib/polylength";
  import type { IndexStatus, OcrProgressEvent } from "$lib/ipc";
  import { listen } from "@tauri-apps/api/event";
  import { loadSettings, saveSettings } from "$lib/settings";
  import type { ViewerMode } from "$lib/settings";
  import SearchPanel from "./components/SearchPanel.svelte";
  import { SearchStore, computeViewportSearchOverlay, type UnifiedSearchHit, type SearchGroup, type DocSearchInput } from "$lib/search-store.svelte";
  import { createPasswordCache, getCachedPassword, setCachedPassword } from "$lib/password-cache";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { DocumentInfo, ImageQualityPreset } from "$lib/ipc";
  import { MarkupStore } from "$lib/markup-store.svelte";
  import { buildMarkup } from "$lib/markup-tools";
  import { existingSearchHighlightIds } from "$lib/markup-select";
  import { TakeoffStore } from "$lib/takeoff-store.svelte";
  import { DocTabStore } from "$lib/doc-tabs.svelte";
  import type { ViewportSnapshot } from "$lib/viewport";
  import DocumentHistoryPanel from "./components/DocumentHistoryPanel.svelte";
  import { loadRecentDocs, saveRecentDocs, upsertMru } from "$lib/recent-docs";
  import type { RecentDoc } from "$lib/recent-docs";
  import SettingsDialog from "./components/SettingsDialog.svelte";
  import AboutDialog from "./components/AboutDialog.svelte";
  import UpdateNotification from "./components/UpdateNotification.svelte";
  import ErrorBanner from "./components/ErrorBanner.svelte";
  import ToolChestPanel from "./components/ToolChestPanel.svelte";
  import Accordion from "./components/Accordion.svelte";
  import { ToolChestStore } from "$lib/toolchest-store.svelte";
  import ActivationGate from "./components/ActivationGate.svelte";
  import LicenseGraceWarning from "./components/LicenseGraceWarning.svelte";
  import { getLicenseStatus, checkInIfActivated, isUsable } from "$lib/license";
  import type { LicenseState } from "$lib/license";
  import UndoRedoControls from "./components/UndoRedoControls.svelte";
  import {
    resolveUndoRedoShortcut,
    resolveSearchShortcut,
    resolveViewerModeToggleShortcut,
  } from "$lib/keyboard-shortcuts";
  import { runDocOpAndReseed, formatBytes } from "$lib/docops-handlers";
  import {
    baseName,
    samePath,
    prepareDocOp,
    exportFormatFromPath,
    exportDoneMessage,
  } from "$lib/doc-op-flow";
  import type { DocOpKind, DocOpMode } from "$lib/doc-op-flow";
  import { dirtyDocIds, advanceQuit } from "$lib/close-guard";
  import type { QuitDecision } from "$lib/close-guard";
  import ReadingView from "./components/ReadingView.svelte";
  import PageIndicator from "./components/PageIndicator.svelte";
  import ThumbnailPanel from "./components/ThumbnailPanel.svelte";
  import ExtractPagesDialog from "./components/ExtractPagesDialog.svelte";
  import BookmarksPanel from "./components/BookmarksPanel.svelte";
  import LinksPanel from "./components/LinksPanel.svelte";
  import NumberPagesDialog from "./components/NumberPagesDialog.svelte";
  import NavActionDialog from "./components/NavActionDialog.svelte";
  import ConfirmDialog from "./components/ConfirmDialog.svelte";
  import { getPageSize } from "$lib/ipc";
  import { confirmExternalOpen, prepareExternalOpen, type LabelStyle, type LinkView, type NavTarget, type TargetInput } from "$lib/navigation-ipc";
  import { bookmarkForMarkup, currentViewDest, jumpFor, type JumpTarget } from "$lib/navigation";

  // ---------------------------------------------------------------------------
  // S2b client entitlement gate - null while the initial (offline, fast) check
  // is in flight, then either usable ("valid" or "grace" - app content
  // renders) or a locked-out reason (ActivationGate renders instead). See
  // handleActivated / maybeInitializeAppContent / checkInIfActivated.
  // ---------------------------------------------------------------------------
  let licenseState = $state<LicenseState | null>(null);
  /** Guards against double-initializing app content when the background
   * online check-in resolves after the fast local read already started it
   * (or self-heals an unusable local read into a usable one). */
  let appContentStarted = false;
  /** Grace warning is shown once per launch; dismissing it (or opening
   * Settings from it) hides it for the rest of this process's lifetime. */
  let graceWarningDismissed = $state(false);

  // ---------------------------------------------------------------------------
  // Multi-doc state
  // ---------------------------------------------------------------------------
  const tabStore = new DocTabStore();

  /** Convenience alias for the currently active tab (null when no docs open). */
  const activeTab = $derived(tabStore.activeTab);

  /**
   * Mirror the active tab into the Rust backend for the MCP `get_active_document`/
   * `list_open_documents` tools (MCP server design, Phase 2a) - there is otherwise no
   * backend concept of "the active document" at all, since tab state is Svelte-only.
   * A single effect watching `tabStore.activeDocId` covers every path that changes it
   * (open, switch tab, close tab, keyboard next/prev-tab) without a call at each of
   * those individual call sites - see `set_active_document`'s Rust doc comment.
   */
  $effect(() => {
    setActiveDocument(tabStore.activeDocId).catch((e) => {
      console.error("setActiveDocument failed (non-fatal - MCP active-doc tracking only):", e);
    });
  });

  // Tool Chest (spec "Tools & Tool Sets") - workspace-level, not per-document; one
  // instance lives for the app's lifetime and is shared across every open tab.
  const toolChestStore = new ToolChestStore();

  // ---------------------------------------------------------------------------
  // Search (search-parity dispatch: current doc / open docs / folder+subfolders,
  // grouped-by-file results, click-to-navigate, markup-text search layer).
  // ---------------------------------------------------------------------------
  const searchStore = new SearchStore({ searchDocument, searchFolder, searchPaths });
  let searchPanelVisible = $state(false);
  let searchFolderPath = $state<string | null>(null);
  let searchFolderIndexStatus = $state<IndexStatus | null>(null);
  let searchJumpNonce = 0;
  let viewportJumpRequest = $state<
    {
      page: number;
      rect?: [number, number, number, number];
      markupId?: string;
      view?: { left: number; top: number; zoom: number | null };
      nonce: number;
    } | null
  >(null);

  // ---------------------------------------------------------------------------
  // Navigator: bookmarks, page labels, links (docs/navigation.md "Document structure").
  // The data lives in each tab's NavigationStore; this is the glue that needs App-level
  // pieces: the viewport jump request, the confirm-before-opening dialog for web
  // addresses, and the dialogs that several panels share.
  // ---------------------------------------------------------------------------
  /** The web address (as normalised by the backend) the user is being asked about, with the
   *  token that answers the question. */
  let externalOpen = $state<{ token: string; uri: string } | null>(null);
  /** Number Pages dialog; `selected` pre-selects the thumbnails' selection. */
  let numberPages = $state<{ selected: number[] } | null>(null);
  /** The link whose action is being changed (Links panel > Edit action). */
  let editingLink = $state<LinkView | null>(null);

  // Read the structure once per tab, in the background.
  $effect(() => {
    const t = activeTab;
    if (t && !t.navStore.loaded) void t.navStore.refresh();
  });

  // A navigation edit refused because the PDF changed on disk gets the same recovery dialog
  // as a refused save (reload and keep my mark-ups / Save As).
  $effect(() => {
    const t = activeTab;
    const err = t?.navStore.error;
    if (t && err && isDiskChangedError(err)) {
      t.navStore.clearError();
      void showStaleDialog(t.docId);
    }
  });

  function jumpTo(j: JumpTarget) {
    searchJumpNonce += 1;
    viewportJumpRequest = { page: j.page, rect: j.rect, view: j.view, nonce: searchJumpNonce };
  }

  /** Run a bookmark's or link's target. A web address is never opened without asking. */
  function followTarget(t: NavTarget) {
    if (t.kind === "page") {
      const j = jumpFor(t);
      if (j) jumpTo(j);
    } else if (t.kind === "uri") {
      void askToOpen(t.uri);
    } else if (t.kind === "unsupported") {
      docOpsStatus = `This action (${t.action}) is not supported, so it was not run.`;
    } else if (t.kind === "blocked") {
      docOpsStatus = `This address was blocked (${t.reason}).`;
    } else if (t.kind === "broken") {
      docOpsStatus = "The page this points to no longer exists in the document.";
    }
  }

  /** Validate the address in the backend and show the user what will actually be opened. */
  async function askToOpen(uri: string) {
    try {
      externalOpen = await prepareExternalOpen(uri);
    } catch (e) {
      docOpsStatus = `This address cannot be opened: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  async function confirmExternalUri() {
    const q = externalOpen;
    externalOpen = null;
    if (!q) return;
    try {
      await confirmExternalOpen(q.token);
    } catch (e) {
      docOpsStatus = `Could not open the address: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  /** The page and view on screen now, as a destination. */
  async function getCurrentView(): Promise<TargetInput | null> {
    const t = activeTab;
    if (!t) return null;
    const snap = t.viewportSnapshot;
    const inherit: TargetInput = {
      kind: "page",
      page: snap.pageIndex,
      view: { kind: "xyz", left: null, top: null, zoom: null },
    };
    // In reading mode the scroll offsets belong to the whole column, not one page.
    if (viewerMode === "reading") return inherit;
    try {
      const size = await getPageSize(t.docId, snap.pageIndex);
      return currentViewDest(snap.pageIndex, snap.zoom, snap.scrollX, snap.scrollY, size.height_pts);
    } catch {
      return inherit;
    }
  }

  /** A bookmark for the selected mark-up, if exactly one is selected. */
  function selectionBookmark() {
    const sel = activeTab?.store.selectedMarkups ?? [];
    return sel.length === 1 ? bookmarkForMarkup(sel[0]) : null;
  }

  async function applyNumberPages(r: {
    pages: number[];
    style: LabelStyle | null;
    prefix: string;
    start: number;
  }) {
    const t = activeTab;
    numberPages = null;
    if (!t) return;
    await t.navStore.edit({ op: "labels_number", ...r });
    if (t.navStore.error) openError = t.navStore.error;
  }

  async function renamePageLabel(page: number, text: string) {
    const t = activeTab;
    if (!t) return;
    await t.navStore.edit({ op: "label_rename", page, text });
    if (t.navStore.error) openError = t.navStore.error;
  }

  async function createLink(target: TargetInput) {
    const t = activeTab;
    const pending = t?.navStore.pendingLink;
    if (!t || !pending) return;
    t.navStore.pendingLink = null;
    await t.navStore.edit({ op: "link_create", page: pending.page, rect: pending.rect, target });
    if (t.navStore.error) openError = t.navStore.error;
  }

  async function changeLinkTarget(target: TargetInput) {
    const t = activeTab;
    const link = editingLink;
    editingLink = null;
    if (!t || !link || link.id === null) return;
    await t.navStore.edit({ op: "link_set_target", page: link.page, id: link.id, target });
    if (t.navStore.error) openError = t.navStore.error;
  }
  let folderIndexPollTimer: ReturnType<typeof setInterval> | null = null;

  function docSearchInput(tab: { docId: string; doc: DocumentInfo; store: MarkupStore }): DocSearchInput {
    return {
      docId: tab.docId,
      label: tab.doc.path.split(/[\\/]/).pop() ?? tab.doc.path,
      path: tab.doc.path,
      markups: tab.store.markups,
    };
  }

  // Search-result highlight overlay for the active tab (PR #86 review fix,
  // 2026-08-31: Viewport already renders searchHits/activeSearchHitIdx as
  // on-page highlight rects — jumpRequest alone only centers the viewport,
  // it never draws the box the owner's directive explicitly asks for).
  // Computation lives in computeViewportSearchOverlay (search-store.svelte.ts)
  // so it's unit-testable without mounting the whole app.
  const searchOverlay = $derived(
    computeViewportSearchOverlay(
      searchStore.groups,
      searchStore.flatHits,
      searchStore.activeFlatIndex,
      activeTab?.docId ?? null,
      activeTab?.doc.path ?? null
    )
  );

  /** True when every currently-checked search hit already has its own Highlight markup
   *  (see `existingSearchHighlightIds` / the Highlight-Checked toggle below) - drives
   *  the "Highlight Checked" button's label so the results panel visibly reads as a
   *  toggle ("Clear Highlights" once everything checked is already highlighted) rather
   *  than an always-add action with no visible way back (owner defect 2026-09-07).
   *  Computed here, not in SearchPanel, because it needs MarkupStore/tab access
   *  SearchPanel deliberately doesn't have (see its own header comment). */
  const checkedAlreadyHighlighted = $derived.by(() => {
    const checked = searchStore.checkedHits;
    if (checked.length === 0) return false;
    return checked.every((ref) => {
      const hit = ref.hit;
      if (hit.kind !== "text" || !hit.rect || !hit.docId) return false;
      const tab = tabStore.tabs.find((t) => t.docId === hit.docId);
      if (!tab) return false;
      return existingSearchHighlightIds(tab.store.markups, hit.page, hit.rect).length > 0;
    });
  });

  /** Run a search for searchStore.query against searchStore's current scope. */
  async function runSearch() {
    if (searchStore.scope === "document") {
      if (!activeTab) return;
      await searchStore.run({ scope: "document", doc: docSearchInput(activeTab) });
    } else if (searchStore.scope === "page") {
      if (!activeTab) return;
      await searchStore.run({
        scope: "page",
        doc: docSearchInput(activeTab),
        page: activeTab.viewportSnapshot.pageIndex,
      });
    } else if (searchStore.scope === "open") {
      await searchStore.run({ scope: "open", docs: tabStore.tabs.map(docSearchInput) });
    } else if (searchStore.scope === "recents") {
      await searchStore.run({ scope: "recents", paths: recentDocs.map((d) => d.path) });
    } else {
      await searchStore.run({ scope: "folder" });
    }
  }

  /** Open the search panel (toolbar button / Cmd/Ctrl+F), auto-expanding the left panel. */
  async function openSearchPanel() {
    searchPanelVisible = true;
    if (leftCollapsed) leftCollapsed = false;
    await tick();
    (document.querySelector('[data-testid="search-input"]') as HTMLInputElement | null)?.focus();
  }

  /**
   * The text-selection (I-beam) tool's action bar "Search" button - runs a
   * document-scope search for the exact selected text. Viewport hands up the
   * text (via the onsearchtext prop) rather than searching itself: App.svelte
   * already owns SearchStore/openSearchPanel/runSearch, and duplicating that
   * here would be a second search pipeline for the same feature.
   */
  async function handleSearchTextSelection(text: string) {
    const trimmed = text.trim();
    if (!trimmed) return;
    searchStore.query = trimmed;
    await openSearchPanel();
    await runSearch();
  }

  function pollFolderIndexStatus() {
    if (folderIndexPollTimer) clearInterval(folderIndexPollTimer);
    folderIndexPollTimer = setInterval(async () => {
      const status = await getFolderIndexStatus();
      searchFolderIndexStatus = status;
      if (status.state.kind !== "Indexing" && folderIndexPollTimer) {
        clearInterval(folderIndexPollTimer);
        folderIndexPollTimer = null;
      }
    }, 1000);
  }

  /** Folder-scope search: pick a folder, open its Tantivy index, poll indexing progress. */
  async function pickSearchFolder() {
    const selected = await open({ directory: true, multiple: false, title: "Choose a folder to search" });
    if (!selected || Array.isArray(selected)) return;
    searchFolderPath = selected as string;
    searchFolderIndexStatus = await openFolderIndex(searchFolderPath);
    pollFolderIndexStatus();
    if (searchStore.query.trim()) await runSearch();
  }

  /**
   * A search result was clicked/keyboard-activated: resolve its navigation
   * target (already-open tab by docId, or a folder-scope file path that may
   * need opening), switch to it, then hand Viewport a fresh jump request.
   */
  async function handleSearchJump(hit: UnifiedSearchHit, _group: SearchGroup) {
    if (hit.docId) {
      tabStore.switchTab(hit.docId);
    } else if (hit.filePath) {
      await openFilePath(hit.filePath);
      if (!tabStore.findByPath(hit.filePath)) return; // open failed — openError already shown
    } else {
      return;
    }
    searchJumpNonce += 1;
    viewportJumpRequest = { page: hit.page, rect: hit.rect, markupId: hit.markupId, nonce: searchJumpNonce };
  }

  /** F3 / Shift+F3 — jump to the next/prev result without touching the panel. */
  function stepSearchResult(dir: "next" | "prev") {
    const ref = dir === "next" ? searchStore.focusNext() : searchStore.focusPrev();
    if (ref) void handleSearchJump(ref.hit, searchStore.groups[ref.groupIndex]);
  }


  /**
   * "Check Options" -> Highlight Checked (Bluebeam parity, confirmed via the
   * official "How to Search PDFs" / "How To Use Visual Search" tutorials:
   * check results, apply Highlight/Underline/Hyperlink/etc to all of them at
   * once — this build ships Highlight, the rest are named follow-ups).
   *
   * TOGGLE, not always-add (owner defect 2026-09-07: searching "proposed" in a title
   * block and clicking Highlight repeatedly "keeps adding a new highlight over the
   * existing ones... and there's no way to turn it off" - each click created ANOTHER
   * Highlight markup stacked on the same spot, and stacked translucent fills compound
   * toward opaque, eventually hiding the text underneath). For each checked hit: if a
   * Highlight markup already exists at that EXACT rect (this action's own prior output -
   * `existingSearchHighlightIds`), DELETE it instead of creating another; only create
   * when none exists. This also self-heals any stack already created by the pre-fix
   * behavior - the first click on an N-times-stacked result clears every one of them
   * (`store.delete` per id, not just one), the next click re-adds a single highlight.
   * Each created markup uses `tab.store.draftAppearance` unchanged - the G9 Bluebeam
   * interop fix already established Highlight's default appearance as a translucent
   * (CA 0.35, multiply-blend) fill, not opaque; nothing here overrides that, so a SINGLE
   * highlight was never the problem - only the uncapped stacking was.
   *
   * Scoped to kind==="text" hits belonging to an ALREADY-OPEN tab (`docId`
   * set) — a folder/recents-scope hit for a file that isn't open has no
   * MarkupStore to write into. Auto-opening every referenced file for a bulk
   * action was judged too surprising for a first pass; skipped hits are
   * counted and reported rather than silently dropped.
   */
  async function applyHighlightToChecked() {
    const checked = searchStore.checkedHits;
    if (checked.length === 0) return;

    let identity;
    try {
      identity = await getUserIdentity();
    } catch (e) {
      docOpsStatus = `Highlight Checked failed: could not load user identity (${e})`;
      return;
    }

    let applied = 0;
    let removed = 0;
    let skipped = 0;
    const now = new Date().toISOString();

    for (const ref of checked) {
      const hit = ref.hit;
      if (hit.kind !== "text" || !hit.rect || !hit.docId) {
        skipped += 1;
        continue;
      }
      const tab = tabStore.tabs.find((t) => t.docId === hit.docId);
      if (!tab) {
        skipped += 1;
        continue;
      }

      const existingIds = existingSearchHighlightIds(tab.store.markups, hit.page, hit.rect);
      if (existingIds.length > 0) {
        for (const id of existingIds) tab.store.delete(id);
        removed += 1;
        continue;
      }

      const [left, bottom, right, top] = hit.rect;
      const m = buildMarkup({
        markupType: "Highlight",
        page: hit.page,
        geometry: {
          Quads: [
            [
              { x: left, y: top },
              { x: right, y: top },
              { x: left, y: bottom },
              { x: right, y: bottom },
            ],
          ],
        },
        appearance: tab.store.draftAppearance,
        identity,
        now,
        id: crypto.randomUUID(),
      });
      tab.store.create(m);
      applied += 1;
    }

    searchStore.uncheckAll();
    const parts: string[] = [];
    if (applied > 0) parts.push(`highlighted ${applied} result${applied !== 1 ? "s" : ""}`);
    if (removed > 0) parts.push(`cleared ${removed} existing highlight${removed !== 1 ? "s" : ""}`);
    if (skipped > 0) parts.push(`skipped ${skipped} not currently open`);
    docOpsStatus = parts.length > 0 ? `${parts.join("; ")}.` : "Nothing to do.";
  }

  // Per-operation busy flags (apply to the active tab's document).
  let openError = $state<string | null>(null);
  let isOpening = $state(false);
  let isSaving = $state(false);
  /** Doc whose save was refused because the PDF changed on disk (recovery dialog open). */
  let staleDocId = $state<string | null>(null);
  /** Whether redline holds the loaded bytes, so "Save As (rebuild)" is available. */
  let staleCanRebuild = $state(true);

  async function showStaleDialog(docId: string) {
    staleCanRebuild = await canRebuildFromOriginal(docId).catch(() => false);
    staleDocId = docId;
  }
  let isFlattening = $state(false);
  let isOptimizing = $state(false);
  let isRedacting = $state(false);
  /** OCR (Phase 2c-ii) - single global run gate, matching isFlattening/isOptimizing/
   *  isRedacting's own convention (one DocOps action at a time, app-wide). Tracks WHICH
   *  doc is running so a background auto-OCR run on an inactive tab doesn't overwrite
   *  the status banner the user is actually looking at. */
  let isOcrRunning = $state(false);
  let ocrRunningDocId = $state<string | null>(null);
  /** Compression/quality control for the Optimize action (spec §8 image downsampling) -
   *  Bluebeam-style preset: High (minimal loss) / Balanced (default) / Small (aggressive). */
  let imageQualityPreset = $state<ImageQualityPreset>("balanced");
  /** Transient success feedback for the DocOps actions (Flatten/Optimize/Redact) - these
   *  have no other visible confirmation (Optimize changes nothing on screen at all;
   *  Flatten/Redact bake content that was already visible). Without this, a successful
   *  no-op action and a real one look identical - see docops-handlers.ts's doc comment. */
  let docOpsStatus = $state<string | null>(null);

  /** Document-rewriting action awaiting the user's confirm (copy vs replace); null when closed. */
  let docOpPrompt = $state<DocOpKind | null>(null);
  /** Bumped after an automatic snapshot so the History panel re-lists versions. */
  let historyNonce = $state(0);
  /** Bumped after a version restore so the Viewport remounts and re-renders tiles. */
  let reloadNonce = $state(0);
  /** Apply Redactions needs a Redact authoring tool to be useful; none exists yet
   *  (UX audit F5, owner decision D3: hide it until one ships). Flip when it lands. */
  const REDACT_AUTHORING_AVAILABLE = false;
  /** Window width in CSS px - drives the toolbar's overflow menu (UX audit F4). */
  let viewportWidth = $state(1440);
  const COMPACT_TOOLBAR_BELOW = 1280;
  const compactToolbar = $derived(viewportWidth < COMPACT_TOOLBAR_BELOW);
  /** Doc ids still to be asked about (save / discard) while the window is closing;
   *  empty when no close is in progress. */
  let quitQueue = $state<string[]>([]);

  // --- Save-prompt dialog state ---
  /** docId of the document awaiting save/discard/cancel decision; null when dialog is hidden. */
  let savePromptDocId = $state<string | null>(null);
  let savePromptFilename = $state("");

  // --- Password prompt dialog state ---
  /** Path awaiting a password to open; null when the dialog is hidden. */
  let passwordPromptPath = $state<string | null>(null);
  /** Set on a retry after a wrong password; null on the first prompt for a file. */
  let passwordPromptError = $state<string | null>(null);
  /** Session-only password cache (never persisted) - avoids re-prompting for a
      file already unlocked earlier in this session. */
  const passwordCache = createPasswordCache();

  // --- Save-unprotected-copy prompt state (offered whenever an encrypted PDF
  //     finishes opening - manual entry, cache reuse, or known-list auto-try). ---
  let unprotectedCopyPromptDocId = $state<string | null>(null);
  let isSavingUnprotectedCopy = $state(false);

  // --- Compare panel state (M6 Phase 1.1) ---
  let compareVisible = $state(false);
  let comparePathA = $state("");
  let comparePathB = $state("");

  // Cleanup handle for the Tauri drag-drop listener.
  let _dropUnlisten: (() => void) | undefined;
  // Cleanup handle for the OCR per-page progress event listener (Phase 2c-ii).
  let _ocrProgressUnlisten: (() => void) | undefined;
  // Cleanup handle for the window close-requested guard.
  let _closeUnlisten: (() => void) | undefined;

  // ---------------------------------------------------------------------------
  // Recent-docs MRU list (Document History panel)
  // ---------------------------------------------------------------------------
  let recentDocs = $state<RecentDoc[]>([]);

  /** Record a successful open in the MRU list and persist it. */
  async function recordRecentDoc(doc: DocumentInfo) {
    const entry: RecentDoc = {
      path: doc.path,
      file_name: doc.path.split(/[\\/]/).at(-1) ?? doc.path,
      last_opened: new Date().toISOString(),
      page_count: doc.page_count,
    };
    recentDocs = upsertMru(recentDocs, entry);
    // Persist asynchronously — failure is non-fatal.
    saveRecentDocs(recentDocs).catch(() => {});
  }

  // ---------------------------------------------------------------------------
  // Auto-open (§20 GUI smoke / floor-machine runbook)
  // ---------------------------------------------------------------------------
  async function autoOpenIfRequested() {
    try {
      const path = await invoke<string | null>("auto_open_path");
      if (path) await openFilePath(path);
    } catch (e) {
      openError = `auto-open failed: ${String(e)}`;
    }
  }

  /**
   * Everything that used to run unconditionally in onMount now runs only
   * once the license gate has passed - either on first mount (already
   * licensed from a prior activation) or right after ActivationGate reports
   * a successful activation (handleActivated below).
   */
  async function initializeAppContent() {
    // Load the MRU list from the backend (non-blocking; failure is non-fatal).
    loadRecentDocs().then((docs) => { recentDocs = docs; }).catch(() => {});

    // OCR per-page progress (Phase 2c-ii) - only updates the visible banner when the
    // event's doc_id matches the run this session is currently tracking (see
    // ocrRunningDocId's doc comment).
    _ocrProgressUnlisten = await listen<OcrProgressEvent>("ocr-progress", (event) => {
      const p = event.payload;
      if (p.doc_id !== ocrRunningDocId) return;
      docOpsStatus = `OCR running - page ${p.page_index + 1}: ${p.pages_done} of ${p.pages_total} page${p.pages_total === 1 ? "" : "s"} scanned (${p.lines_found} line${p.lines_found === 1 ? "" : "s"} found on this page)...`;
    });

    await autoOpenIfRequested();
    // File drop: open each dropped PDF into a new tab (same dedup logic as File>Open).
    _dropUnlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
      if (event.payload.type !== "drop") return;
      const pdfs = (event.payload.paths as string[]).filter((p) =>
        p.toLowerCase().endsWith(".pdf"),
      );
      if (pdfs.length === 0) return;
      if (isOpening) return;
      // Open each dropped PDF (first one focused, others added as background tabs).
      for (const pdf of pdfs) {
        await openFilePath(pdf);
      }
    });
  }

  /** Runs initializeAppContent exactly once, whenever `state` first becomes
   * usable (valid or grace) - whether that's from the initial fast local
   * read, ActivationGate's onActivated callback, or a background check-in
   * that self-heals a previously unusable read. */
  async function maybeInitializeAppContent(state: LicenseState) {
    if (appContentStarted || !isUsable(state)) return;
    appContentStarted = true;
    await initializeAppContent();
  }

  /** ActivationGate calls this after a successful activate_license. */
  async function handleActivated(state: LicenseState) {
    licenseState = state;
    await maybeInitializeAppContent(state);
  }

  onMount(async () => {
    // Window close guard: intercept close-requested while any tab has unsaved mark-ups.
    // preventDefault must run synchronously inside the handler (Tauri destroys the window
    // right after an awaited handler returns unless it was prevented). Not available in a
    // plain browser (vite dev without Tauri), so a failure here is non-fatal.
    getCurrentWindow()
      .onCloseRequested((event) => {
        const dirty = dirtyDocIds(tabStore.tabs);
        if (dirty.length === 0) return;
        event.preventDefault();
        quitQueue = dirty;
      })
      .then((unlisten) => {
        _closeUnlisten = unlisten;
      })
      .catch((e) => console.error("close guard unavailable:", e));

    // Reading mode is a persisted viewer preference (owner request 2026-09-15) - load it
    // up front, independent of the license flow below. A load failure just keeps the
    // single-page default rather than blocking startup.
    loadSettings()
      .then((settings) => { viewerMode = settings.viewer_mode; })
      .catch(() => {});

    licenseState = await getLicenseStatus().catch(
      (e): LicenseState => ({ state: "invalid", reason: e instanceof Error ? e.message : String(e) }),
    );

    // Online is authoritative (2026-08-05 launch model): whenever a stored
    // activation exists at all - regardless of whether the fast offline
    // read says valid/grace/expired/invalid - always attempt the online
    // check-in on launch. A reachable server can revoke instantly (the
    // check-in resolves to "revoked", which flips the gate closed even if
    // the local read was "valid") or heal a stale local read via a fresh
    // renew (e.g. "expired" -> "valid"). Fire-and-forget: this never blocks
    // startup, which renders from the fast local read immediately below.
    checkInIfActivated(licenseState).then((updated) => {
      if (!updated) return;
      licenseState = updated;
      void maybeInitializeAppContent(updated);
    });

    await maybeInitializeAppContent(licenseState);
  });

  onDestroy(() => {
    _dropUnlisten?.();
    _ocrProgressUnlisten?.();
    _closeUnlisten?.();
    if (folderIndexPollTimer) clearInterval(folderIndexPollTimer);
  });

  // Viewer mode: "single" (original tiled zoom/pan viewer) or "reading" (continuous
  // scrolling column). Persisted setting (see settings.ts/storage/settings.rs), loaded in
  // the onMount above; defaults to "single" until that load resolves.
  let viewerMode = $state<ViewerMode>("single");

  /** Toggle button + Ctrl/Cmd+Shift+R - persists the choice for next launch. A save
   *  failure is non-fatal (logged, not surfaced) - the toggle itself already applied. */
  async function toggleViewerMode() {
    viewerMode = viewerMode === "single" ? "reading" : "single";
    try {
      const current = await loadSettings();
      await saveSettings({ ...current, viewer_mode: viewerMode });
    } catch (e) {
      console.error("Failed to persist viewer mode:", e);
    }
  }

  // Panel collapse state
  let leftCollapsed  = $state(false);
  let rightCollapsed = $state(false);
  let bottomCollapsed = $state(true);
  /** Which view the bottom panel shows: the Markups List or the takeoff Quantities. */
  let bottomTab = $state<"markups" | "quantities">("markups");

  // Settings dialog visibility
  let settingsOpen = $state(false);
  let aboutOpen = $state(false);
  /** Pages selected in ThumbnailPanel awaiting extraction - null when the dialog is
   *  closed, an array (possibly unsorted/duplicated as clicked) while it's open. */
  let extractPagesDialogIndices = $state<number[] | null>(null);

  // ---------------------------------------------------------------------------
  // Open flow — dedup by path, new tab per file
  // ---------------------------------------------------------------------------

  /**
   * Core open logic shared by File>Open dialog, file-drop, and auto-open.
   * - If the path is already open, switch to its tab (dedup).
   * - Otherwise open a new PDFium document, create a tab, and activate it.
   *
   * `password` is omitted on the first attempt (falling back to a cached
   * password from earlier this session, if any). If the backend reports the
   * file is encrypted, this shows PasswordPromptDialog instead of the generic
   * error banner; submitting the dialog re-invokes this function with the
   * entered password (see handlePasswordSubmit).
   */
  async function openFilePath(path: string, password?: string, remember = false) {
    // Dedup: if this path is already open, just switch to it.
    const existing = tabStore.findByPath(path);
    if (existing) {
      tabStore.switchTab(existing.docId);
      return;
    }

    openError = null;
    isOpening = true;
    try {
      const pw = password ?? getCachedPassword(passwordCache, path);
      const doc: DocumentInfo = await openDocument(path, pw);
      if (pw) setCachedPassword(passwordCache, path, pw);
      passwordPromptPath = null;
      passwordPromptError = null;

      const store = new MarkupStore(doc.doc_id, {
        add: addMarkup,
        update: updateMarkup,
        remove: deleteMarkup,
      });
      const ts = new TakeoffStore();
      tabStore.addTab(doc, store, ts, doc.was_encrypted);

      // Record successful open in the MRU history.
      void recordRecentDoc(doc);

      // Encrypted-PDF follow-ups: a typed password is remembered when the user ticked
      // the box in the password dialog (one dialog, not a second prompt), then a
      // non-modal banner offers an unprotected copy.
      if (doc.was_encrypted) {
        if (password && remember) {
          rememberPassword(password).catch((e) => {
            openError = `Remember password failed: ${e instanceof Error ? e.message : String(e)}`;
          });
        }
        unprotectedCopyPromptDocId = doc.doc_id;
      }

      // Load markups and scales asynchronously (non-blocking).
      // Scales are non-critical (a failure leaves the stored values as they are). Both
      // load before seeding so measurements can be normalised once under the document's
      // scales: computed-only - no undo frame, document stays clean - so the panels show
      // the numbers the exports compute.
      const scalesP = listScales(doc.doc_id).catch(() => [] as ScaleRecord[]);
      Promise.all([loadMarkups(doc.doc_id), scalesP])
        .then(([m, scales]) => {
          ts.seedScales(scales);
          store.seed(normaliseMeasurements(m, scales));
        })
        .catch((e) => { openError = `Load markups failed: ${e}`; });

      // Auto-OCR-on-open (Phase 2c-ii, opt-in via Settings - default off). Best-effort,
      // non-blocking: a failed settings load or detection must never slow or break
      // opening the document, so any error here is swallowed rather than surfaced.
      loadSettings()
        .then((settings) => (settings.auto_ocr_on_open ? documentNeedsOcr(doc.doc_id) : false))
        .then((needsOcr) => {
          if (needsOcr) void runOcrForDoc(doc.doc_id, store);
        })
        .catch(() => {});
    } catch (e) {
      const message = String(e);
      if (message === ERR_PASSWORD_REQUIRED) {
        passwordPromptPath = path;
        passwordPromptError = null;
      } else if (message === ERR_WRONG_PASSWORD) {
        passwordPromptPath = path;
        passwordPromptError = "Incorrect password. Try again.";
      } else {
        openError = message;
      }
    } finally {
      isOpening = false;
    }
  }

  /** PasswordPromptDialog submit handler: retry the open with the entered password. */
  async function handlePasswordSubmit(password: string, remember: boolean) {
    const path = passwordPromptPath;
    if (!path) return;
    await openFilePath(path, password, remember);
  }

  /** PasswordPromptDialog cancel handler: abandon the open attempt cleanly. */
  function handlePasswordCancel() {
    passwordPromptPath = null;
    passwordPromptError = null;
  }

  // ---------------------------------------------------------------------------
  // Encrypted-PDF follow-up: save-unprotected-copy
  // (backlog #10/#9 - save-unprotected-copy + known-password-list).
  // ---------------------------------------------------------------------------

  /**
   * Shared save-unprotected-copy flow: prompts for a destination (defaulting
   * to `<name>_unprotected.pdf` next to the source) and saves a decrypted
   * copy with no open password. Used by both the auto-prompt-on-open and the
   * toolbar/menu action.
   */
  async function saveUnprotectedCopyFor(docId: string) {
    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (!tab) return;
    const base = tab.doc.path.replace(/\.pdf$/i, "");
    const defaultPath = `${base}_unprotected.pdf`;
    const dest = await saveDialog({ defaultPath, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!dest) return;
    isSavingUnprotectedCopy = true;
    openError = null;
    try {
      await saveUnprotectedCopy(docId, dest);
    } catch (e) {
      openError = `Save Unprotected Copy failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSavingUnprotectedCopy = false;
    }
  }

  /** Auto-prompt on open confirmed: save an unprotected copy now. */
  function handleUnprotectedCopyPromptConfirm() {
    const docId = unprotectedCopyPromptDocId;
    unprotectedCopyPromptDocId = null;
    if (docId) void saveUnprotectedCopyFor(docId);
  }

  /** Auto-prompt on open declined. */
  function handleUnprotectedCopyPromptCancel() {
    unprotectedCopyPromptDocId = null;
  }

  /** Toolbar/menu action: save an unprotected copy of the active tab's document. */
  async function handleSaveUnprotectedCopyMenuAction() {
    if (!activeTab) return;
    await saveUnprotectedCopyFor(activeTab.docId);
  }

  async function handleOpenFile() {
    if (isOpening) return;
    const selected = await open({
      title: "Open PDF",
      filters: [{ name: "PDF Documents", extensions: ["pdf"] }],
      multiple: true, // allow multi-select to open several tabs at once
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    for (const p of paths) {
      await openFilePath(p as string);
    }
  }

  // ---------------------------------------------------------------------------
  // Close flow — tab × button and Cmd/Ctrl+W
  // ---------------------------------------------------------------------------

  /**
   * Low-level close: remove from store + release PDFium handle.
   * Does NOT check dirty state. Gate at the callers that check dirty.
   */
  async function doCloseTab(docId: string) {
    tabStore.closeTab(docId);
    dropLayerStore(docId);
    try {
      await closeDocument(docId);
    } catch {
      // Non-fatal: the tab is already gone from the UI.
    }
  }

  /**
   * Public close entry point (called by tab × button and Cmd/Ctrl+W).
   * If the document has unsaved changes, show the save-prompt dialog.
   * Otherwise close immediately.
   */
  async function closeTab(docId: string) {
    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (!tab) return;

    if (tab.store.dirty) {
      savePromptFilename = tab.doc.path.split(/[\\/]/).at(-1) ?? tab.doc.path;
      savePromptDocId = docId;
      return;
    }

    await doCloseTab(docId);
  }

  /** Save-prompt: user chose Save — save, clear dirty, then close. */
  async function handleSavePromptSave() {
    const docId = savePromptDocId;
    savePromptDocId = null;
    if (!docId) return;

    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (!tab) return;

    isSaving = true;
    openError = null;
    try {
      await tab.store.flush();
      await saveDocument(docId);
      tab.store.clearDirty();
      await doCloseTab(docId);
    } catch (e) {
      openError = `Save failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  /** Save-prompt: user chose Don't Save — close immediately, discarding changes. */
  async function handleSavePromptDiscard() {
    const docId = savePromptDocId;
    savePromptDocId = null;
    if (docId) await doCloseTab(docId);
  }

  /** Save-prompt: user chose Cancel — keep the document open, dismiss dialog. */
  function handleSavePromptCancel() {
    savePromptDocId = null;
  }

  // ---------------------------------------------------------------------------
  // Tab switching — save viewport snapshot before switching away
  // ---------------------------------------------------------------------------

  function handleTabClick(docId: string) {
    // The active Viewport's onviewportchange fires on every state change,
    // so the snapshot in tabStore is already current. Just switch.
    tabStore.switchTab(docId);
  }

  /** Called by the active Viewport on every zoom/page/scroll change. */
  function handleViewportChange(snapshot: ViewportSnapshot) {
    if (tabStore.activeDocId) {
      tabStore.saveViewportSnapshot(tabStore.activeDocId, snapshot);
    }
  }

  /**
   * Called by ThumbnailPanel after a rotate/delete/reorder IPC resolves (PR-B,
   * 2026-09-15 - the first GUI surface for these page ops; they previously existed
   * MCP-only). A delete changes the document's page count, which every other page-aware
   * surface (PageIndicator, ReadingView, the tab bar) reads from `activeTab.doc.page_count`
   * - refetch it here so they all stay in sync rather than each polling separately.
   */
  async function handleThumbnailPageOp() {
    const docId = tabStore.activeDocId;
    if (!docId) return;
    try {
      const newCount = await getPageCount(docId);
      tabStore.tabs = tabStore.tabs.map((t) =>
        t.docId === docId ? { ...t, doc: { ...t.doc, page_count: newCount } } : t,
      );
    } catch (e) {
      console.error("Failed to refresh page count after a page operation:", e);
    }
    // Page labels and links follow the page order, and the undo history was cleared.
    void tabStore.tabs.find((t) => t.docId === docId)?.navStore.refresh();
  }

  // ---------------------------------------------------------------------------
  // Save handlers (operate on the active tab)
  // ---------------------------------------------------------------------------

  async function handleSave() {
    if (!activeTab || isSaving) return;
    openError = null;
    isSaving = true;
    const docId = activeTab.docId;
    try {
      await activeTab.store.flush();
      await saveDocument(docId);
      activeTab.store.clearDirty();
    } catch (e) {
      if (isDiskChangedError(e)) await showStaleDialog(docId);
      else openError = `Save failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  /** Point a tab at a new file path after the backend re-homed the document (Save As). */
  function adoptPath(docId: string, dest: string) {
    tabStore.tabs = tabStore.tabs.map((t) =>
      t.docId === docId ? { ...t, doc: { ...t.doc, path: dest } } : t,
    );
  }

  async function handleSaveAs() {
    if (!activeTab || isSaving) return;
    openError = null;
    const dest = await saveDialog({ filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!dest) return;
    isSaving = true;
    const docId = activeTab.docId;
    try {
      await activeTab.store.flush();
      await saveDocumentAs(docId, dest);
      activeTab.store.clearDirty();
      adoptPath(docId, dest);
    } catch (e) {
      if (isDiskChangedError(e)) await showStaleDialog(docId);
      else openError = `Save As failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  // The PDF changed on disk under redline: nothing was written. Two recoveries, neither of
  // which writes over the changed file (see StaleFileDialog).

  /** Save As (rebuild): the mark-ups go to a new file built from the bytes redline loaded. */
  async function handleStaleRebuild() {
    const docId = staleDocId;
    staleDocId = null;
    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (!tab || isSaving) return;
    const dest = await saveDialog({
      title: "Save your mark-ups to a new file",
      defaultPath: tab.doc.path.replace(/\.pdf$/i, "") + "-rebuilt.pdf",
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!dest) {
      staleDocId = docId; // backed out of the file dialog: offer the choice again
      return;
    }
    openError = null;
    isSaving = true;
    try {
      await tab.store.flush();
      await saveDocumentAsRebuild(tab.docId, dest);
      tab.store.clearDirty();
      adoptPath(tab.docId, dest);
      void tab.navStore.refresh(); // the rebuilt file is the PDF as it was opened
      docOpsStatus = `Your mark-ups were saved to ${baseName(dest)}, rebuilt from the PDF as you opened it.`;
    } catch (e) {
      openError = `Save As (rebuild) failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  /** Reload and keep my mark-ups: re-read the changed file, re-apply the unsaved edits. */
  async function handleStaleReload() {
    const docId = staleDocId;
    staleDocId = null;
    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (!tab || isSaving) return;
    openError = null;
    isSaving = true;
    try {
      await tab.store.flush();
      const report = await reloadKeepingMarkups(tab.docId);
      // Computed-only normalisation under the document's scales, like on open, so the panels
      // and any legend show the numbers the exports compute after a reload too.
      tab.store.seed(normaliseMeasurements(await listMarkups(tab.docId), tab.takeoffStore.scales));
      tab.store.dirty = report.applied > 0; // re-applied edits are still unsaved
      await handleThumbnailPageOp();
      void layerStoreFor(tab.docId).reset(); // the file changed: its layers may have too
      reloadNonce += 1;
      docOpsStatus = `Reloaded the changed file and re-applied ${report.applied} of your edits.`;
      if (report.skipped.length > 0) {
        openError = `Could not re-apply: ${report.skipped.join("; ")}`;
      } else if (report.notes.length > 0) {
        docOpsStatus += ` ${report.notes.join("; ")}`;
      }
    } catch (e) {
      openError = `Reload failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  // ---------------------------------------------------------------------------
  // DocOps handlers (M5) — operate on the active tab
  // ---------------------------------------------------------------------------

  /**
   * Flatten / Optimise / Apply Redactions rewrite the document, so each one opens
   * DocOpDialog first (UX audit F1): what will happen, then copy (default) or replace.
   */
  function requestDocOp(kind: DocOpKind) {
    if (!activeTab || isFlattening || isOptimizing || isRedacting || isSaving) return;
    docOpPrompt = kind;
  }

  async function handleDocOpConfirm(mode: DocOpMode) {
    const kind = docOpPrompt;
    docOpPrompt = null;
    if (kind) await performDocOp(kind, mode);
  }

  function setDocOpBusy(kind: DocOpKind, busy: boolean) {
    if (kind === "flatten") isFlattening = busy;
    else if (kind === "optimize") isOptimizing = busy;
    else isRedacting = busy;
  }

  /** The backend operation itself plus the user-facing summary line. */
  async function runDocOpBody(kind: DocOpKind, docId: string, store: MarkupStore): Promise<string> {
    if (kind === "flatten") {
      const count = await runDocOpAndReseed(docId, store, { loadMarkups }, () => flattenDocument(docId));
      return count === 0
        ? "Flatten: no annotations with an appearance found on this document - nothing to flatten."
        : `Flatten complete - ${count} annotation${count === 1 ? "" : "s"} baked into page content.`;
    }
    if (kind === "optimize") {
      const report = await runDocOpAndReseed(
        docId,
        store,
        { loadMarkups },
        () => optimizeDocument(docId, 2, imageQualityPreset),
      );
      const saved = report.bytes_before - report.bytes_after;
      const { images_recompressed, images_downsampled, images_total } = report.image_stats;
      const imageNote =
        images_total > 0
          ? images_recompressed > 0
            ? ` (${images_recompressed} of ${images_total} image${images_total === 1 ? "" : "s"} recompressed, ${images_downsampled} downsampled)`
            : ` (0 of ${images_total} image${images_total === 1 ? "" : "s"} could be recompressed further)`
          : "";
      return saved > 0
        ? `Optimise complete - file size reduced from ${formatBytes(report.bytes_before)} to ${formatBytes(report.bytes_after)} (saved ${formatBytes(saved)})${imageNote}.`
        : `Optimise complete - the file was already optimal; no size reduction${imageNote}.`;
    }
    await runDocOpAndReseed(docId, store, { loadMarkups }, () => redactDocument(docId, [], true));
    return "Apply Redactions complete.";
  }

  /**
   * Run a document-rewriting action safely (UX audit F1). "copy" (the default) writes the
   * current document to a new file first and runs the action on that copy, leaving the
   * original untouched. "replace" saves, takes an automatic History snapshot, then rewrites
   * the original; if the snapshot fails the action is NOT run.
   */
  async function performDocOp(kind: DocOpKind, requested: DocOpMode) {
    const tab = activeTab;
    if (!tab) return;
    const docId = tab.docId;
    const store = tab.store;
    openError = null;
    docOpsStatus = null;
    setDocOpBusy(kind, true);
    try {
      const prepared = await prepareDocOp(kind, requested, {
        currentPath: tab.doc.path,
        flush: () => store.flush(),
        pickCopyDest: (defaultPath) =>
          saveDialog({
            title: "Save the result as a copy",
            defaultPath,
            filters: [{ name: "PDF", extensions: ["pdf"] }],
          }),
        isOpenInAnotherTab: (dest) =>
          tabStore.tabs.some((t) => t.docId !== docId && samePath(t.doc.path, dest)),
        saveAs: (dest) => saveDocumentAs(docId, dest),
        save: () => saveDocument(docId),
        snapshot: async (label) => {
          await snapshotVersion(docId, label);
          historyNonce += 1;
        },
        adoptPath: (dest) => adoptPath(docId, dest),
        clearDirty: () => store.clearDirty(),
      });
      if (!prepared.proceed) return; // user backed out of the file dialog: nothing has changed
      const note = prepared.note;
      const summary = await runDocOpBody(kind, docId, store);
      docOpsStatus = `${note}${summary}`;
    } catch (e) {
      const name = kind === "flatten" ? "Flatten" : kind === "optimize" ? "Optimise" : "Apply Redactions";
      openError = `${name} failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      setDocOpBusy(kind, false);
    }
  }

  /** Re-sync the UI after the History panel restored an older version of the file. */
  async function handleVersionRestore() {
    const tab = activeTab;
    if (!tab) return;
    try {
      const markups = await loadMarkups(tab.docId);
      tab.store.seed(normaliseMeasurements(markups, tab.takeoffStore.scales));
      tab.store.clearDirty();
      await handleThumbnailPageOp();
      void layerStoreFor(tab.docId).reset(); // the restored file has its own layers
      reloadNonce += 1;
      docOpsStatus = "Version restored from History.";
    } catch (e) {
      openError = `Reload after restore failed: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  /**
   * Export the mark-up list (XLSX or CSV, chosen by the file extension in the save dialog).
   * Available whenever the document has any mark-up, not only measurements (UX audit F3).
   */
  async function handleExportMarkupList() {
    const tab = activeTab;
    if (!tab || tab.store.markups.length === 0) return;
    openError = null;
    docOpsStatus = null;
    const dest = await saveDialog({
      title: "Export mark-up list",
      defaultPath: `${tab.doc.path.replace(/\.pdf$/i, "")}-markups.xlsx`,
      filters: [
        { name: "Excel workbook", extensions: ["xlsx"] },
        { name: "CSV", extensions: ["csv"] },
      ],
    });
    if (!dest) return;
    try {
      await tab.store.flush();
      await exportMarkupList(tab.docId, dest, exportFormatFromPath(dest));
      docOpsStatus = exportDoneMessage(dest, tab.store.markups.length);
    } catch (e) {
      openError = `Export failed: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  // ---------------------------------------------------------------------------
  // Window close guard (UX audit F2) - ask about each tab with unsaved mark-ups
  // before the window goes away.
  // ---------------------------------------------------------------------------

  async function finishQuitStep(decision: QuitDecision) {
    const step = advanceQuit(quitQueue, decision);
    if (step.action === "prompt") {
      quitQueue = step.queue;
    } else if (step.action === "abort") {
      quitQueue = [];
    } else {
      quitQueue = [];
      try {
        await getCurrentWindow().destroy();
      } catch (e) {
        openError = `Could not close the window: ${e instanceof Error ? e.message : String(e)}`;
      }
    }
  }

  async function handleQuitSave() {
    const docId = quitQueue[0];
    const tab = tabStore.tabs.find((t) => t.docId === docId);
    if (tab) {
      try {
        await tab.store.flush();
        await saveDocument(docId);
        tab.store.clearDirty();
      } catch (e) {
        // Keep the window open: closing now would lose this tab's mark-ups.
        if (isDiskChangedError(e)) await showStaleDialog(docId);
        else openError = `Save failed: ${e instanceof Error ? e.message : String(e)}`;
        quitQueue = [];
        return;
      }
    }
    await finishQuitStep("save");
  }

  async function handleQuitDiscard() {
    tabStore.tabs.find((t) => t.docId === quitQueue[0])?.store.clearDirty();
    await finishQuitStep("discard");
  }

  function handleQuitCancel() {
    void finishQuitStep("cancel");
  }

  /**
   * OCR (Phase 2c-ii) - shared by the toolbar action and the auto-OCR-on-open trigger,
   * same flush -> op -> reseed pattern the other DocOps handlers use (docops-handlers.ts).
   * `docId`/`store` are passed explicitly rather than always reading `activeTab` because
   * auto-OCR-on-open must be able to run against a just-opened document that isn't
   * necessarily the currently focused tab (e.g. several PDFs opened at once via drag-drop).
   */
  async function runOcrForDoc(docId: string, store: MarkupStore) {
    if (isOcrRunning) return;
    openError = null;
    docOpsStatus = null;
    isOcrRunning = true;
    ocrRunningDocId = docId;
    try {
      const report = await runDocOpAndReseed(
        docId,
        store,
        { loadMarkups },
        () => runOcrDocument(docId),
      );
      const skippedNote =
        report.pages_skipped_existing_text > 0
          ? ` (${report.pages_skipped_existing_text} page${report.pages_skipped_existing_text === 1 ? "" : "s"} already had text, skipped)`
          : "";
      docOpsStatus =
        report.pages_ocred === 0
          ? `OCR: no pages needed scanning${skippedNote || " - document is empty"}.`
          : `OCR complete - ${report.pages_ocred} page${report.pages_ocred === 1 ? "" : "s"} scanned, ${report.lines_embedded} line${report.lines_embedded === 1 ? "" : "s"} embedded${skippedNote}.`;
    } catch (e) {
      openError = `OCR failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isOcrRunning = false;
      ocrRunningDocId = null;
    }
  }

  /** Toolbar/menu action: OCR the active tab's document. */
  async function handleRunOcr() {
    if (!activeTab || isOcrRunning) return;
    await runOcrForDoc(activeTab.docId, activeTab.store);
  }

  // ---------------------------------------------------------------------------
  // Compare handlers (M6 Phase 1.1)
  // ---------------------------------------------------------------------------

  async function handlePickCompareA() {
    const selected = await open({
      title: "Select old PDF (File A)",
      filters: [{ name: "PDF Documents", extensions: ["pdf"] }],
      multiple: false,
    });
    if (selected && !Array.isArray(selected)) comparePathA = selected as string;
  }

  async function handlePickCompareB() {
    const selected = await open({
      title: "Select new PDF (File B)",
      filters: [{ name: "PDF Documents", extensions: ["pdf"] }],
      multiple: false,
    });
    if (selected && !Array.isArray(selected)) comparePathB = selected as string;
  }

  const COMPARE_BROWSE = "__browse__";

  /** A Compare picker changed: an open tab's path, or "Browse…" for a file on disk. */
  function onCompareSelect(side: "A" | "B", value: string) {
    if (value === COMPARE_BROWSE) {
      void (side === "A" ? handlePickCompareA() : handlePickCompareB());
      return;
    }
    if (side === "A") comparePathA = value;
    else comparePathB = value;
  }

  /** Open/close the Compare bar; opening presets File A to the active tab. */
  function toggleCompare() {
    compareVisible = !compareVisible;
    if (compareVisible && !comparePathA && activeTab) comparePathA = activeTab.doc.path;
  }

  // ---------------------------------------------------------------------------
  // Toolbar menus (UX audit F4): File / Edit / Document / View groups, with the less
  // common actions folded into a "More" menu on narrow windows.
  // ---------------------------------------------------------------------------

  const docBusy = $derived(isFlattening || isOptimizing || isRedacting || isSaving);
  const hasMarkups = $derived((activeTab?.store.markups.length ?? 0) > 0);

  const documentMenuItems = $derived.by<ToolbarMenuItem[]>(() => {
    const items: ToolbarMenuItem[] = [
      {
        id: "flatten",
        label: "Flatten annotations…",
        title: "Bake annotation appearances into page content (asks before it runs)",
        disabled: !activeTab || docBusy,
        onselect: () => requestDocOp("flatten"),
      },
      {
        id: "optimize",
        label: "Optimise file size…",
        title: "Remove unused objects, compress streams and recompress images (asks before it runs)",
        disabled: !activeTab || docBusy,
        onselect: () => requestDocOp("optimize"),
      },
    ];
    if (REDACT_AUTHORING_AVAILABLE) {
      items.push({
        id: "redact",
        label: "Apply Redactions…",
        title: "Permanently cover all Redact-marked regions (asks before it runs)",
        disabled: !activeTab || docBusy,
        onselect: () => requestDocOp("redact"),
      });
    }
    items.push(
      {
        id: "number-pages",
        label: "Number pages…",
        title: "Label the pages (E-101, A-1, iv, ...) - shown in the page box, thumbnails and the Markups List",
        disabled: !activeTab || !activeTab.navStore.fileEditable || docBusy,
        onselect: () => (numberPages = { selected: [] }),
      },
      {
        id: "ocr",
        label: isOcrRunning && ocrRunningDocId === activeTab?.docId ? "OCR running…" : "OCR this document",
        title: "Recognise scanned text and embed it as an invisible, searchable layer",
        disabled: !activeTab || isOcrRunning || isSaving,
        onselect: () => void handleRunOcr(),
      },
      {
        id: "unprotected",
        label: isSavingUnprotectedCopy ? "Saving…" : "Save Unprotected Copy…",
        title: "Save a copy of this PDF with its password protection removed",
        disabled: !activeTab?.isEncrypted || isSavingUnprotectedCopy,
        separator: true,
        onselect: () => void handleSaveUnprotectedCopyMenuAction(),
      },
    );
    for (const [value, label] of [
      ["high", "High quality"],
      ["balanced", "Balanced"],
      ["small", "Small file"],
    ] as const) {
      items.push({
        id: `quality-${value}`,
        label: `${imageQualityPreset === value ? "✓" : "\u2003"} Optimise images: ${label}`,
        title: "Image quality used by Optimise (trades file size against image quality)",
        separator: value === "high",
        onselect: () => (imageQualityPreset = value),
      });
    }
    return items;
  });

  const moreMenuItems = $derived.by<ToolbarMenuItem[]>(() => [
    { id: "save-as", label: "Save As…", disabled: !activeTab || isSaving, onselect: () => void handleSaveAs() },
    {
      id: "export-list",
      label: "Export mark-up list…",
      disabled: !hasMarkups,
      onselect: () => void handleExportMarkupList(),
    },
    {
      id: "find",
      label: "Find (Ctrl/Cmd+F)",
      separator: true,
      onselect: () => (searchPanelVisible ? (searchPanelVisible = false) : void openSearchPanel()),
    },
    { id: "compare", label: compareVisible ? "Hide Compare" : "Compare revisions", onselect: toggleCompare },
    {
      id: "reading",
      label: viewerMode === "reading" ? "Switch to single-page view" : "Switch to reading mode",
      onselect: () => void toggleViewerMode(),
    },
    { id: "left", label: "Toggle left sidebar", separator: true, onselect: () => (leftCollapsed = !leftCollapsed) },
    { id: "right", label: "Toggle right sidebar", onselect: () => (rightCollapsed = !rightCollapsed) },
    { id: "bottom", label: "Toggle markups list / quantities panel", onselect: () => (bottomCollapsed = !bottomCollapsed) },
    { id: "settings", label: "Settings", separator: true, onselect: () => (settingsOpen = true) },
    { id: "about", label: "About Redline", onselect: () => (aboutOpen = true) },
  ]);

  // ---------------------------------------------------------------------------
  // Keyboard shortcuts
  // ---------------------------------------------------------------------------

  function handleKeydown(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;

    // Cmd/Ctrl+F — open/focus search; F3 / Shift+F3 — next/prev
    // result. See keyboard-shortcuts.ts for why "next"/"prev" aren't gated on editable
    // targets (Find-next legitimately fires while e.g. a markup comment field has focus).
    const searchAction = resolveSearchShortcut(e);
    if (searchAction === "open") {
      e.preventDefault();
      void openSearchPanel();
      return;
    }
    if (searchAction === "next" || searchAction === "prev") {
      if (searchStore.flatHits.length > 0) {
        e.preventDefault();
        stepSearchResult(searchAction);
        return;
      }
    }

    // Ctrl/Cmd+Shift+R — toggle reading mode (owner request 2026-09-15).
    if (resolveViewerModeToggleShortcut(e)) {
      e.preventDefault();
      void toggleViewerMode();
      return;
    }

    // Cmd/Ctrl+Z — undo; Cmd/Ctrl+Shift+Z / Cmd/Ctrl+Y — redo. Resolver returns null (and
    // does nothing here) while a text/callout inline editor or other input has focus, so
    // the field keeps its own native undo (see keyboard-shortcuts.ts).
    const undoRedoAction = resolveUndoRedoShortcut(e);
    if (undoRedoAction) {
      e.preventDefault();
      if (undoRedoAction === "undo") activeTab?.store.undo();
      else activeTab?.store.redo();
      return;
    }

    // Cmd/Ctrl+S — save active tab
    if (mod && e.key.toLowerCase() === "s" && !e.shiftKey) {
      e.preventDefault();
      handleSave();
      return;
    }

    // Cmd/Ctrl+W — close active tab
    if (mod && e.key.toLowerCase() === "w") {
      e.preventDefault();
      if (activeTab) closeTab(activeTab.docId);
      return;
    }

    // Ctrl+Tab — next tab
    if (e.ctrlKey && e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      if (tabStore.tabs.length > 1 && tabStore.activeDocId) {
        const idx = tabStore.tabs.findIndex((t) => t.docId === tabStore.activeDocId);
        const next = tabStore.tabs[(idx + 1) % tabStore.tabs.length];
        tabStore.switchTab(next.docId);
      }
      return;
    }

    // Ctrl+Shift+Tab — previous tab
    if (e.ctrlKey && e.key === "Tab" && e.shiftKey) {
      e.preventDefault();
      if (tabStore.tabs.length > 1 && tabStore.activeDocId) {
        const idx = tabStore.tabs.findIndex((t) => t.docId === tabStore.activeDocId);
        const prev = tabStore.tabs[(idx - 1 + tabStore.tabs.length) % tabStore.tabs.length];
        tabStore.switchTab(prev.docId);
      }
      return;
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} bind:innerWidth={viewportWidth} />

{#if licenseState === null}
  <!-- Initial licence check in flight - avoid a flash of the activation gate
       for the common case (already-activated install). -->
  <div class="app-shell license-checking"><p class="license-checking-text" role="status">Checking licence…</p></div>
{:else if !isUsable(licenseState)}
  <ActivationGate licenseState={licenseState} onActivated={handleActivated} />
{:else}
<div class="app-shell">
  <!-- Toolbar -->
  <header class="toolbar">
    <div class="toolbar-left">
      <span class="app-name">Redline</span>

      <!-- File -->
      <div class="toolbar-group" role="group" aria-label="File">
        <button class="btn-toolbar" onclick={handleOpenFile} disabled={isOpening}>
          {isOpening ? "Opening…" : "Open PDF"}
        </button>
        <button class="btn-toolbar" onclick={handleSave} disabled={!activeTab || isSaving} title="Save (Cmd/Ctrl+S)">
          {isSaving ? "Saving…" : "Save"}
        </button>
        {#if !compactToolbar}
          <button class="btn-toolbar" onclick={handleSaveAs} disabled={!activeTab || isSaving} title="Save As…">
            Save As…
          </button>
          <button
            class="btn-toolbar"
            data-testid="export-markup-list"
            onclick={handleExportMarkupList}
            disabled={!hasMarkups}
            title={hasMarkups
              ? "Export every mark-up (type, page, subject, author, contents, status) to Excel or CSV"
              : "Add a mark-up to enable the mark-up list export"}
          >
            Export mark-up list…
          </button>
        {/if}
      </div>

      <span class="toolbar-sep" aria-hidden="true"></span>

      <!-- Edit -->
      <div class="toolbar-group" role="group" aria-label="Edit">
        <UndoRedoControls store={activeTab?.store ?? null} />
        {#if !compactToolbar}
          <button
            class="btn-toolbar"
            onclick={() => (searchPanelVisible ? (searchPanelVisible = false) : void openSearchPanel())}
            title="Find (Cmd/Ctrl+F)"
          >
            {searchPanelVisible ? "🔍 Find ▲" : "🔍 Find"}
          </button>
        {/if}
      </div>

      <span class="toolbar-sep" aria-hidden="true"></span>

      <!-- Document: rare and destructive operations live in one menu -->
      <ToolbarMenu
        label="Document"
        title="Flatten, optimise, OCR and other operations that rewrite the file"
        items={documentMenuItems}
        disabled={!activeTab}
        testId="document-menu"
      />

      {#if activeTab}
        <PageIndicator
          currentPage={activeTab.viewportSnapshot.pageIndex}
          pageCount={activeTab.doc.page_count}
          labels={activeTab.navStore.labels}
          onjump={(idx) => {
            searchJumpNonce += 1;
            viewportJumpRequest = { page: idx, nonce: searchJumpNonce };
          }}
        />
      {/if}
    </div>
    <div class="toolbar-right">
      {#if compactToolbar}
        <!-- Narrow window: the rest of File / Edit / View collapses into one menu. -->
        <ToolbarMenu label="More" items={moreMenuItems} align="right" testId="more-menu" />
      {:else}
        <!-- View -->
        <div class="toolbar-group" role="group" aria-label="View">
          <button
            class="btn-toolbar btn-compare-toggle"
            onclick={toggleCompare}
            title="Toggle compare panel - diff two PDF revisions (spec §10)"
          >
            {compareVisible ? "Compare ▲" : "Compare"}
          </button>
          <button
            class="btn-toolbar btn-icon"
            class:btn-icon-active={viewerMode === "reading"}
            onclick={toggleViewerMode}
            title="Toggle continuous reading mode (Ctrl/Cmd+Shift+R)"
          >{viewerMode === "reading" ? "📖 Reading" : "📖"}</button>
          <button
            class="btn-toolbar btn-icon"
            class:btn-icon-active={!leftCollapsed}
            onclick={() => (leftCollapsed = !leftCollapsed)}
            title="Toggle left sidebar"
            aria-label="Toggle left sidebar"
            aria-pressed={!leftCollapsed}
          >
            <svg class="sidebar-icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" />
              <rect x="1.5" y="2.5" width="5" height="11" rx="1.5" fill="currentColor" />
            </svg>
          </button>
          <button
            class="btn-toolbar btn-icon"
            class:btn-icon-active={!rightCollapsed}
            onclick={() => (rightCollapsed = !rightCollapsed)}
            title="Toggle right sidebar (Properties)"
            aria-label="Toggle right sidebar"
            aria-pressed={!rightCollapsed}
          >
            <svg class="sidebar-icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" />
              <rect x="9.5" y="2.5" width="5" height="11" rx="1.5" fill="currentColor" />
            </svg>
          </button>
          <button
            class="btn-toolbar btn-icon"
            class:btn-icon-active={!bottomCollapsed}
            onclick={() => (bottomCollapsed = !bottomCollapsed)}
            title="Toggle markups list / quantities panel"
            aria-label="Toggle markups list / quantities panel"
            aria-pressed={!bottomCollapsed}
          >
            <svg class="sidebar-icon" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
              <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" />
              <rect x="1.5" y="9" width="13" height="4.5" rx="1.5" fill="currentColor" />
            </svg>
          </button>
          <button
            class="btn-toolbar btn-icon"
            onclick={() => (settingsOpen = true)}
            title="Settings"
            aria-label="Settings"
          >⚙</button>
          <button
            class="btn-toolbar btn-icon"
            onclick={() => (aboutOpen = true)}
            title="About Redline"
            aria-label="About Redline"
          >ⓘ</button>
        </div>
      {/if}
    </div>
  </header>

  <!-- Tab bar (multi-doc) -->
  <TabBar
    tabs={tabStore.tabs}
    activeDocId={tabStore.activeDocId}
    ontabclick={handleTabClick}
    ontabclose={closeTab}
    onmoveTab={(from, to) => tabStore.moveTab(from, to)}
  />

  <!-- Compare panel — collapsible, below tab bar (M6 Phase 1.1, spec §10) -->
  {#if compareVisible}
    <div class="compare-bar">
      <div class="compare-bar-pickers">
        {#each [["A", "File A (old)", comparePathA], ["B", "File B (new)", comparePathB]] as [side, label, current] (side)}
          <label class="compare-pick">
            <span class="compare-pick-label">{label}</span>
            <select
              class="toolbar-select"
              value={current}
              aria-label={label}
              onchange={(e) => onCompareSelect(side as "A" | "B", (e.target as HTMLSelectElement).value)}
            >
              <option value="" disabled>Choose a file…</option>
              {#if current && !tabStore.tabs.some((t) => t.doc.path === current)}
                <option value={current}>{baseName(current as string)}</option>
              {/if}
              {#each tabStore.tabs as t (t.docId)}
                <option value={t.doc.path}>{baseName(t.doc.path)}{t.docId === tabStore.activeDocId ? " (current tab)" : ""}</option>
              {/each}
              <option value={COMPARE_BROWSE}>Browse…</option>
            </select>
          </label>
        {/each}
      </div>
      <ComparePanel pathA={comparePathA} pathB={comparePathB} />
    </div>
  {/if}

  {#if activeTab}
    <ToolPalette store={activeTab.store} navStore={activeTab.navStore} />
  {/if}

  {#if openError}
    <div class="error-banner">{openError}</div>
  {/if}

  {#if activeTab?.store.mirrorError}
    <!-- Markup sync/lock refusal feedback (review finding, PR #92 2026-09-01): mirrorError
         was previously set by markup-store.svelte.ts but never read anywhere in the
         frontend - a locked-markup edit or a genuine sync failure had no visible signal
         at all. Reuses the existing error-banner style. -->
    <div class="error-banner">{activeTab.store.mirrorError}</div>
  {/if}

  {#if unprotectedCopyPromptDocId !== null}
    <!-- Non-modal offer (was a third modal in the encrypted-open chain, UX audit F9). -->
    <div class="docops-status-banner unprotected-offer" role="status">
      <span>This PDF is password-protected. Save a copy with no open password?</span>
      <button class="btn-toolbar" onclick={handleUnprotectedCopyPromptConfirm}>Save copy…</button>
      <button class="btn-toolbar" onclick={handleUnprotectedCopyPromptCancel}>Dismiss</button>
    </div>
  {/if}

  {#if docOpsStatus}
    <div class="docops-status-banner">{docOpsStatus}</div>
  {/if}

  <!-- Main 3-column body -->
  <div class="body-row">
    <!-- Left panel -->
    {#if !leftCollapsed}
      <aside class="panel panel-left" data-testid="panel-left">
        <!-- Search (search-parity: current doc / open docs / folder+subfolders). Placed
             first — the toolbar Find button / Cmd-Ctrl-F is the primary way in.

             Owner-reported defects (2026-09-08, v0.3.20 live use), both fixed here.
             Terminology (owner-set): the left/right panel-left/panel-right elements are
             SIDEBARS; the collapsible modules stacked inside one (Search, Tool Chest,
             Properties, etc.) are PANELS. Existing identifiers (panel-left,
             --panel-left-width, .panel-section, data-testid="panel-left") are unchanged -
             only new prose uses the new words.
             1) "opening the search panel changes the sidebar to a different width" -
                sidebar width is a property of the SIDEBAR, not of whichever panel happens
                to be showing. The old class:panel-left--search width override (and the
                --panel-left-width-search token) is gone; panel-left keeps ONE width
                (--panel-left-width) whether or not Search is open, matching the
                persisted/default sidebar width like every other panel switch.
             2) "the other panel boxes" (Recent Documents / Tool Chest / Navigator) "get
                hidden behind search" - per spec §17 a sidebar hosts a STACK of
                individually collapsible panels, so Search no longer swaps them out via
                {:else}; it renders ABOVE them with a bounded height
                (.panel-section--search, capped so it can't crowd the rest of the stack
                out) while its own result list (.search-groups) scrolls internally -
                Recent Documents/Tool Chest/Navigator stay visible and reachable below
                it, with their existing Accordion collapse/expand affordances untouched.
                One side effect worth knowing: these three panels no longer unmount when
                Search opens (they used to live behind the {:else} branch) - they now
                stay mounted the whole time, which is strictly safer for their Accordion
                sessionStorage-backed collapsed state (see ToolChestPanel.svelte's
                comment) than the old unmount/remount cycle was.

             (Superseded 2026-09-07 comment: Search "takes over the whole left column" (the
             pre-2026-09-08 wording for this sidebar) - no longer true, see above.) -->
        {#if searchPanelVisible}
          <div class="panel-section panel-section--search">
            <div class="panel-header">
              Search
              <button class="btn-icon-close" onclick={() => (searchPanelVisible = false)} aria-label="Close search">✕</button>
            </div>
            <div class="panel-body panel-body-flush">
              <SearchPanel
                store={searchStore}
                folderPath={searchFolderPath}
                folderIndexStatus={searchFolderIndexStatus}
                onSearch={runSearch}
                onPickFolder={pickSearchFolder}
                onJump={handleSearchJump}
                onHighlightChecked={applyHighlightToChecked}
                highlightCheckedAlreadyApplied={checkedAlreadyHighlighted}
              />
            </div>
          </div>
        {/if}
        <!-- Document History section (MRU list). Owner defect (2026-09-07):
             "the recent documents panel needs to be able to accordion -
             actually, all subpanels in the side panels" - every left/right
             panel section below is now collapsible via the shared Accordion
             component (src/components/Accordion.svelte), remembering its
             state for this app session via sessionStorage (storageKey).
             Always rendered (stacked below Search when it's open - see the
             panel-section--search comment above). -->
        <div class="panel-section panel-section--recent">
          <Accordion
            title="Recent Documents"
            storageKey="left-recent-docs"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-recent-docs"
          >
            <DocumentHistoryPanel
              recentDocs={recentDocs}
              onOpen={openFilePath}
            />
          </Accordion>
        </div>
        <!-- Tool Chest (spec "Tools & Tool Sets") - Tool Sets + Recent Tools; click a
             tool to make it active. Discoverable here regardless of which doc tab is
             active (Tool Sets are a workspace resource, not per-document). -->
        <div class="panel-section panel-section--secondary">
          <Accordion
            title="Tool Chest"
            storageKey="left-toolchest"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-tool-chest"
          >
            <ToolChestPanel toolChest={toolChestStore} markupStore={activeTab?.store ?? null} />
          </Accordion>
        </div>
        <!-- Bookmarks and Links (docs/navigation.md "Document structure"). -->
        <div class="panel-section panel-section--nav">
          <Accordion
            title="Bookmarks"
            storageKey="left-bookmarks"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-bookmarks"
          >
            {#if activeTab}
              <BookmarksPanel
                nav={activeTab.navStore}
                pageCount={activeTab.doc.page_count}
                currentPage={activeTab.viewportSnapshot.pageIndex}
                onfollow={followTarget}
                {getCurrentView}
                getSelectionBookmark={selectionBookmark}
              />
            {:else}
              <p class="panel-hint muted">Open a PDF to begin.</p>
            {/if}
          </Accordion>
        </div>
        <div class="panel-section panel-section--nav">
          <Accordion
            title="Links"
            storageKey="left-links"
            defaultCollapsed={true}
            bodyClass="panel-body panel-body-flush"
            testId="accordion-links"
          >
            {#if activeTab}
              <LinksPanel
                nav={activeTab.navStore}
                onjump={jumpTo}
                onfollow={followTarget}
                onedit={(l) => (editingLink = l)}
              />
            {:else}
              <p class="panel-hint muted">Open a PDF to begin.</p>
            {/if}
          </Accordion>
        </div>
        <!-- Navigator: page thumbnails (PR-B, 2026-09-15). -->
        <div class="panel-section panel-section--secondary">
          <Accordion
            title="Navigator"
            storageKey="left-navigator"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-navigator"
          >
            {#if activeTab}
              <ThumbnailPanel
                docId={activeTab.docId}
                pageCount={activeTab.doc.page_count}
                currentPage={activeTab.viewportSnapshot.pageIndex}
                onjump={(idx) => {
                  searchJumpNonce += 1;
                  viewportJumpRequest = { page: idx, nonce: searchJumpNonce };
                }}
                onextract={(indices) => (extractPagesDialogIndices = indices)}
                onPageOp={handleThumbnailPageOp}
                labels={activeTab.navStore.labels}
                onnumber={(sel) => (numberPages = { selected: sel })}
                onrenamelabel={renamePageLabel}
              />
            {:else}
              <p class="panel-hint muted">Open a PDF to begin.</p>
            {/if}
          </Accordion>
        </div>
        <!-- Layers: the PDF's optional content groups. Show/hide is a view setting (not
             written to the file); see docs/layers.md. -->
        <div class="panel-section panel-section--secondary">
          <Accordion
            title="Layers"
            storageKey="left-layers"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-layers"
          >
            {#if activeTab}
              {#key activeTab.docId}
                <LayersPanel
                  docId={activeTab.docId}
                  sync={{
                    flush: () => activeTab.store.flush(),
                    clearDirty: () => activeTab.store.clearDirty(),
                  }}
                />
              {/key}
            {:else}
              <p class="panel-hint muted">Open a PDF to see its layers.</p>
            {/if}
          </Accordion>
        </div>
        <!-- History: version snapshots (automatic before every in-place Flatten /
             Optimise / Redactions, plus manual). Makes the restore path reachable (UX
             audit F1). The key re-lists versions after an automatic snapshot. -->
        <div class="panel-section panel-section--secondary">
          <Accordion
            title="History"
            storageKey="left-history"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-history"
          >
            {#if activeTab}
              {#key activeTab.docId + ":" + historyNonce}
                <VersionPanel
                  docId={activeTab.docId}
                  hasUnsavedMarkups={activeTab.store.dirty}
                  onRestore={handleVersionRestore}
                />
              {/key}
            {:else}
              <p class="panel-hint muted">Open a PDF to see its saved versions.</p>
            {/if}
          </Accordion>
        </div>
      </aside>
    {/if}

    <!-- Centre viewport — only one Viewport/ReadingView mounted at a time. Mode is a
         persisted toggle (viewerMode), not per-tab - see toggleViewerMode. -->
    <main class="viewport-container">
      {#if activeTab}
        <!-- Key forces a remount both when switching tabs AND when toggling viewer mode,
             so initialState (zoom/page/scroll snapshot) takes effect fresh either way. -->
        {#key activeTab.docId + ":" + viewerMode + ":" + reloadNonce}
          {#if viewerMode === "reading"}
            <!-- ReadingView has no MarkupStore/ToolPalette - the I-beam text-selection
                 tool (and its onsearchtext callback) doesn't exist in this mode, so
                 there is nothing to wire here (redline#text-select-ux). -->
            <ReadingView
              docInfo={activeTab.doc}
              initialState={activeTab.viewportSnapshot}
              onviewportchange={handleViewportChange}
              jumpRequest={viewportJumpRequest ? { page: viewportJumpRequest.page, nonce: viewportJumpRequest.nonce } : null}
            />
          {:else}
            <Viewport
              docInfo={activeTab.doc}
              store={activeTab.store}
              takeoffStore={activeTab.takeoffStore}
              initialState={activeTab.viewportSnapshot}
              onviewportchange={handleViewportChange}
              onsearchtext={handleSearchTextSelection}
              jumpRequest={viewportJumpRequest}
              searchHits={searchOverlay.hits}
              activeSearchHitIdx={searchOverlay.activeIdx}
              navStore={activeTab.navStore}
              onfollowlink={followTarget}
            />
          {/if}
        {/key}
      {:else}
        <div class="empty-state">
          <p>Open a PDF to begin</p>
          <button class="btn-primary" onclick={handleOpenFile} disabled={isOpening}>
            Open PDF
          </button>
        </div>
      {/if}
    </main>

    <!-- Right panel -->
    {#if !rightCollapsed}
      <aside class="panel panel-right">
        <Accordion
          title="Properties"
          storageKey="right-properties"
          bodyClass="panel-body panel-body-flush"
          testId="accordion-properties"
        >
          {#if activeTab}
            <PropertiesPanel store={activeTab.store} takeoffStore={activeTab.takeoffStore} currentPage={activeTab.viewportSnapshot.pageIndex} />
          {:else}
            <p class="panel-hint muted">Select a markup to edit its properties.</p>
          {/if}
        </Accordion>
      </aside>
    {/if}
  </div>

  <!-- Bottom panel - Markups List / Measurement quantities (spec §17) -->
  {#if !bottomCollapsed}
    <div class="bottom-panel" class:bottom-panel-tall={bottomTab === "markups"}>
      <div class="bottom-tabs" role="tablist" aria-label="Bottom panel">
        <button role="tab" class="bottom-tab" aria-selected={bottomTab === "markups"}
          class:active={bottomTab === "markups"} onclick={() => (bottomTab = "markups")}>Markups</button>
        <button role="tab" class="bottom-tab" aria-selected={bottomTab === "quantities"}
          class:active={bottomTab === "quantities"} onclick={() => (bottomTab = "quantities")}>Quantities</button>
      </div>
      <div class="bottom-body" data-testid="bottom-panel-body">
        {#if !activeTab}
          <p class="panel-hint muted">Open a PDF to see {bottomTab === "markups" ? "its mark-ups" : "quantities"}.</p>
        {:else if bottomTab === "markups"}
          {#key activeTab.docId}
            <MarkupsListPanel
              store={activeTab.store}
              docId={activeTab.docId}
              pageLabels={activeTab.navStore.labels}
              onjump={(t) => {
                searchJumpNonce += 1;
                viewportJumpRequest = { page: t.page, rect: t.rect, nonce: searchJumpNonce };
              }}
            />
          {/key}
        {:else}
          <Accordion
            title="Quantities"
            storageKey="bottom-panel"
            bodyClass="panel-body panel-body-flush"
            testId="accordion-bottom-panel"
          >
            <MeasurementPanel
              store={activeTab.store}
              takeoffStore={activeTab.takeoffStore}
              docId={activeTab.docId}
              currentPage={activeTab.viewportSnapshot.pageIndex}
            />
          </Accordion>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Save-prompt dialog — shown when closing a document with unsaved changes -->
  {#if savePromptDocId !== null}
    <SavePromptDialog
      filename={savePromptFilename}
      onSave={handleSavePromptSave}
      onDiscard={handleSavePromptDiscard}
      onCancel={handleSavePromptCancel}
    />
  {/if}

  <!-- Changed-on-disk recovery - shown when a save is refused because another program rewrote the PDF -->
  {#if staleDocId !== null}
    {@const staleTab = tabStore.tabs.find((t) => t.docId === staleDocId)}
    {#if staleTab}
      <StaleFileDialog
        filename={baseName(staleTab.doc.path)}
        canRebuild={staleCanRebuild}
        onRebuild={handleStaleRebuild}
        onReload={handleStaleReload}
        onCancel={() => (staleDocId = null)}
      />
    {/if}
  {/if}

  <!-- Password prompt dialog - shown when open_document reports an encrypted PDF -->
  {#if passwordPromptPath !== null}
    <PasswordPromptDialog
      filename={passwordPromptPath.split(/[\\/]/).at(-1) ?? passwordPromptPath}
      errorHint={passwordPromptError}
      onSubmit={handlePasswordSubmit}
      onCancel={handlePasswordCancel}
    />
  {/if}

  <!-- Flatten / Optimise / Apply Redactions confirm: what happens, copy vs replace -->
  {#if docOpPrompt !== null && activeTab}
    <DocOpDialog
      kind={docOpPrompt}
      path={activeTab.doc.path}
      onConfirm={handleDocOpConfirm}
      onCancel={() => (docOpPrompt = null)}
    />
  {/if}

  <!-- Window-close guard: one save prompt per tab with unsaved mark-ups -->
  {#if quitQueue.length > 0}
    {@const quitTab = tabStore.tabs.find((t) => t.docId === quitQueue[0])}
    {#if quitTab}
      <SavePromptDialog
        filename={baseName(quitTab.doc.path)}
        onSave={handleQuitSave}
        onDiscard={handleQuitDiscard}
        onCancel={handleQuitCancel}
      />
    {/if}
  {/if}

  <!-- Settings dialog -->
  {#if settingsOpen}
    <SettingsDialog
      onClose={() => (settingsOpen = false)}
      onLicenseChanged={(state) => {
        licenseState = state;
        void maybeInitializeAppContent(state);
      }}
    />
  {/if}

  <!-- About dialog -->
  {#if aboutOpen}
    <AboutDialog onClose={() => (aboutOpen = false)} />
  {/if}

  <!-- Extract pages dialog (PR-C, 2026-09-15) -->
  {#if extractPagesDialogIndices && activeTab}
    <ExtractPagesDialog
      docId={activeTab.docId}
      docPath={activeTab.doc.path}
      pageIndices={extractPagesDialogIndices}
      onClose={() => (extractPagesDialogIndices = null)}
      onExtracted={() => (extractPagesDialogIndices = null)}
    />
  {/if}

  <!-- Navigator dialogs (docs/navigation.md "Document structure") -->
  {#if numberPages && activeTab}
    <NumberPagesDialog
      pageCount={activeTab.doc.page_count}
      currentPage={activeTab.viewportSnapshot.pageIndex}
      selectedPages={numberPages.selected}
      onConfirm={(r) => void applyNumberPages(r)}
      onCancel={() => (numberPages = null)}
    />
  {/if}
  {#if activeTab?.navStore.pendingLink}
    <NavActionDialog
      title="New link"
      confirmLabel="Create link"
      pageCount={activeTab.doc.page_count}
      labels={activeTab.navStore.labels}
      currentPage={activeTab.navStore.pendingLink.page}
      onCancel={() => { if (activeTab) activeTab.navStore.pendingLink = null; }}
      onConfirm={(t) => void createLink(t)}
    />
  {/if}
  {#if editingLink && activeTab}
    <NavActionDialog
      title="Link action"
      pageCount={activeTab.doc.page_count}
      labels={activeTab.navStore.labels}
      currentPage={editingLink.page}
      initial={editingLink.target}
      onCancel={() => (editingLink = null)}
      onConfirm={(t) => void changeLinkTarget(t)}
    />
  {/if}
  {#if externalOpen}
    <ConfirmDialog
      title="Open this address?"
      message={externalOpen.uri}
      hint="This address comes from the PDF. Open it only if you trust it."
      confirmLabel="Open"
      cancelLabel="Cancel"
      onConfirm={() => void confirmExternalUri()}
      onCancel={() => (externalOpen = null)}
    />
  {/if}

  <!-- License grace-period warning: shown once per launch while running on
       an offline-expired token still inside the server's grace window (see
       maybeInitializeAppContent / checkInIfActivated above). Non-blocking -
       app content renders behind it. -->
  {#if licenseState.state === "grace" && !graceWarningDismissed}
    <LicenseGraceWarning
      state={licenseState}
      onOpenSettings={() => {
        graceWarningDismissed = true;
        settingsOpen = true;
      }}
      onDismiss={() => (graceWarningDismissed = true)}
    />
  {/if}

  <UpdateNotification />
</div>
{/if}

<!-- Crash-guard: mounted unconditionally, outside the license gate, so an uncaught
     error during license checks / activation is caught too, not just post-activation. -->
<ErrorBanner />

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--color-bg);
    color: var(--color-text);
  }

  /* --- Toolbar --- */
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--toolbar-height);
    padding: 0 var(--space-3);
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
    gap: var(--space-3);
  }
  .toolbar-left, .toolbar-right {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .toolbar-group {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .toolbar-sep {
    width: 1px;
    height: 20px;
    background: var(--color-border);
    margin: 0 var(--space-1);
  }
  .sidebar-icon {
    display: block;
  }
  .compare-pick {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
  }
  .unprotected-offer {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .license-checking-text {
    margin: 0;
    padding: var(--space-5);
    color: var(--color-text-secondary);
    font-size: var(--font-size-sm);
  }
  .app-name {
    font-weight: 600;
    font-size: var(--font-size-base);
    color: var(--color-primary);
    margin-right: var(--space-2);
  }
  .btn-icon-active {
    background: var(--color-bg-active);
    border-color: var(--color-primary);
  }

  /* --- Buttons --- */
  .btn-toolbar {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-3);
    transition: background 120ms;
  }
  .btn-toolbar:hover:not(:disabled) { background: var(--color-bg-hover); }
  .btn-toolbar:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-toolbar.btn-icon { padding: var(--space-1) var(--space-2); }
  .btn-toolbar.btn-compare-toggle {
    border-color: var(--color-primary, #2563eb);
    color: var(--color-primary, #2563eb);
  }
  .btn-toolbar.btn-compare-toggle:hover {
    background: var(--color-primary-surface, #eff6ff);
  }
  .toolbar-select {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-2);
  }
  .toolbar-select:disabled { opacity: 0.5; cursor: not-allowed; }

  /* --- Compare bar (M6) --- */
  .compare-bar {
    flex-shrink: 0;
    border-bottom: 1px solid var(--color-border, #e5e7eb);
    background: var(--color-surface-raised, #f9fafb);
    display: flex;
    flex-direction: column;
    max-height: 420px;
    overflow: hidden;
  }
  .compare-bar-pickers {
    display: flex;
    gap: var(--space-2, 6px);
    padding: var(--space-2, 6px) var(--space-3, 8px);
    border-bottom: 1px solid var(--color-border, #e5e7eb);
  }

  .btn-primary {
    background: var(--color-primary);
    border: none;
    border-radius: var(--radius-md);
    color: var(--color-text-inverse);
    cursor: pointer;
    font-size: var(--font-size-base);
    font-weight: 600;
    padding: var(--space-2) var(--space-5);
    transition: background 120ms;
  }
  .btn-primary:hover:not(:disabled) { background: var(--color-primary-hover); }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  /* --- Error banner --- */
  .error-banner {
    background: var(--color-danger);
    color: #fff;
    font-size: var(--font-size-sm);
    padding: var(--space-2) var(--space-4);
    flex-shrink: 0;
  }

  /* DocOps (Flatten/Optimize/Redact) success feedback - these actions have no other
     visible confirmation, so a distinct banner reports what actually happened (see
     docOpsStatus / docops-handlers.ts). */
  .docops-status-banner {
    background: var(--color-success, #3ba55d);
    color: #fff;
    font-size: var(--font-size-sm);
    padding: var(--space-2) var(--space-4);
    flex-shrink: 0;
  }

  /* --- Body row (3 columns) --- */
  .body-row {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  /* --- Panels --- */
  .panel {
    background: var(--color-bg-panel);
    border-right: 1px solid var(--color-border);
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    overflow: hidden;
  }
  /* Fixed sidebar width regardless of which panel(s) are showing (owner defect fix,
     2026-09-08) - width is a property of the sidebar, not of the panel. */
  .panel-left  { width: var(--panel-left-width); }
  .panel-right { width: var(--panel-right-width); border-right: none; border-left: 1px solid var(--color-border); }

  .panel-header {
    font-size: var(--font-size-xs);
    font-weight: 600;
    color: var(--color-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  /* .panel-body / .panel-body-flush moved to src/lib/styles.css (global) -
     Accordion.svelte applies them via its `bodyClass` prop to an element IT
     creates, so a component-scoped rule here would not match (Svelte's
     scoped-CSS hash only reaches elements the OWNING component's own
     template renders directly). See Accordion.svelte's header comment. */
  .panel-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    margin: 0 0 var(--space-2);
  }
  .panel-hint.muted { color: var(--color-text-muted); }

  /* --- Left sidebar panels (search + history + tool chest + navigator, stacked) --- */
  .panel-section {
    display: flex;
    flex-direction: column;
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  /* Recent Documents gets more room than Tool Chest/Navigator, but never all of it -
     keyed off its own class (not :first-child) because Search can render above it. */
  .panel-section--recent {
    flex: 1;
    overflow: hidden;
    max-height: 55%;
  }
  .panel-section--secondary {
    flex: 1;
    overflow: hidden;
  }
  /* Bookmarks and Links size to their content (a collapsed one is just its header) up to
     a share of the sidebar, then scroll inside - they must not each claim an equal slice
     of the column like the other stacked panels do. */
  .panel-section.panel-section--nav {
    flex: 0 1 auto;
    min-height: 0;
    max-height: 45%;
    overflow: hidden;
  }
  /* Search stacks ABOVE Recent Documents/Tool Chest/Navigator (owner defect fix,
     2026-09-08 - see the panel-section--search comment in the template), not instead
     of them - bounded to a fraction of the sidebar so the rest of the stack stays
     visible/reachable below it. Its own result list (.search-groups, SearchPanel.svelte)
     already scrolls internally within this box. */
  .panel-section.panel-section--search {
    flex: 1;
    max-height: 45%;
    overflow: hidden;
  }
  .panel-section--search .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .btn-icon-close {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: inherit;
    line-height: 1;
    padding: 0;
  }
  .btn-icon-close:hover {
    color: var(--color-text);
  }

  /* --- Viewport container --- */
  .viewport-container {
    flex: 1;
    overflow: hidden;
    background: var(--color-bg);
    position: relative;
  }

  /* --- Empty state --- */
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: var(--space-4);
    color: var(--color-text-muted);
  }
  .empty-state p { font-size: var(--font-size-lg); margin: 0; }

  /* --- Bottom panel --- */
  .bottom-panel.bottom-panel-tall {
    height: 300px;
  }

  .bottom-tabs {
    display: flex;
    gap: var(--space-1);
    padding: 0 var(--space-2);
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }

  .bottom-tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--color-text-secondary);
    padding: var(--space-1) var(--space-3);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }

  .bottom-tab.active {
    color: var(--color-text);
    border-bottom-color: var(--color-primary);
  }

  .bottom-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .bottom-panel {
    height: var(--bottom-panel-height);
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-panel);
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }
</style>
