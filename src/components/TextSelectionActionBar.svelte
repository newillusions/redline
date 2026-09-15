<script lang="ts">
  /**
   * Floating action bar / status shown after a text-selection (I-beam tool)
   * drag ends - owner feedback on v0.3.22 (verbatim): "you often make a
   * selection with nothing happening at the end". Before this component, a
   * completed selection had no visible outcome beyond the translucent
   * highlight itself; Enter (commit as Highlight) and Ctrl/Cmd+C (copy) were
   * real but undiscoverable keyboard-only affordances with no on-screen cue.
   *
   * Two modes, driven by Viewport's textSelBar (reactive - see its doc comment
   * there for why):
   *  - "selection": a real range was selected. Shows "N characters selected"
   *    plus Copy / Highlight / Search buttons - the same three actions a
   *    browser's own text-selection popover offers, and the closest existing
   *    UI metaphor users already know.
   *  - "empty": the drag/click ended with no text under it (an image-only
   *    region, or a scanned page with no OCR text layer). Mirrors the exact
   *    wording SearchPanel.svelte already uses for a no-results search, so the
   *    app gives one consistent answer for "why can't I find/select text
   *    here" rather than inventing new copy for the same underlying cause.
   *
   * Positioned by the caller (screen-space x/y, already clamped into the
   * viewport - see Viewport's clampBarPos). This component owns only its own
   * look, not its placement.
   */
  const {
    x,
    y,
    mode,
    charCount,
    onCopy,
    onHighlight,
    onSearchText,
  }: {
    x: number;
    y: number;
    mode: "selection" | "empty";
    charCount: number;
    onCopy: () => void;
    onHighlight: () => void;
    onSearchText: () => void;
  } = $props();
</script>

<div class="text-sel-bar" style:left="{x}px" style:top="{y}px" role="group" aria-label="Text selection actions">
  {#if mode === "selection"}
    <div class="text-sel-status" role="status" aria-live="polite">
      {charCount} character{charCount === 1 ? "" : "s"} selected
    </div>
    <div class="text-sel-actions">
      <button class="text-sel-btn" title="Copy (Ctrl/Cmd+C)" onclick={onCopy}>Copy</button>
      <button class="text-sel-btn" title="Highlight (Enter)" onclick={onHighlight}>Highlight</button>
      <button class="text-sel-btn" title="Search for this text" onclick={onSearchText}>Search</button>
    </div>
  {:else}
    <div class="text-sel-status text-sel-empty" role="status" aria-live="polite">
      No text selected. If this is a scanned or image-only PDF, it may have no searchable text layer — Auto-OCR can add one.
    </div>
  {/if}
</div>

<style>
  .text-sel-bar {
    position: absolute;
    z-index: 30;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    background: var(--color-bg-panel);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    box-shadow: 0 4px 16px rgba(0 0 0 / 0.3);
    max-width: 240px;
  }
  .text-sel-status {
    font-size: var(--font-size-sm);
    color: var(--color-text);
    white-space: nowrap;
  }
  .text-sel-empty {
    white-space: normal;
    color: var(--color-text-secondary);
    max-width: 220px;
  }
  .text-sel-actions {
    display: flex;
    gap: var(--space-2);
  }
  .text-sel-btn {
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-active);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }
  .text-sel-btn:hover { background: var(--color-bg-hover); }
</style>
