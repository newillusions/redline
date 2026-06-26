<script lang="ts">
  /**
   * SearchPanel — in-document text search (M4 S3) + folder full-text search
   * (M4 S4, spec §4).
   *
   * Two modes:
   *   "document" — searches the open PDF via PDFium text extraction (S3).
   *   "folder"   — searches the Tantivy folder index (S4); only available when
   *                `folderPath` prop is provided.
   *
   * Props:
   *   docId       — open document id (required; used in document mode)
   *   pageCount   — total pages in the open document (document mode)
   *   folderPath  — optional; if provided a mode toggle appears
   *   onHits      — document-mode callback: receives SearchHit[]
   *   onJump      — document-mode result click: (page, hitIndex)
   *   onFolderHits — folder-mode callback: receives FolderSearchHit[]
   *   onFolderJump — folder-mode result click: (filePath, pageNumber)
   */
  import {
    searchDocument,
    searchFolder,
    type SearchHit,
    type FolderSearchHit,
  } from "$lib/ipc";

  interface Props {
    docId: string;
    pageCount: number;
    /** If provided, the Folder mode tab is shown. */
    folderPath?: string;
    onHits?: (hits: SearchHit[]) => void;
    onJump?: (page: number, hitIndex: number) => void;
    onFolderHits?: (hits: FolderSearchHit[]) => void;
    onFolderJump?: (filePath: string, pageNumber: number) => void;
  }

  const {
    docId,
    pageCount,
    folderPath,
    onHits,
    onJump,
    onFolderHits,
    onFolderJump,
  }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  type Mode = "document" | "folder";
  let mode = $state<Mode>("document");

  let query = $state("");
  let caseSensitive = $state(false);
  let wholeWord = $state(false);

  let hits = $state<SearchHit[]>([]);
  let folderHits = $state<FolderSearchHit[]>([]);
  let searching = $state(false);
  let error = $state<string | null>(null);
  let activeHitIdx = $state<number | null>(null);

  // ---------------------------------------------------------------------------
  // Search
  // ---------------------------------------------------------------------------

  async function runSearch() {
    const q = query.trim();
    if (!q) {
      clearResults();
      return;
    }

    searching = true;
    error = null;
    activeHitIdx = null;

    try {
      if (mode === "folder") {
        const results = await searchFolder(q);
        folderHits = results;
        onFolderHits?.(results);
      } else {
        const results = await searchDocument(docId, q, caseSensitive, wholeWord);
        hits = results;
        onHits?.(results);
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      hits = [];
      folderHits = [];
      if (mode === "folder") onFolderHits?.([]);
      else onHits?.([]);
    } finally {
      searching = false;
    }
  }

  function clearResults() {
    hits = [];
    folderHits = [];
    error = null;
    activeHitIdx = null;
    onHits?.([]);
    onFolderHits?.([]);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      runSearch();
    } else if (e.key === "Escape") {
      query = "";
      clearResults();
    }
  }

  function jumpTo(idx: number) {
    activeHitIdx = idx;
    const hit = hits[idx];
    if (hit) {
      onJump?.(hit.page, idx);
    }
  }

  function jumpToFolder(idx: number) {
    activeHitIdx = idx;
    const hit = folderHits[idx];
    if (hit) {
      onFolderJump?.(hit.file_path, hit.page_number);
    }
  }

  function switchMode(next: Mode) {
    mode = next;
    clearResults();
  }

  // Derived helpers — document mode
  const pageHitCounts = $derived(
    hits.reduce<Record<number, number>>((acc, h) => {
      acc[h.page] = (acc[h.page] ?? 0) + 1;
      return acc;
    }, {})
  );
  const pagesWithHits = $derived(Object.keys(pageHitCounts).length);

  // Derived helpers — folder mode
  const fileHitCounts = $derived(
    folderHits.reduce<Record<string, number>>((acc, h) => {
      acc[h.file_path] = (acc[h.file_path] ?? 0) + 1;
      return acc;
    }, {})
  );
  const filesWithHits = $derived(Object.keys(fileHitCounts).length);

  const activeHits = $derived(mode === "folder" ? folderHits : hits);
