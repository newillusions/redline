<script lang="ts">
  /**
   * SearchPanel — in-document text search UI (M4 S3, spec §4).
   *
   * Renders a query input + options + result list.  Calls `searchDocument`
   * IPC on submit; passes hits back to the Viewport via the `onHits` callback
   * so the Viewport can overlay highlight rects on the current page.
   *
   * Props:
   *   docId       — open document id
   *   pageCount   — total pages (for result context)
   *   onHits      — called with the full hit list whenever search runs
   *   onJump      — called when a result row is clicked: (page, hitIndex)
   */
  import { searchDocument, type SearchHit } from "$lib/ipc";

  interface Props {
    docId: string;
    pageCount: number;
    /** Callback: receives the full SearchHit array (or [] on clear/error). */
    onHits?: (hits: SearchHit[]) => void;
    /** Callback: jump the viewport to this page + hit index. */
    onJump?: (page: number, hitIndex: number) => void;
  }

  const { docId, pageCount, onHits, onJump }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let query = $state("");
  let caseSensitive = $state(false);
  let wholeWord = $state(false);

  let hits = $state<SearchHit[]>([]);
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
      const results = await searchDocument(docId, q, caseSensitive, wholeWord);
      hits = results;
      onHits?.(results);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      hits = [];
      onHits?.([]);
    } finally {
      searching = false;
    }
  }

  function clearResults() {
    hits = [];
    error = null;
    activeHitIdx = null;
    onHits?.([]);
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

  // Derived: group hit count by page for the summary line.
  const pageHitCounts = $derived(
    hits.reduce<Record<number, number>>((acc, h) => {
      acc[h.page] = (acc[h.page] ?? 0) + 1;
      return acc;
    }, {})
  );

  const pagesWithHits = $derived(Object.keys(pageHitCounts).length);
</script>

<div class="search-panel" role="search" aria-label="Document text search">
  <!-- Query input row -->
  <div class="search-input-row">
    <input
      class="search-input"
      type="search"
      placeholder="Search document…"
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
    {#if hits.length > 0 || error}
      <button
        class="search-clear"
        onclick={() => { query = ""; clearResults(); }}
        aria-label="Clear search"
      >
        ✕
      </button>
    {/if}
  </div>

  <!-- Options row -->
  <div class="search-options">
    <label class="search-opt">
      <input type="checkbox" bind:checked={caseSensitive} /> Aa
    </label>
    <label class="search-opt">
      <input type="checkbox" bind:checked={wholeWord} /> Word
    </label>
  </div>

  <!-- Status / summary -->
  {#if error}
    <div class="search-error" role="alert">{error}</div>
  {:else if searching}
    <div class="search-status">Searching {pageCount} pages…</div>
  {:else if query.trim() && hits.length === 0}
    <div class="search-status">No results</div>
  {:else if hits.length > 0}
    <div class="search-summary">
      {hits.length} result{hits.length !== 1 ? "s" : ""} on {pagesWithHits} page{pagesWithHits !== 1 ? "s" : ""}
    </div>
  {/if}

  <!-- Result list -->
  {#if hits.length > 0}
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

  .search-result-snippet {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
