<script lang="ts">
  /**
   * SavePromptDialog — asks the user whether to save, discard, or cancel
   * when closing a document with unsaved changes.
   *
   * Keyboard: Enter = Save, Escape = Cancel.
   * Accessibility: native <dialog> element with role="dialog" and aria-modal.
   *
   * Svelte 5 runes. CSS custom properties only, no Tailwind.
   */

  const {
    filename,
    onSave,
    onDiscard,
    onCancel,
  }: {
    /** The display name of the document being closed (basename, not full path). */
    filename: string;
    /** Called when the user chooses to save before closing. */
    onSave: () => void;
    /** Called when the user chooses to discard changes and close. */
    onDiscard: () => void;
    /** Called when the user cancels (keeps the document open). */
    onCancel: () => void;
  } = $props();

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      onSave();
    }
  }
</script>

<!-- Backdrop -->
<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<!-- Dialog -->
<dialog
  open
  class="save-prompt"
  aria-modal="true"
  aria-label="Unsaved changes"
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">
    Do you want to save changes to <strong>{filename}</strong>?
  </p>
  <p class="prompt-hint">Your changes will be lost if you don't save.</p>

  <div class="button-row">
    <button class="btn-save" onclick={onSave}>Save</button>
    <button class="btn-discard" onclick={onDiscard}>Don't Save</button>
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

  .save-prompt {
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
    min-width: 340px;
    max-width: 480px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    outline: none;
    /* Reset browser <dialog> default margin */
    margin: 0;
  }

  .prompt-message {
    font-size: var(--font-size-base);
    font-weight: 600;
    margin: 0 0 var(--space-2);
    color: var(--color-text);
  }

  .prompt-hint {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin: 0 0 var(--space-5);
  }

  .button-row {
    display: flex;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  /* Shared button base */
  .btn-save,
  .btn-discard,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    transition: background 100ms, border-color 100ms;
    border: 1px solid transparent;
  }

  .btn-save {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border-color: var(--color-primary);
    font-weight: 600;
  }
  .btn-save:hover {
    background: var(--color-primary-hover);
    border-color: var(--color-primary-hover);
  }

  .btn-discard {
    background: var(--color-bg-active);
    color: var(--color-danger);
    border-color: var(--color-danger);
  }
  .btn-discard:hover {
    background: var(--color-danger);
    color: #fff;
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
