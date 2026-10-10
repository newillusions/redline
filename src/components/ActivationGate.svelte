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
  import { onMount } from "svelte";
  import { activateLicense, renewLicense, getLicenseInfo, isUsable } from "$lib/license";
  import type { LicenseState } from "$lib/license";
  import {
    gateHint,
    activationRefusedText,
    activationErrorText,
    retryOnlineFailureText,
    normaliseCode,
  } from "$lib/licence-copy";

  const {
    licenseState,
    onActivated,
  }: {
    licenseState: LicenseState;
    onActivated: (state: LicenseState) => void;
  } = $props();

  let code = $state("");
  let submitting = $state(false);
  let retrying = $state(false);
  let error = $state<string | null>(null);
  let inputEl = $state<HTMLInputElement | null>(null);
  /** This computer's device fingerprint, so staff can quote it to the administrator. */
  let deviceId = $state<string | null>(null);
  let copied = $state(false);

  const headline: Record<LicenseState["state"], string> = {
    valid: "", // never rendered - App.svelte only mounts this gate when not usable
    grace: "", // never rendered - grace is usable; see LicenseGraceWarning instead
    missing: "Activate Redline",
    expired: "Licence expired",
    invalid: "Licence invalid",
    revoked: "Licence revoked",
  };

  const hint = $derived(gateHint(licenseState));

  onMount(() => {
    inputEl?.focus();
    getLicenseInfo()
      .then((info) => {
        deviceId = info.device_fingerprint || null;
      })
      .catch(() => {
        // Non-fatal: the gate works without showing the device id.
      });
  });

  async function copyDeviceId() {
    if (!deviceId) return;
    try {
      await navigator.clipboard.writeText(deviceId);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      // Clipboard unavailable: the id stays selectable on screen.
    }
  }

  async function handleRetryOnline() {
    if (retrying) return;
    retrying = true;
    error = null;
    try {
      const next = await renewLicense();
      if (isUsable(next)) {
        onActivated(next);
      } else {
        error = retryOnlineFailureText(next);
        // A server refusal changes what the gate should say (revoked, not expired).
        if (next.state === "revoked" || next.state === "invalid") onActivated(next);
      }
    } catch (e) {
      error = activationErrorText(e instanceof Error ? e.message : String(e));
    } finally {
      retrying = false;
    }
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    const trimmed = normaliseCode(code);
    if (submitting || !trimmed) return;
    submitting = true;
    error = null;
    try {
      const next = await activateLicense(trimmed);
      if (next.state === "valid") {
        onActivated(next);
      } else if (next.state === "invalid") {
        error = activationRefusedText(next.reason);
      } else {
        error = "Activation did not complete. Check the code and try again.";
      }
    } catch (e) {
      error = activationErrorText(e instanceof Error ? e.message : String(e));
    } finally {
      submitting = false;
    }
  }
</script>

<div class="gate-shell">
  <form class="gate-card" onsubmit={handleSubmit}>
    <h1 class="gate-title">{headline[licenseState.state]}</h1>
    <p class="gate-hint">{hint}</p>

    {#if licenseState.state === "expired"}
      <button
        class="gate-submit"
        type="button"
        data-testid="retry-online"
        onclick={handleRetryOnline}
        disabled={retrying || submitting}
      >
        {retrying ? "Retrying…" : "Retry online"}
      </button>
    {/if}

    <label class="gate-label" for="activation-code">Activation code</label>
    <input
      id="activation-code"
      class="gate-input"
      type="text"
      autocomplete="off"
      spellcheck="false"
      placeholder="XXXX-XXXX-XXXX"
      bind:value={code}
      bind:this={inputEl}
      disabled={submitting}
    />

    {#if error}
      <p class="gate-error">{error}</p>
    {/if}

    <button class="gate-submit" type="submit" disabled={submitting || !code.trim()}>
      {submitting ? "Activating…" : "Activate"}
    </button>

    {#if deviceId}
      <div class="gate-device" data-testid="device-id">
        <span class="gate-label">Device ID</span>
        <code class="gate-device-id">{deviceId}</code>
        <button class="gate-copy" type="button" onclick={copyDeviceId}>
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
    {/if}
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

  .gate-device {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    padding-top: var(--space-2);
    border-top: 1px solid var(--color-border);
  }
  .gate-device-id {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    font-size: var(--font-size-xs);
    color: var(--color-text-secondary);
    user-select: all;
  }
  .gate-copy {
    background: transparent;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-xs);
    padding: var(--space-1) var(--space-3);
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
