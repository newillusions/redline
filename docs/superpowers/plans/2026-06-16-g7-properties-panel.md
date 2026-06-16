# G7 — Properties Panel (detailed TDD breakdown)

**Group:** S2 / G7. **Branch:** `feat/g7-properties-panel` (off `main` @ 7f57758). **Date:** 2026-06-16.
**Parent plan:** `docs/superpowers/plans/2026-06-14-s2-markup-authoring.md` (G7 map).
**Scope decided 2026-06-16 (user):**
- **Full panel** — appearance (color, line weight, opacity, fill, line style, font family+size) **plus** contents (note text), subject, layer.
- **Include the Rust `/DA` base-font interop fix now** (external viewers honor the chosen base-14 family; family already round-trips redline-to-redline via `/RLFontFamily`).
- **Multi-select edits apply to ALL selected** as one undo frame (reuses G6 `applyBatch`); mixed values render an indeterminate/blank control.

## Architecture fit (what exists — do NOT rebuild)
- `MarkupStore`: `selectedIds`, `selectedMarkups` getter, `draftAppearance = $state<Appearance>`, `update(before, after)`, `applyBatch(pairs)` (one undo frame), `delete`, undo/redo. `bumpAudit(m, by, now)` in `markup-tools.ts` (G6).
- `Appearance` (`ipc.ts`): `{ color, line_weight, opacity, fill: string|null, line_style: "Solid"|"Dashed"|"Dotted", font: {family,size_pt}|null }`. `Markup` also has `contents: string|null`, `subject: string|null`, `layer: string|null`, `audit`.
- `App.svelte`: right panel placeholder at lines ~220-228 (`<aside class="panel panel-right">` → "Tool Chest · Markups (M2)"). `store` is in scope. Identity: `get_user_identity()` IPC (the panel needs an identity for the audit bump; App does not currently load it - Viewport does. See G7.3 note).
- `annotation.rs:359-372`: FreeText `/DA` is pinned to `/Helv`; `/RLFontFamily`+`/RLFontSize` carry the exact family losslessly. `from_dict` reads them back (`:490`).

## §6 property persistence (binding)
Edits mutate the in-session markup envelope and mirror via the existing per-op queue on Save (no new IPC - `update_markup` already carries the whole markup). Appearance maps to standard annotation keys on save (already implemented for color/weight/opacity/fill/style); font maps to `/DA`+`/RLFont*` (G7.2 corrects the `/DA` base font).

---

## G7.1 — Pure property helpers (`src/lib/markup-properties.ts`) + unit tests
No DOM/Svelte. The panel stays thin by delegating the patch + indeterminate logic here.

```ts
import type { Markup, Appearance, UserRef } from "./ipc";

/** Base-14 font families offered by the picker (the only families with reliable
 *  cross-viewer /DA rendering). Display label -> Appearance.font.family value. */
export const FONT_FAMILIES: readonly string[]; // ["Helvetica","Times","Courier"]
export const FONT_SIZES: readonly number[];     // e.g. [8,9,10,11,12,14,18,24,36]

/** Clone m with a shallow appearance patch applied + audit bumped. No mutation. */
export function patchAppearance(m: Markup, patch: Partial<Appearance>, by: UserRef, now: string): Markup;

/** Clone m with contents/subject/layer patched + audit bumped. No mutation. */
export function patchFields(m: Markup, patch: { contents?: string|null; subject?: string|null; layer?: string|null }, by: UserRef, now: string): Markup;

/** The shared value of a field across markups, or undefined if they differ (indeterminate).
 *  Uses strict equality on the projected value (primitive fields only). */
export function commonValue<T>(markups: Markup[], get: (m: Markup) => T): T | undefined;
```
Notes: `patchAppearance` = `bumpAudit({ ...m, appearance: { ...m.appearance, ...patch } }, by, now)` (reuse `bumpAudit`, do not re-implement the audit). Font patch via appearance: `{ font: { family, size_pt } }` or `{ font: null }`.

