<script lang="ts">
  /**
   * Page indicator + jump control — "current / total" with an editable field to type a
   * page number and jump (owner request 2026-09-15, part of the reading-mode feature
   * bundle). Lives in App.svelte's toolbar, visible in both viewer modes: reading mode
   * updates `currentPage` continuously as the user scrolls (ReadingView's onviewportchange),
   * single-page mode updates it on page navigation the same way Viewport already tracks
   * pageIndex — either way this component only ever renders/parses, it owns no viewer state.
   *
   * A separate leaf component (mirrors UndoRedoControls' rationale) so it's directly
   * testable with @testing-library/svelte without mounting App.svelte or a real Viewport.
   */
  import { parsePageJumpInput } from "$lib/reading-mode";

  interface Props {
    /** 0-based index of the current page, or null when no document is open. */
    currentPage: number | null;
    pageCount: number;
    /** Called with a 0-based page index when the user commits a valid jump. */
    onjump: (pageIndex: number) => void;
  }

  const { currentPage, pageCount, onjump }: Props = $props();

  let editing = $state(false);
  let draft = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);

  // Focus + select the input the instant it mounts (editing flips true, THEN the input
  // renders and this effect fires) — mirrors the click-to-select-all convention every
  // other editable numeric field in this app uses (e.g. Viewport's zoom-percent input).
  $effect(() => {
    if (editing) {
      inputEl?.focus();
      inputEl?.select();
    }
  });

  /** 1-based display value shown when not editing (or as the input's default). */
  const displayValue = $derived(currentPage === null ? "" : String(currentPage + 1));

  // Set by cancel() so the blur that Escape's own DOM removal triggers doesn't also
  // commit a jump - Escape must discard the draft unconditionally, not just skip
  // re-parsing it. Cleared at the top of commit() so a NORMAL (non-Escape) blur or
  // Enter still commits.
  let cancelling = false;

  function startEdit() {
    if (pageCount <= 0) return;
    draft = displayValue;
    editing = true;
  }

  function commit() {
    if (cancelling) {
      cancelling = false;
      editing = false;
      return;
    }
    const parsed = parsePageJumpInput(draft, pageCount);
    if (parsed !== null) onjump(parsed);
    // Invalid input: silently revert to the current page rather than leaving a stuck
    // bad value in the field (mirrors Viewport.svelte's zoom-percent input contract).
    editing = false;
  }

  function cancel() {
    cancelling = true;
    editing = false;
  }
</script>

<div class="page-indicator" aria-label="Page navigation">
  {#if editing}
    <input
      bind:this={inputEl}
      class="page-indicator-input"
      type="text"
      inputmode="numeric"
      aria-label="Jump to page"
      value={draft}
      onclick={(e) => e.currentTarget.select()}
      oninput={(e) => (draft = (e.target as HTMLInputElement).value)}
      onblur={commit}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape") {
          e.preventDefault();
          cancel();
        }
      }}
    />
  {:else}
    <button
      class="page-indicator-display"
      onclick={startEdit}
      disabled={pageCount <= 0}
      title="Type a page number and press Enter to jump"
    >
      {currentPage === null ? "—" : currentPage + 1}
    </button>
  {/if}
  <span class="page-indicator-total">/ {pageCount}</span>
</div>


<style>
  .page-indicator {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
  }

  .page-indicator-display {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 2px var(--space-2);
    min-width: 2.5em;
    text-align: center;
  }

  .page-indicator-display:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .page-indicator-display:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .page-indicator-input {
    background: var(--color-bg);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    padding: 2px var(--space-2);
    width: 3em;
    text-align: center;
  }

  .page-indicator-total {
    color: var(--color-text-muted);
  }
</style>
