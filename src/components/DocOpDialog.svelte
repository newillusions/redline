<script lang="ts">
  /**
   * DocOpDialog - confirmation for the document-rewriting actions (Flatten, Optimise,
   * Apply Redactions). States what will happen, then asks whether to write a copy
   * (default, original untouched) or replace the original (an automatic History
   * snapshot is taken first). UX audit F1, 2026-10-10.
   *
   * Keyboard: Enter = continue, Escape = cancel. Follows the ConfirmDialog pattern:
   * native <dialog>, CSS custom properties only.
   */
  import { docOpDialogCopy, baseName } from "$lib/doc-op-flow";
  import type { DocOpKind, DocOpMode } from "$lib/doc-op-flow";

  const {
    kind,
    path,
    onConfirm,
    onCancel,
  }: {
    kind: DocOpKind;
    /** Full path of the document the action will run on. */
    path: string;
    onConfirm: (mode: DocOpMode) => void;
    onCancel: () => void;
  } = $props();

  const copy = $derived(docOpDialogCopy(kind, baseName(path)));
  let mode = $state<DocOpMode>("copy");

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      onConfirm(mode);
    }
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<dialog open class="docop-dialog" aria-modal="true" aria-label={copy.title} onkeydown={handleKeyDown}>
  <h2 class="docop-title">{copy.title}</h2>
  <p class="docop-summary">{copy.summary}</p>

  <fieldset class="docop-choices">
    <legend class="docop-legend">Where should the result go?</legend>
    <label class="docop-choice">
      <input type="radio" name="docop-mode" value="copy" bind:group={mode} />
      <span>{copy.copyLabel}</span>
    </label>
    <label class="docop-choice">
      <input type="radio" name="docop-mode" value="replace" bind:group={mode} />
      <span>{copy.replaceLabel}</span>
    </label>
  </fieldset>

  <div class="button-row">
    <button class="btn-confirm" onclick={() => onConfirm(mode)}>{copy.confirmLabel}</button>
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

  .docop-dialog {
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

  .docop-title {
    margin: 0 0 var(--space-2);
    font-size: var(--font-size-base);
    font-weight: 600;
  }

  .docop-summary {
    margin: 0 0 var(--space-4);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
  }

  .docop-choices {
    margin: 0 0 var(--space-5);
    padding: 0;
    border: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .docop-legend {
    padding: 0;
    margin-bottom: var(--space-2);
    font-size: var(--font-size-xs);
    font-weight: 600;
    color: var(--color-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .docop-choice {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }

  .button-row {
    display: flex;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  .btn-confirm,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
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

  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
  .btn-cancel:hover {
    background: var(--color-bg-hover);
  }
</style>
