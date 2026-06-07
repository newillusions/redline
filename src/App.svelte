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
  import { onMount } from "svelte";
  import Viewport from "./components/Viewport.svelte";
  import { openDocument, closeDocument } from "$lib/ipc";
  import { open } from "@tauri-apps/plugin-dialog";
  import { invoke } from "@tauri-apps/api/core";
  import type { DocumentInfo } from "$lib/ipc";

  // --- App state ---
  let currentDoc = $state<DocumentInfo | null>(null);
  let openError = $state<string | null>(null);
  let isOpening = $state(false);

  // --- Auto-open for the §20 GUI smoke / floor-machine runbook ---
  // If the backend reports REDLINE_OPEN_PDF (env var read in Rust), open it on
  // startup without the file dialog. Lets `cargo tauri dev` launch straight into a
  // corpus file for scripted/repeatable bench runs.
  async function autoOpenIfRequested() {
    try {
      const path = await invoke<string | null>("auto_open_path");
      if (path) {
        currentDoc = await openDocument(path);
      }
    } catch (e) {
      openError = `auto-open failed: ${String(e)}`;
    }
  }

  onMount(autoOpenIfRequested);

  // Panel collapse state
  let leftCollapsed  = $state(false);
  let rightCollapsed = $state(false);
  let bottomCollapsed = $state(true);

  // --- Actions ---
  async function handleOpenFile() {
    openError = null;
    isOpening = true;
    try {
      const selected = await open({
        title: "Open PDF",
        filters: [{ name: "PDF Documents", extensions: ["pdf"] }],
        multiple: false,
      });

      if (!selected || Array.isArray(selected)) {
        isOpening = false;
        return;
      }

      // Close existing doc first
      if (currentDoc) {
        await closeDocument(currentDoc.doc_id);
        currentDoc = null;
      }

      currentDoc = await openDocument(selected as string);
    } catch (e) {
      openError = String(e);
    } finally {
      isOpening = false;
    }
  }
</script>

<div class="app-shell">
  <!-- Toolbar -->
  <header class="toolbar">
    <div class="toolbar-left">
      <span class="app-name">Redline</span>
      <button class="btn-toolbar" onclick={handleOpenFile} disabled={isOpening}>
        {isOpening ? "Opening…" : "Open PDF"}
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
      {#if currentDoc}
        <Viewport docInfo={currentDoc} />
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
        <div class="panel-body">
          <p class="panel-hint muted">Tool Chest · Markups (M2)</p>
        </div>
      </aside>
    {/if}
  </div>

  <!-- Bottom panel — Markups/Comments list (spec §17) -->
  {#if !bottomCollapsed}
    <div class="bottom-panel">
      <div class="panel-header">Markups / Comments</div>
      <div class="panel-body">
        <p class="panel-hint muted">Markup list with status/filter (M2)</p>
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
