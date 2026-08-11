# Grouped & multi-layered markups — design

Owner-directed (verbatim, 2026-08-11): "we do also need to get grouped geometry, and
multi layered markups sorted out as well." This closes the largest remaining
Bluebeam-interop gap named by `bb_interop_conformance.rs`'s calibration test: 33/77
(43%) of real corpus items carry a `<Child>` and are architecturally unsupported
today (`toolchest::btx` module doc, obs:ullyvzs86ncoa70itfdi).

## 1. Evidence: what Bluebeam actually writes

Decoded every real grouped `<ToolChestItem>`/`<Child>` pair in the 4-file/77-item
corpus (`bench/corpus/btx/`) independently of the existing importer, to see the raw
annotation-dictionary keys, not guess at them.

**Finding 1 — it's the standard PDF group mechanism, not a bespoke Bluebeam one.**
Every group uses ISO 32000-1 §12.5.6.2's markup-annotation reply/group construct:
one **head** annotation, and one-or-more **follower** annotations that carry
`/RT /Group` + `/IRT <head's object ref>`. In the `.btx` toolset export format
(pre-placement, no object graph yet) these are staged as `Temp`-prefixed string
placeholders — `TempNameID` (head's own id), `TempIRT` (follower → head's id),
`TempRT`, `TempGroupNestingName` — that resolve to real `/NM`/`/IRT`/`/RT` at
placement time. Example (`emittiv-markups.btx`, "emittiv cloud + callout"):

```
HEAD (FreeText, the callout text):
  /TempNameID /VAGBOPRCOEIUSDVI
  /TempGroupNesting [(emittiv cloud + callout) /VAGBOPRCOEIUSDVI /NEZCMZLMIKJANVBC]
  /TempGroupNestingName /VAGBOPRCOEIUSDVI        <- equals its own id
  /GroupNesting [...]                             <- same array, NON-Temp (persists to placed PDF)

FOLLOWER (Polygon cloud):
  /TempRT /Group
  /TempIRT /VAGBOPRCOEIUSDVI                      <- points at head's id
  /TempGroupNestingName /NEZCMZLMIKJANVBC          <- its own id (listed in head's roster)
```

**Finding 2 — it's a star topology, not always a pair.** Most groups are 2 members,
but 9 real corpus items have 3–20 members (e.g. a 21-member "Consultant Stamp"
compound: 1 backing Square + 20 FreeText/Line labels). Every follower's `TempIRT`
points at the SAME head; there is no evidence of chains (follower→follower) or
multi-level nesting in this corpus, despite the `GroupNesting`/"Nesting" naming
hinting BB supports it. **Which XML element (`<ToolChestItem>` vs `<Child>`) is
head is not fixed** — role is carried entirely by the presence of `TempNameID`+
`TempGroupNesting` (head) vs `TempRT`+`TempIRT` (follower), never by XML nesting
order. Any implementation must resolve group membership by ID graph, never by
serialization position.

**Finding 3 — `/GroupNesting` is a real, persisted private extension key**, distinct
from `/IRT`/`/RT`. It carries `[display_name, head_id, member1_id, ..., memberN_id]`
on the head only. It's BB's own UI/display metadata (group name, full roster) — not
required for a generic viewer to treat the annotations as grouped (that's `/IRT`+
`/RT` alone, per spec), but useful for fidelity and for BB's own "Grouped: <name>"
UI label.

## 2. Data model: flat `group_id`, not a parent/child tree

**Decision: keep the existing flat model, extend its read/write coverage.**
Redline already has exactly the right primitive — `Markup::group_id: Option<Uuid>`
(G8, shipped): all markups sharing a non-`None` group_id are one selectable/movable
unit (`expandSelectionToGroups` in `markup-select.ts`), private-key-serialized as
`/RLGroup`. This is a symmetric, flat model (no head/follower distinction, no fixed
member count) — which is a **superset** of what BB's star topology needs: import
can flatten BB's head+followers into one shared `group_id`, and export can pick any
deterministic member as the synthesized head. No parent/child struct on `Markup` is
needed or wanted:

- A parent/child tree would duplicate exactly what `group_id` already gives for
  free — shared membership — while adding real cost: `Markup` becomes recursive or
  needs an ownership-vs-reference decision, JSON/undo-history serialization gets
  harder, and the existing selection/move/resize/delete code (which already works
  generically over `Set<id>`) would need group-aware special-casing instead of just
  consuming a larger selection set.
