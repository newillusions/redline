<script lang="ts">
  /**
   * NavActionDialog - "what happens when this is clicked": Jump to a page (a page number or
   * label, with a view) or Open a web address. Used to create and re-point links and to set
   * a bookmark's action (docs/navigation.md RB6, RH2). Only these two exist: nothing here can
   * produce an executable action, and the address is checked again by the backend.
   */
  import type { NavTarget, TargetInput } from "$lib/navigation-ipc";
  import {
    VIEW_CHOICES,
    choiceOf,
    pageText,
    resolvePageInput,
    viewFromChoice,
    type ViewChoice,
  } from "$lib/navigation";

  const {
    title,
    confirmLabel = "OK",
    pageCount,
    labels,
    currentPage,
    initial = null,
    onConfirm,
    onCancel,
  }: {
    title: string;
    confirmLabel?: string;
    pageCount: number;
    labels: readonly string[];
    currentPage: number | null;
    initial?: NavTarget | null;
    onConfirm: (t: TargetInput) => void;
    onCancel: () => void;
  } = $props();

  // The dialog is opened fresh each time; its fields start from the props once.
  /* svelte-ignore state_referenced_locally */
  const startPage = initial?.kind === "page" ? initial.page : (currentPage ?? 0);

  /* svelte-ignore state_referenced_locally */
  let mode = $state<"page" | "url">(initial?.kind === "uri" ? "url" : "page");
  /* svelte-ignore state_referenced_locally */
  let pageInput = $state(pageText(labels, Math.min(Math.max(startPage, 0), Math.max(pageCount - 1, 0))));
  /* svelte-ignore state_referenced_locally */
  let viewChoice = $state<ViewChoice>(initial?.kind === "page" ? choiceOf(initial.view) : "inherit");
  /* svelte-ignore state_referenced_locally */
  let url = $state(initial?.kind === "uri" ? initial.uri : "https://");

  const page = $derived(resolvePageInput(pageInput, labels, pageCount));
  // Mirrors the backend allowlist (document/navigation/uri.rs, which has the last word):
  // http, https or mailto; printable ASCII without spaces; none of " < > \ ^ ` { } |.
  const urlOk = $derived.by(() => {
    const u = url.trim();
    return (
      u.length <= 2048 &&
      /^[\x21-\x7E]+$/.test(u) &&
      !/["<>\\^`{}|]/.test(u) &&
      /^(https?:\/\/[^/?#]|mailto:.)/i.test(u)
    );
  });
  const valid = $derived(mode === "page" ? page !== null : urlOk);

  function confirm() {
    if (!valid) return;
    if (mode === "page" && page !== null) {
      onConfirm({ kind: "page", page, view: viewFromChoice(viewChoice) });
    } else {
      onConfirm({ kind: "uri", uri: url.trim() });
    }
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement) && valid) {
      e.preventDefault();
      confirm();
    }
  }
</script>

<div class="dialog-backdrop" role="presentation" onclick={onCancel}></div>
<dialog open class="nav-action" aria-modal="true" aria-label={title} onkeydown={onKeyDown}>
  <h3>{title}</h3>
  <label class="choice">
    <input type="radio" name="nav-action" value="page" bind:group={mode} />
    Jump to page
  </label>
  {#if mode === "page"}
    <div class="grid">
      <label for="na-page">Page</label>
      <input
        id="na-page"
        type="text"
        bind:value={pageInput}
        aria-invalid={page === null}
        placeholder="3 or E-101"
      />
      <label for="na-view">View</label>
      <select id="na-view" bind:value={viewChoice}>
        {#each VIEW_CHOICES as v (v.value)}<option value={v.value}>{v.label}</option>{/each}
      </select>
    </div>
    {#if page === null}
      <p class="problem" role="alert">Enter a page number from 1 to {pageCount}, or a page label.</p>
    {/if}
  {/if}
  <label class="choice">
    <input type="radio" name="nav-action" value="url" bind:group={mode} />
    Open web address
  </label>
  {#if mode === "url"}
    <div class="grid">
      <label for="na-url">Address</label>
      <input id="na-url" type="text" bind:value={url} aria-invalid={!urlOk} spellcheck="false" />
    </div>
    {#if !urlOk}
      <p class="problem" role="alert">Use an address starting with https://, http:// or mailto:</p>
    {:else}
      <p class="hint">It is always shown to you before it is opened.</p>
    {/if}
  {/if}
  <div class="buttons">
    <button class="btn-confirm" onclick={confirm} disabled={!valid}>{confirmLabel}</button>
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
  .nav-action {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 901;
    margin: 0;
    min-width: 340px;
    max-width: 460px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-panel);
    color: var(--color-text);
    padding: var(--space-5);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.32);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  h3 {
    margin: 0;
    font-size: var(--font-size-base);
  }
  .choice {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--font-size-sm);
    font-weight: 600;
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-2) var(--space-3);
    align-items: center;
    font-size: var(--font-size-sm);
    padding-left: var(--space-5);
  }
  .grid input,
  .grid select {
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--font-size-sm);
  }
  .problem {
    margin: 0;
    padding-left: var(--space-5);
    font-size: var(--font-size-sm);
    color: var(--color-danger);
  }
  .hint {
    margin: 0;
    padding-left: var(--space-5);
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
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
    font-weight: 600;
  }
  .btn-confirm:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-cancel {
    background: var(--color-bg-active);
    color: var(--color-text-secondary);
    border-color: var(--color-border);
  }
</style>