**Tests (`markup-properties.test.ts`):**
- `patchAppearance`: color/weight/opacity/fill/line_style/font each patched; other appearance fields preserved; audit.revision++ and modified_at set; input not mutated.
- `patchFields`: contents/subject/layer patched independently; appearance untouched; audit bumped; no mutation; explicit `null` clears a field.
- `commonValue`: returns the value when all equal; `undefined` when mixed; single-element list returns its value; reads nested (e.g. `m.appearance.color`, `m.appearance.font?.family`).
- `FONT_FAMILIES`/`FONT_SIZES` are non-empty and contain Helvetica/12.

**Exit:** `npx vitest run src/lib/markup-properties.test.ts` green; `npm run check` 0. Commit `feat(g7): pure property-patch + indeterminate-value helpers`.

---

## G7.2 — Rust `/DA` base-font interop fix (`src-tauri/src/markup/annotation.rs`) + tests
Make the FreeText `/DA` reference the correct base-14 font for the chosen family, so Acrobat/Bluebeam render Times/Courier (not always Helvetica). Family still round-trips losslessly via `/RLFontFamily` (unchanged).

- Add `fn base14_da_name(family: &str) -> &'static str` mapping the family to the standard AcroForm base-14 `/DA` resource name. **RESEARCH FIRST (Context7 / PDF 32000-1 §12.7.3.3 AcroForm /DR, or pdf spec):** confirm the conventional names. Expected: Helvetica→`Helv`, Times (incl. "Times New Roman"/"Times-Roman")→`TiRo`, Courier→`Cour`; default→`Helv`. Case-insensitive, match on family prefix.
- Replace the hardcoded `/Helv` at `:367` with `/{base14_da_name(&font.family)}`.
- **Interop caveat to verify (note in code + handover):** a `/DA` font resource name only resolves in an external viewer if it is present in the document AcroForm `/DR` (default resources) or the annotation's own `/DR`. The standard base-14 names are widely recognized without an explicit `/DR`, but if a quick check shows Bluebeam/Acrobat ignores `/TiRo` without `/DR`, fall back to documenting it as a G9 interop item rather than blocking. Do NOT add a half-built `/DR` dict speculatively.

**Tests (in the existing `annotation.rs` test module):**
- A FreeText markup with `font.family = "Times"` emits `/DA` containing `/TiRo` and ` Tf`.
- `family = "Courier New"` → `/Cour`; unknown family → `/Helv`.
- `assert_roundtrip` still holds for each (family preserved via `/RLFontFamily`; `from_dict` already reads `/RLFontFamily`).
- `base14_da_name` unit cases (Helvetica/Times/Courier/unknown, case-insensitive).

**Exit:** `cd src-tauri && cargo test -- --test-threads=1` green; `cargo clippy --all-targets` 0; `cargo fmt`. Commit `feat(g7): map FreeText /DA to the base-14 font for the chosen family`. *(Touches annotation serde - flag for `/code-review` before the G-group ships.)*

---

## G7.3 — `PropertiesPanel.svelte` + App wiring + interaction tests
The component. Props `{ store }: { store: MarkupStore }`.