- BB's own star topology (one head, N followers) is *itself* not symmetric — a
  parent/child model copied from BB's wire format would import that asymmetry into
  redline's data model for no functional gain, since redline's UI has no concept of
  "the group's primary annotation" today.

The only asymmetry that matters is at the **serialization boundary** (read/write),
not in the in-memory model — see §3.

## 3. Round-trip contract

### Read (`document::annots::read_markups`)

Today each annotation dict converts independently via `Markup::from_annotation_dict`,
which has no cross-annotation visibility and therefore no way to assign `group_id`
from `/IRT`+`/RT`. Fix: `read_markups` already holds `(ObjectId, Dictionary)` pairs
for every annotation on a page before conversion — extend it to:

1. Build `oid → index` over the page's annotation list.
2. For each dict with `/RT == Name("Group")` and `/IRT == Reference(target_oid)`,
   record an edge `index → oid_index[target_oid]`.
3. Union-find (or equivalent) over these edges into connected components. A
   component of size 1 (an annotation with no group edges, or an unresolvable
   `/IRT`) is not a group.
4. For every component of size ≥ 2, assign one fresh `Uuid` and set it as
   `group_id` on **every** member's `Markup` (head included — redline's flat model
   doesn't privilege the head after import).
5. `/IRT`/`/RT` alone decides **membership** (who is grouped) — it is the
   externally-visible, standards-based signal, and always wins over a stale/absent
   `/RLGroup` for that purpose. The synthesized group's **value** (the UUID itself)
   instead prefers a `/RLGroup` already carried by any member of the component, when
   one exists, falling back to a freshly minted `Uuid` only when no member has one.
   This matters because write (§3 "Write") now *always* emits real `/IRT`+`/RT`
   alongside `/RLGroup` — without this preference, every reopen of a redline-authored
   file would mint a brand-new group id even though membership never changed
   (verified: this exact regression was caught by the existing fidelity-matrix
   round-trip test and fixed before landing). A genuinely foreign (BB-only) group has
   no `/RLGroup` on any member and gets a fresh id, same as before this refinement.

Union-find naturally degrades a hypothetical multi-level chain (follower→follower)
into one flat group — a safe, conservative approximation given no real corpus
evidence of chains, named explicitly rather than silently assumed correct.

### Write (`document::annots::write_markups`)

Phase 2 already assigns each markup a fresh `ObjectId` via `doc.add_object` in
one pass — extend it to also collect `id() → ObjectId` for every markup with a
`group_id`, then add a **Phase 2.5**: group markups by `group_id`, pick the
**first member in slice order** as the synthesized head (deterministic, no
tie-break logic needed), and patch:

- every non-head member's already-written dict: `/RT = Name("Group")`,
  `/IRT = Reference(head_oid)`.
- the head's dict: unchanged (no `/IRT` needed — matches every real corpus head).

`/RLGroup` continues to be written by `to_annotation_dict` exactly as today (per-
markup, no cross-annotation knowledge needed) — kept as belt-and-braces so a
redline-only round-trip (never touched by another viewer) is exact even if a
future refactor changes head selection, and so `group_id` survives a save even
for a lone member whose group later gains its first sibling.
`/GroupNesting` (the display-name+roster array) is **not** written in this pass —
named as a follow-up (§6); it requires a user-facing group display name redline
has no UI concept of yet, and `/IRT`+`/RT` alone is suffient for the interop
contract (any spec-conformant viewer, including Bluebeam, recognizes the group).

### Contract statement

