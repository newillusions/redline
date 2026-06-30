<script lang="ts">
  /**
   * DocumentHistoryPanel — Most-Recently-Used PDF list in the left panel.
   *
   * Receives the MRU list from App.svelte (which manages loading + updates).
   * Clicking a row reopens the document via the shared `openFilePath` entry
   * point, which deduplicates against already-open tabs via DocTabStore.
   *
   * Missing-file handling: when the list changes, each path is checked against
   * the filesystem. Missing entries are greyed out and unclickable, with a
   * tooltip showing the full path. (Decision: grey-out over silent prune — the
   * user can see deleted entries and tidy them manually.)
   *
   * Persistence is owned by App.svelte (loads at startup via loadRecentDocs,
   * saves on every successful open via saveRecentDocs). The panel is a
   * pure renderer + interaction handler.
   */
  import { checkFileExists } from "$lib/recent-docs";
  import type { RecentDoc } from "$lib/recent-docs";

  interface Props {
    /** MRU list from App.svelte — reactive; updates immediately on new opens. */
    recentDocs: RecentDoc[];
    /** Called when the user selects an entry. Matches App.svelte openFilePath. */
    onOpen: (path: string) => Promise<void>;
  }

  const { recentDocs, onOpen }: Props = $props();

  // ---------------------------------------------------------------------------
  // File-existence state
  // ---------------------------------------------------------------------------

  /** Set of paths known to be missing on disk. Re-checked whenever the list changes. */
  let missingPaths = $state<Set<string>>(new Set());

  $effect(() => {
    const paths = recentDocs.map((d) => d.path);
    if (paths.length === 0) {
      missingPaths = new Set();
      return;
    }
    // Fire existence checks in parallel; update the set when all resolve.
    void Promise.all(
      paths.map((p) => checkFileExists(p).then((ok) => ({ path: p, ok }))),
    ).then((results) => {
      const missing = new Set<string>();
      for (const { path, ok } of results) {
        if (!ok) missing.add(path);
      }
      missingPaths = missing;
    });
  });

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------

  async function handleOpen(entry: RecentDoc) {
    if (missingPaths.has(entry.path)) return; // greyed — do nothing
    await onOpen(entry.path);
  }

  // ---------------------------------------------------------------------------
  // Formatting helpers
  // ---------------------------------------------------------------------------

  function formatDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        year: "numeric",
      });
    } catch {
      return iso;
    }
  }

  /**
   * Return at most 2 trailing directory segments so the subtitle stays short.
   * Works on both POSIX and Windows paths.
   */
  function shortDir(path: string): string {
    const sep = path.includes("\\") ? "\\" : "/";
    const parts = path.split(sep);
    parts.pop(); // drop filename
    if (parts.length === 0) return "";
    return parts.slice(-2).join(sep);
  }
</script>

<div class="history-panel" aria-label="Document history">
  {#if recentDocs.length === 0}
    <p class="hint muted">No recent documents.</p>
    <p class="hint muted small">Open a PDF to start building history.</p>
  {:else}
    <ul class="entry-list" role="listbox" aria-label="Recent documents">
      {#each recentDocs as entry (entry.path)}
        {@const missing = missingPaths.has(entry.path)}
        <li
          class="entry"
          class:entry--missing={missing}
          role="option"
          aria-selected="false"
          aria-disabled={missing}
          tabindex={missing ? -1 : 0}
          title={missing ? `File not found: ${entry.path}` : entry.path}
          onclick={() => handleOpen(entry)}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              void handleOpen(entry);
            }
          }}
        >
          <div class="entry-name">
            {entry.file_name}
            {#if missing}
              <span class="missing-badge" aria-label="File not found">✗</span>
            {/if}
          </div>
          <div class="entry-meta">
            <span class="entry-dir" title={entry.path}>{shortDir(entry.path)}</span>
            <span class="entry-date">{formatDate(entry.last_opened)}</span>
          </div>
          {#if entry.page_count !== undefined && entry.page_count !== null}
            <div class="entry-pages">
              {entry.page_count}
              {entry.page_count === 1 ? "page" : "pages"}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .history-panel {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    margin: var(--space-2) var(--space-3);
    padding: 0;
  }

  .hint.muted {
    color: var(--color-text-muted);
  }

  .hint.small {
    font-size: var(--font-size-xs);
    margin-top: 0;
  }

  /* Entry list */
  .entry-list {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
  }

  .entry {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2) var(--space-3);
    cursor: pointer;
    border-bottom: 1px solid var(--color-border-subtle);
    transition: background 100ms;
    outline-offset: -2px;
  }

  .entry:hover:not(.entry--missing) {
    background: var(--color-bg-hover);
  }

  .entry:focus-visible {
    outline: 2px solid var(--color-primary);
    outline-offset: -2px;
  }

  /* Missing-file state */
  .entry--missing {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .entry--missing:hover {
    background: transparent;
  }

  /* File name row */
  .entry-name {
    font-size: var(--font-size-sm);
    color: var(--color-text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .missing-badge {
    font-size: var(--font-size-xs);
    color: var(--color-danger);
    flex-shrink: 0;
  }

  /* Meta row — dir + date side by side */
  .entry-meta {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    overflow: hidden;
  }

  .entry-dir {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
  }

  .entry-date {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
    flex-shrink: 0;
  }

  /* Page count hint */
  .entry-pages {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
  }
</style>
