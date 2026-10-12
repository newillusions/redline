<script lang="ts">
  /**
   * StaleFileDialog - shown when a save is refused because another program changed the
   * PDF on disk after redline opened it. The user's mark-ups are still in memory; this
   * offers the two ways to keep them (neither writes over the changed file):
   *   - Save As (rebuild): write them to a NEW file built from the bytes redline loaded.
   *   - Reload and keep my mark-ups: re-read the changed file and re-apply the edits.
   * Native <dialog>, CSS custom properties only, Escape = cancel (same pattern as
   * ConfirmDialog).
   */

  const {
    filename,
    canRebuild = true,
    onRebuild,
    onReload,
    onCancel,
  }: {
    filename: string;
    /** False when redline holds no copy of the loaded bytes (file too large to keep). */
    canRebuild?: boolean;
    onRebuild: () => void;
    onReload: () => void;
    onCancel: () => void;
  } = $props();

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    }
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<dialog
  open
  class="stale-prompt"
  aria-modal="true"
  aria-label="The PDF changed on disk"
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">{filename} was changed by another program.</p>
  <p class="prompt-hint">
    Nothing was saved, so your mark-ups are not lost. Saving over the changed file could
    duplicate or remove annotations, so choose how to keep them:
  </p>
  <ul class="prompt-options">
    <li>
      <strong>Save As (rebuild)</strong> writes your mark-ups to a new file, built from the PDF as
      you opened it. The changed file is left alone, and the other program's changes will not be
      in the new file.
    </li>
    <li>
      <strong>Reload and keep my mark-ups</strong> reads the changed file and re-applies your
      unsaved edits where they still fit. Any that cannot be applied are listed.
    </li>
  </ul>

  <div class="button-row">
    <button class="btn-confirm" onclick={onReload}>Reload and keep my mark-ups</button>
    <button class="btn-secondary" onclick={onRebuild} disabled={!canRebuild}>
      Save As (rebuild)…
    </button>
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }

  .stale-prompt {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 901;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    color: var(--color-text);
    padding: var(--space-5);
    min-width: 360px;
    max-width: 520px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    outline: none;
    margin: 0;
  }

  .prompt-message {
    font-size: var(--font-size-base);
    font-weight: 600;
    margin: 0 0 var(--space-2);
  }

  .prompt-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin: 0 0 var(--space-2);
  }

  .prompt-options {
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    margin: 0 0 var(--space-5);
    padding-left: var(--space-5);
  }
  .prompt-options li {
    margin-bottom: var(--space-2);
  }

  .button-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  .btn-confirm,
  .btn-secondary,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    transition: background 100ms, border-color 100ms;
    border: 1px solid transparent;
  }

  .btn-confirm {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border-color: var(--color-primary);
    font-weight: 600;
  }
  .btn-confirm:hover {
    background: var(--color-primary-hover);
    border-color: var(--color-primary-hover);
  }

  .btn-secondary,
  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
  .btn-secondary:hover:not(:disabled),
  .btn-cancel:hover {
    background: var(--color-bg-hover);
  }
  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
