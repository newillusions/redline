<script lang="ts">
  /**
   * SearchPanel — unified search UI (search-parity dispatch: match + improve on
   * Bluebeam's Find, verified against support.bluebeam.com/se/user-manual/menus/
   * window/search-panel.html and the official "How to Search PDFs" /
   * "How To Use Visual Search" tutorials). Five scopes (current document /
   * current page / all open documents / recents / folder+subfolders), results
   * as a sequential list grouped by file with collapsible groups + a
   * match-count header, click-to-navigate (App.svelte opens the file if it
   * isn't already open), a markup-text result kind alongside document text
   * (documented Bluebeam behavior, not our invention — see docs/
   * bluebeam-search-behavior-reference.md), and Bluebeam's "Check Options"
   * bulk-action pattern (check results, apply an action to all of them at
   * once) — this build ships Highlight Checked; Underline/Squiggly/Strike-
   * through/Hyperlink/Redact/Count/Replace are named follow-ups (new markup
   * types or content-mutation features each need their own design pass).
   *
   * All search orchestration (scope state, the grouped hit list, flat
   * next/prev navigation, scope persistence, checkbox selection) lives in
   * SearchStore — this component is presentation + input debouncing only.
   * Navigation and the Highlight-Checked action are owned by App.svelte via
   * the `onJump`/`onHighlightChecked` callbacks (they need MarkupStore/tab
   * access this component deliberately doesn't have).
   */
  import type { SearchStore, SearchScope, UnifiedSearchHit, SearchGroup } from "$lib/search-store.svelte";
  import type { IndexStatus } from "$lib/ipc";

  interface Props {
    store: SearchStore;
    /** Currently chosen folder for Folder-scope search, or null if none picked yet. */
    folderPath: string | null;
    folderIndexStatus: IndexStatus | null;
    /** Run a search for store.query against the store's current scope. */
    onSearch: () => void;
    /** Open a native folder picker (Folder scope, no folder chosen yet). */
    onPickFolder: () => void;
    /** A result was clicked or keyboard-activated. */
    onJump: (hit: UnifiedSearchHit, group: SearchGroup) => void;
    /** Apply a Highlight markup to every checked TEXT result ("Check Options" -> Highlight Checked). */
    onHighlightChecked: () => void;
  }

  const {
    store,
    folderPath,
    folderIndexStatus,
    onSearch,
    onPickFolder,
    onJump,
    onHighlightChecked,
  }: Props = $props();

  const SCOPES: Array<{ value: SearchScope; label: string }> = [
    { value: "document", label: "Doc" },
    { value: "page", label: "Page" },
    { value: "open", label: "Open Docs" },
    { value: "recents", label: "Recents" },
    { value: "folder", label: "Folder+Sub" },
  ];

  // ---------------------------------------------------------------------------
  // Debounced search-as-you-type (300ms) + immediate on Enter.
  // ---------------------------------------------------------------------------
  const DEBOUNCE_MS = 300;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleSearch() {
    if (debounceTimer) clearTimeout(debounceTimer);
    if (!store.query.trim()) {
      store.clear();
      return;
    }
    debounceTimer = setTimeout(() => {
      debounceTimer = null;
      onSearch();
    }, DEBOUNCE_MS);
  }

  function searchNow() {
    if (debounceTimer) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
    onSearch();
  }

  function handleQueryInput() {
    scheduleSearch();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) jumpTo(store.focusPrev());
      else jumpTo(store.focusNext());
      if (store.flatHits.length === 0) searchNow();
    } else if (e.key === "Escape") {
      store.query = "";
      store.clear();
    }
  }

  function handleScopeClick(scope: SearchScope) {
    if (scope === "folder" && !folderPath) {
      onPickFolder();
      store.setScope(scope);
      return;
    }
    store.setScope(scope);
    if (store.query.trim()) searchNow();
  }

  function jumpTo(ref: { groupIndex: number; hitIndex: number; hit: UnifiedSearchHit } | null) {
    if (!ref) return;
    onJump(ref.hit, store.groups[ref.groupIndex]);
  }

  function clickResult(groupIndex: number, hitIndex: number) {
    store.focusAt(store.flatHits.findIndex((r) => r.groupIndex === groupIndex && r.hitIndex === hitIndex));
    const group = store.groups[groupIndex];
    onJump(group.hits[hitIndex], group);
  }

  function flatIndexOf(groupIndex: number, hitIndex: number): number {
    return store.flatHits.findIndex((r) => r.groupIndex === groupIndex && r.hitIndex === hitIndex);
  }

  // Single-group scopes (Document/Page) skip the group-header chrome —
  // Bluebeam-parity: a per-file header is redundant when there is only ever
  // one possible file.
  const showGroupHeaders = $derived(
    store.scope !== "document" && store.scope !== "page" && store.groups.length > 0
  );

  const SCOPE_PLACEHOLDER: Record<SearchScope, string> = {
    document: "this document",
    page: "this page",
    open: "open documents",
    recents: "recent files",
    folder: "folder",
  };

  function toggleChecked(e: Event, groupIndex: number, hitIndex: number) {
    e.stopPropagation();
    store.toggleChecked(groupIndex, hitIndex);
  }