</script>

<div class="search-panel" role="search" aria-label="Search">

  <!-- Mode tabs (only shown when folderPath is provided) -->
  {#if folderPath}
    <div class="search-tabs" role="tablist" aria-label="Search mode">
      <button
        class="search-tab"
        class:active={mode === "document"}
        role="tab"
        aria-selected={mode === "document"}
        onclick={() => switchMode("document")}
      >Doc</button>
      <button
        class="search-tab"
        class:active={mode === "folder"}
        role="tab"
        aria-selected={mode === "folder"}
        data-testid="folder-tab"
        onclick={() => switchMode("folder")}
      >Folder</button>
    </div>
  {/if}

  <!-- Query input row -->
  <div class="search-input-row">
    <input
      class="search-input"
      type="search"
      placeholder={mode === "folder" ? "Search folder…" : "Search document…"}
      bind:value={query}
      onkeydown={handleKeydown}
      aria-label="Search query"
      disabled={searching}
    />
    <button
      class="search-btn"
      onclick={runSearch}
      disabled={searching || !query.trim()}
      aria-label="Find"
    >
      {searching ? "…" : "Find"}
    </button>
    {#if activeHits.length > 0 || error}
      <button
        class="search-clear"
        onclick={() => { query = ""; clearResults(); }}
        aria-label="Clear search"
      >
        ✕
      </button>
    {/if}
  </div>

  <!-- Options row (document mode only) -->
  {#if mode === "document"}
    <div class="search-options">
      <label class="search-opt">
        <input type="checkbox" bind:checked={caseSensitive} /> Aa
      </label>
      <label class="search-opt">
        <input type="checkbox" bind:checked={wholeWord} /> Word
      </label>
    </div>
  {/if}

  <!-- Status / summary -->
  {#if error}
    <div class="search-error" role="alert">{error}</div>
  {:else if searching}
    <div class="search-status">
      {mode === "folder" ? "Searching folder…" : `Searching ${pageCount} pages…`}
    </div>
  {:else if query.trim() && activeHits.length === 0}
    <div class="search-status">No results</div>
  {:else if mode === "document" && hits.length > 0}
    <div class="search-summary">
      {hits.length} result{hits.length !== 1 ? "s" : ""} on {pagesWithHits} page{pagesWithHits !== 1 ? "s" : ""}
    </div>
  {:else if mode === "folder" && folderHits.length > 0}
    <div class="search-summary">
      {folderHits.length} result{folderHits.length !== 1 ? "s" : ""} across {filesWithHits} file{filesWithHits !== 1 ? "s" : ""}
    </div>
  {/if}

  <!-- Document mode result list -->
  {#if mode === "document" && hits.length > 0}
    <ol class="search-results" aria-label="Search results">
      {#each hits as hit, idx (idx)}
        <li
          class="search-result"
          class:active={activeHitIdx === idx}
          role="option"
          aria-selected={activeHitIdx === idx}
          onclick={() => jumpTo(idx)}
          onkeydown={(e) => e.key === "Enter" && jumpTo(idx)}
          tabindex="0"
        >
          <span class="search-result-page">p.{hit.page + 1}</span>
          <span class="search-result-snippet">{hit.snippet}</span>
        </li>
      {/each}
    </ol>
  {/if}

  <!-- Folder mode result list -->
  {#if mode === "folder" && folderHits.length > 0}
    <ol class="search-results" aria-label="Folder search results">
      {#each folderHits as hit, idx (idx)}
        <li
          class="search-result search-result--folder"
          class:active={activeHitIdx === idx}
          role="option"
          aria-selected={activeHitIdx === idx}
          onclick={() => jumpToFolder(idx)}
          onkeydown={(e) => e.key === "Enter" && jumpToFolder(idx)}
          tabindex="0"
        >
          <span class="search-result-file">{hit.file_path.split("/").pop()}</span>
          <span class="search-result-page">p.{hit.page_number}</span>
          <!-- Tantivy snippet HTML: only <b> tags, safe to render -->
          <span class="search-result-snippet">{@html hit.snippet}</span>
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .search-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2, 4px);
    padding: var(--space-3, 8px);
    background: var(--color-surface, #1e1e2e);
    color: var(--color-text, #cdd6f4);
    font-size: var(--text-sm, 12px);
    height: 100%;
    overflow: hidden;
  }

  .search-tabs {
    display: flex;
    gap: var(--space-1, 2px);
    border-bottom: 1px solid var(--color-border, #45475a);
    padding-bottom: var(--space-1, 2px);
  }

  .search-tab {
    background: none;
    border: none;
    border-radius: var(--radius-sm, 3px) var(--radius-sm, 3px) 0 0;
    color: var(--color-text-muted, #6c7086);
    cursor: pointer;
    font-size: inherit;
    padding: var(--space-1, 2px) var(--space-3, 8px);
  }

  .search-tab:hover {
    color: var(--color-text, #cdd6f4);
  }

  .search-tab.active {
    color: var(--color-accent, #89b4fa);
    border-bottom: 2px solid var(--color-accent, #89b4fa);
  }

  .search-input-row {
    display: flex;
    gap: var(--space-2, 4px);
    align-items: center;
  }

  .search-input {
    flex: 1;
    min-width: 0;
    padding: var(--space-1, 2px) var(--space-2, 4px);
    background: var(--color-surface-raised, #313244);
    color: var(--color-text, #cdd6f4);
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    font-size: inherit;
  }

  .search-input:focus {
    outline: 2px solid var(--color-accent, #89b4fa);
    outline-offset: -1px;
  }

  .search-btn {
    padding: var(--space-1, 2px) var(--space-3, 8px);
    background: var(--color-accent, #89b4fa);
    color: var(--color-surface, #1e1e2e);
    border: none;
    border-radius: var(--radius-sm, 3px);
    cursor: pointer;
    font-size: inherit;
    white-space: nowrap;
  }

  .search-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .search-clear {
    background: none;
    border: none;
    color: var(--color-text-muted, #6c7086);
    cursor: pointer;
    padding: 0 var(--space-1, 2px);
    font-size: inherit;
  }

  .search-options {
    display: flex;
    gap: var(--space-3, 8px);
    align-items: center;
  }

  .search-opt {
    display: flex;
    gap: var(--space-1, 2px);
    align-items: center;
    cursor: pointer;
    user-select: none;
  }

  .search-status,
  .search-summary {
    color: var(--color-text-muted, #6c7086);
    font-size: var(--text-xs, 11px);
    padding: var(--space-1, 2px) 0;
  }

  .search-error {
    color: var(--color-error, #f38ba8);
    font-size: var(--text-xs, 11px);
  }

  .search-results {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    flex: 1;
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
  }

  .search-result {
    display: flex;
    gap: var(--space-2, 4px);
    align-items: baseline;
    padding: var(--space-2, 4px) var(--space-2, 4px);
    cursor: pointer;
    border-bottom: 1px solid var(--color-border, #45475a);
  }

  .search-result:last-child {
    border-bottom: none;
  }

  .search-result:hover,
  .search-result:focus {
    background: var(--color-surface-raised, #313244);
    outline: none;
  }

  .search-result.active {
    background: var(--color-accent-muted, #1e3a5f);
    border-left: 2px solid var(--color-accent, #89b4fa);
  }

  .search-result-page {
    color: var(--color-text-muted, #6c7086);
    font-size: var(--text-xs, 11px);
    flex-shrink: 0;
    min-width: 3em;
  }

  .search-result-file {
    color: var(--color-text-muted, #6c7086);
    font-size: var(--text-xs, 11px);
    flex-shrink: 0;
    max-width: 10em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .search-result-snippet {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }

  /* Tantivy <b> highlight in folder results */
  .search-result--folder .search-result-snippet :global(b) {
    color: var(--color-accent, #89b4fa);
    font-weight: 600;
  }
</style>
