<script lang="ts">
  /**
   * Accordion — shared collapsible-section header + body, used by every
   * sub-panel inside the side panels (owner defect, 2026-09-07: "the recent
   * documents panel needs to be able to accordion - actually, all subpanels
   * in the side panels").
   *
   * Two modes, picked automatically by whether `collapsed` is passed:
   *   - CONTROLLED (pass `collapsed` + `ontoggle`): the caller owns the
   *     source of truth. Used where collapse state already lives elsewhere -
   *     SearchStore's per-group `collapsedKeys` (so the existing Collapse
   *     All/Expand All toolbar keeps working) and ToolChestPanel's
   *     per-set state where a caller wants explicit control.
   *   - UNCONTROLLED (omit `collapsed`): the component owns its own state,
   *     seeded from `defaultCollapsed` (default expanded). Pass `storageKey`
   *     to remember the collapsed state for the rest of THIS app session
   *     (sessionStorage - cleared on next launch, not the disk-backed
   *     AppSettings mechanism). This is what the owner asked for
   *     ("remembered per sub-panel for the session") without adding a new
   *     Rust-side settings field for an open-ended set of accordion keys,
   *     which would not be a trivial change to the existing settings.ts /
   *     load_settings|save_settings IPC contract.
   *
   * Keyboard: a real <button> already gets a native click from Enter/Space
   * in every real browser engine, but jsdom does not reliably synthesize
   * that (a known jsdom gap), so this component handles both keys
   * explicitly - correct on every platform and directly testable.
   *
   * Header look is owned entirely by `variant` (below), NOT by a caller-passed
   * class string. Svelte scopes each component's CSS with a per-component
   * hash; a class NAME forwarded as a prop and applied by THIS component to
   * an element it creates does not carry the CALLER's hash, so a caller's
   * `.panel-header`/`.group-header`-style rule silently fails to match here
   * (confirmed both by svelte-check's "unused CSS selector" warning in every
   * caller that tried it, and visually in the gui-harness: headers rendered
   * in the wrong case/weight/colour). Content passed via the `children`/
   * `titleExtra`/`headerExtra` SNIPPETS does not have this problem - a
   * snippet's markup is compiled as part of the DEFINING component, so its
   * own scoped classes (e.g. a caller's ".group-count" inside `titleExtra`)
   * keep working normally. `bodyClass` is still safe to use for the two
   * global (non-scoped) layout classes in src/lib/styles.css
   * (`.panel-body`/`.panel-body-flush`) - global rules match regardless of
   * scope hash.
   */
  import type { Snippet } from "svelte";

  interface Props {
    /** Header title text. */
    title: string;
    /** Body content, shown when expanded. */
    children: Snippet;
    /** Non-interactive content rendered inside the header button, after the
     *  title (e.g. a result-count badge). */
    titleExtra?: Snippet;
    /** Interactive content rendered as a SIBLING after the header button
     *  (e.g. a delete-set button) - kept outside the button to avoid
     *  nesting interactive elements. */
    headerExtra?: Snippet;
    /** Controlled mode: current collapsed state. Omit for uncontrolled mode. */
    collapsed?: boolean;
    /** Called with the NEW collapsed value after any toggle, in either mode. */
    ontoggle?: (collapsed: boolean) => void;
    /** Uncontrolled mode only: initial state before any persisted value is
     *  found. Default false (expanded). */
    defaultCollapsed?: boolean;
    /** Uncontrolled mode only: sessionStorage key for remembering collapse
     *  state for this app session. Omit for no persistence. */
    storageKey?: string;
    /** Header look: "panel" (default) matches a top-level side-panel section
     *  header (uppercase, small, muted) - Recent Documents/Tool Chest/
     *  Navigator/Properties/the bottom panel. "nested" matches a sub-item
     *  inside one of those (a search-results group, a tool-chest set) -
     *  normal case, a filled header row so nested items read as grouped. */
    variant?: "panel" | "nested";
    /** Extra class(es) for the body wrapper - use the global panel-body /
     *  panel-body-flush classes (src/lib/styles.css) here, not a
     *  component-scoped one (see the note above). */
    bodyClass?: string;
    testId?: string;
  }

  const {
    title,
    children,
    titleExtra,
    headerExtra,
    collapsed: collapsedProp,
    ontoggle,
    defaultCollapsed = false,
    storageKey,
    variant = "panel",
    bodyClass,
    testId,
  }: Props = $props();

  const controlled = $derived(collapsedProp !== undefined);

  function storageFullKey(key: string): string {
    return `redline.accordion.${key}`;
  }

  function readPersisted(): boolean | null {
    if (!storageKey) return null;
    try {
      const raw = sessionStorage.getItem(storageFullKey(storageKey));
      return raw === null ? null : raw === "1";
    } catch {
      // Blocked/unavailable sessionStorage (private mode, etc.) must not break the UI.
      return null;
    }
  }

  let internalCollapsed = $state(readPersisted() ?? defaultCollapsed);

  const isCollapsed = $derived(controlled ? (collapsedProp as boolean) : internalCollapsed);

  function toggle() {
    const next = !isCollapsed;
    if (!controlled) {
      internalCollapsed = next;
      if (storageKey) {
        try {
          sessionStorage.setItem(storageFullKey(storageKey), next ? "1" : "0");
        } catch {
          // Best-effort only.
        }
      }
    }
    ontoggle?.(next);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " " || e.key === "Spacebar") {
      e.preventDefault();
      toggle();
    }
  }
