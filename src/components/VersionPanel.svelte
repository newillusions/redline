<script lang="ts">
  /**
   * VersionPanel — version history list + snapshot + restore (M4 S2, spec §15/§18).
   *
   * Displays the saved version snapshots for the open document (newest first).
   * The user can take a new snapshot (with an optional label) and restore any prior
   * snapshot back over the live PDF.
   */
  import {
    listDocumentVersions,
    snapshotVersion,
    restoreDocumentVersion,
    type VersionRecord,
  } from "$lib/ipc";

  interface Props {
    docId: string;
    /** Called after a successful restore so the parent can reload the document. */
    onRestore?: () => void;
  }

  const { docId, onRestore }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let versions = $state<VersionRecord[]>([]);
  let labelInput = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  // ---------------------------------------------------------------------------
  // Load versions on mount and when docId changes
  // ---------------------------------------------------------------------------

  $effect(() => {
    void loadVersions();
  });

  async function loadVersions() {
    try {
      versions = await listDocumentVersions(docId);
    } catch (e) {
      error = String(e);
    }
  }

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------

  async function handleSnapshot() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await snapshotVersion(docId, labelInput.trim() || null);
      labelInput = "";
      await loadVersions();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function handleRestore(versionId: string) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await restoreDocumentVersion(docId, versionId);
      await loadVersions();
      onRestore?.();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Format an RFC3339 timestamp as a readable local date/time. */
  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return iso;
    }
  }
</script>

<div class="version-panel">
  <div class="version-panel__header">
    <span class="version-panel__title">Version History</span>
  </div>

  <!-- Snapshot controls -->
  <div class="version-panel__snapshot">
    <input
      class="version-panel__label-input"
      data-testid="label-input"
      type="text"
      placeholder="Optional label…"
      bind:value={labelInput}
      disabled={busy}
    />
    <button
      class="version-panel__snapshot-btn"
      data-testid="snapshot-btn"
      onclick={handleSnapshot}
      disabled={busy}
    >
      Save snapshot
    </button>
  </div>

  {#if error}
    <div class="version-panel__error" role="alert">{error}</div>
  {/if}

  <!-- Version list (newest first) -->
  <ul class="version-panel__list" role="list">
    {#each versions as ver (ver.id)}
      <li class="version-panel__item" data-testid="version-item">
        <div class="version-panel__item-info">
          <span class="version-panel__item-date">{fmtDate(ver.created_at)}</span>
          {#if ver.label}
            <span class="version-panel__item-label">{ver.label}</span>
          {/if}
          <span class="version-panel__item-file">{ver.filename}</span>
        </div>
        <button
          class="version-panel__restore-btn"
          data-testid="restore-btn"
          onclick={() => handleRestore(ver.id)}
          disabled={busy}
        >
          Restore
        </button>
      </li>
    {/each}

    {#if versions.length === 0}
      <li class="version-panel__empty">No snapshots yet</li>
    {/if}
  </ul>
</div>

<style>
  .version-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2, 8px);
    padding: var(--space-3, 12px);
    font-size: var(--text-sm, 13px);
    height: 100%;
    overflow: hidden;
  }

  .version-panel__header {
    display: flex;
    align-items: center;
    gap: var(--space-2, 8px);
  }

  .version-panel__title {
    font-weight: 600;
    color: var(--color-text-primary, #1a1a1a);
  }

  .version-panel__snapshot {
    display: flex;
    gap: var(--space-2, 8px);
  }

  .version-panel__label-input {
    flex: 1;
    padding: var(--space-1, 4px) var(--space-2, 8px);
    border: 1px solid var(--color-border, #d1d5db);
    border-radius: var(--radius-sm, 4px);
    font-size: var(--text-sm, 13px);
    background: var(--color-surface, #fff);
    color: var(--color-text-primary, #1a1a1a);
  }

  .version-panel__snapshot-btn {
    padding: var(--space-1, 4px) var(--space-2, 8px);
    background: var(--color-primary, #2563eb);
    color: var(--color-on-primary, #fff);
    border: none;
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
    font-size: var(--text-sm, 13px);
    white-space: nowrap;
  }

  .version-panel__snapshot-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .version-panel__error {
    color: var(--color-error, #dc2626);
    font-size: var(--text-xs, 11px);
    padding: var(--space-1, 4px);
    background: var(--color-error-surface, #fef2f2);
    border-radius: var(--radius-sm, 4px);
  }

  .version-panel__list {
    list-style: none;
    padding: 0;
    margin: 0;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
  }

  .version-panel__item {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2, 8px);
    padding: var(--space-2, 8px);
    border: 1px solid var(--color-border, #e5e7eb);
    border-radius: var(--radius-sm, 4px);
    background: var(--color-surface, #fff);
  }

  .version-panel__item-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .version-panel__item-date {
    font-size: var(--text-xs, 11px);
    color: var(--color-text-secondary, #6b7280);
  }

  .version-panel__item-label {
    font-size: var(--text-sm, 13px);
    color: var(--color-text-primary, #1a1a1a);
    font-weight: 500;
  }

  .version-panel__item-file {
    font-size: 10px;
    color: var(--color-text-tertiary, #9ca3af);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .version-panel__restore-btn {
    padding: var(--space-1, 4px) var(--space-2, 8px);
    background: none;
    border: 1px solid var(--color-border, #d1d5db);
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
    font-size: var(--text-xs, 11px);
    color: var(--color-text-primary, #1a1a1a);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .version-panel__restore-btn:hover:not(:disabled) {
    background: var(--color-surface-hover, #f3f4f6);
  }

  .version-panel__restore-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .version-panel__empty {
    padding: var(--space-3, 12px);
    text-align: center;
    color: var(--color-text-secondary, #6b7280);
    font-size: var(--text-sm, 13px);
  }
</style>
