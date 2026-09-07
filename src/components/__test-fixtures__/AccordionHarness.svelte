<script lang="ts">
  /**
   * Test-only fixture for Accordion.test.ts. Accordion's `children` (and
   * optional `headerExtra`) props are Svelte 5 Snippets, which can only be
   * constructed with real `{#snippet}` blocks inside a `.svelte` template -
   * not passed in as plain functions from a `.ts` test file. This tiny
   * wrapper supplies fixed, predictable content ("body content" text, an
   * optional delete-style headerExtra button) and forwards the props each
   * Accordion.test.ts case actually varies.
   *
   * Not part of the shipped app - lives under __test-fixtures__ so it reads
   * as test-only, matching how e.g. SearchPanel.test.ts/ToolChestPanel.test.ts
   * exercise the real components' OWN internal snippets rather than needing
   * a fixture (Accordion is generic, so its tests need one).
   */
  import Accordion from "../Accordion.svelte";

  const {
    title,
    testId,
    defaultCollapsed = false,
    storageKey,
    withHeaderExtra = false,
    collapsed,
    ontoggle,
  }: {
    title: string;
    testId: string;
    defaultCollapsed?: boolean;
    storageKey?: string;
    withHeaderExtra?: boolean;
    /** Controlled-mode pass-through; omit for uncontrolled-mode tests. */
    collapsed?: boolean;
    ontoggle?: (collapsed: boolean) => void;
  } = $props();
</script>

<Accordion {title} {testId} {defaultCollapsed} {storageKey} {collapsed} {ontoggle}>
  {#snippet headerExtra()}
    {#if withHeaderExtra}
      <button data-testid="header-extra-btn" type="button">✕</button>
    {/if}
  {/snippet}
  <p>body content</p>
</Accordion>
