<script lang="ts">
  /**
   * Shown once per launch whenever `licenseState.state === "grace"` - the
   * app is running on an offline-expired token still inside the server's
   * grace window. Non-blocking (App.svelte renders the normal app content
   * behind/alongside this), unlike ActivationGate which fully blocks.
   *
   * Per Martin's spec (2026-08-05): "warnings when you open the app each
   * time... stating plainly how long is left and that it will stop
   * working" - so this states the real deadline date AND a day count, with
   * one clear action (reactivate) rather than a vague "license expiring"
   * notice. Follows the ConfirmDialog pattern: native <dialog>, CSS custom
   * properties only, Escape dismisses.
   */
  import type { LicenseGrace } from "$lib/license";

  const {
    state,
    onOpenSettings,
    onDismiss,
  }: {
    state: LicenseGrace;
    onOpenSettings: () => void;
    onDismiss: () => void;
  } = $props();

  const deadlineLabel = $derived.by(() => {
    const d = new Date(state.grace_deadline);
    return Number.isNaN(d.getTime())
      ? state.grace_deadline
      : d.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
  });

  const dayWord = $derived(state.days_remaining === 1 ? "day" : "days");

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onDismiss();
    }
  }
</script>

<!-- Backdrop (non-blocking intent, but the dialog itself still needs a
     click-away/Escape affordance like every other modal in this app). -->
<div class="dialog-backdrop" role="presentation" onclick={onDismiss} onkeydown={null}></div>

<dialog
  open
  class="grace-warning"
  aria-modal="true"
  aria-label="License offline"
  onkeydown={handleKeyDown}
>
  <h3 class="grace-title">Redline could not verify your license online</h3>
  <p class="grace-message">
    This device has been offline since {new Date(state.expired_at).toLocaleDateString(undefined, {
      year: "numeric",
      month: "long",
      day: "numeric",
    })}. Redline will keep working for
    <strong>{state.days_remaining} {dayWord}</strong>
    (until {deadlineLabel}), then stop working until it can reconnect or a new activation code is entered.
  </p>

  <div class="button-row">
    <button class="btn-primary" onclick={onOpenSettings}>Reactivate in Settings…</button>
    <button class="btn-secondary" onclick={onDismiss}>Continue</button>
  </div>
</dialog>

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 1900;
  }

  .grace-warning {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 1901;
    border: 1px solid var(--color-warning, var(--color-border));
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    color: var(--color-text);
    padding: var(--space-5);
    min-width: 380px;
    max-width: 480px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    outline: none;
    margin: 0;
  }

  .grace-title {
    margin: 0 0 var(--space-2);
    font-size: var(--font-size-base);
    font-weight: 600;
    color: var(--color-text);
  }

  .grace-message {
    margin: 0 0 var(--space-5);
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
    line-height: 1.5;
  }

  .button-row {
    display: flex;
    gap: var(--space-2);
    justify-content: flex-end;
  }

  .btn-primary,
  .btn-secondary {
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-4);
    cursor: pointer;
    transition: background 100ms, border-color 100ms;
    border: 1px solid transparent;
  }

  .btn-primary {
    background: var(--color-primary);
    color: var(--color-text-inverse);
    border-color: var(--color-primary);
    font-weight: 600;
  }
  .btn-primary:hover {
    background: var(--color-primary-hover);
  }

  .btn-secondary {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
  .btn-secondary:hover {
    background: var(--color-bg-hover);
  }
</style>