</script>

<div class="search-panel" role="search" aria-label="Search">
  <div class="search-tabs" role="tablist" aria-label="Search scope">
    {#each SCOPES as s (s.value)}
      <button
        class="search-tab"
        class:active={store.scope === s.value}
        role="tab"
        aria-selected={store.scope === s.value}
        data-testid={`scope-tab-${s.value}`}
        onclick={() => handleScopeClick(s.value)}
      >{s.label}</button>
    {/each}
  </div>

  {#if store.scope === "folder"}
    <div class="folder-row">
      {#if folderPath}
        <button class="folder-path" onclick={onPickFolder} title={folderPath}>
          📁 {folderPath.split(/[\\/]/).pop()}
        </button>
        {#if folderIndexStatus?.state.kind === "Indexing"}
          <span class="folder-status">
            Indexing {folderIndexStatus.state.current_file}…
          </span>
        {:else if folderIndexStatus}
          <span class="folder-status muted">
            {folderIndexStatus.indexed_files} file{folderIndexStatus.indexed_files !== 1 ? "s" : ""} indexed
          </span>
        {/if}
      {:else}
        <button class="folder-path folder-path--empty" onclick={onPickFolder}>
          Choose a folder…
        </button>
      {/if}
    </div>
  {/if}

  <div class="search-input-row">
    <input
      class="search-input"
      type="search"
      placeholder={`Search ${SCOPE_PLACEHOLDER[store.scope]}…`}
      bind:value={store.query}
      oninput={handleQueryInput}
      onkeydown={handleKeydown}
      aria-label="Search query"
      disabled={store.searching}
      data-testid="search-input"
    />
    <button
      class="search-btn"
      onclick={searchNow}
      disabled={store.searching || !store.query.trim()}
      aria-label="Find"
    >
      {store.searching ? "…" : "Find"}
    </button>
    {#if store.totalHitCount > 0 || store.error}
      <button
        class="search-clear"
        onclick={() => { store.query = ""; store.clear(); }}
        aria-label="Clear search"
      >✕</button>
    {/if}
  </div>

  <div class="search-options">
    <label class="search-opt">
      <input type="checkbox" bind:checked={store.caseSensitive} onchange={() => { if (store.query.trim()) searchNow(); }} /> Aa
    </label>
    <label class="search-opt">
      <input type="checkbox" bind:checked={store.wholeWord} onchange={() => { if (store.query.trim()) searchNow(); }} /> Word
    </label>
    {#if store.flatHits.length > 0}
      <div class="search-nav" role="group" aria-label="Next/previous result">
        <button class="nav-btn" onclick={() => jumpTo(store.focusPrev())} title="Previous (Shift+F3)">‹</button>
        <span class="nav-pos">{(store.activeFlatIndex ?? 0) + 1} / {store.flatHits.length}</span>
        <button class="nav-btn" onclick={() => jumpTo(store.focusNext())} title="Next (F3)">›</button>
      </div>
    {/if}
  </div>

  {#if store.error}
    <div class="search-error" role="alert">{store.error}</div>
  {:else if store.searching}
    <div class="search-status">Searching…</div>
  {:else if store.query.trim() && store.groups.length === 0}
    <div class="search-status">No results</div>
  {:else if store.totalHitCount > 0}
    <div class="search-summary">
      {store.totalHitCount} result{store.totalHitCount !== 1 ? "s" : ""}
      {#if store.scope !== "document"}
        across {store.fileCount} file{store.fileCount !== 1 ? "s" : ""}
      {/if}
    </div>
  {/if}

  {#if store.groups.length > 0}
    <!-- "Check Options" toolbar — Bluebeam's Check All/Uncheck All + collapse-all
         (video-confirmed minus/plus icons) + the lightning-bolt bulk-action menu,
         reduced here to its one shipped action, Highlight Checked. -->
    <div class="check-options" role="toolbar" aria-label="Result selection and bulk actions">
      <button class="check-opt-btn" onclick={() => store.checkAll()} title="Check All">☑ All</button>
      <button class="check-opt-btn" onclick={() => store.uncheckAll()} title="Uncheck All">☐ None</button>
      <button class="check-opt-btn" onclick={() => store.collapseAll()} title="Collapse All">− Collapse</button>
      <button class="check-opt-btn" onclick={() => store.expandAll()} title="Expand All">+ Expand</button>
      <button
        class="check-opt-btn check-opt-btn--primary"
        onclick={onHighlightChecked}
        disabled={store.checkedHits.length === 0}
        title="Apply a Highlight markup to every checked text result"
      >
        ⚡ Highlight Checked ({store.checkedHits.length})
      </button>
    </div>

    <div class="search-groups" aria-label="Search results">
      {#each store.groups as group, groupIndex (group.key)}
        <div class="search-group">
          {#if showGroupHeaders}
            <button
              class="group-header"
              onclick={() => store.toggleGroupCollapsed(group.key)}
              aria-expanded={!store.isGroupCollapsed(group.key)}
            >
              <span class="group-caret">{store.isGroupCollapsed(group.key) ? "▸" : "▾"}</span>
              <span class="group-label">{group.label}</span>
              <span class="group-count">{group.hits.length}</span>
            </button>
          {/if}
          {#if !showGroupHeaders || !store.isGroupCollapsed(group.key)}
            <ol class="search-results">
              {#each group.hits as hit, hitIndex (hitIndex)}
                <li
                  class="search-result"
                  class:active={flatIndexOf(groupIndex, hitIndex) === store.activeFlatIndex}
                  role="option"
                  aria-selected={flatIndexOf(groupIndex, hitIndex) === store.activeFlatIndex}
                  onclick={() => clickResult(groupIndex, hitIndex)}
                  onkeydown={(e) => e.key === "Enter" && clickResult(groupIndex, hitIndex)}
                  tabindex="0"
                >
                  <input
                    type="checkbox"
                    class="search-result-check"
                    checked={hit.checked}
                    onclick={(e) => toggleChecked(e, groupIndex, hitIndex)}
                    aria-label="Select this result"
                  />
                  <span class="search-result-page">p.{hit.page + 1}</span>
                  {#if hit.kind === "markup"}
                    <span class="search-result-kind" title="Markup comment/note">markup</span>
                  {/if}
                  {#if hit.snippetHtml}
                    <!-- Tantivy snippet HTML: only <b> tags, safe to render. -->
                    <span class="search-result-snippet">{@html hit.snippet}</span>
                  {:else}
                    <span class="search-result-snippet">{hit.snippet}</span>
                  {/if}
                </li>
              {/each}
            </ol>
          {/if}
        </div>
      {/each}
    </div>
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
    flex-wrap: wrap;
  }

  .search-tab {
    background: none;
    border: none;
    border-radius: var(--radius-sm, 3px) var(--radius-sm, 3px) 0 0;
    color: var(--color-text-muted, #6c7086);
    cursor: pointer;
    font-size: inherit;
    padding: var(--space-1, 2px) var(--space-3, 8px);
    white-space: nowrap;
  }

  .search-tab:hover {
    color: var(--color-text, #cdd6f4);
  }

  .search-tab.active {
    color: var(--color-accent, #89b4fa);
    border-bottom: 2px solid var(--color-accent, #89b4fa);
  }

  .folder-row {
    display: flex;
    align-items: center;
    gap: var(--space-2, 4px);
    flex-wrap: wrap;
  }

  .folder-path {
    background: var(--color-surface-raised, #313244);
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    color: var(--color-text, #cdd6f4);
    cursor: pointer;
    font-size: var(--text-xs, 11px);
    padding: var(--space-1, 2px) var(--space-2, 4px);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .folder-path--empty {
    color: var(--color-accent, #89b4fa);
  }

  .folder-status {
    font-size: var(--text-xs, 11px);
    color: var(--color-accent, #89b4fa);
  }

  .folder-status.muted {
    color: var(--color-text-muted, #6c7086);
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
    flex-wrap: wrap;
  }

  .search-opt {
    display: flex;
    gap: var(--space-1, 2px);
    align-items: center;
    cursor: pointer;
    user-select: none;
  }

  .search-nav {
    display: flex;
    align-items: center;
    gap: var(--space-1, 2px);
    margin-left: auto;
  }

  .nav-btn {
    background: none;
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    color: var(--color-text, #cdd6f4);
    cursor: pointer;
    padding: 0 var(--space-2, 4px);
    font-size: inherit;
  }

  .nav-pos {
    color: var(--color-text-muted, #6c7086);
    font-size: var(--text-xs, 11px);
    min-width: 3.5em;
    text-align: center;
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

  .check-options {
    display: flex;
    gap: var(--space-1, 2px);
    flex-wrap: wrap;
    align-items: center;
    padding: var(--space-1, 2px) 0;
    border-bottom: 1px solid var(--color-border, #45475a);
  }

  .check-opt-btn {
    background: var(--color-surface-raised, #313244);
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    color: var(--color-text, #cdd6f4);
    cursor: pointer;
    font-size: var(--text-xs, 11px);
    padding: 2px var(--space-2, 4px);
    white-space: nowrap;
  }

  .check-opt-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .check-opt-btn--primary {
    margin-left: auto;
    background: var(--color-accent, #89b4fa);
    color: var(--color-surface, #1e1e2e);
    border-color: var(--color-accent, #89b4fa);
  }

  .check-opt-btn--primary:disabled {
    background: var(--color-surface-raised, #313244);
    color: var(--color-text-muted, #6c7086);
    border-color: var(--color-border, #45475a);
  }

  .search-result-check {
    flex-shrink: 0;
    cursor: pointer;
  }

  .search-groups {
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-2, 4px);
  }

  .search-group {
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    overflow: hidden;
  }

  .group-header {
    display: flex;
    align-items: center;
    gap: var(--space-2, 4px);
    width: 100%;
    background: var(--color-surface-raised, #313244);
    border: none;
    color: var(--color-text, #cdd6f4);
    cursor: pointer;
    font-size: inherit;
    font-weight: 600;
    padding: var(--space-1, 2px) var(--space-2, 4px);
    text-align: left;
  }

  .group-caret {
    color: var(--color-text-muted, #6c7086);
    width: 1em;
    flex-shrink: 0;
  }

  .group-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .group-count {
    color: var(--color-text-muted, #6c7086);
    font-size: var(--text-xs, 11px);
    font-weight: 400;
    flex-shrink: 0;
  }

  .search-results {
    list-style: none;
    margin: 0;
    padding: 0;
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

  .search-result-kind {
    background: var(--color-surface, #1e1e2e);
    border: 1px solid var(--color-border, #45475a);
    border-radius: var(--radius-sm, 3px);
    color: var(--color-text-muted, #6c7086);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    padding: 0 3px;
    flex-shrink: 0;
  }

  .search-result-snippet {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }

  /* Tantivy <b> highlight in folder results */
  .search-result-snippet :global(b) {
    color: var(--color-accent, #89b4fa);
    font-weight: 600;
  }
</style>
