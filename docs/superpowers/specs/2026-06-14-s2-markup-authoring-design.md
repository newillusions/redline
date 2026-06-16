# S2: Markup Authoring, Editing & Undo/Redo — Design

**Status:** approved 2026-06-14. **Slice:** S2 (M2b) of the v1 roadmap
(`docs/superpowers/plans/2026-06-12-v1-completion-roadmap.md`). **Predecessor:** S2a
(overlay display) — merged on `feat/s2a-markup-overlay` (`ef4163d`, `3eb9dd5`, `c49aa57`).

## Goal

Author, edit, and delete the full v1 markup type set directly in the GUI, with
command-pattern undo/redo and persistence through the existing S1 save pipeline. The
overlay (display, S2a) becomes interactive. Measurement tools, Tool Chest/Tool Sets,
stamps, and `.btx` import are explicitly **out** (S5/S7).

This is the roadmap's "big one," taken as a single branch per the user's decision
(no S2b–S2e split), sequenced internally so each task group leaves the tree green and
runnable.

## Scope

**In:** Rectangle, Ellipse, Line, Arrow, Polyline, Polygon, Cloud, Highlight, Pen/Ink,
Text, Callout · select / move / resize · Properties panel (appearance + contents +
subject + layer) · comments (the `contents` note field; the reserved `thread` stays
unused per spec §6) · grouping · command-pattern undo/redo (§12 line 244, §15).

**Out (with reason):**
- **Stamp placement → S5.** The Stamp tool needs a Tool Chest to choose a stamp source,
  which does not exist until S5. Including it here would break the module boundary.
- **Measurement tools → S7** (need geometry path extraction + calibration).
- **Sidecar audit log → S4.** S2 emits the per-op events the sidecar will consume, but
  the durable append-only log lands in S4.

## Architecture

### Frontend — 3 new modules + 1 panel

| Module | Responsibility | Tested by |
|---|---|---|
| `src/lib/markup-store.svelte.ts` | **In-session source of truth.** Reactive `$state`: `markups[]`, `selectedIds`, `activeTool`, `draftAppearance`. Every mutation routes through the history and schedules an async backend mirror. | vitest |
| `src/lib/markup-commands.ts` | Command pattern: `CreateCmd` / `UpdateCmd` / `DeleteCmd` / `GroupCmd`, each with `apply` + `invert`; a `History` with undo/redo stacks and gesture coalescing. Pure (no DOM). | vitest |
| `src/lib/markup-tools.ts` | Per-tool interaction state machines; converts pointer events → PDF-space geometry via the existing tested `screenToPdfUserSpace`. | vitest (geometry builders); manual (pointer UX) |
| `src/components/PropertiesPanel.svelte` (right column) | Binds the selected markup's `Appearance` + `contents`/`subject`/`layer`. With no selection, edits `draftAppearance` for the next-drawn markup. | manual |

`Viewport.svelte` gains the interactive overlay (pointer capture, selection chrome, inline
text editor); a tool palette lives in the left column. `App.svelte` already holds the
reactive `markups` state and threads it to the viewport (S2a) — it now owns the store
instance and wires Save to force-flush the mirror first.

### Data flow & the undo/sync contract

```
pointer / UI event
  → markup-tools builds geometry or a field patch
  → store.create / update / delete(...)
        → pushes a Command onto History            (undoable)
        → mutates the reactive markups[]            (overlay re-renders via existing $derived)
        → enqueues the matching backend op on an ordered async queue
  ───────────────────────────────────────────────────────────────
  undo() / redo() → History inverts / replays the Command → enqueues the inverse / forward op
  Save → drain the op queue (await), THEN save_document (reads the Rust store → PDF)
```

**Source-of-truth split:** the Svelte store is authoritative *in-session*; the Rust store
is a **mirror + save buffer**. The S1 save pipeline already reads the Rust store and writes
the PDF, so the contract is: keep the Rust store in lock-step via per-op IPC, and guarantee
a full drain before Save.

**Per-op granular sync (decided — comprehensive over bulk-replace).** Each committed
command maps to exactly one backend op:

| Command | forward op | inverse op (undo) |
|---|---|---|
| `CreateCmd` | `add_markup` | `delete_markup` |
| `UpdateCmd` (move/resize/edit/group) | `update_markup` | `update_markup` (prior snapshot) |
| `DeleteCmd` | `delete_markup` | `add_markup` |

Rationale: this lays the exact event rails the S4 sidecar audit consumes (create / edit /
delete, each with `user_id` + timestamp), so there is no sync rework later; and it maps 1:1
onto the command pattern (each command already carries its forward + inverse), so it is
barely more code than a bulk replace. Bulk-replace was rejected because it would have to be
torn out and re-granularized in S4.

**Ordered async queue:** committed ops append to a FIFO; a single drainer processes them in
order so the mirror never races or reorders. A failed op surfaces a non-blocking banner
(like `openError`) and halts the drain; the Svelte store stays authoritative so no
in-session work is lost, and the drain retries on the next mutation or before Save. Rapid
edits are naturally serialized by the queue (no debounce needed for correctness; a short
coalesce window on drags is handled at the command layer, below).

**Gesture coalescing:** a move/resize drag mutates a *live preview* (no command, no op) on
each `pointermove`; on `pointerup` a single `UpdateCmd` carrying the before→after snapshot
is committed — so one drag = one undo step = one `update_markup` op, not 60 frames.

## Interaction model — 5 archetypes (`markup-tools.ts`)