> Opening a Bluebeam-authored PDF (or `.btx` Tool Set) with grouped markups, then
> saving from redline, preserves group membership: every markup that was grouped
> on read is still grouped (via `/IRT`+`/RT`) on write, with the same member set.
> The specific head chosen may change (redline picks the first member in save
> order, not necessarily BB's original head) — acceptable, since group membership
> and NOT head identity is the externally-observable contract (no viewer UI is
> known to expose "which member is the head").

## 4. `.btx` Tool Set import → compound Tool → placement

The corpus's grouped items are Tool Set exports, not already-placed PDF pages — the
interop-critical path for *this* corpus is `toolchest::btx::import_btx_bytes`, which
today silently discards every `<Child>` (module doc, "NAMED, NOT FIXED").

**`Tool` gains an optional `children: Vec<ToolChild>`** (new, small struct: same
shape as the fields `Tool::from_markup` already snapshots — `markup_type`,
`appearance`, `subject`, `geometry`), where `geometry` for a Drawing-mode child is
its own fixed template (verified from the corpus: each `<Child>`'s `<X>`/`<Y>` are
independent offsets from the tool's own origin, not relative to a prior child — so
each child's already-decoded `/Rect`-derived geometry is placed via the *same*
click-anchor delta as the parent, not chained).

`btx.rs` parsing changes from "one optional `<Child>`" to "zero or more `<Child>`
elements" (the corpus has items with up to 20), each independently decoded through
the same `Markup::from_annotation_dict` reuse the importer already does for the
parent — no new dict-parsing logic, just applying the existing one N times instead
of once.

**Placement** (`Viewport.svelte::createPlacedMarkup`): for a tool with children,
generate **one fresh `group_id`**, translate the tool's own geometry AND every
child's geometry by the same `(click_point − anchor)` delta already computed by
`translateToolGeometry`, and call `store.create` once per member (parent + each
child) with that shared `group_id`. This is additive to the existing single-markup
path (Properties-mode tools and childless Drawing-mode tools are unaffected) and
does not require new selection/move code — G8's `expandSelectionToGroups` already
makes an N-member group move/select/delete as one unit for free.

## 5. Layer semantics — scope cut

Two *already-distinct* "layer" concepts exist in the codebase; conflating them
would be a real design error, so naming the boundary explicitly:

- **`Markup::layer: Option<String>`** → private `/RLLayer` — a free-text
  organizational tag, user-editable (`markup-properties.ts`), exported into
  takeoff spreadsheets. Not PDF optional content. Unaffected by this work.
- **`Markup::optional_content: Option<OptionalContent>`** → standard `/OC` — real
  PDF optional-content-group membership, preserved verbatim on round-trip since
  PR #78 (BB-interop fix wave). This is the "layer" a Bluebeam/Acrobat user
  actually sees and toggles.

**MUST-have (this work): round-trip intact — already shipped (PR #78).** A layered
BB markup keeps its `/OC` value through redline unchanged. No further work needed
for the interop contract itself.

**Full layer management (create an OCG, assign a markup to it, toggle visibility
in-app) is OUT of this pass** — genuinely separate scope: it needs an OCG-tree
model (`/OCProperties` catalog entry, order/visibility state), UI to browse/create
layers, and a decision on whether redline surfaces BB's foreign OCGs read-only or
lets users author new ones. Named as a follow-up milestone, not attempted here —
the owner's "multi layered markups sorted out" is satisfied for the *round-trip*
half by the existing `/OC` pass-through; the *authoring* half is a distinct,
larger feature.

## 6. Migration / back-compat

- `group_id` is `#[serde(default)]` already (G8) — no JSON migration needed.
- Pre-this-fix saved files with `/RLGroup` only (no `/IRT`/`/RT`) continue to
  import correctly (existing `/RLGroup` fallback in `from_annotation_dict` is
  unchanged) and will gain `/IRT`/`/RT` on next save (upgrade-in-place, no
  explicit migration step).
- Files saved by redline BEFORE this fix that came from a BB import with grouped
  markups: the grouping was silently dropped on that original import (pre-fix
  behavior) — those files have no group information to recover; this is a known,
  named data-loss window that closes going forward, not something this fix can
  retroactively repair.

## 7. Staged implementation plan

| Stage | Scope | Acceptance |
|---|---|---|
| **1 (this PR)** | `document::annots` read/write group core; `toolchest::btx` `<Child>`(s) → `Tool.children`; `Viewport.svelte` placement fan-out (bounded — reuses existing translate/create primitives) | Harness: 33/77 grouped corpus items move from "known-bad" to structurally-conformant (group membership recoverable + re-emittable); full regression battery green |
| **2 (follow-up, not built)** | Explicit group/ungroup UI action (currently implicit via shared `group_id` only); `/GroupNesting` display-name+roster fidelity on write; multi-level nesting if real evidence ever surfaces it | Named here so it isn't silently assumed done |
| **3 (follow-up, not built)** | Full OCG layer management (create/assign/toggle) | Separate design doc when scoped |

Stage 1 is what this dispatch implements. Stage 2/3 are explicitly deferred, not
partially attempted.
