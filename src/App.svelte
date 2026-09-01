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
  import ComparePanel from "./components/ComparePanel.svelte";
  import TabBar from "./components/TabBar.svelte";
  import SavePromptDialog from "./components/SavePromptDialog.svelte";
  import PasswordPromptDialog from "./components/PasswordPromptDialog.svelte";
  import ConfirmDialog from "./components/ConfirmDialog.svelte";
  import { openDocument, closeDocument, loadMarkups, listScales, saveDocument, saveDocumentAs, saveUnprotectedCopy, rememberPassword, addMarkup, updateMarkup, deleteMarkup, flattenDocument, optimizeDocument, redactDocument, ERR_PASSWORD_REQUIRED, ERR_WRONG_PASSWORD, searchDocument, searchFolder, searchPaths, openFolderIndex, getFolderIndexStatus, getUserIdentity } from "$lib/ipc";
  import type { IndexStatus } from "$lib/ipc";
  import SearchPanel from "./components/SearchPanel.svelte";
  import { SearchStore, computeViewportSearchOverlay, type UnifiedSearchHit, type SearchGroup, type DocSearchInput } from "$lib/search-store.svelte";
  import { createPasswordCache, getCachedPassword, setCachedPassword } from "$lib/password-cache";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import type { DocumentInfo, ImageQualityPreset } from "$lib/ipc";
  import { MarkupStore } from "$lib/markup-store.svelte";
  import { buildMarkup } from "$lib/markup-tools";
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
  import { ToolChestStore } from "$lib/toolchest-store.svelte";
  import ActivationGate from "./components/ActivationGate.svelte";
  import LicenseGraceWarning from "./components/LicenseGraceWarning.svelte";
  import { getLicenseStatus, checkInIfActivated, isUsable } from "$lib/license";
  import type { LicenseState } from "$lib/license";
  import UndoRedoControls from "./components/UndoRedoControls.svelte";
  import { resolveUndoRedoShortcut, resolveSearchShortcut } from "$lib/keyboard-shortcuts";
  import { runDocOpAndReseed, formatBytes } from "$lib/docops-handlers";

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
    { page: number; rect?: [number, number, number, number]; markupId?: string; nonce: number } | null
  >(null);
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
    docOpsStatus =
      skipped > 0
        ? `Highlighted ${applied} result${applied !== 1 ? "s" : ""}; skipped ${skipped} not currently open.`
        : `Highlighted ${applied} result${applied !== 1 ? "s" : ""}.`;
  }

  // Per-operation busy flags (apply to the active tab's document).
  let openError = $state<string | null>(null);
  let isOpening = $state(false);
  let isSaving = $state(false);
  let isFlattening = $state(false);
  let isOptimizing = $state(false);
  let isRedacting = $state(false);
  /** Compression/quality control for the Optimize action (spec §8 image downsampling) -
   *  Bluebeam-style preset: High (minimal loss) / Balanced (default) / Small (aggressive). */
  let imageQualityPreset = $state<ImageQualityPreset>("balanced");
  /** Transient success feedback for the DocOps actions (Flatten/Optimize/Redact) - these
   *  have no other visible confirmation (Optimize changes nothing on screen at all;
   *  Flatten/Redact bake content that was already visible). Without this, a successful
   *  no-op action and a real one look identical - see docops-handlers.ts's doc comment. */
  let docOpsStatus = $state<string | null>(null);

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

  // --- Remember-password prompt state (offered after a successful MANUAL
  //     password entry - never after a cache-reuse or known-list auto-try). ---
  let rememberPasswordPrompt = $state<{ docId: string; password: string } | null>(null);

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
    if (folderIndexPollTimer) clearInterval(folderIndexPollTimer);
  });

  // Panel collapse state
  let leftCollapsed  = $state(false);
  let rightCollapsed = $state(false);
  let bottomCollapsed = $state(true);

  // Settings dialog visibility
  let settingsOpen = $state(false);
  let aboutOpen = $state(false);

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
  async function openFilePath(path: string, password?: string) {
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

      // Encrypted-PDF follow-ups: offer to remember a just-typed password,
      // then (either way) offer to save an unprotected copy. `password` here
      // is this function's ARGUMENT (only set on a manual retry submitted
      // from PasswordPromptDialog) - cache-reuse and known-list auto-try
      // leave it undefined, so those skip straight to the copy offer.
      if (doc.was_encrypted) {
        if (password) {
          rememberPasswordPrompt = { docId: doc.doc_id, password };
        } else {
          unprotectedCopyPromptDocId = doc.doc_id;
        }
      }

      // Load markups and scales asynchronously (non-blocking).
      loadMarkups(doc.doc_id)
        .then((m) => { store.seed(m); })
        .catch((e) => { openError = `Load markups failed: ${e}`; });
      listScales(doc.doc_id)
        .then((scales) => { ts.seedScales(scales); })
        .catch(() => {}); // scales are non-critical
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
  async function handlePasswordSubmit(password: string) {
    const path = passwordPromptPath;
    if (!path) return;
    await openFilePath(path, password);
  }

  /** PasswordPromptDialog cancel handler: abandon the open attempt cleanly. */
  function handlePasswordCancel() {
    passwordPromptPath = null;
    passwordPromptError = null;
  }

  // ---------------------------------------------------------------------------
  // Encrypted-PDF follow-ups: remember-password prompt + save-unprotected-copy
  // (backlog #10/#9 - save-unprotected-copy + known-password-list).
  // ---------------------------------------------------------------------------

  /** Remember-password prompt confirmed: persist it, then offer the unprotected-copy save. */
  async function handleRememberPasswordConfirm() {
    const pending = rememberPasswordPrompt;
    rememberPasswordPrompt = null;
    if (!pending) return;
    try {
      await rememberPassword(pending.password);
    } catch (e) {
      openError = `Remember password failed: ${e instanceof Error ? e.message : String(e)}`;
    }
    unprotectedCopyPromptDocId = pending.docId;
  }

  /** Remember-password prompt declined: still offer the unprotected-copy save. */
  function handleRememberPasswordDecline() {
    const pending = rememberPasswordPrompt;
    rememberPasswordPrompt = null;
    if (pending) unprotectedCopyPromptDocId = pending.docId;
  }

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

  // ---------------------------------------------------------------------------
  // Save handlers (operate on the active tab)
  // ---------------------------------------------------------------------------

  async function handleSave() {
    if (!activeTab || isSaving) return;
    openError = null;
    isSaving = true;
    try {
      await activeTab.store.flush();
      await saveDocument(activeTab.docId);
      activeTab.store.clearDirty();
    } catch (e) {
      openError = `Save failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  async function handleSaveAs() {
    if (!activeTab || isSaving) return;
    openError = null;
    const dest = await saveDialog({ filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!dest) return;
    isSaving = true;
    try {
      await activeTab.store.flush();
      await saveDocumentAs(activeTab.docId, dest);
      activeTab.store.clearDirty();
      // Update the path in the active tab's doc record.
      tabStore.tabs = tabStore.tabs.map((t) =>
        t.docId === activeTab.docId
          ? { ...t, doc: { ...t.doc, path: dest } }
          : t,
      );
    } catch (e) {
      openError = `Save As failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  // ---------------------------------------------------------------------------
  // DocOps handlers (M5) — operate on the active tab
  // ---------------------------------------------------------------------------

  async function handleFlatten() {
    if (!activeTab || isFlattening) return;
    openError = null;
    docOpsStatus = null;
    isFlattening = true;
    const docId = activeTab.docId;
    const store = activeTab.store;
    try {
      const count = await runDocOpAndReseed(
        docId,
        store,
        { loadMarkups },
        () => flattenDocument(docId),
      );
      docOpsStatus =
        count === 0
          ? "Flatten: no annotations with an appearance found on this document - nothing to flatten."
          : `Flatten complete - ${count} annotation${count === 1 ? "" : "s"} baked into page content.`;
    } catch (e) {
      openError = `Flatten failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isFlattening = false;
    }
  }

  async function handleOptimize() {
    if (!activeTab || isOptimizing) return;
    openError = null;
    docOpsStatus = null;
    isOptimizing = true;
    const docId = activeTab.docId;
    const store = activeTab.store;
    try {
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
      docOpsStatus =
        saved > 0
          ? `Optimize complete - file size reduced from ${formatBytes(report.bytes_before)} to ${formatBytes(report.bytes_after)} (saved ${formatBytes(saved)})${imageNote}.`
          : `Optimize complete - the file was already optimal; no size reduction${imageNote}.`;
    } catch (e) {
      openError = `Optimize failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isOptimizing = false;
    }
  }

  async function handleRedact() {
    if (!activeTab || isRedacting) return;
    openError = null;
    docOpsStatus = null;
    isRedacting = true;
    const docId = activeTab.docId;
    const store = activeTab.store;
    try {
      await runDocOpAndReseed(
        docId,
        store,
        { loadMarkups },
        () => redactDocument(docId, [], true),
      );
      docOpsStatus = "Apply Redactions complete.";
    } catch (e) {
      openError = `Redact failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isRedacting = false;
    }
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

<svelte:window onkeydown={handleKeydown} />

{#if licenseState === null}
  <!-- Initial license check in flight - avoid a flash of the activation gate
       for the common case (already-activated install). -->
  <div class="app-shell license-checking"></div>
{:else if !isUsable(licenseState)}
  <ActivationGate licenseState={licenseState} onActivated={handleActivated} />
{:else}
<div class="app-shell">
  <!-- Toolbar -->
  <header class="toolbar">
    <div class="toolbar-left">
      <span class="app-name">Redline</span>
      <button class="btn-toolbar" onclick={handleOpenFile} disabled={isOpening}>
        {isOpening ? "Opening…" : "Open PDF"}
      </button>
      <button
        class="btn-toolbar"
        onclick={() => (searchPanelVisible ? (searchPanelVisible = false) : void openSearchPanel())}
        title="Find (Cmd/Ctrl+F)"
      >
        {searchPanelVisible ? "🔍 Find ▲" : "🔍 Find"}
      </button>
      <button class="btn-toolbar" onclick={handleSave} disabled={!activeTab || isSaving} title="Save (Cmd/Ctrl+S)">
        {isSaving ? "Saving…" : "Save"}
      </button>
      <button class="btn-toolbar" onclick={handleSaveAs} disabled={!activeTab || isSaving} title="Save As…">
        Save As…
      </button>
      <UndoRedoControls store={activeTab?.store ?? null} />
      <button
        class="btn-toolbar"
        onclick={handleSaveUnprotectedCopyMenuAction}
        disabled={!activeTab?.isEncrypted || isSavingUnprotectedCopy}
        title="Save a copy of this PDF with its password protection removed"
      >
        {isSavingUnprotectedCopy ? "Saving…" : "Save Unprotected Copy…"}
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleFlatten}
        disabled={!activeTab || isFlattening || isSaving}
        title="Flatten — bake annotation appearances into page content (irreversible)"
      >
        {isFlattening ? "Flattening…" : "Flatten"}
      </button>
      <select
        class="toolbar-select"
        bind:value={imageQualityPreset}
        disabled={!activeTab || isOptimizing || isSaving}
        title="Image quality for Optimize — trades file size against raster image quality (Bluebeam-style compression preset)"
        aria-label="Optimize image quality preset"
      >
        <option value="high">High quality</option>
        <option value="balanced">Balanced</option>
        <option value="small">Small file</option>
      </select>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleOptimize}
        disabled={!activeTab || isOptimizing || isSaving}
        title="Optimize — remove unused objects, compress streams, and recompress images to reduce file size"
      >
        {isOptimizing ? "Optimizing…" : "Optimize"}
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleRedact}
        disabled={!activeTab || isRedacting || isSaving}
        title="Apply Redactions — permanently cover all Redact-marked regions with solid-black overlays (irreversible)"
      >
        {isRedacting ? "Redacting…" : "Apply Redactions"}
      </button>
      <button
        class="btn-toolbar btn-compare-toggle"
        onclick={() => (compareVisible = !compareVisible)}
        title="Toggle compare panel — diff two PDF revisions (spec §10)"
      >
        {compareVisible ? "Compare ▲" : "Compare"}
      </button>
      {#if activeTab}
        <span class="doc-pages">{activeTab.doc.page_count} pages</span>
      {/if}
    </div>
    <div class="toolbar-right">
      <button
        class="btn-toolbar btn-icon"
        onclick={() => (leftCollapsed = !leftCollapsed)}
        title="Toggle left panel"
      >☰</button>
      <button
        class="btn-toolbar btn-icon"
        onclick={() => (rightCollapsed = !rightCollapsed)}
        title="Toggle right panel"
      >☰</button>
      <button
        class="btn-toolbar btn-icon"
        onclick={() => (bottomCollapsed = !bottomCollapsed)}
        title="Toggle markups list"
      >▼</button>
      <button
        class="btn-toolbar btn-icon"
        onclick={() => (settingsOpen = true)}
        title="Settings"
      >⚙</button>
      <button
        class="btn-toolbar btn-icon"
        onclick={() => (aboutOpen = true)}
        title="About Redline"
      >ⓘ</button>
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
        <button class="btn-toolbar" onclick={handlePickCompareA}>
          {comparePathA ? "A: " + comparePathA.split(/[\\/]/).at(-1) : "Pick File A (old)…"}
        </button>
        <button class="btn-toolbar" onclick={handlePickCompareB}>
          {comparePathB ? "B: " + comparePathB.split(/[\\/]/).at(-1) : "Pick File B (new)…"}
        </button>
      </div>
      <ComparePanel pathA={comparePathA} pathB={comparePathB} />
    </div>
  {/if}

  {#if activeTab}
    <ToolPalette store={activeTab.store} />
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

  {#if docOpsStatus}
    <div class="docops-status-banner">{docOpsStatus}</div>
  {/if}

  <!-- Main 3-column body -->
  <div class="body-row">
    <!-- Left panel -->
    {#if !leftCollapsed}
      <aside class="panel panel-left">
        <!-- Search (search-parity: current doc / open docs / folder+subfolders). Placed
             first — the toolbar Find button / Cmd-Ctrl-F is the primary way in. -->
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
              />
            </div>
          </div>
        {/if}
        <!-- Document History section (MRU list) -->
        <div class="panel-section">
          <div class="panel-header">Recent Documents</div>
          <div class="panel-body panel-body-flush">
            <DocumentHistoryPanel
              recentDocs={recentDocs}
              onOpen={openFilePath}
            />
          </div>
        </div>
        <!-- Tool Chest (spec "Tools & Tool Sets") - Tool Sets + Recent Tools; click a
             tool to make it active. Discoverable here regardless of which doc tab is
             active (Tool Sets are a workspace resource, not per-document). -->
        <div class="panel-section panel-section--secondary">
          <div class="panel-header">Tool Chest</div>
          <div class="panel-body panel-body-flush">
            <ToolChestPanel toolChest={toolChestStore} markupStore={activeTab?.store ?? null} />
          </div>
        </div>
        <!-- Navigator placeholder (M4 - thumbnails/bookmarks/layers) -->
        <div class="panel-section panel-section--secondary">
          <div class="panel-header">Navigator</div>
          <div class="panel-body">
            {#if activeTab}
              <p class="panel-hint">Thumbnails · Bookmarks · Layers</p>
              <p class="panel-hint muted">(M4)</p>
            {:else}
              <p class="panel-hint muted">Open a PDF to begin.</p>
            {/if}
          </div>
        </div>
      </aside>
    {/if}

    <!-- Centre viewport — only one Viewport mounted at a time -->
    <main class="viewport-container">
      {#if activeTab}
        <!-- Key forces Viewport to remount when switching tabs, so initialState
             (zoom/page/scroll snapshot) takes effect fresh for each tab. -->
        {#key activeTab.docId}
          <Viewport
            docInfo={activeTab.doc}
            store={activeTab.store}
            takeoffStore={activeTab.takeoffStore}
            initialState={activeTab.viewportSnapshot}
            onviewportchange={handleViewportChange}
            jumpRequest={viewportJumpRequest}
            searchHits={searchOverlay.hits}
            activeSearchHitIdx={searchOverlay.activeIdx}
          />
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
        <div class="panel-header">Properties</div>
        <div class="panel-body panel-body-flush">
          {#if activeTab}
            <PropertiesPanel store={activeTab.store} />
          {:else}
            <p class="panel-hint muted">Select a markup to edit its properties.</p>
          {/if}
        </div>
      </aside>
    {/if}
  </div>

  <!-- Bottom panel — Markups / Measurement quantities (spec §17) -->
  {#if !bottomCollapsed}
    <div class="bottom-panel">
      <div class="panel-header">
        {#if activeTab}
          Takeoff — Quantities
        {:else}
          Markups / Comments
        {/if}
      </div>
      <div class="panel-body panel-body-flush">
        {#if activeTab}
          <MeasurementPanel
            store={activeTab.store}
            takeoffStore={activeTab.takeoffStore}
            docId={activeTab.docId}
          />
        {:else}
          <p class="panel-hint muted">Open a PDF to see measurements.</p>
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

  <!-- Password prompt dialog - shown when open_document reports an encrypted PDF -->
  {#if passwordPromptPath !== null}
    <PasswordPromptDialog
      filename={passwordPromptPath.split(/[\\/]/).at(-1) ?? passwordPromptPath}
      errorHint={passwordPromptError}
      onSubmit={handlePasswordSubmit}
      onCancel={handlePasswordCancel}
    />
  {/if}

  <!-- Remember-password prompt - shown after a successful MANUAL password entry -->
  {#if rememberPasswordPrompt !== null}
    <ConfirmDialog
      title="Remember password?"
      message="Remember this password so future opens of encrypted PDFs try it automatically?"
      hint="Stored obfuscated on this device - not a secure credential store, but not plaintext either."
      confirmLabel="Remember"
      cancelLabel="Not now"
      onConfirm={handleRememberPasswordConfirm}
      onCancel={handleRememberPasswordDecline}
    />
  {/if}

  <!-- Save-unprotected-copy prompt - shown whenever an encrypted PDF finishes opening -->
  {#if unprotectedCopyPromptDocId !== null}
    <ConfirmDialog
      title="Save an unprotected copy?"
      message="This PDF is password-protected. Save a copy with no open password?"
      confirmLabel="Save Copy…"
      cancelLabel="Not now"
      onConfirm={handleUnprotectedCopyPromptConfirm}
      onCancel={handleUnprotectedCopyPromptCancel}
    />
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
  .app-name {
    font-weight: 600;
    font-size: var(--font-size-base);
    color: var(--color-primary);
    margin-right: var(--space-2);
  }
  .doc-pages {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
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
  .btn-toolbar.btn-docops {
    border-color: var(--color-warning, #b45309);
    color: var(--color-warning, #b45309);
  }
  .btn-toolbar.btn-docops:hover:not(:disabled) {
    background: var(--color-warning-surface, #fffbeb);
  }
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
  .panel-body {
    flex: 1;
    overflow-y: auto;
    padding: var(--space-3);
  }
  .panel-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    margin: 0 0 var(--space-2);
  }
  .panel-hint.muted { color: var(--color-text-muted); }

  /* --- Left panel sections (history + navigator stacked) --- */
  .panel-section {
    display: flex;
    flex-direction: column;
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  /* History panel gets more room; Navigator placeholder collapses to fit-content. */
  .panel-section:first-child {
    flex: 1;
    overflow: hidden;
    max-height: 55%;
  }
  .panel-section--secondary {
    flex: 1;
    overflow: hidden;
  }
  /* Search is the primary feature while open — give it more room than the
     default first-child 55% cap, and don't let other sections shrink it. */
  .panel-section.panel-section--search {
    flex: 2;
    max-height: 75%;
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
  .panel-body-flush {
    padding: 0;
    overflow-y: auto;
    flex: 1;
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
  .bottom-panel {
    height: var(--bottom-panel-height);
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-panel);
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }
</style>
