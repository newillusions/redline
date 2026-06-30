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
  import { onMount, onDestroy } from "svelte";
  import Viewport from "./components/Viewport.svelte";
  import ToolPalette from "./components/ToolPalette.svelte";
  import PropertiesPanel from "./components/PropertiesPanel.svelte";
  import MeasurementPanel from "./components/MeasurementPanel.svelte";
  import ComparePanel from "./components/ComparePanel.svelte";
  import TabBar from "./components/TabBar.svelte";
  import SavePromptDialog from "./components/SavePromptDialog.svelte";
  import { openDocument, closeDocument, loadMarkups, listScales, saveDocument, saveDocumentAs, addMarkup, updateMarkup, deleteMarkup, flattenDocument, optimizeDocument, redactDocument } from "$lib/ipc";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import type { DocumentInfo } from "$lib/ipc";
  import { MarkupStore } from "$lib/markup-store.svelte";
  import { TakeoffStore } from "$lib/takeoff-store.svelte";
  import { DocTabStore } from "$lib/doc-tabs.svelte";
  import type { ViewportSnapshot } from "$lib/viewport";
  import DocumentHistoryPanel from "./components/DocumentHistoryPanel.svelte";
  import { loadRecentDocs, saveRecentDocs, upsertMru } from "$lib/recent-docs";
  import type { RecentDoc } from "$lib/recent-docs";

  // ---------------------------------------------------------------------------
  // Multi-doc state
  // ---------------------------------------------------------------------------
  const tabStore = new DocTabStore();

  /** Convenience alias for the currently active tab (null when no docs open). */
  const activeTab = $derived(tabStore.activeTab);

  // Per-operation busy flags (apply to the active tab's document).
  let openError = $state<string | null>(null);
  let isOpening = $state(false);
  let isSaving = $state(false);
  let isFlattening = $state(false);
  let isOptimizing = $state(false);
  let isRedacting = $state(false);

  // --- Save-prompt dialog state ---
  /** docId of the document awaiting save/discard/cancel decision; null when dialog is hidden. */
  let savePromptDocId = $state<string | null>(null);
  let savePromptFilename = $state("");

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

  onMount(async () => {
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
  });

  onDestroy(() => { _dropUnlisten?.(); });

  // Panel collapse state
  let leftCollapsed  = $state(false);
  let rightCollapsed = $state(false);
  let bottomCollapsed = $state(true);

  // ---------------------------------------------------------------------------
  // Open flow — dedup by path, new tab per file
  // ---------------------------------------------------------------------------

  /**
   * Core open logic shared by File>Open dialog, file-drop, and auto-open.
   * - If the path is already open, switch to its tab (dedup).
   * - Otherwise open a new PDFium document, create a tab, and activate it.
   */
  async function openFilePath(path: string) {
    // Dedup: if this path is already open, just switch to it.
    const existing = tabStore.findByPath(path);
    if (existing) {
      tabStore.switchTab(existing.docId);
      return;
    }

    openError = null;
    isOpening = true;
    try {
      const doc: DocumentInfo = await openDocument(path);
      const store = new MarkupStore(doc.doc_id, {
        add: addMarkup,
        update: updateMarkup,
        remove: deleteMarkup,
      });
      const ts = new TakeoffStore();
      tabStore.addTab(doc, store, ts);

      // Record successful open in the MRU history.
      void recordRecentDoc(doc);

      // Load markups and scales asynchronously (non-blocking).
      loadMarkups(doc.doc_id)
        .then((m) => { store.seed(m); })
        .catch((e) => { openError = `Load markups failed: ${e}`; });
      listScales(doc.doc_id)
        .then((scales) => { ts.seedScales(scales); })
        .catch(() => {}); // scales are non-critical
    } catch (e) {
      openError = String(e);
    } finally {
      isOpening = false;
    }
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
    isFlattening = true;
    try {
      await activeTab.store.flush();
      await flattenDocument(activeTab.docId);
    } catch (e) {
      openError = `Flatten failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isFlattening = false;
    }
  }

  async function handleOptimize() {
    if (!activeTab || isOptimizing) return;
    openError = null;
    isOptimizing = true;
    try {
      await activeTab.store.flush();
      await optimizeDocument(activeTab.docId);
    } catch (e) {
      openError = `Optimize failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isOptimizing = false;
    }
  }

  async function handleRedact() {
    if (!activeTab || isRedacting) return;
    openError = null;
    isRedacting = true;
    try {
      await activeTab.store.flush();
      await redactDocument(activeTab.docId, [], true);
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

<div class="app-shell">
  <!-- Toolbar -->
  <header class="toolbar">
    <div class="toolbar-left">
      <span class="app-name">Redline</span>
      <button class="btn-toolbar" onclick={handleOpenFile} disabled={isOpening}>
        {isOpening ? "Opening…" : "Open PDF"}
      </button>
      <button class="btn-toolbar" onclick={handleSave} disabled={!activeTab || isSaving} title="Save (Cmd/Ctrl+S)">
        {isSaving ? "Saving…" : "Save"}
      </button>
      <button class="btn-toolbar" onclick={handleSaveAs} disabled={!activeTab || isSaving} title="Save As…">
        Save As…
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleFlatten}
        disabled={!activeTab || isFlattening || isSaving}
        title="Flatten — bake annotation appearances into page content (irreversible)"
      >
        {isFlattening ? "Flattening…" : "Flatten"}
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleOptimize}
        disabled={!activeTab || isOptimizing || isSaving}
        title="Optimize — remove unused objects and compress streams to reduce file size"
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

  <!-- Main 3-column body -->
  <div class="body-row">
    <!-- Left panel -->
    {#if !leftCollapsed}
      <aside class="panel panel-left">
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
        <!-- Navigator placeholder (M4 — thumbnails/bookmarks/layers) -->
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
</div>

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
