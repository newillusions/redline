<script lang="ts">
  import { onMount } from "svelte";
  import type { MarkupStore } from "$lib/markup-store.svelte";
  import type { UserRef } from "$lib/ipc";
  import { getUserIdentity } from "$lib/ipc";
  import {
    patchAppearance,
    patchFields,
    patchStatus,
    patchNoteIcon,
    NOTE_ICONS,
    commonValue,
    FONT_FAMILIES,
    FONT_SIZES,
  } from "$lib/markup-properties";
  import type { MarkupWorkflow } from "$lib/ipc";
  import { patchAddReply, patchDeleteReply } from "$lib/markup-properties";
  import { reviewSummary } from "$lib/markup-review";

  const { store }: { store: MarkupStore } = $props();

  let identity = $state<UserRef | null>(null);

  onMount(() => {
    getUserIdentity()
      .then((u) => { identity = u; })
      .catch(() => {});
  });

  const selected = $derived(store.selectedMarkups);
  const mode = $derived(selected.length === 0 ? "draft" : "selection");

  // True only when every relevant markup is a Text or Callout - the ONLY types with a
  // FreeText box border (`outline_color`), distinct from the glyph colour. Gates the "Box
  // outline" control so it never shows for shape types where it is meaningless (e.g.
  // Rectangle has no box border of its own - its border IS its stroke) and can't be
  // mistaken for a second fill/outline setting on those types.
  const isTextBoxContext = $derived(
    mode === "draft"
      ? store.activeTool === "Text" || store.activeTool === "Callout"
      : selected.length > 0 && selected.every((m) => m.markup_type === "Text" || m.markup_type === "Callout"),
  );

  // Text mark-up (Underline / StrikeOut / Squiggly) and the sticky Note have no fill, line
  // style or box: showing those controls would offer settings that do nothing. Colour,
  // weight (rule thickness), opacity, subject, status and author stay.
  const isRuleOrNote = (t: string) => t === "Underline" || t === "StrikeOut" || t === "Squiggly" || t === "Note";
  const hideShapeControls = $derived(
    mode === "draft"
      ? isRuleOrNote(store.activeTool)
      : selected.length > 0 && selected.every((m) => isRuleOrNote(m.markup_type)),
  );
  const isNoteContext = $derived(
    mode === "draft" ? store.activeTool === "Note" : selected.length > 0 && selected.every((m) => m.markup_type === "Note"),
  );

  // ---------------------------------------------------------------------------
  // Commit helpers
  // ---------------------------------------------------------------------------

  function by(fallback: UserRef): UserRef {
    return identity ?? fallback;
  }

  function commitAppearancePatch(patch: Parameters<typeof patchAppearance>[1]) {
    if (mode === "draft") {
      // Immutable replace (not Object.assign) so draftAppearance gets a new
      // object reference. This satisfies Svelte 5 reactivity and, critically,
      // avoids mutating a reference that existing markups still hold — belt-
      // and-suspenders on top of buildMarkup's own clone.
      store.draftAppearance = { ...store.draftAppearance, ...patch };
    } else {
      const now = new Date().toISOString();
      const pairs = selected.map((m) => ({
        before: m,
        after: patchAppearance(m, patch, by(m.audit.modified_by), now),
      }));
      store.applyBatch(pairs);
    }
  }

  function commitFieldPatch(patch: Parameters<typeof patchFields>[1]) {
    const now = new Date().toISOString();
    const pairs = selected.map((m) => ({
      before: m,
      after: patchFields(m, patch, by(m.audit.modified_by), now),
    }));
    store.applyBatch(pairs);
  }

  function commitNoteIcon(icon: string) {
    const now = new Date().toISOString();
    const pairs = selected.map((m) => ({ before: m, after: patchNoteIcon(m, icon, by(m.audit.modified_by), now) }));
    store.applyBatch(pairs);
  }

  function noteIconValue(): string {
    return commonValue(selected, (m) => m.note?.icon ?? "Note") ?? "";
  }

  function authorValue(): string {
    return commonValue(selected, (m) => m.audit.created_by.display_name) ?? "";
  }

  function commitStatusPatch(status: MarkupWorkflow["status"]) {
    const now = new Date().toISOString();
    const pairs = selected.map((m) => ({
      before: m,
      after: patchStatus(m, status, by(m.audit.modified_by), now),
    }));
    store.applyBatch(pairs);
  }

  // ---------------------------------------------------------------------------
  // Control value resolution
  // ---------------------------------------------------------------------------

  function colorValue(): string {
    if (mode === "draft") return store.draftAppearance.color;
    return commonValue(selected, (m) => m.appearance.color) ?? "";
  }

  function lineWeightValue(): number | "" {
    if (mode === "draft") return store.draftAppearance.line_weight;
    const v = commonValue(selected, (m) => m.appearance.line_weight);
    return v ?? "";
  }

  function opacityValue(): number {
    if (mode === "draft") return store.draftAppearance.opacity;
    return commonValue(selected, (m) => m.appearance.opacity) ?? 1;
  }

  function opacityDisplay(): string {
    const v = opacityValue();
    return `${Math.round((v as number) * 100)}%`;
  }

  function fillValue(): string {
    if (mode === "draft") return store.draftAppearance.fill ?? "#ffffff";
    return commonValue(selected, (m) => m.appearance.fill ?? "") ?? "#ffffff";
  }

  function noFillChecked(): boolean {
    if (mode === "draft") return store.draftAppearance.fill === null;
    const v = commonValue(selected, (m) => m.appearance.fill);
    // if all null -> checked; if undefined (mixed) -> leave unchecked
    return v === null;
  }

  // Box-border colour for Text/Callout. Falls back to the glyph colour when unset, so the
  // swatch always shows the effective border; editing it writes a distinct outline_color.
  function outlineColorValue(): string {
    if (mode === "draft") return store.draftAppearance.outline_color ?? store.draftAppearance.color;
    return commonValue(selected, (m) => m.appearance.outline_color ?? m.appearance.color) ?? "";
  }

  // Fill alpha, independent of overall opacity. Defaults to 1 (fully opaque fill) when unset.
  function fillOpacityValue(): number {
    if (mode === "draft") return store.draftAppearance.fill_opacity ?? 1;
    return commonValue(selected, (m) => m.appearance.fill_opacity ?? 1) ?? 1;
  }

  function fillOpacityDisplay(): string {
    return `${Math.round((fillOpacityValue() as number) * 100)}%`;
  }

  function lineStyleValue(): string {
    if (mode === "draft") return store.draftAppearance.line_style;
    return commonValue(selected, (m) => m.appearance.line_style) ?? "";
  }

  function fontFamilyValue(): string {
    if (mode === "draft") return store.draftAppearance.font?.family ?? "Helvetica";
    return commonValue(selected, (m) => m.appearance.font?.family ?? "") ?? "";
  }

  function fontSizeValue(): number | "" {
    if (mode === "draft") return store.draftAppearance.font?.size_pt ?? 12;
    const v = commonValue(selected, (m) => m.appearance.font?.size_pt ?? 0);
    return v ?? "";
  }

  function contentsValue(): string {
    return commonValue(selected, (m) => m.contents ?? "") ?? "";
  }

  function subjectValue(): string {
    return commonValue(selected, (m) => m.subject ?? "") ?? "";
  }

  function layerValue(): string {
    return commonValue(selected, (m) => m.layer ?? "") ?? "";
  }

  function statusValue(): string {
    return commonValue(selected, (m) => m.workflow.status) ?? "";
  }

  /** 1-based page display for the properties panel (read-only). */
  function pageDisplay(): string {
    const v = commonValue(selected, (m) => m.page);
    if (v === undefined) return "Multiple";
    return String((v as number) + 1);
  }

  // ---------------------------------------------------------------------------
  // Event handlers
  // ---------------------------------------------------------------------------

  function onColorInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitAppearancePatch({ color: v });
  }

  function onLineWeightInput(e: Event) {
    const raw = parseFloat((e.target as HTMLInputElement).value);
    if (!isNaN(raw)) commitAppearancePatch({ line_weight: raw });
  }

  function onOpacityInput(e: Event) {
    const raw = parseFloat((e.target as HTMLInputElement).value);
    if (!isNaN(raw)) commitAppearancePatch({ opacity: raw });
  }

  function onFillColorInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    if (!noFillChecked()) commitAppearancePatch({ fill: v });
  }

  function onNoFillChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    if (checked) {
      commitAppearancePatch({ fill: null });
    } else {
      // restore a default fill color
      commitAppearancePatch({ fill: "#ffffff" });
    }
  }

  function onOutlineColorInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitAppearancePatch({ outline_color: v });
  }

  function onFillOpacityInput(e: Event) {
    const raw = parseFloat((e.target as HTMLInputElement).value);
    if (!isNaN(raw)) commitAppearancePatch({ fill_opacity: raw });
  }

  function onLineStyleChange(e: Event) {
    const v = (e.target as HTMLSelectElement).value as "Solid" | "Dashed" | "Dotted";
    commitAppearancePatch({ line_style: v });
  }

  function onFontFamilyChange(e: Event) {
    const family = (e.target as HTMLSelectElement).value;
    const currentFont = mode === "draft"
      ? store.draftAppearance.font
      : commonValue(selected, (m) => m.appearance.font);
    const size_pt = currentFont?.size_pt ?? 12;
    commitAppearancePatch({ font: { family, size_pt } });
  }

  function onFontSizeChange(e: Event) {
    const size_pt = parseFloat((e.target as HTMLSelectElement).value);
    if (isNaN(size_pt)) return;
    const currentFont = mode === "draft"
      ? store.draftAppearance.font
      : commonValue(selected, (m) => m.appearance.font);
    const family = currentFont?.family ?? "Helvetica";
    commitAppearancePatch({ font: { family, size_pt } });
  }

  function onContentsInput(e: Event) {
    const v = (e.target as HTMLTextAreaElement).value;
    commitFieldPatch({ contents: v || null });
  }

  function onSubjectInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitFieldPatch({ subject: v || null });
  }

  function onLayerInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    commitFieldPatch({ layer: v || null });
  }

  // Reply thread + status history (single selection). Replies persist as standard PDF
  // /IRT annotations, so Bluebeam Revu and Acrobat show them.
  const single = $derived(selected.length === 1 ? selected[0] : null);
  const review = $derived(single ? reviewSummary(single) : null);
  let replyDraft = $state("");
  let replyTo = $state<string | null>(null);

  function fmtWhen(iso: string): string {
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
  }

  function submitReply() {
    if (!single) return;
    const after = patchAddReply(
      single,
      replyDraft,
      by(single.audit.modified_by),
      new Date().toISOString(),
      replyTo,
    );
    if (after === single) return;
    store.applyBatch([{ before: single, after }]);
    replyDraft = "";
    replyTo = null;
  }

  function removeReply(id: string) {
    if (!single) return;
    const after = patchDeleteReply(single, id, by(single.audit.modified_by), new Date().toISOString());
    if (after !== single) store.applyBatch([{ before: single, after }]);
  }

  function onReplyKey(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submitReply();
    }
  }

  function onStatusChange(e: Event) {
    const v = (e.target as HTMLSelectElement).value as MarkupWorkflow["status"];
    commitStatusPatch(v);
  }