| Archetype | Tools | Gesture → geometry |
|---|---|---|
| Drag-draw | Rectangle, Ellipse, Line, Arrow, Highlight | down = anchor, move = preview, up = commit → `Rect` (rect/ellipse/highlight) or 2-pt `Polyline` (line/arrow) |
| Multi-click | Polyline, Polygon, Cloud | click = add vertex, dblclick/Enter = finish, Esc = cancel → `Polyline` (Cloud closed + scalloped render) |
| Freehand | Pen/Ink | down → sampled move points → up = one `Ink` stroke (point-simplified) |
| Text-entry | Text, Callout | click places anchor; inline screen-positioned `<textarea>` commits `contents` + `font`; Callout adds a leader `Polyline` |
| Select/transform | Select | click hit-tests topmost (shift = multi-select); drag-inside = move; drag-handle = resize; Delete key = delete |

**Tool / capture model.** The active tool drives overlay capture:
- **Hand tool** = pan — today's viewport-root drag; overlay stays `pointer-events: none`.
- **Select + all draw tools** = overlay flips to `pointer-events: auto` and captures the gesture.
- Middle-mouse / space-drag pans regardless of the active tool (nice-to-have; implement if cheap).

**Rendering additions** (extend the S2a `SvgShape` union, keep `markup-render.ts` pure):
- a `text` kind (rendered as SVG `<text>`, font-scaled by zoom);
- Cloud = closed polygon with a scalloped stroke path;
- selection chrome (bounding box + 8 resize handles) rendered in the overlay layer;
- an inline editor overlay (HTML `<textarea>`, screen-positioned) for Text/Callout entry.

## Backend + model changes (Rust — risky-diff surface, gets `/code-review`)

1. **`document/store.rs`:** `update(doc_id, markup)` (swap by id, error if absent),
   `delete(doc_id, id)` (remove). Unit-tested.
2. **`commands/document.rs`:** `update_markup`, `delete_markup` Tauri commands + handler
   registration. `add_markup` already exists. `ipc.ts`: `updateMarkup`, `deleteMarkup`.
3. **Minimal user identity (§12 g):** `get_user_identity` command backed by a small
   persisted `identity.json` in the app config dir — generates a stable `user_id` (UUID) +
   editable display name (OS-username default) on first run. Required because every created
   markup's `audit.created_by` needs a real `UserRef`. S4 promotes this to the full
   `user_id ↔ display-name` registry; the shape is forward-compatible.
4. **Grouping field (sequenced last — the cut-line):** add `group_id: Option<Uuid>` to the
   Rust `Markup` envelope + serde and `group_id: string | null` to the TS `Markup`.
   Persists as an app-namespaced `/RLGroup` annotation key (round-trips in the existing
   serde; no sidecar needed). Default `null`.

**Audit bump on edit:** `update_markup` preserves the immutable `id` and bumps
`modified_by` / `modified_at` / `revision`. The frontend builds the updated markup with the
identity from `get_user_identity`; the store enforces the id is unchanged.

## Error handling

- **Mirror op failure:** non-blocking banner; drain halts; Svelte store remains
  authoritative; retry on next mutation or before Save. Save refuses (surfaces the error)
  if the queue cannot drain, so we never persist a stale Rust store.
- **Unknown `doc_id` / absent markup on update-delete:** backend returns `Err`; surfaced as
  the mirror-failure banner.
- **Text editor commit with empty contents:** no markup created (drawing-mode no-op).
- **Multi-click / freehand cancel (Esc):** discards the in-progress draft, no command.

## Testing

- **vitest:** command stack (apply/invert/undo/redo, coalescing); geometry builders
  (gesture → geometry, reusing the tested `screenToPdfUserSpace`); hit-testing;
  appearance/contents patches; group/ungroup commands.
- **Rust:** `store.update` / `store.delete`; `/RLGroup` serde round-trip; identity
  persistence (first-run generate, second-run reuse).
- **Manual GUI (user-gated, final group):** draw every tool; edit/move/resize; properties;
  comments; group/ungroup; undo/redo across mixed ops; Save → external-viewer
  (Acrobat/Bluebeam) check; overlay stays glued to the page at extreme zoom and through
  page navigation.

## Task-group sequencing (one branch, ordered; tests green at each gate)

- **G1 — Backend rails:** store `update`/`delete` + `update_markup`/`delete_markup` commands
  + ipc wrappers + minimal `get_user_identity`.
- **G2 — Undo/sync core:** `markup-commands.ts` (command stack) + `markup-store.svelte.ts`
  (reactive SoT) + ordered async op-queue mirror + flush-before-save wiring.
- **G3 — Drag-draw tools:** Rectangle/Ellipse/Line/Arrow/Highlight + tool palette + overlay
  pointer capture. First end-to-end "draw → undo → save" demo.
- **G4 — Multi-click + freehand:** Polyline/Polygon/Cloud/Ink + cloud scallop render + ink
  render.
- **G5 — Text/Callout:** inline text editor + `text` render kind.
- **G6 — Select/transform:** hit-test + selection chrome/handles + move/resize + delete.
- **G7 — Properties panel:** appearance/contents/subject/layer bound to selection + draft.
- **G8 — Grouping (cut-line):** `group_id` model field (Rust + TS + `/RLGroup` serde) +
  group/ungroup commands + group-aware select/move.
- **G9 — Ship:** GUI verify + handover + roadmap tick + `/code-review` (render/serde
  touchpoints) + `/sendit`.

## Standing gates (every group)

TDD (failing test first); `cargo test` (`--test-threads=1`) + `cargo clippy --all-targets`
(0 warnings) + `cargo fmt --check` + `npm run check` (0 errors) + vitest green; corpus
tests manually before any render-path merge; `/code-review` before `/sendit` (this slice
touches render + annotation serde); conventional commits with `Co-Authored-By`.
