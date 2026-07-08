<script lang="ts">
  /**
   * S2b activation gate - blocks the whole app until a valid, device-bound
   * token is present. Shown by App.svelte whenever licenseState.state !==
   * "valid" (missing / invalid / expired all render here, with a reason-
   * specific message so a locked-out user knows why, not just that they are).
   *
   * Follows the ConfirmDialog/SavePromptDialog pattern: CSS custom
   * properties only, no Tailwind. Full-screen rather than a <dialog> overlay,
   * since there is no app content behind it to dim.
   */
  import { activateLicense } from "$lib/license";
  import type { LicenseState } from "$lib/license";

  const {
    licenseState,
    onActivated,
  }: {
    licenseState: LicenseState;
    onActivated: (state: LicenseState) => void;
  } = $props();

  let code = $state("");
  let submitting = $state(false);
  let error = $state<string | null>(null);

  const headline: Record<LicenseState["state"], string> = {
    valid: "", // never rendered - App.svelte only mounts this gate when not valid
    missing: "Activate Redline",
    expired: "License expired",
    invalid: "License invalid",
  };

  const hint = $derived.by(() => {
    if (licenseState.state === "invalid") {
      if (licenseState.reason === "device_mismatch") {
        return "This activation is bound to a different device. Contact the administrator for a new activation code.";
      }
      return `This installation's license could not be verified (${licenseState.reason}). Enter a new activation code below.`;
    }
    if (licenseState.state === "expired") {
      return "Your license has expired and could not be renewed automatically. Enter a new activation code, or contact the administrator.";
    }
    return "Enter the activation code provided by your administrator to unlock Redline on this device.";
  });

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (submitting || !code.trim()) return;
    submitting = true;
    error = null;
    try {
      const next = await activateLicense(code.trim());
      if (next.state === "valid") {
        onActivated(next);
      } else if (next.state === "invalid") {
        error = `Activation refused (${next.reason}).`;
      } else {
        error = "Activation did not complete. Check the code and try again.";
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }
</script>

<div class="gate-shell">
  <form class="gate-card" onsubmit={handleSubmit}>
    <h1 class="gate-title">{headline[licenseState.state]}</h1>
    <p class="gate-hint">{hint}</p>

    <label class="gate-label" for="activation-code">Activation code</label>
    <input
      id="activation-code"
      class="gate-input"
      type="text"
      autocomplete="off"
      spellcheck="false"
      placeholder="XXXX-XXXX-XXXX"
      bind:value={code}
      disabled={submitting}
    />

    {#if error}
      <p class="gate-error">{error}</p>
    {/if}

    <button class="gate-submit" type="submit" disabled={submitting || !code.trim()}>
      {submitting ? "Activating…" : "Activate"}
    </button>
  </form>
</div>

<style>
  .gate-shell {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background: var(--color-bg);
    color: var(--color-text);
  }

  .gate-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    width: 360px;
    padding: var(--space-6, 24px) var(--space-5);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
  }

  .gate-title {
    margin: 0;
    font-size: var(--font-size-lg);
    font-weight: 600;
    color: var(--color-text);
  }

  .gate-hint {
    margin: 0;
    font-size: var(--font-size-sm);
    color: var(--color-text-secondary);
  }

  .gate-label {
    font-size: var(--font-size-xs);
    font-weight: 600;
    color: var(--color-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .gate-input {
    font-size: var(--font-size-base);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-active);
    color: var(--color-text);
  }
  .gate-input:disabled {
    opacity: 0.6;
  }

  .gate-error {
    margin: 0;
    font-size: var(--font-size-sm);
    color: var(--color-danger, #dc2626);
  }

  .gate-submit {
    margin-top: var(--space-2);
    background: var(--color-primary);
    border: none;
    border-radius: var(--radius-md);
    color: var(--color-text-inverse);
    cursor: pointer;
    font-size: var(--font-size-base);
    font-weight: 600;
    padding: var(--space-2) var(--space-5);
    transition: background 120ms;
  }
  .gate-submit:hover:not(:disabled) {
    background: var(--color-primary-hover);
  }
  .gate-submit:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
