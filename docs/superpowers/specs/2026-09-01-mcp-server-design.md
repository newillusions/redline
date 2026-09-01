# Redline MCP Server — Design

**Status:** proposed 2026-09-01, design-only (no code in this pass). **Charter decision:**
`decision:pkyh3se8vqcjdv5ft1je` (Martin, 2026-08-31: "including an mcp would be very
helpful, i think" — chosen over staying a pure GUI app). **Predecessor context:** the
2026-08-31 Bluebeam-currency report (`~/dev-reports/2026-08-31-bluebeam-currency.md`)
found Bluebeam Max ($590/user/yr, above our Complete-tier subscription) headlining
MCP-based AI editing as a paid differentiator — this spec is redline's answer, scoped to
what the repo's actual architecture and data model support today, not a feature-parity
chase.

## 1. Purpose and consumers

**Consumer:** a Claude Code session (this workspace's own agents, or any MCP client)
driving redline programmatically, either unattended (batch operations across a folder of
drawings) or attended (Martin co-driving markup work alongside a running redline
session). There is no other consumer in scope — no third-party integration, no
non-Claude MCP client has been asked for.

**Grounded workflows** (the owner's own framing, "batch review annotations, extract
markup schedules, apply standard comment sets" — plus what the codebase already proves
out):

- **Extract a markup schedule.** M3 already ships a Markup List XLSX/CSV export
  (`m-m3` roadmap milestone, done 2026-06-25). An MCP tool that lists/reads markups and
  triggers this existing export turns "what did the reviewer mark up on this set?" into
  a single agent-driven query instead of a manual GUI export per file.
- **Batch review annotations.** `search::indexer`/Tantivy folder search and the shipped
  markup-comment search (`src/lib/markup-search.ts`, confirmed real parity with Bluebeam
  per `docs/bluebeam-search-behavior-reference.md` §4) already answer "find every markup
  matching X across a folder." An MCP `search_markups` tool is a thin wrapper over
  infrastructure that exists and is tested today — not new domain modeling.
- **Apply standard comment sets.** The markup envelope (`src-tauri/src/markup/mod.rs`)
  already carries `subject`/`contents`/`layer`/`workflow.status` — creating/updating a
  markup with a standard note (e.g. "verify fire rating," matching the door-schedule
  fixture used throughout the annotation test suite) is a direct application of the
  existing `Markup::new`/`to_annotation_dict` path, not a new capability.

**Explicitly not grounded, so not proposed for v1:** anything requiring capabilities
redline's architecture has no path to (Visual Search's symbol-matching — see
`bluebeam-search-behavior-reference.md` §2, named there as "a large, separate future
project," not a search-parity gap), or domain concepts redline has never modeled
(`/Link` hyperlink annotations, per the same reference doc §6). Tool sprawl toward
Bluebeam's action menu (redaction-from-search, count-from-search, replace-checked) is
named there as real, separately-scoped follow-up work with its own design needs — it is
not re-proposed here as MCP surface just because Bluebeam exposes it via AI.

## 2. Architecture: embedded companion process, not a standalone file-mutating binary

### The two options, weighed against redline's actual state

**Option A — standalone binary, direct `.pdf` file access.** A new `[[bin]]` target
(the crate already has precedent: `src-tauri/src/bin/bench.rs` drives
`redline_lib::render::RenderEngine` headlessly, no Tauri, no webview — proof the core
crate is usable outside the GUI shell). This binary would open a `.pdf` with `lopdf`
directly, read/write markups via `document::annots`/`markup::annotation`, and be the MCP
stdio server itself.

**Option B — embedded companion: a thin MCP process that talks to the running redline
app.** The Tauri app (already running, with `MarkupStore` as its `AppState`, per
`src-tauri/src/document/store.rs`) opens a local RPC listener when a document is open.
`redline-mcp` — a new, small `[[bin]]` target in the same crate — is the actual MCP
stdio server Claude Code launches; it has no PDF logic of its own and is a pure protocol
translator: MCP JSON-RPC over stdio on one side, the app's local RPC over a Unix domain
socket (macOS) / named pipe (Windows) on the other. Tool calls become the exact same
`commands/*.rs` operations the Svelte frontend already invokes via Tauri `invoke` —
routed through one more transport, not a parallel code path.

### Recommendation: Option B

The deciding factor is **single-writer correctness**, and it is a harder problem than
the lock question the charter decision names. `MarkupStore` is explicitly the
**in-memory** source of truth for unsaved edits (`document/store.rs` module doc: "single
source of truth for unsaved markup state; the save pipeline flushes it to the PDF").
Redline's `MtimeCache` (same file) exists only to detect a file changed *since redline
last read it* — there is no locking, leasing, or conflict-detection protocol for a
*second* writer touching the same file while redline holds it open with unsaved
changes. A standalone binary (Option A) writing directly to disk while the GUI has the
same file open is not a hypothetical edge case for this consumer profile — "co-driving
alongside a running redline session" is the owner's own stated use case — and the
failure mode is a straightforward lost-update: whichever side saves second silently
discards the other's unsaved work, with no error, no conflict signal, and no path to
recovery (redline has no merge or diff-against-store step in its save pipeline today).
This is strictly worse than the markup-lock gap the decision cites: a lock check can
refuse one bad edit; a dual-writer race can silently destroy an unrelated one.

Option B avoids inventing that failure mode by construction — there is exactly one
process (the running app) that ever mutates `MarkupStore` or calls `document::save`,
regardless of whether the caller is the Svelte frontend or the MCP bridge. It also
directly serves the co-driving workflow: mutations an agent makes appear live in the
GUI's overlay immediately (same `MarkupStore`), are undo/redo-able through the existing
command-pattern history (shipped, per the mission record's `undo-redo-ui-gap` item), and
a human watching the window sees exactly what the agent is doing as it happens — which a
file-level binary cannot offer at all.

The cost is real and is named plainly: Option B requires the app to be running with the
target document open (an MCP call against a closed app returns a clear "redline is not
running / that document is not open" error, never a silent fallback to direct file
access), and it requires building a new local-RPC bridge inside the Tauri app rather than
just calling core-crate functions from a second binary. That infrastructure cost is
accepted deliberately — it is what buys correctness against the two-writer race, which no
amount of careful MCP-side code can buy on its own once a second process is touching the
file directly.

**Concretely, three components:**
1. **`redline` (existing GUI binary)** gains a local RPC listener, started when a
   document is opened, torn down when the last document closes. It re-exposes the
   existing `commands/*.rs` functions (already `State<AppState>`-scoped) behind a small
   socket-facing dispatcher — no new business logic, a new transport for logic that
   already exists.
2. **`redline-mcp` (new `[[bin]]` target, same crate)** — the actual MCP stdio server.
   Pure protocol translation (MCP JSON-RPC ↔ the local socket's RPC). No PDF/model code.
3. **The local socket/pipe** is loopback-only (see §5) — never a network listener.

## 3. Tool surface v1

Deliberately small, and split into two waves matching the rollout in §6. Every tool
name below is illustrative, not final — schemas are sketched at the level needed to
validate the design, not to freeze wire format.

### Wave 1 — read-only (ship first)

| Tool | Input | Output | Grounding |
|---|---|---|---|
| `list_markups` | `doc_id`, optional `page`, `type_filter[]`, `status_filter[]` | array of markup summaries: `id`, `markup_type`, `page`, `subject`, `contents` (truncated), `locked`, `locked_contents`, `workflow.status`, `count_set.name?` | Direct read of `MarkupStore` via the existing per-doc markup list, same data the Properties panel already renders. |
| `read_markup` | `markup_id` | the full `Markup` envelope (all fields in `markup/mod.rs`, serde-shaped) | `Markup` already derives `Serialize` — this is the existing JSON shape, not a new one. |
| `search_markups` | `query`, `scope` (`document`\|`page`\|`open_docs`\|`recents`\|`folder`), `case_sensitive?`, `whole_word?` | grouped-by-file hit list (location + matched text/markup id) | Reuses shipped search infra (`search_document`/`search_paths`/Tantivy `search_folder`, markup-comment search) — confirmed real parity with Bluebeam's "Search Markups," not an invention (`bluebeam-search-behavior-reference.md` §4). |
| `export_markup_schedule` | `doc_id`, `format` (`csv`\|`xlsx`) | file path of the generated export | Wraps the existing M3 Markup List export command — no new export logic. |

### Wave 2 — mutating (ship only after the lock guard in §4 exists and is tested)

| Tool | Input | Output | Grounding |
|---|---|---|---|
| `create_markup` | `doc_id`, `markup_type`, `page`, `geometry`, `appearance`, `contents?`, `subject?`, `layer?` | the created `Markup` (with assigned `id`) | Direct call to `Markup::new` + `MarkupStore` insert — the same path `markup-commands.ts`'s `CreateCmd` already uses from the frontend. |
| `update_markup` | `markup_id`, partial field set (`contents?`, `appearance?`, `workflow.status?`, ...) | the updated `Markup` | Whole-annotation update in v1 (no field-level patch API yet — see the granularity note in §4). Refused if the lock guard blocks it. |
| `delete_markup` | `markup_id` | confirmation | Refused if the lock guard blocks it. |
| `save_document` | `doc_id` | confirmation + revision count | Calls the existing save command (`document::save`) — mutations above are in-memory (`MarkupStore`) and undo-able until this is called, exactly matching the GUI's own explicit-save model. |

No batch/bulk tool is proposed in v1. Bluebeam's "Check Options" bulk actions
(highlight/underline/redact/count/replace-checked) are named in the search reference doc
as real, separately-scoped follow-ups, several with their own destructive-action design
questions (redaction, search-and-replace) explicitly flagged there as needing owner
sign-off before any one-click exposure — that applies at least as strongly to an
AI-driven MCP tool as to a GUI button, so bulk mutation is out of scope here, not merely
deferred quietly.

## 4. Markup locks — the first-class invariant

**Redline has no lock concept today.** This must be stated plainly rather than assumed
solved: `Markup::annot_flags: i32` (`markup/mod.rs`) stores the raw ISO 32000-1 §12.5.3
Table 165 `/F` annotation-flags bit field losslessly on every round-trip (confirmed by
grep across `src-tauri/src` — `annot_flags` appears only in serialization, the default-4
constant, and round-trip tests; there is no `is_locked()` accessor, no lock check in any
`commands/*.rs` file, and no UI enforcement in the frontend). A foreign Bluebeam/Acrobat
annotation's `Locked` bit already survives open→save→reopen unmolested purely because
the whole `i32` round-trips — but nothing today *reads* that bit to refuse an edit,
whether the edit comes from the GUI's own Properties panel or (once built) an MCP tool.
This is exactly the gap the charter decision's own citing of Bluebeam 21.10 ("MCP could
override locked markups when executing prompts") warns against, and it is not yet
closed anywhere in this codebase, not just in the MCP-shaped slice of it.

### The two relevant bits (ISO 32000-1 Table 165)

- **Bit 8, `Locked` (decimal value 128 / `0x80`):** the annotation may not be deleted or
  have any of its properties modified by the user, other than `/Contents`.
- **Bit 10, `LockedContents` (decimal value 512 / `0x200`):** the `/Contents` (and, by
  extension, the appearance it drives) may not be modified, but other properties —
  including deletion — remain editable.

### Minimal lock-respect contract for v1

1. Add `Markup::is_locked(&self) -> bool` and `Markup::is_contents_locked(&self) -> bool`
   as plain bit tests on `annot_flags` (`markup/mod.rs`, next to the existing
   `MarkupType::is_measurement`) — a self-contained, ~5-line, fully unit-testable
   addition with no dependency on the MCP work.
2. **Build this as a shared `redline_lib` guard, not an MCP-only check.** The GUI itself
   does not enforce these bits today either. Implementing the refusal once — inside the
   command layer both the Svelte frontend and the `redline-mcp` bridge call into (§2) —
   fixes the gap for both surfaces from one change, and is a stronger invariant than an
   MCP-side-only check that a future GUI code path could still bypass.
3. **v1 granularity is coarse, and the contract is scoped to match it, deliberately.**
   `update_markup` (§3) mutates the whole annotation state in one call — there is no
   field-level patch API yet. The spec-correct fine-grained behavior (`LockedContents`
   permits deletion and non-content property changes; `Locked` permits only `/Contents`
   edits) cannot be honored safely at this granularity without risking a "the caller only
   meant to move it but the whole call got through and also changed the note" gap. **v1
   treats `Locked` OR `LockedContents` as "refuse the whole mutation"** for
   `create_markup`/`update_markup`/`delete_markup` alike. This is a deliberate
   simplification, named as such — not a misreading of the ISO distinction — and is the
   safe direction to simplify in (refuse more than the spec strictly requires, never
   less). Revisit once/if a field-level patch tool is ever proposed.
4. **Refusal is a structured error naming the markup and the blocking flag**, e.g.
   `{"error": "markup_locked", "markup_id": "...", "flag": "Locked"}` — never a silent
   no-op and never a partial write. This gives the calling agent something concrete to
   relay back to the user ("markup X is locked, I can't apply that edit") instead of
   retrying blindly or reporting a false success.
5. **`list_markups`/`read_markup` (Wave 1, §3) surface `locked`/`locked_contents` in
   their output schema.** An agent should be able to see a markup is locked before
   attempting a doomed mutation, not only discover it via a refusal after the fact.
6. **No lock-override tool or flag is proposed, at any privilege level.** Bluebeam's own
   bug was an MCP path that *could* override a lock; the fix here is that no such path
   exists to begin with, not that one exists and defaults to off.

## 5. Auth and exposure

**Local-only, v1, no exceptions.** `redline-mcp` speaks stdio to its Claude Code parent
process — the same pattern as any local dev MCP server, no HTTP/SSE transport. The
companion local socket between `redline-mcp` and the running GUI app (§2) is
loopback-only: a Unix domain socket under a per-user runtime directory with `0600`
permissions on macOS, a named pipe with an ACL restricted to the current user's SID on
Windows — never a TCP listener, never bound to a network interface. No credentials are
stored or transmitted; the socket's filesystem/ACL permissions are the entire access
control. This matches CLAUDE.md's explicit v1 non-goal of cloud/real-time collaboration
(spec §2) and needs no new auth infrastructure — remote/multi-user MCP exposure is out of
scope for this design entirely, not merely deferred.

## 6. Rollout: read-only first, mutation gated on the lock guard existing

**Wave 1 (§3) ships before any mutating tool is proposed for implementation.** Two
independent reasons, not one:

- The local socket-RPC bridge (§2) is new infrastructure. Four read-only tools are
  enough to prove the transport end-to-end (app running → socket → `redline-mcp` →
  Claude Code) against real documents without any risk of a bad write, and are useful on
  their own (schedule export, cross-folder search) even before mutation exists.
- **Wave 2 must not ship before §4's lock guard is built and tested.** This is the
  direct, literal answer to the design constraint the charter decision states: redline's
  MCP must not repeat Bluebeam 21.10's mistake. The only way to guarantee that is
  sequencing — no `create_markup`/`update_markup`/`delete_markup` tool exists in any
  shipped build until `is_locked`/`is_contents_locked` and the shared command-layer
  refusal (§4 items 1–4) have their own TDD pass and are exercised by tests covering:
  a locked foreign (Bluebeam-authored) annotation, a locked redline-authored one, an
  unlocked one (control case), and `LockedContents`-only. `save_document` ships in the
  same wave as the other three mutating tools, not earlier — a save tool with nothing
  yet able to mutate `MarkupStore` via MCP has no purpose on its own.

Grooming/scoping this design pass itself is the gate before any implementation task is
opened, per the decision's own "scoping/design pass to be groomed before any build."
This document is that pass; it does not authorize starting Wave 1 implementation on its
own — that is a separate owner-gated go/no-go, consistent with how every other milestone
in this project has been gated (see the mission record's `architect-gate-2026-07` item:
"no large-set doc-surgery features without a docops backend decision" — the same posture
applies here: design first, gate, then build).

## Open questions for the owner (not resolved by this design)

1. **Does Wave 1 alone satisfy the "helpful" bar Martin described, or is mutation the
   actual point?** If the primary desired workflow is "AI edits markups for me," Wave 1
   is a smaller deliverable than that implies and should be scoped as a checkpoint, not
   the whole charter.
2. **Where does `redline-mcp` get distributed/launched from?** Same installer as
   `redline` itself (a second executable in the app bundle), or a separate lightweight
   package a Claude Code session installs independently? Affects packaging, not the
   architecture above.
3. **Multi-document sessions.** The local RPC surface is sketched per-document
   (`doc_id`-scoped); redline supports tabbed multi-file sessions. Whether `list_markups`
   without a `doc_id` should mean "the active tab" or "every open tab" needs a decision
   before Wave 1's schema is frozen.
