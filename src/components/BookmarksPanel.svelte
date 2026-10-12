<script lang="ts">
  /**
   * BookmarksPanel - the document outline as a tree (Revu's Bookmarks panel).
   * Behaviour spec: docs/navigation.md "Document structure" (rows B1-B16 / RB1-RB10).
   *
   * Every bookmark title comes from the PDF and is rendered as text only. Edits write the
   * PDF straight away (NavigationStore); Undo / Redo step through this session's edits.
   * Keyboard (focus in the tree): arrows move, Enter follows, F2 renames, Delete deletes,
   * Ctrl/Cmd+Z / Shift+Ctrl/Cmd+Z undo / redo.
   */
  import { untrack } from "svelte";
  import ToolbarMenu, { type ToolbarMenuItem } from "./ToolbarMenu.svelte";
  import NavActionDialog from "./NavActionDialog.svelte";
  import PageRangePicker from "./PageRangePicker.svelte";
  import type { NavigationStore } from "$lib/navigation-store.svelte";
  import type { BookmarkView, NavTarget, Place, TargetInput } from "$lib/navigation-ipc";
  import {
    bookmarksOnPage,
    defaultBookmarkTitle,
    dropSpec,
    findBookmark,
    isFollowable,
    moveSpec,
    openIds,
    parentId,
    predictNewId,
    reconcileExpanded,
    targetText,
    visibleRows,
    ancestorIds,
    type MoveOp,
  } from "$lib/navigation";

  const {
    nav,
    pageCount,
    currentPage,
    onfollow,
    getCurrentView,
    getSelectionBookmark = undefined,
  }: {
    nav: NavigationStore;
    pageCount: number;
    currentPage: number | null;
    /** Run a bookmark's target: go to the page, or ask before opening an address. */
    onfollow: (t: NavTarget) => void;
    /** The page and view on screen now, as a destination (null when nothing is open). */
    getCurrentView: () => Promise<TargetInput | null>;
    /** A title and destination for the mark-up that is selected, or null when none is. */
    getSelectionBookmark?: () => { title: string; target: TargetInput } | null;
  } = $props();

  let selectedId = $state<string | null>(null);
  let expanded = $state<Set<string>>(new Set());
  let renamingId = $state<string | null>(null);
  let renameText = $state("");
  let notice = $state<string | null>(null);
  let dialog = $state<null | "action" | "props" | "bm_from_labels" | "labels_from_bm">(null);
  let rangePages = $state<number[] | null>(null);
  let dragId = $state<string | null>(null);
  let dropHint = $state<{ overId: string; place: Place } | null>(null);

  // Properties dialog fields.
  let propTitle = $state("");
  let propHasColor = $state(false);
  let propColor = $state("#000000");
  let propBold = $state(false);
  let propItalic = $state(false);

  // Keep the expansion state across edits; start from the file's /Count signs.
  let lastRevision = -1;
  let prevNodes: readonly BookmarkView[] | null = null;
  $effect(() => {
    const rev = nav.revision;
    const nodes = nav.bookmarks;
    if (rev === lastRevision) return;
    untrack(() => {
      expanded =
        lastRevision < 0 ? openIds(nodes) : reconcileExpanded(prevNodes, nodes, expanded);
      if (selectedId && !findBookmark(nodes, selectedId)) selectedId = null;
    });
    lastRevision = rev;
    prevNodes = nodes;
  });

  const rows = $derived(visibleRows(nav.bookmarks, expanded));
  const onThisPage = $derived(bookmarksOnPage(nav.bookmarks, currentPage ?? -1));
  const selected = $derived(selectedId ? findBookmark(nav.bookmarks, selectedId) : null);
  const readOnly = $derived(nav.readOnlyReason);
  const canEdit = $derived(readOnly === null && nav.info !== null && !nav.busy);
  const outline = $derived(nav.info?.outline ?? null);

  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  function colorCss(c: [number, number, number] | null): string | undefined {
    if (!c) return undefined;
    const [r, g, b] = c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255));
    return `rgb(${r}, ${g}, ${b})`;
  }

  function toggle(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  function select(id: string) {
    selectedId = id;
    notice = null;
  }

  function follow(n: BookmarkView) {
    if (isFollowable(n.target)) {
      onfollow(n.target);
    } else {
      notice = targetText(n.target, nav.labels);
    }
  }

  function onRowClick(n: BookmarkView) {
    if (renamingId === n.id) return;
    select(n.id);
    follow(n);
  }

  // ---- edits ------------------------------------------------------------------------

  async function add(place: Place) {
    if (!canEdit) return;
    if (place === "child" && !selected) return;
    const dest = await getCurrentView();
    if (!dest || dest.kind !== "page") {
      notice = "Open a page first.";
      return;
    }
    const anchor = selected;
    const predicted = predictNewId(nav.bookmarks, anchor?.id ?? null, place);
    const ok = await nav.edit({
      op: "bookmark_add",
      anchor: anchor?.id ?? null,
      anchor_expect: anchor?.title ?? null,
      place,
      title: defaultBookmarkTitle(nav.labels, dest.page),
      target: dest,
    });
    if (ok) {
      selectedId = predicted;
      startRename(predicted);
    }
  }

  async function addFromSelection() {
    const sel = getSelectionBookmark?.();
    if (!sel || !canEdit) return;
    const anchor = selected;
    const predicted = predictNewId(nav.bookmarks, anchor?.id ?? null, "after");
    const ok = await nav.edit({
      op: "bookmark_add",
      anchor: anchor?.id ?? null,
      anchor_expect: anchor?.title ?? null,
      place: "after",
      title: sel.title,
      target: sel.target,
    });
    if (ok) selectedId = predicted;
  }

  function startRename(id: string | null) {
    if (!id || !canEdit) return;
    const n = findBookmark(nav.bookmarks, id);
    if (!n) return;
    renamingId = id;
    renameText = n.title;
  }

  async function commitRename() {
    const id = renamingId;
    renamingId = null;
    if (!id) return;
    const n = findBookmark(nav.bookmarks, id);
    const title = renameText.trim();
    if (!n || title === "" || title === n.title) return;
    await nav.edit({ op: "bookmark_rename", id, expect: n.title, title });
  }

  async function remove() {
    const n = selected;
    if (!n || !canEdit) return;
    const ok = await nav.edit({ op: "bookmark_delete", id: n.id, expect: n.title });
    if (ok) selectedId = null;
  }

  async function move(op: MoveOp) {
    const n = selected;
    if (!n || !canEdit) return;
    const spec = moveSpec(nav.bookmarks, n.id, op);
    if (!spec) return;
    await nav.edit({
      op: "bookmark_move",
      id: n.id,
      expect: n.title,
      anchor: spec.anchor.id,
      anchor_expect: spec.anchor.title,
      place: spec.place,
    });
    // The moved node's old id now names something else; re-select by clearing.
    selectedId = null;
  }

  async function setToCurrentView() {
    const n = selected;
    if (!n || !canEdit) return;
    const dest = await getCurrentView();
    if (!dest) return;
    await nav.edit({ op: "bookmark_set_target", id: n.id, expect: n.title, target: dest });
  }

  async function saveCollapseState() {
    if (!canEdit) return;
    await nav.edit({ op: "bookmark_set_open", open: [...expanded] });
  }

  function openProps() {
    const n = selected;
    if (!n) return;
    propTitle = n.title;
    propHasColor = n.color !== null;
    propColor = n.color
      ? "#" + n.color.map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("")
      : "#000000";
    propBold = n.bold;
    propItalic = n.italic;
    dialog = "props";
  }

  async function applyProps() {
    const n = selected;
    dialog = null;
    if (!n) return;
    let title = n.title;
    const t = propTitle.trim();
    if (t !== "" && t !== n.title) {
      if (!(await nav.edit({ op: "bookmark_rename", id: n.id, expect: n.title, title: t }))) return;
      title = t;
    }
    const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(propColor);
    const color: [number, number, number] | null =
      propHasColor && m ? [parseInt(m[1], 16) / 255, parseInt(m[2], 16) / 255, parseInt(m[3], 16) / 255] : null;
    const same =
      n.bold === propBold &&
      n.italic === propItalic &&
      JSON.stringify(n.color) === JSON.stringify(color);
    if (!same) {
      await nav.edit({
        op: "bookmark_set_style",
        id: n.id,
        expect: title,
        color,
        bold: propBold,
        italic: propItalic,
      });
    }
  }

  async function confirmRange() {
    const kind = dialog;
    const pages = rangePages;
    dialog = null;
    if (!pages || pages.length === 0) return;
    if (kind === "bm_from_labels") await nav.edit({ op: "bookmarks_from_labels", pages });
    else if (kind === "labels_from_bm") await nav.edit({ op: "labels_from_bookmarks", pages });
  }

  // ---- drag and drop ----------------------------------------------------------------

  function onDragStart(e: DragEvent, id: string) {
    if (!canEdit) return e.preventDefault();
    dragId = id;
    e.dataTransfer?.setData("text/plain", id);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function onDragOver(e: DragEvent, id: string) {
    if (dragId === null) return;
    const el = e.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    const spec = dropSpec(nav.bookmarks, dragId, id, r.height > 0 ? (e.clientY - r.top) / r.height : 0.5);
    if (!spec) {
      dropHint = null;
      return;
    }
    e.preventDefault();
    dropHint = { overId: id, place: spec.place };
  }

  async function onDrop(e: DragEvent) {
    e.preventDefault();
    const hint = dropHint;
    const from = dragId ? findBookmark(nav.bookmarks, dragId) : null;
    dragId = null;
    dropHint = null;
    if (!hint || !from) return;
    const over = findBookmark(nav.bookmarks, hint.overId);
    if (!over) return;
    await nav.edit({
      op: "bookmark_move",
      id: from.id,
      expect: from.title,
      anchor: over.id,
      anchor_expect: over.title,
      place: hint.place,
    });
    selectedId = null;
  }

  // ---- keyboard ---------------------------------------------------------------------

  function onKeyDown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
      e.preventDefault();
      void (e.shiftKey ? nav.redo() : nav.undo());
      return;
    }
    if (renamingId !== null || dialog !== null) return;
    if (e.altKey && !e.ctrlKey && !e.metaKey) {
      const op: MoveOp | null =
        e.key === "ArrowUp" ? "up" : e.key === "ArrowDown" ? "down" : e.key === "ArrowRight" ? "indent" : e.key === "ArrowLeft" ? "outdent" : null;
      if (op) {
        e.preventDefault();
        void move(op);
        return;
      }
    }
    const i = rows.findIndex((r) => r.node.id === selectedId);
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        if (rows.length > 0) select(rows[Math.min(rows.length - 1, i + 1)].node.id);
        break;
      case "ArrowUp":
        e.preventDefault();
        if (rows.length > 0) select(rows[Math.max(0, i - 1)].node.id);
        break;
      case "ArrowRight":
        if (i >= 0 && rows[i].node.children.length > 0) {
          e.preventDefault();
          if (!rows[i].expanded) toggle(rows[i].node.id);
          else select(rows[i].node.children[0].id);
        }
        break;
      case "ArrowLeft":
        if (i >= 0) {
          e.preventDefault();
          if (rows[i].expanded) toggle(rows[i].node.id);
          else {
            const p = parentId(rows[i].node.id);
            if (p) select(p);
          }
        }
        break;
      case "Enter":
        if (selected) {
          e.preventDefault();
          follow(selected);
        }
        break;
      case "F2":
        e.preventDefault();
        startRename(selectedId);
        break;
      case "Delete":
      case "Backspace":
        if (selected) {
          e.preventDefault();
          void remove();
        }
        break;
    }
  }

  // ---- menus ------------------------------------------------------------------------

  const canMove = (op: MoveOp) =>
    canEdit && selected !== null && moveSpec(nav.bookmarks, selected.id, op) !== null;

  const moreItems = $derived<ToolbarMenuItem[]>([
    {
      id: "add-markup",
      label: "Add bookmark for the selected mark-up",
      title: "Bookmark the page and the area around the mark-up you have selected",
      disabled: !canEdit || !getSelectionBookmark?.(),
      onselect: () => void addFromSelection(),
    },
    { id: "add-before", label: "Add before selection", disabled: !canEdit || !selected, onselect: () => void add("before") },
    { id: "add-child", label: "Add child of selection", disabled: !canEdit || !selected, onselect: () => void add("child") },
    { id: "up", separator: true, label: "Move up (Alt+Up)", disabled: !canMove("up"), onselect: () => void move("up") },
    { id: "down", label: "Move down (Alt+Down)", disabled: !canMove("down"), onselect: () => void move("down") },
    { id: "indent", label: "Indent: make a child of the one above (Alt+Right)", disabled: !canMove("indent"), onselect: () => void move("indent") },
    { id: "outdent", label: "Outdent: move out of the parent (Alt+Left)", disabled: !canMove("outdent"), onselect: () => void move("outdent") },
    { id: "rename", separator: true, label: "Rename", disabled: !canEdit || !selected, onselect: () => startRename(selectedId) },
    { id: "view", label: "Set to current view", disabled: !canEdit || !selected, onselect: () => void setToCurrentView() },
    { id: "action", label: "Action...", disabled: !canEdit || !selected, onselect: () => (dialog = "action") },
    { id: "props", label: "Properties...", disabled: !canEdit || !selected, onselect: openProps },
    { id: "bm_from_labels", label: "Create from page labels...", separator: true, disabled: !canEdit || pageCount === 0, onselect: () => (dialog = "bm_from_labels") },
    { id: "labels_from_bm", label: "Create page labels from bookmarks...", disabled: !canEdit || nav.bookmarks.length === 0, onselect: () => (dialog = "labels_from_bm") },
    { id: "collapse", label: "Save collapse state", separator: true, disabled: !canEdit || nav.bookmarks.length === 0, onselect: () => void saveCollapseState() },
  ]);

  // The current page's bookmarks stay visible: open their ancestors when the page changes.
  $effect(() => {
    const ids = onThisPage;
    if (ids.size === 0) return;
    untrack(() => {
      const next = new Set(expanded);
      let changed = false;
      for (const id of ids) {
        for (const a of ancestorIds(id)) {
          if (!next.has(a)) {
            next.add(a);
            changed = true;
          }
        }
      }
      if (changed) expanded = next;
    });
  });
