<script lang="ts">
  /**
   * LinksPanel - the document's links in a list with a filter box (Revu's Links panel,
   * docs/navigation.md RH4): select to jump to the link, Go to run it, Edit Action to
   * re-point it, Delete to remove it. Link targets come from the PDF and are shown as text;
   * an address is only ever opened after the user confirms it (App.svelte).
   */
  import type { NavigationStore } from "$lib/navigation-store.svelte";
  import type { LinkView, NavTarget } from "$lib/navigation-ipc";
  import {
    filterLinks,
    isFollowable,
    linkJump,
    pageText,
    targetText,
    type JumpTarget,
  } from "$lib/navigation";

  const {
    nav,
    onjump,
    onfollow,
    onedit,
  }: {
    nav: NavigationStore;
    /** Show the link's location in the viewport. */
    onjump: (j: JumpTarget) => void;
    /** Run the link's target (go to the page, or ask before opening an address). */
    onfollow: (t: NavTarget) => void;
    /** Re-point a link (App.svelte shows the action dialog). */
    onedit: (l: LinkView) => void;
  } = $props();

  let query = $state("");
  let selectedKey = $state<string | null>(null);

  const keyOf = (l: LinkView, i: number) => l.id ?? `inline:${l.page}:${i}`;
  const all = $derived(nav.links);
  const shown = $derived(filterLinks(all, query, nav.labels));
  const selected = $derived(
    selectedKey === null ? null : (shown.find((l) => keyOf(l, all.indexOf(l)) === selectedKey) ?? null),
  );
  const canEdit = $derived(nav.fileEditable && !nav.busy);

  function select(l: LinkView) {
    selectedKey = keyOf(l, all.indexOf(l));
    onjump(linkJump(l));
  }

  async function remove() {
    const l = selected;
    if (!l || l.id === null || !canEdit) return;
    const ok = await nav.edit({ op: "link_delete", page: l.page, id: l.id });
    if (ok) selectedKey = null;
  }
</script>

<div class="links" data-testid="links-panel">
  <div class="bar">
    <button
      class="tb"
      class:active={nav.linkMode}
      aria-pressed={nav.linkMode}
      title="Draw a new link area on the page (Esc to stop)"
      disabled={!canEdit && !nav.linkMode}
      onclick={() => (nav.linkMode ? nav.endLinkMode() : nav.beginLinkMode())}
    >Add link</button>
    <input
      class="filter"
      type="search"
      placeholder="Filter links"
      aria-label="Filter links"
      bind:value={query}
    />
  </div>
  <div class="bar">
    <button class="tb" disabled={!selected || !isFollowable(selected.target)} onclick={() => selected && onfollow(selected.target)}>Go</button>
    <button class="tb" disabled={!selected || selected.id === null || !canEdit} onclick={() => selected && onedit(selected)}>Edit action...</button>
    <button
      class="tb"
      disabled={!selected || selected.id === null || !canEdit}
      title={selected && selected.id === null ? "This link is embedded in the page and cannot be edited" : "Delete the selected link"}
      onclick={() => void remove()}
    >Delete</button>
    <span class="spacer"></span>
    <button class="tb icon" title="Undo (the last bookmark, label or link edit)" aria-label="Undo" disabled={!nav.canUndo} onclick={() => void nav.undo()}>&#x21B6;</button>
    <button class="tb icon" title="Redo" aria-label="Redo" disabled={!nav.canRedo} onclick={() => void nav.redo()}>&#x21B7;</button>
  </div>
  {#if nav.fileEditable === false && nav.info}
    <p class="note warn" role="status">{nav.readOnlyReason}</p>
  {/if}
  {#if nav.error}
    <p class="note error" role="alert">{nav.error}</p>
  {/if}
  {#if nav.linkMode}
    <p class="note" role="status">Drag on the page to draw the link area. Esc to stop.</p>
  {/if}

  {#if nav.info === null}
    <p class="note muted">{nav.loaded ? "Links could not be read." : "Reading links..."}</p>
  {:else if shown.length === 0}
    <p class="note muted">{all.length === 0 ? "This PDF has no links." : "No links match."}</p>
  {:else}
    <ul class="list" role="listbox" aria-label="Links">
      {#each shown as l (keyOf(l, all.indexOf(l)))}
        <li role="option" aria-selected={selectedKey === keyOf(l, all.indexOf(l))}>
          <button
            class="item"
            class:selected={selectedKey === keyOf(l, all.indexOf(l))}
            class:dim={!isFollowable(l.target)}
            onclick={() => select(l)}
            ondblclick={() => isFollowable(l.target) && onfollow(l.target)}
            title={targetText(l.target, nav.labels)}
          >
            <span class="where">{pageText(nav.labels, l.page)}</span>
            <span class="what">{targetText(l.target, nav.labels)}</span>
          </button>
        </li>
      {/each}
    </ul>
    <p class="note muted">
      {all.length} link{all.length === 1 ? "" : "s"}{#if nav.info.links_truncated}, list cut at the display limit{/if}
    </p>
  {/if}
</div>

<style>
  .links {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: var(--font-size-sm);
  }
  .bar {
    display: flex;
    gap: var(--space-1);
    align-items: center;
    padding: var(--space-1) var(--space-2);
  }
  .filter {
    flex: 1;
    min-width: 0;
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--font-size-sm);
  }
  .tb {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-2);
    transition: background 120ms;
  }
  .tb:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }
  .tb:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .tb.icon {
    min-width: 1.8em;
    padding-left: var(--space-1);
    padding-right: var(--space-1);
  }
  .spacer {
    flex: 1;
  }
  .tb.active {
    background: var(--color-primary);
    color: var(--color-text-inverse);
  }
  .note {
    margin: 0;
    padding: var(--space-1) var(--space-3);
    color: var(--color-text-secondary);
  }
  .note.warn {
    color: var(--color-warning);
  }
  .note.error {
    color: var(--color-danger);
  }
  .muted {
    color: var(--color-text-muted);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item {
    display: flex;
    gap: var(--space-2);
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--color-text);
    padding: 3px var(--space-3);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }
  .item:hover {
    background: var(--color-bg-hover);
  }
  .item.selected {
    background: var(--color-bg-active);
  }
  .item.dim {
    opacity: 0.6;
  }
  .where {
    flex: none;
    min-width: 3.5em;
    color: var(--color-text-secondary);
  }
  .what {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
