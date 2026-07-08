<script lang="ts">
  /**
   * StampPromptDialog - collects placement-time values for a dynamic stamp's
   * `PromptedText` fields (spec "Stamps") before the stamp's text is composed and
   * baked into its appearance. One text input per label, in the order the labels
   * were extracted from the tool's `DynamicField[]` (see `extractPromptedLabels`
   * in `$lib/markup-tools`) - `onSubmit` receives the values back in that same
   * order, ready to hand to `composeStampText`'s `prompted` array.
   *
   * Keyboard: Enter on the last field (or anywhere) submits; Escape cancels.
   * Follows the PasswordPromptDialog pattern: native <dialog>, CSS custom
   * properties only, no Tailwind.
   */

  const {
    labels,
    onSubmit,
    onCancel,
  }: {
    /** One label per PromptedText field, in field order. */
    labels: string[];
    /** Called with one value per label, in the same order, when the user submits. */
    onSubmit: (values: string[]) => void;
    /** Called when the user cancels - placement is abandoned cleanly. */
    onCancel: () => void;
  } = $props();

  let values = $state<string[]>(labels.map(() => ""));

  function submit() {
    onSubmit(values);
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
  class="stamp-prompt"
  aria-modal="true"
  aria-label="Stamp details"
  onkeydown={handleKeyDown}
>
  <p class="prompt-message">Enter stamp details</p>

  {#each labels as label, i (i)}
    <label class="field-label" for="stamp-prompt-field-{i}">{label}</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      id="stamp-prompt-field-{i}"
      class="field-input"
      type="text"
      bind:value={values[i]}
      autofocus={i === 0}
      aria-label={label}
    />
  {/each}

  <div class="button-row">
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
    <button class="btn-submit" onclick={submit}>Place Stamp</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }

  .stamp-prompt {
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
  .btn-submit:hover {
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