</script>

<div class="bookmarks" data-testid="bookmarks-panel" role="presentation" onkeydown={onKeyDown}>
  <div class="toolbar" role="toolbar" aria-label="Bookmark tools">
    <button class="tb" title="Add a bookmark for this page" disabled={!canEdit} onclick={() => void add("after")}>Add</button>
    <button class="tb" title="Delete the selected bookmark (Delete)" aria-label="Delete bookmark" disabled={!canEdit || !selected} onclick={() => void remove()}>Delete</button>
    <ToolbarMenu label="More" fixed items={moreItems} disabled={nav.info === null} />
    <span class="spacer"></span>
    <button class="tb icon" title="Undo (Ctrl/Cmd+Z)" aria-label="Undo" disabled={!nav.canUndo} onclick={() => void nav.undo()}>&#x21B6;</button>
    <button class="tb icon" title="Redo (Shift+Ctrl/Cmd+Z)" aria-label="Redo" disabled={!nav.canRedo} onclick={() => void nav.redo()}>&#x21B7;</button>
  </div>

  {#if readOnly}
    <p class="banner warn" role="status">{readOnly}</p>
  {/if}
  {#if nav.error}
    <p class="banner error" role="alert">
      {nav.error}
      <button class="dismiss" aria-label="Dismiss" onclick={() => nav.clearError()}>x</button>
    </p>
  {/if}
  {#if notice}
    <p class="banner" role="status">{notice}</p>
  {/if}

  {#if nav.info === null}
    <p class="empty muted">{nav.loaded ? "Bookmarks could not be read." : "Reading bookmarks..."}</p>
  {:else if rows.length === 0}
    <p class="empty muted">This PDF has no bookmarks. Use Add to bookmark the page you are on.</p>
  {:else}
    <div
      class="tree"
      role="tree"
      tabindex="0"
      aria-label="Bookmarks"
      ondragend={() => { dragId = null; dropHint = null; }}
    >
      {#each rows as r (r.node.id)}
        {@const n = r.node}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:selected={selectedId === n.id}
          class:on-page={onThisPage.has(n.id)}
          class:drop-before={dropHint?.overId === n.id && dropHint.place === "before"}
          class:drop-after={dropHint?.overId === n.id && dropHint.place === "after"}
          class:drop-child={dropHint?.overId === n.id && dropHint.place === "child"}
          role="treeitem"
          tabindex="-1"
          aria-selected={selectedId === n.id}
          aria-expanded={n.children.length > 0 ? r.expanded : undefined}
          style:padding-left={`${6 + r.depth * 14}px`}
          draggable={canEdit && renamingId === null}
          title={targetText(n.target, nav.labels)}
          onclick={() => onRowClick(n)}
          ondblclick={() => startRename(n.id)}
          ondragstart={(e) => onDragStart(e, n.id)}
          ondragover={(e) => onDragOver(e, n.id)}
          ondrop={onDrop}
        >
          {#if n.children.length > 0}
            <button
              class="caret"
              tabindex="-1"
              aria-label={r.expanded ? "Collapse" : "Expand"}
              onclick={(e) => { e.stopPropagation(); toggle(n.id); }}
            >{r.expanded ? "▾" : "▸"}</button>
          {:else}
            <span class="caret-gap"></span>
          {/if}
          {#if renamingId === n.id}
            <input
              class="rename"
              use:focusSelect
              bind:value={renameText}
              aria-label="Bookmark title"
              maxlength="1024"
              onclick={(e) => e.stopPropagation()}
              onblur={() => void commitRename()}
              onkeydown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") { e.preventDefault(); void commitRename(); }
                else if (e.key === "Escape") { e.preventDefault(); renamingId = null; }
              }}
            />
          {:else}
            <span
              class="label"
              class:bold={n.bold}
              class:italic={n.italic}
              class:dim={!isFollowable(n.target)}
              style:color={colorCss(n.color)}
            >{n.title === "" ? "(untitled)" : n.title}</span>
            {#if n.target.kind === "broken"}
              <span class="flag broken" title="The page this bookmark pointed to is gone" aria-label="Broken bookmark">!</span>
            {:else if n.target.kind === "uri"}
              <span class="flag" title={n.target.uri} aria-label="Web address">&#x2197;</span>
            {:else if onThisPage.has(n.id)}
              <span class="flag here" title="On the page you are viewing" aria-label="On this page">&#x25C0;</span>
            {/if}
          {/if}
        </div>
      {/each}
    </div>
    <p class="count muted">
      {outline?.total ?? 0} bookmark{outline?.total === 1 ? "" : "s"}{#if outline && outline.broken > 0}, {outline.broken} broken{/if}
    </p>
  {/if}
</div>

{#if dialog === "action" && selected}
  <NavActionDialog
    title="Bookmark action"
    {pageCount}
    labels={nav.labels}
    {currentPage}
    initial={selected.target}
    onCancel={() => (dialog = null)}
    onConfirm={(t) => {
      const n = selected;
      dialog = null;
      if (n) void nav.edit({ op: "bookmark_set_target", id: n.id, expect: n.title, target: t });
    }}
  />
{/if}

{#if dialog === "props" && selected}
  <div class="dialog-backdrop" role="presentation" onclick={() => (dialog = null)}></div>
  <dialog open class="modal" aria-modal="true" aria-label="Bookmark properties" onkeydown={(e) => { if (e.key === "Escape") { e.preventDefault(); dialog = null; } }}>
    <h3>Bookmark properties</h3>
    <div class="grid">
      <label for="bp-title">Title</label>
      <input id="bp-title" type="text" bind:value={propTitle} maxlength="1024" />
      <span>Colour</span>
      <span class="inline">
        <input type="checkbox" bind:checked={propHasColor} aria-label="Use a colour" />
        <input type="color" bind:value={propColor} disabled={!propHasColor} aria-label="Colour" />
      </span>
      <span>Style</span>
      <span class="inline">
        <label><input type="checkbox" bind:checked={propBold} /> Bold</label>
        <label><input type="checkbox" bind:checked={propItalic} /> Italic</label>
      </span>
    </div>
    <div class="buttons">
      <button class="btn-confirm" onclick={() => void applyProps()}>OK</button>
      <button class="btn-cancel" onclick={() => (dialog = null)}>Cancel</button>
    </div>
  </dialog>
{/if}

{#if dialog === "bm_from_labels" || dialog === "labels_from_bm"}
  <div class="dialog-backdrop" role="presentation" onclick={() => (dialog = null)}></div>
  <dialog open class="modal" aria-modal="true" aria-label={dialog === "bm_from_labels" ? "Create bookmarks from page labels" : "Create page labels from bookmarks"} onkeydown={(e) => { if (e.key === "Escape") { e.preventDefault(); dialog = null; } }}>
    <h3>{dialog === "bm_from_labels" ? "Create bookmarks from page labels" : "Create page labels from bookmarks"}</h3>
    <p class="hint">
      {dialog === "bm_from_labels"
        ? "One bookmark per page, titled with the page's label, added after the existing bookmarks."
        : "A page that has a bookmark takes the bookmark's title as its label."}
    </p>
    <PageRangePicker {pageCount} {currentPage} onchange={(p) => (rangePages = p)} />
    <div class="buttons">
      <button class="btn-confirm" disabled={!rangePages || rangePages.length === 0} onclick={() => void confirmRange()}>OK</button>
      <button class="btn-cancel" onclick={() => (dialog = null)}>Cancel</button>
    </div>
  </dialog>
{/if}

<style>
  .bookmarks {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: var(--font-size-sm);
  }
  .toolbar {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2);
    background: var(--color-bg-panel);
    border-bottom: 1px solid var(--color-border-subtle);
  }
  .toolbar .spacer {
    flex: 1;
  }
  .tb {
    background: var(--color-bg-active);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    color: var(--color-text);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: var(--space-1) var(--space-2);
    transition: background 120ms;
  }
  .tb:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }
  .tb:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .tb.icon {
    min-width: 1.8em;
    padding-left: var(--space-1);
    padding-right: var(--space-1);
  }
  .banner {
    margin: 0;
    padding: var(--space-1) var(--space-2);
    color: var(--color-text-secondary);
    background: var(--color-bg-panel-alt);
    border-bottom: 1px solid var(--color-border-subtle);
  }
  .banner.warn {
    color: var(--color-warning);
  }
  .banner.error {
    color: var(--color-danger);
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .dismiss {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
  }
  .empty,
  .count {
    margin: 0;
    padding: var(--space-2) var(--space-3);
  }
  .muted {
    color: var(--color-text-muted);
  }
  .tree {
    outline: none;
  }
  .tree:focus-visible {
    outline: 1px solid var(--color-primary);
    outline-offset: -1px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    min-height: 24px;
    padding-right: var(--space-2);
    cursor: default;
    border-top: 2px solid transparent;
    border-bottom: 2px solid transparent;
  }
  .row:hover {
    background: var(--color-bg-hover);
  }
  .row.selected {
    background: var(--color-bg-active);
  }
  .row.drop-before {
    border-top-color: var(--color-primary);
  }
  .row.drop-after {
    border-bottom-color: var(--color-primary);
  }
  .row.drop-child {
    outline: 1px dashed var(--color-primary);
    outline-offset: -2px;
  }
  .caret,
  .caret-gap {
    width: 16px;
    flex: none;
    text-align: center;
  }
  .caret {
    background: none;
    border: none;
    color: var(--color-text-secondary);
    cursor: pointer;
    padding: 0;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-text);
  }
  .label.bold {
    font-weight: 700;
  }
  .label.italic {
    font-style: italic;
  }
  .label.dim {
    opacity: 0.6;
  }
  .flag {
    flex: none;
    color: var(--color-text-secondary);
  }
  .flag.here {
    color: var(--color-primary);
  }
  .flag.broken {
    color: var(--color-danger);
    font-weight: 700;
  }
  .rename {
    flex: 1;
    min-width: 0;
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-sm);
    padding: 1px var(--space-1);
    font-size: var(--font-size-sm);
  }
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 900;
  }
  .modal {
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
  .modal h3 {
    margin: 0;
    font-size: var(--font-size-base);
  }
  .hint {
    margin: 0;
    color: var(--color-text-secondary);
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-2) var(--space-3);
    align-items: center;
  }
  .grid input[type="text"] {
    background: var(--color-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--font-size-sm);
  }
  .inline {
    display: flex;
    align-items: center;
    gap: var(--space-3);
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
