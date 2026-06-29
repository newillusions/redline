<script lang="ts">
  /**
   * App root — 3-column dockable layout (spec §17).
   *
   * Layout:
   *   ┌──────────────────────────────────────────────┐
   *   │  Toolbar (top, full width)                   │
   *   ├─────────────┬──────────────────┬─────────────┤
   *   │  Left panel │  Viewport (PDF)  │ Right panel │
   *   │  (collapsible)│               │ (collapsible)│
   *   ├─────────────┴──────────────────┴─────────────┤
   *   │  Bottom panel (Markups/Comments list)         │
   *   │  (collapsible)                                │
   *   └──────────────────────────────────────────────┘
   *
   * M1: static layout + real PDF viewport. Full drag-rearrange (dockview-core)
   * lands in M2 once the layout proves stable.
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
  import { openDocument, closeDocument, loadMarkups, listScales, saveDocument, saveDocumentAs, addMarkup, updateMarkup, deleteMarkup, flattenDocument, optimizeDocument, redactDocument } from "$lib/ipc";
  import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import type { DocumentInfo } from "$lib/ipc";
  import { MarkupStore } from "$lib/markup-store.svelte";
  import { TakeoffStore } from "$lib/takeoff-store.svelte";

  // --- App state ---
  let currentDoc = $state<DocumentInfo | null>(null);
  let store = $state<MarkupStore | null>(null);
  let takeoffStore = $state<TakeoffStore | null>(null);
  let openError = $state<string | null>(null);
  let isOpening = $state(false);
  let isSaving = $state(false);
  let isFlattening = $state(false);
  let isOptimizing = $state(false);
  let isRedacting = $state(false);

  // --- Compare panel state (M6 Phase 1.1) ---
  let compareVisible = $state(false);
  let comparePathA = $state("");
  let comparePathB = $state("");

  // Cleanup handle for the Tauri drag-drop listener (Fix 4: file drop to open).
  let _dropUnlisten: (() => void) | undefined;

  // --- Auto-open for the §20 GUI smoke / floor-machine runbook ---
  // If the backend reports REDLINE_OPEN_PDF (env var read in Rust), open it on
  // startup without the file dialog. Lets `cargo tauri dev` launch straight into a
  // corpus file for scripted/repeatable bench runs.
  async function autoOpenIfRequested() {
    try {
      const path = await invoke<string | null>("auto_open_path");
      if (path) {
        const doc = await openDocument(path);
        store = new MarkupStore(doc.doc_id, { add: addMarkup, update: updateMarkup, remove: deleteMarkup });
        takeoffStore = new TakeoffStore();
        currentDoc = doc;
        loadMarkups(doc.doc_id)
          .then((m) => { store?.seed(m); })
          .catch((e) => { openError = `Load markups failed: ${e}`; });
        listScales(doc.doc_id)
          .then((scales) => { takeoffStore?.seedScales(scales); })
          .catch(() => {}); // scales are non-critical; fail silently
      }
    } catch (e) {
      openError = `auto-open failed: ${String(e)}`;
    }
  }

  onMount(async () => {
    await autoOpenIfRequested();
    // Fix 4: file drop opens a PDF exactly like File>Open.
    // Ignore non-PDF drops and honour the single-document model (drop replaces current doc).
    _dropUnlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
      if (event.payload.type !== "drop") return;
      const pdfs = (event.payload.paths as string[]).filter((p) =>
        p.toLowerCase().endsWith(".pdf"),
      );
      if (pdfs.length === 0) return;
      if (isOpening) return;
      await openFilePath(pdfs[0]);
    });
  });

  onDestroy(() => { _dropUnlisten?.(); });

  // Panel collapse state
  let leftCollapsed  = $state(false);
  let rightCollapsed = $state(false);
  let bottomCollapsed = $state(true);

  // --- Actions ---

  /**
   * Core open logic shared by File>Open dialog and file-drop (Fix 4).
   * Closes the current document first (single-document app), then opens the given path.
   */
  async function openFilePath(path: string) {
    openError = null;
    isOpening = true;
    try {
      if (currentDoc) {
        await closeDocument(currentDoc.doc_id);
        currentDoc = null;
      }
      const doc = await openDocument(path);
      store = new MarkupStore(doc.doc_id, { add: addMarkup, update: updateMarkup, remove: deleteMarkup });
      takeoffStore = new TakeoffStore();
      currentDoc = doc;
      loadMarkups(doc.doc_id)
        .then((m) => { store?.seed(m); })
        .catch((e) => { openError = `Load markups failed: ${e}`; });
      listScales(doc.doc_id)
        .then((scales) => { takeoffStore?.seedScales(scales); })
        .catch(() => {}); // scales are non-critical; fail silently
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
      multiple: false,
    });
    if (!selected || Array.isArray(selected)) return;
    await openFilePath(selected as string);
  }

  // --- Save handlers ---
  async function handleSave() {
    if (!currentDoc || isSaving) return;
    openError = null;
    isSaving = true;
    try {
      await store?.flush();
      await saveDocument(currentDoc.doc_id);
    } catch (e) {
      openError = `Save failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  async function handleSaveAs() {
    if (!currentDoc || isSaving) return;
    openError = null;
    const dest = await saveDialog({ filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (!dest) return;
    isSaving = true;
    try {
      await store?.flush();
      await saveDocumentAs(currentDoc.doc_id, dest);
      currentDoc = { ...currentDoc, path: dest };
    } catch (e) {
      openError = `Save As failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isSaving = false;
    }
  }

  // --- DocOps handlers (M5) ---
  async function handleFlatten() {
    if (!currentDoc || isFlattening) return;
    openError = null;
    isFlattening = true;
    try {
      // Flush any unsaved in-memory markups first so the flatten sees current annotations.
      await store?.flush();
      await flattenDocument(currentDoc.doc_id);
    } catch (e) {
      openError = `Flatten failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isFlattening = false;
    }
  }

  async function handleOptimize() {
    if (!currentDoc || isOptimizing) return;
    openError = null;
    isOptimizing = true;
    try {
      // Flush pending markups so the optimizer operates on the current annotation state.
      await store?.flush();
      await optimizeDocument(currentDoc.doc_id);
    } catch (e) {
      openError = `Optimize failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isOptimizing = false;
    }
  }

  async function handleRedact() {
    if (!currentDoc || isRedacting) return;
    openError = null;
    isRedacting = true;
    try {
      // Flush any unsaved in-memory markups first (e.g. pending Redact annotations).
      await store?.flush();
      // Apply: (1) any explicit regions (none from the toolbar — caller passes [])
      //        (2) all /Subtype /Redact annotations on every page.
      await redactDocument(currentDoc.doc_id, [], true);
    } catch (e) {
      openError = `Redact failed: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      isRedacting = false;
    }
  }

  // --- Compare handlers (M6 Phase 1.1) ---
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

  // --- Keyboard shortcuts ---
  function handleKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s" && !e.shiftKey) {
      e.preventDefault();
      handleSave();
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
      <button class="btn-toolbar" onclick={handleSave} disabled={!currentDoc || isSaving} title="Save (Cmd/Ctrl+S)">
        {isSaving ? "Saving…" : "Save"}
      </button>
      <button class="btn-toolbar" onclick={handleSaveAs} disabled={!currentDoc || isSaving} title="Save As…">
        Save As…
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleFlatten}
        disabled={!currentDoc || isFlattening || isSaving}
        title="Flatten — bake annotation appearances into page content (irreversible)"
      >
        {isFlattening ? "Flattening…" : "Flatten"}
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleOptimize}
        disabled={!currentDoc || isOptimizing || isSaving}
        title="Optimize — remove unused objects and compress streams to reduce file size"
      >
        {isOptimizing ? "Optimizing…" : "Optimize"}
      </button>
      <button
        class="btn-toolbar btn-docops"
        onclick={handleRedact}
        disabled={!currentDoc || isRedacting || isSaving}
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
      {#if currentDoc}
        <span class="doc-name">{currentDoc.path.split(/[\\/]/).at(-1)}</span>
        <span class="doc-pages">{currentDoc.page_count} pages</span>
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

  <!-- Compare panel — collapsible, below toolbar (M6 Phase 1.1, spec §10) -->
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

  {#if store}
    <ToolPalette {store} />
  {/if}

  {#if openError}
    <div class="error-banner">{openError}</div>
  {/if}

  <!-- Main 3-column body -->
  <div class="body-row">
    <!-- Left panel -->
    {#if !leftCollapsed}
      <aside class="panel panel-left">
        <div class="panel-header">Navigator</div>
        <div class="panel-body">
          {#if currentDoc}
            <p class="panel-hint">Thumbnails · Bookmarks · Layers</p>
            <p class="panel-hint muted">(M4)</p>
          {:else}
            <p class="panel-hint muted">Open a PDF to begin.</p>
          {/if}
        </div>
      </aside>
    {/if}

    <!-- Centre viewport -->
    <main class="viewport-container">
      {#if currentDoc && store && takeoffStore}
        <Viewport docInfo={currentDoc} {store} {takeoffStore} />
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
          {#if store}
            <PropertiesPanel {store} />
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
        {#if currentDoc && store && takeoffStore}
          Takeoff — Quantities
        {:else}
          Markups / Comments
        {/if}
      </div>
      <div class="panel-body panel-body-flush">
        {#if currentDoc && store && takeoffStore}
          <MeasurementPanel {store} {takeoffStore} docId={currentDoc.doc_id} />
        {:else}
          <p class="panel-hint muted">Open a PDF to see measurements.</p>
        {/if}
      </div>
    </div>
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
  .doc-name {
    font-size: var(--font-size-sm);
    color: var(--color-text);
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  /* DocOps buttons use a muted warning tint to signal an irreversible operation. */
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
