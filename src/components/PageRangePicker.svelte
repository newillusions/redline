<script lang="ts">
  /**
   * PageRangePicker - the "Page Range" choice shared by Number Pages and Create bookmarks
   * from page labels: All Pages, Current, Selected (when thumbnails are selected) or a
   * Custom list such as `1-3, 5, 9` (docs/navigation.md L3, B14). Reports the resolved
   * 0-based pages (ascending, unique) or null while the choice cannot be used.
   */
  import { pagesForRange, type RangeMode } from "$lib/navigation";

  const {
    pageCount,
    currentPage,
    selectedPages = [],
    onchange,
  }: {
    pageCount: number;
    currentPage: number | null;
    selectedPages?: readonly number[];
    onchange: (pages: number[] | null) => void;
  } = $props();

  /* svelte-ignore state_referenced_locally */
  let mode = $state<RangeMode>(selectedPages.length > 0 ? "selected" : "all");
  let custom = $state("");

  const pages = $derived(pagesForRange(mode, pageCount, currentPage, selectedPages, custom));
  const problem = $derived(
    mode === "custom" && pages === null
      ? `Use page numbers such as 1-3, 5, 9 (this document has ${pageCount}).`
      : null,
  );

  $effect(() => {
    onchange(pages);
  });
</script>

<fieldset class="range" aria-label="Page range">
  <legend>Page range</legend>
  <label><input type="radio" name="range" value="all" bind:group={mode} /> All pages</label>
  <label>
    <input type="radio" name="range" value="current" bind:group={mode} disabled={currentPage === null} />
    Current{currentPage !== null ? ` (${currentPage + 1})` : ""}
  </label>
  {#if selectedPages.length > 0}
    <label>
      <input type="radio" name="range" value="selected" bind:group={mode} />
      Selected ({selectedPages.length})
    </label>
  {/if}
  <label class="custom">
    <input type="radio" name="range" value="custom" bind:group={mode} />
    Custom
    <input
      type="text"
      class="custom-input"
      aria-label="Custom page range"
      placeholder="1-3, 5, 9"
      bind:value={custom}
      onfocus={() => (mode = "custom")}
      oninput={() => (mode = "custom")}
    />
  </label>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
</fieldset>

<style>
  .range {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--font-size-sm);
  }
  legend {
    color: var(--color-text-secondary);
    padding: 0 var(--space-1);
  }
  label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text);
  }
  .custom-input {
    flex: 1;
    min-width: 0;
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--font-size-sm);
  }
  .problem {
    margin: 0;
    color: var(--color-danger);
  }
</style>
