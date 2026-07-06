<script lang="ts">
  /**
   * PasswordPromptDialog - asks for a PDF's password when `open_document`
   * reports the file is encrypted (PASSWORD_REQUIRED) or that a previously
   * entered password was wrong (WRONG_PASSWORD, shown via `errorHint`).
   *
   * Keyboard: Enter = submit, Escape = cancel.
   * Follows the SavePromptDialog pattern: native <dialog>, CSS custom
   * properties only, no Tailwind.
   */

  const {
    filename,
    errorHint,
    onSubmit,
    onCancel,
  }: {
    /** Display name of the file being opened (basename, not full path). */
    filename: string;
    /** Set when re-prompting after a wrong password; null on the first prompt. */
    errorHint: string | null;
    /** Called with the entered password when the user submits. */
    onSubmit: (password: string) => void;
    /** Called when the user cancels - the open attempt is abandoned cleanly. */
    onCancel: () => void;
  } = $props();

  let password = $state("");

  function submit() {
    if (!password) return;
    onSubmit(password);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  }
</script>

<!-- Backdrop -->
<div class="dialog-backdrop" role="presentation" onclick={onCancel} onkeydown={null}></div>

<!-- Dialog -->
<dialog
  open
  class="password-prompt"
  aria-modal="true"
  aria-label="Password required"
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">
    <strong>{filename}</strong> is password-protected.
  </p>

  {#if errorHint}
    <p class="prompt-error">{errorHint}</p>
  {/if}

  <label class="field-label" for="pdf-password">Password</label>
  <!-- svelte-ignore a11y_autofocus -->
  <input
    id="pdf-password"
    class="field-input"
    type="password"
    bind:value={password}
    autofocus
    aria-label="PDF password"
  />

  <div class="button-row">
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
    <button class="btn-submit" onclick={submit} disabled={!password}>Open</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }

  .password-prompt {
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

  .prompt-error {
    font-size: var(--font-size-sm);
    color: var(--color-danger);
    margin: 0 0 var(--space-2);
  }

  .field-label {
    display: block;
    font-size: var(--font-size-sm);
    color: var(--color-text);
    margin-bottom: var(--space-1);
  }

  .field-input {
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-input);
    color: var(--color-text);
    font-size: var(--font-size-base);
    margin-bottom: var(--space-5);
    box-sizing: border-box;
  }

  .button-row {
    display: flex;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  .btn-submit,
  .btn-cancel {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    transition: background 100ms, border-color 100ms;
    border: 1px solid transparent;
  }

  .btn-submit {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border-color: var(--color-primary);
    font-weight: 600;
  }
  .btn-submit:hover:not(:disabled) {
    background: var(--color-primary-hover);
    border-color: var(--color-primary-hover);
  }
  .btn-submit:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