</script>

<div class="accordion">
  <div class="accordion-header-row">
    <button
      type="button"
      class="accordion-header"
      class:variant-panel={variant === "panel"}
      class:variant-nested={variant === "nested"}
      onclick={toggle}
      onkeydown={handleKeydown}
      aria-expanded={!isCollapsed}
      data-testid={testId}
    >
      <span class="accordion-caret" aria-hidden="true">{isCollapsed ? "▸" : "▾"}</span>
      <span class="accordion-title">{title}</span>
      {#if titleExtra}{@render titleExtra()}{/if}
    </button>
    {#if headerExtra}
      <span class="accordion-header-extra">{@render headerExtra()}</span>
    {/if}
  </div>
  {#if !isCollapsed}
    <div class="accordion-body {bodyClass ?? ''}">
      {@render children()}
    </div>
  {/if}
</div>

<style>
  .accordion {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .accordion-header-row {
    display: flex;
    align-items: stretch;
    flex-shrink: 0;
  }

  .accordion-header {
    display: flex;
    align-items: center;
    gap: var(--space-2, 8px);
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    margin: 0;
    cursor: pointer;
    font: inherit;
    color: inherit;
    text-align: left;
  }

  /* Top-level side-panel section header (Recent Documents / Tool Chest /
     Navigator / Properties / the bottom panel) - matches the app shell's
     established .panel-header look. */
  .accordion-header.variant-panel {
    font-size: var(--font-size-xs, 11px);
    font-weight: 600;
    color: var(--color-text-secondary, #aeaeb2);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: var(--space-2, 8px) var(--space-3, 12px);
    border-bottom: 1px solid var(--color-border, #3a3a3c);
  }

  /* Nested sub-item header (a search-results group, a tool-chest set) -
     normal case, filled row so grouped items read as a unit. */
  .accordion-header.variant-nested {
    font-size: var(--font-size-sm, 12px);
    font-weight: 600;
    color: var(--color-text, #f2f2f7);
    background: var(--color-bg-active, #3a3a40);
    padding: var(--space-1, 4px) var(--space-2, 8px);
    border-radius: var(--radius-sm, 4px);
  }

  .accordion-caret {
    flex-shrink: 0;
    width: 1em;
    color: var(--color-text-muted, #636366);
  }

  .accordion-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .accordion-header-extra {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }

  .accordion-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
