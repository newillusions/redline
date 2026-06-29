<script lang="ts">
  /**
   * TabBar — horizontal tab strip for multi-document editing (feat/tabbed-multi-file).
   *
   * Shows one tab per open document with: filename label, active highlight, × close button.
   * Designed to sit between the toolbar and the main body row in App.svelte.
   *
   * Svelte 5 runes. Matches the app dark theme (CSS custom properties, no Tailwind).
   */
  import type { DocTab } from "$lib/doc-tabs.svelte";

  const {
    tabs,
    activeDocId,
    ontabclick,
    ontabclose,
  }: {
    tabs: DocTab[];
    activeDocId: string | null;
    /** Called when the user clicks a tab to switch to it. */
    ontabclick: (docId: string) => void;
    /** Called when the user clicks the × button on a tab. */
    ontabclose: (docId: string) => void;
  } = $props();

  function filename(path: string): string {
    return path.split(/[\\/]/).at(-1) ?? path;
  }
</script>

{#if tabs.length > 0}
  <div class="tab-bar" role="tablist" aria-label="Open documents">
    {#each tabs as tab (tab.docId)}
      <div
        class="tab"
        class:tab-active={tab.docId === activeDocId}
        role="tab"
        aria-selected={tab.docId === activeDocId}
        tabindex={tab.docId === activeDocId ? 0 : -1}
        title={tab.doc.path}
        onclick={() => ontabclick(tab.docId)}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            ontabclick(tab.docId);
          }
        }}
      >
        <span class="tab-label">{filename(tab.doc.path)}</span>
        <button
          class="tab-close"
          aria-label={`Close ${filename(tab.doc.path)}`}
          title="Close tab (Cmd/Ctrl+W)"
          onclick={(e) => {
            e.stopPropagation();
            ontabclose(tab.docId);
          }}
        >×</button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .tab-bar {
    display: flex;
    align-items: stretch;
    height: 34px;
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    overflow-x: auto;
    overflow-y: hidden;
    flex-shrink: 0;
    /* Hide scrollbar but keep scrollability for many tabs */
    scrollbar-width: none;
  }
  .tab-bar::-webkit-scrollbar { display: none; }

  .tab {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: 0 var(--space-2) 0 var(--space-3);
    min-width: 100px;
    max-width: 200px;
    border-right: 1px solid var(--color-border);
    border-bottom: 2px solid transparent;
    cursor: pointer;
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    white-space: nowrap;
    user-select: none;
    -webkit-user-select: none;
    transition: background 100ms, color 100ms;
    flex-shrink: 0;
    outline: none;
  }
  .tab:hover {
    background: var(--color-bg-hover);
    color: var(--color-text-secondary);
  }
  .tab:focus-visible {
    outline: 2px solid var(--color-primary);
    outline-offset: -2px;
  }
  .tab.tab-active {
    background: var(--color-bg);
    color: var(--color-text);
    border-bottom-color: var(--color-primary);
  }

  .tab-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tab-close {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0;
    flex-shrink: 0;
    transition: background 100ms, color 100ms;
  }
  .tab-close:hover {
    background: var(--color-bg-active);
    color: var(--color-text);
  }
  .tab.tab-active .tab-close {
    color: var(--color-text-secondary);
  }
  .tab.tab-active .tab-close:hover {
    background: var(--color-bg-active);
    color: var(--color-danger);
  }
</style>
