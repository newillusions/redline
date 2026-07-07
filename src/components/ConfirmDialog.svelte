<script lang="ts">
  /**
   * ConfirmDialog - generic Yes/No confirmation prompt. Shared by the
   * "remember this password?" and "save an unprotected copy?" flows so both
   * get one native <dialog> implementation instead of near-duplicate ones.
   *
   * Keyboard: Enter = confirm, Escape = cancel.
   * Follows the SavePromptDialog/PasswordPromptDialog pattern: native
   * <dialog>, CSS custom properties only, no Tailwind.
   */

  const {
    title,
    message,
    hint = null,
    confirmLabel = "Yes",
    cancelLabel = "No",
    onConfirm,
    onCancel,
  }: {
    title: string;
    message: string;
    hint?: string | null;
    confirmLabel?: string;
    cancelLabel?: string;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      onConfirm();
    }
  }
</script>

<!-- Backdrop -->
<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<!-- Dialog -->
<dialog
  open
  class="confirm-prompt"
  aria-modal="true"
  aria-label={title}
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">{message}</p>
  {#if hint}
    <p class="prompt-hint">{hint}</p>
  {/if}

  <div class="button-row">
    <button class="btn-confirm" onclick={onConfirm}>{confirmLabel}</button>
    <button class="btn-cancel" onclick={onCancel}>{cancelLabel}</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }

  .confirm-prompt {
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

  .btn-confirm,
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

  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
  .btn-cancel:hover {
    background: var(--color-bg-hover);
  }
</style>