</script>

<div class="properties-panel">
  {#if mode === "draft"}
    <p class="panel-mode-hint muted">Defaults for new markups</p>
  {:else}
    <p class="panel-mode-hint selected-count">{selected.length} markup(s) selected</p>
  {/if}

  <!-- Appearance group -->
  <section class="prop-group">
    <h3 class="prop-group-title">Appearance</h3>

    <!-- Color -->
    <div class="prop-row">
      <label for="prop-color" class="prop-label">Color</label>
      <div class="prop-control color-row">
        <div class="color-swatch" style="background: {colorValue() || 'transparent'}"></div>
        <input
          id="prop-color"
          type="color"
          data-field="color"
          data-indeterminate={colorValue() === "" ? "true" : undefined}
          value={colorValue() || "#000000"}
          oninput={onColorInput}
        />
      </div>
    </div>

    <!-- Line weight -->
    <div class="prop-row">
      <label for="prop-line-weight" class="prop-label">Line weight (pt)</label>
      <input
        id="prop-line-weight"
        type="number"
        data-field="line_weight"
        min="0"
        step="0.5"
        value={lineWeightValue()}
        oninput={onLineWeightInput}
        class="prop-number"
      />
    </div>

    <!-- Opacity: stroke/line opacity. For shapes with a Fill (below), this is independent
         of Fill opacity - setting one never moves the other. For line-only shapes with no
         Fill control (Line/Arrow/Polyline/Ink/Callout leader/count markers), this is the
         shape's only opacity control. -->
    <div class="prop-row">
      <label for="prop-opacity" class="prop-label">Opacity</label>
      <div class="prop-control opacity-row">
        <input
          id="prop-opacity"
          type="range"
          data-field="opacity"
          min="0"
          max="1"
          step="0.05"
          value={opacityValue()}
          oninput={onOpacityInput}
          class="prop-range"
        />
        <span class="opacity-readout muted">{opacityDisplay()}</span>
      </div>
    </div>

    <!-- Fill group: ONE consolidated block (colour + no-fill toggle + independent fill
         opacity) - previously split across three separate, confusingly-ordered rows with
         "Box outline" wedged between the fill colour and its opacity, reading as duplicated
         fill settings. Box outline (a distinct, FreeText-only concept) lives in its own
         section below, gated to Text/Callout so it never shows for shapes that have no
         box of their own. -->
    {#if !hideShapeControls}
    <div class="prop-subgroup">
      <p class="prop-subgroup-title muted">Fill</p>

      <div class="prop-row">
        <label for="prop-fill" class="prop-label">Colour</label>
        <div class="prop-control fill-row">
          <input
            id="prop-fill"
            type="color"
            data-field="fill_color"
            value={fillValue()}
            disabled={noFillChecked()}
            oninput={onFillColorInput}
          />
          <label class="no-fill-label">
            <input
              type="checkbox"
              data-field="no_fill"
              checked={noFillChecked()}
              onchange={onNoFillChange}
            />
            No fill
          </label>
        </div>
      </div>

      <!-- Fill opacity: independent of the overall/stroke Opacity control above - setting
           one never moves the other (see Appearance.opacity docs in src-tauri/src/markup/mod.rs). -->
      <div class="prop-row">
        <label for="prop-fill-opacity" class="prop-label">Opacity</label>
        <div class="prop-control opacity-row">
          <input
            id="prop-fill-opacity"
            type="range"
            data-field="fill_opacity"
            min="0"
            max="1"
            step="0.05"
            value={fillOpacityValue()}
            oninput={onFillOpacityInput}
            class="prop-range"
          />
          <span class="opacity-readout muted">{fillOpacityDisplay()}</span>
        </div>
      </div>
    </div>

    {/if}

    <!-- Text box border (Text/Callout FreeText box border colour - semantically distinct
         from both the fill above and the glyph/line colour: it is the rectangle drawn
         around a Text or Callout note. Only shown for those types so it is never mistaken
         for a second fill/outline setting on shapes that have no box of their own. -->
    {#if isTextBoxContext}
      <div class="prop-row">
        <label for="prop-outline" class="prop-label">Text box border</label>
        <div class="prop-control color-row">
          <div class="color-swatch" style="background: {outlineColorValue() || 'transparent'}"></div>
          <input
            id="prop-outline"
            type="color"
            data-field="outline_color"
            data-indeterminate={outlineColorValue() === "" ? "true" : undefined}
            value={outlineColorValue() || "#000000"}
            oninput={onOutlineColorInput}
          />
        </div>
      </div>
    {/if}

    <!-- Line style -->
    {#if !hideShapeControls}
    <div class="prop-row">
      <label for="prop-line-style" class="prop-label">Line style</label>
      <select
        id="prop-line-style"
        data-field="line_style"
        value={lineStyleValue()}
        onchange={onLineStyleChange}
        class="prop-select"
      >
        {#if !lineStyleValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        <option value="Solid">Solid</option>
        <option value="Dashed">Dashed</option>
        <option value="Dotted">Dotted</option>
      </select>
    </div>
    {/if}
  </section>

  <!-- Font group (shown in both draft and selection mode) -->
  <section class="prop-group">
    <h3 class="prop-group-title">Font</h3>

    <div class="prop-row">
      <label for="prop-font-family" class="prop-label">Family</label>
      <select
        id="prop-font-family"
        data-field="font_family"
        value={fontFamilyValue()}
        onchange={onFontFamilyChange}
        class="prop-select"
      >
        {#if !fontFamilyValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        {#each FONT_FAMILIES as family (family)}
          <option value={family}>{family}</option>
        {/each}
      </select>
    </div>

    <div class="prop-row">
      <label for="prop-font-size" class="prop-label">Size (pt)</label>
      <select
        id="prop-font-size"
        data-field="font_size"
        value={fontSizeValue()}
        onchange={onFontSizeChange}
        class="prop-select"
      >
        {#if !fontSizeValue()}
          <option value="" disabled selected>Mixed</option>
        {/if}
        {#each FONT_SIZES as size (size)}
          <option value={size}>{size}</option>
        {/each}
      </select>
    </div>
  </section>

  <!-- Text fields (selection mode only) -->
  {#if mode === "selection"}
    <section class="prop-group">
      <h3 class="prop-group-title">Content</h3>

      <div class="prop-row prop-row-stacked">
        <label for="prop-contents" class="prop-label">Contents</label>
        <textarea
          id="prop-contents"
          data-field="contents"
          rows="3"
          value={contentsValue()}
          oninput={onContentsInput}
          class="prop-textarea"
          placeholder={contentsValue() === "" && selected.length > 1 ? "Mixed" : ""}
        ></textarea>
      </div>

      <div class="prop-row">
        <label for="prop-subject" class="prop-label">Subject</label>
        <input
          id="prop-subject"
          type="text"
          data-field="subject"
          value={subjectValue()}
          oninput={onSubjectInput}
          class="prop-text"
          placeholder={subjectValue() === "" && selected.length > 1 ? "Mixed" : ""}
        />
      </div>

      <div class="prop-row">
        <span class="prop-label">Author</span>
        <span class="prop-value muted" data-field="author">{authorValue() || "Mixed"}</span>
      </div>

      {#if isNoteContext}
        <div class="prop-row">
          <label for="prop-note-icon" class="prop-label">Icon</label>
          <select
            id="prop-note-icon"
            data-field="note_icon"
            value={noteIconValue()}
            onchange={(e) => commitNoteIcon((e.target as HTMLSelectElement).value)}
            class="prop-select"
          >
            {#if !noteIconValue()}
              <option value="" disabled selected>Mixed</option>
            {/if}
            {#each NOTE_ICONS as icon (icon)}
              <option value={icon}>{icon}</option>
            {/each}
          </select>
        </div>
      {/if}

      <div class="prop-row">
        <label for="prop-layer" class="prop-label">Layer</label>
        <input
          id="prop-layer"
          type="text"
          data-field="layer"
          value={layerValue()}
          oninput={onLayerInput}
          class="prop-text"
          placeholder={layerValue() === "" && selected.length > 1 ? "Mixed" : ""}
        />
      </div>

      <div class="prop-row">
        <span class="prop-label">Page</span>
        <span class="prop-value muted">{pageDisplay()}</span>
      </div>
    </section>

    <!-- Workflow group: review status over the reserved workflow fields (spec §6 decision
         f) that already round-trip through the PDF via /RLWorkflowExtra (PR #52). This is
         the first UI surface that writes to them - assignee/thread stay unedited for now. -->
    <section class="prop-group">
      <h3 class="prop-group-title">Workflow</h3>

      <div class="prop-row">
        <label for="prop-status" class="prop-label">Status</label>
        <select
          id="prop-status"
          data-field="status"
          value={statusValue()}
          onchange={onStatusChange}
          class="prop-select"
        >
          {#if !statusValue()}
            <option value="" disabled selected>Mixed</option>
          {/if}
          <option value="None">None</option>
          <option value="Accepted">Accepted</option>
          <option value="Rejected">Rejected</option>
          <option value="Completed">Completed</option>
          <option value="Cancelled">Cancelled</option>
        </select>
      </div>

      {#if review && single}
        {#if review.statusBy && review.statusAt}
          <p class="status-meta muted" data-field="status-meta">
            Set by {review.statusBy}, {fmtWhen(review.statusAt)}
          </p>
        {/if}

        <h4 class="prop-subtitle">Replies ({review.replyCount})</h4>
        <ul class="reply-thread" data-field="reply-thread">
          {#each review.thread as row (row.reply.id)}
            <li class="reply" style:margin-left="{row.depth * 12}px" data-reply-id={row.reply.id}>
              <div class="reply-head">
                <span class="reply-author">{row.reply.author.display_name || "Unknown"}</span>
                <span class="muted">{fmtWhen(row.reply.at)}</span>
              </div>
              <div class="reply-body">{row.reply.contents}</div>
              <div class="reply-actions">
                <button type="button" class="link-btn" onclick={() => (replyTo = row.reply.id)}>Reply</button>
                <button type="button" class="link-btn" onclick={() => removeReply(row.reply.id)}>Delete</button>
              </div>
            </li>
          {/each}
        </ul>
        {#if replyTo}
          <p class="muted reply-target">
            Replying to a reply
            <button type="button" class="link-btn" onclick={() => (replyTo = null)}>cancel</button>
          </p>
        {/if}
        <textarea
          class="prop-textarea"
          data-field="reply-input"
          rows="2"
          placeholder="Reply (Ctrl+Enter to send)"
          bind:value={replyDraft}
          onkeydown={onReplyKey}
        ></textarea>
        <button type="button" class="prop-button" data-action="send-reply" onclick={submitReply} disabled={!replyDraft.trim()}>
          Reply
        </button>

        {#if (single.workflow.status_history ?? []).length > 0}
          <h4 class="prop-subtitle">Status history</h4>
          <ul class="status-history" data-field="status-history">
            {#each [...(single.workflow.status_history ?? [])].reverse() as c (c.id)}
              <li>
                <span class="reply-author">{c.state}</span>
                {#if c.model !== "Review"}<span class="muted">({c.model})</span>{/if}
                {#if c.on_reply}<span class="muted">(on a reply)</span>{/if}
                <span class="muted">{c.author.display_name || "Unknown"}, {fmtWhen(c.at)}</span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>
  {/if}
</div>

<style>
  .properties-panel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    overflow-y: auto;
    flex: 1;
  }

  .panel-mode-hint {
    font-size: var(--font-size-sm);
    margin: 0 0 var(--space-1) 0;
    font-weight: 500;
  }

  .panel-mode-hint.muted {
    color: var(--color-text-muted);
  }

  .panel-mode-hint.selected-count {
    color: var(--color-text);
  }

  .prop-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2) 0;
    border-bottom: 1px solid var(--color-border);
  }

  .prop-group:last-child {
    border-bottom: none;
  }

  .prop-group-title {
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-muted);
    margin: 0 0 var(--space-1) 0;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  /* Fill subgroup: visually unifies colour + no-fill + opacity as ONE consolidated
     control (fixes "Fill shown twice" - these previously read as three separate,
     unrelated rows with Box outline wedged in between). */
  .prop-subgroup {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2);
    margin: var(--space-1) 0;
    background: var(--color-bg-active, transparent);
    border-radius: var(--radius-sm);
  }

  .prop-subgroup-title {
    font-size: var(--font-size-sm);
    font-weight: 500;
    margin: 0;
  }

  .prop-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--space-7);
  }

  .prop-row-stacked {
    flex-direction: column;
    align-items: flex-start;
  }

  .prop-label {
    flex: 0 0 auto;
    min-width: 90px;
    font-size: var(--font-size-sm);
    color: var(--color-text);
  }

  .prop-control {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .color-row {
    gap: var(--space-2);
  }

  .color-swatch {
    width: var(--space-5);
    height: var(--space-5);
    border-radius: var(--radius-sm);
    border: 1px solid var(--color-border);
    flex-shrink: 0;
  }

  .opacity-row {
    gap: var(--space-2);
  }

  .prop-range {
    flex: 1;
    accent-color: var(--color-primary);
  }

  .opacity-readout {
    flex: 0 0 auto;
    font-size: var(--font-size-sm);
    min-width: 36px;
    text-align: right;
  }

  .fill-row {
    gap: var(--space-2);
  }

  .no-fill-label {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--font-size-sm);
    color: var(--color-text);
    cursor: pointer;
  }

  .prop-number,
  .prop-text {
    flex: 1;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .prop-subtitle {
    margin: var(--space-2) 0 var(--space-1) 0;
    font-size: var(--font-size-sm);
    font-weight: 600;
  }
  .status-meta {
    margin: 0;
    font-size: var(--font-size-sm);
  }
  .reply-thread,
  .status-history {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--font-size-sm);
  }
  .reply-head {
    display: flex;
    gap: var(--space-2);
    justify-content: space-between;
  }
  .reply-author {
    font-weight: 600;
  }
  .reply-body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .reply-actions {
    display: flex;
    gap: var(--space-2);
  }
  .link-btn {
    background: none;
    border: none;
    padding: 0;
    color: var(--color-primary);
    cursor: pointer;
    font-size: var(--font-size-sm);
  }
  .prop-button {
    align-self: flex-end;
  }

  .prop-select {
    flex: 1;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .prop-textarea {
    width: 100%;
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-input, var(--color-bg-active));
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    resize: vertical;
    font-family: inherit;
  }

  .prop-value {
    flex: 1;
    font-size: var(--font-size-sm);
  }

  .muted {
    color: var(--color-text-muted);
  }
</style>
