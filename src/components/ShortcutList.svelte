<script lang="ts">
  /** The full keyboard-shortcut table (lib/shortcuts.ts), shared by the `?` overlay and Settings. */
  import { shortcutGroups } from "$lib/shortcuts";

  const groups = shortcutGroups();
</script>

<div class="shortcut-list">
  {#each groups as g (g.title)}
    <div class="shortcuts-title">{g.title}</div>
    {#each g.rows as r (r.keys + r.label)}
      <div class="shortcuts-row"><span>{r.keys}</span><span>{r.label}</span></div>
    {/each}
  {/each}
</div>

<style>
  .shortcuts-title {
    color: var(--color-text);
    font-weight: 600;
    margin-bottom: var(--space-2);
  }
  .shortcuts-title:not(:first-child) { margin-top: var(--space-3); }
  .shortcuts-row {
    display: flex;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-1) 0;
    border-bottom: 1px solid var(--color-border);
  }
  .shortcuts-row span:first-child { color: var(--color-text-secondary); }
  .shortcuts-row span:last-child {
    color: var(--color-text-muted);
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    text-align: right;
  }
</style>