**Identity:** the audit bump needs a `UserRef`. App does not currently hold identity (Viewport loads it internally). Decision: **lift identity to App** - App calls `getUserIdentity()` on mount, stores it, and passes it to BOTH `<Viewport>` (new optional prop, falls back to its own load) and `<PropertiesPanel>`. Simpler alternative if lifting is messy: PropertiesPanel loads its own identity on mount (same pattern as Viewport). Pick the lighter one; state it in the report. The panel must still function (edit geometry-free fields) if identity is null, using `identity ?? m.audit.modified_by` (mirror G6's fallback).

**Mode:**
- `selected = store.selectedMarkups` (all selected, any page). `mode = selected.length === 0 ? "draft" : "selection"`.
- **draft mode:** edits mutate `store.draftAppearance` fields directly ($state; no undo - it is the template for new markups). Show appearance + font only (no contents/subject/layer - those are per-markup). A header line: "Defaults for new markups".
- **selection mode:** each control's value = `commonValue(selected, getter)`; `undefined` → indeterminate (blank value / "Mixed" placeholder). On change, build `pairs = selected.map(m => ({ before: m, after: patchAppearance/patchFields(m, patch, by, now) }))` and `store.applyBatch(pairs)` → one undo frame. Header: `N markup(s) selected`.

**Controls (design tokens / semantic classes only - no Tailwind, no hardcoded hex except via tokens):**
- Color: `<input type="color">` bound to appearance.color (+ a small swatch).
- Line weight: `<input type="number" min=0 step=0.5>` (pts).
- Opacity: `<input type="range" min=0 max=1 step=0.05>` + % readout.
- Fill: color input + a "No fill" checkbox that sets `fill = null`.
- Line style: `<select>` Solid/Dashed/Dotted.
- Font family: `<select>` from `FONT_FAMILIES`; Size: `<select>`/number from `FONT_SIZES`. (Appearance.font; when setting on a selection whose markups are not Text/Callout, still allowed - it is harmless metadata. Optionally only show the font group when the selection includes a Text/Callout or in draft mode; choose and note it.)
- Contents (selection only): `<textarea>` bound to markup.contents. Subject, Layer (selection only): `<input type="text">`.
- Empty state when `!store` or no doc: "Select a markup to edit its properties."

**App wiring:** replace the right-panel placeholder body (App.svelte ~223-226) with `<PropertiesPanel {store} {identity} />` when `store` exists (keep the "Properties" panel-header). Keep the panel collapsible (existing `rightCollapsed`).

**Interaction tests (`PropertiesPanel.test.ts`, jsdom, @testing-library/svelte):**
- No selection → draft mode: changing the color input updates `store.draftAppearance.color`; no undo entry created.
- Single selection: panel shows that markup's color/weight/opacity; changing color → `store.markups[i].appearance.color` updated, ONE undo reverts, `ipc.update` called once (await drain).
- Multi-selection (2, different colors): color control is indeterminate; setting a color applies to BOTH (one undo reverts both; `ipc.update` twice).
- Contents edit on a Text markup updates `contents` (one undo).
- Subject + layer edits persist on the markup.
- Font family change sets `appearance.font.family` on the selection.
- Indeterminate: two markups with different line_weight → the weight field reads blank/Mixed.

**Exit:** `npx vitest run` (full) green; `npm run check` 0. Commit `feat(g7): properties panel - appearance, font, contents/subject/layer; multi-select batch`.

---

## G7.4 — Exit gates + housekeeping
- Standing gates: `npm run test` all green, `npm run check` 0, `cd src-tauri && cargo test -- --test-threads=1` green, `cargo clippy --all-targets` 0, `cargo fmt`.
- `/code-review` (high) on the **`annotation.rs` `/DA` change** before considering ship (serde touchpoint).
- Update `.claude/HANDOVER.md` (G7 done; note the font `/DA` external-viewer fidelity is confirmed at G9) + KB + mission record. Roadmap: G7 ✓, next G8 grouping.
- Commit docs.

## Risks / watch-items
- **`/DA` interop is the sharp edge:** the resource-name-vs-`/DR` resolution (G7.2 caveat). Keep the change minimal and lossless (RLFontFamily is the source of truth); do not block G7 on perfect external-viewer rendering - that is a G9 verification.
- **Identity lifting** to App must not regress Viewport's own identity load (keep Viewport's fallback).
- **Indeterminate writes:** editing an indeterminate control must set ALL selected to the new value (not just the differing ones) - test it.
- **Draft vs selection** must not cross-contaminate: draft edits never touch committed markups; selection edits never touch draftAppearance.

## Self-review
- Spec coverage: §6 appearance + contents/subject/layer editing ✓; one-undo-per-edit incl. multi-select ✓ (applyBatch); font picker + `/DA` interop ✓ (G7.2). bumpAudit reused (no dup audit logic).
- Parallelism: **G7.1 (TS helpers) and G7.2 (Rust /DA) are independent** - dispatch together. G7.3 depends on G7.1. G7.4 last.
