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

## Implementation notes (Phase 1, 2026-09-01)

Phase 1 shipped the FULL tool surface in one PR, per the owner's scope decision
(2026-09-01, "We need mutation as well... And the ability to flatten and reduce file
size through the mcp."), not the read-only-first/mutation-gated rollout §6 above
originally proposed - the lock guard was built as a build prerequisite instead, per
that same decision, and every mutating tool ships behind it from day one.

**Flatten/reduce-file-size assessment (§3's owner-added pair): both primitives already
existed.** `commands::docops::flatten_document`/`optimize_document` (M5, PR #67/#69)
were already full, tested, shipped Tauri commands wrapping `docops::flatten_annotations`/
`optimize_in_place_with_images`. No new core PDF logic was needed - both MCP tools
(`flatten_document`, `reduce_file_size`) are thin pass-throughs to these exact existing
commands via `AppHandle::state()`. Neither is a phase-2 deferral.

**Lock guard**: `Markup::is_locked`/`is_contents_locked` (§4 items 1-2) landed in
`markup/mod.rs`, and the refusal (§4 items 3-4, structured `markup_locked` error) is
wired into `document::store::MarkupStore::update`/`delete` - the single choke point both
`commands::document::update_markup`/`delete_markup` (GUI) and the MCP bridge's
`update_markup`/`delete_markup` tools call through, closing the GUI gap in the same
change as designed. `create_markup` has no lock check (nothing to lock before a markup
exists). TDD per §6's four cases (locked foreign, locked redline-authored, unlocked
control, LockedContents-only) at both the `markup::check_not_locked` level and the
`MarkupStore` level.

**Architecture as designed**: `redline-mcp` (new `[[bin]]`, pure protocol translator,
no PDFium/Tauri dependency) speaks MCP JSON-RPC 2.0 over stdio to its Claude Code
parent, and line-delimited JSON (`rpc::protocol::RpcRequest`/`RpcResponse`) over a local
socket to the running GUI app. Every tool call resolves to the exact existing
`commands::*`/`MarkupStore` functions the Svelte frontend already invokes - no parallel
mutation path.

**Named deviations from the design's illustrative sketch** (§3 said schemas were
"sketched... not to freeze wire format" - these are the concrete choices made filling
that in):
- **Socket lifecycle is per-app, not per-document.** §2/§6 described starting the
  bridge when a document opens and tearing it down when the last one closes. Phase 1
  starts it once in `setup()`, for the app's full lifetime. The security posture is
  identical (filesystem-permission gated either way) and a call against an unopened
  `doc_id` still gets the exact "unknown doc_id" refusal the design requires - see
  `rpc` module's doc comment for the full reasoning.
- **`search_markups` searches markup subject/contents text, not raw PDF text**, and
  only `scope: "document"` is implemented - `page`/`open_docs`/`recents`/`folder` are
  named in the schema but refused with a clear error, not silently narrowed.
- **`export_markup_schedule` takes no path input; it generates one** next to the source
  document (`<stem>-markup-schedule-<timestamp>.<ext>`) and returns it - matching the
  design's stated OUTPUT ("file path of the generated export") rather than the
  underlying `export_markup_list` Tauri command's GUI-dialog-supplied-path shape.
- **`update_markup` is a v1-simplified partial merge**: a field can be SET but not
  cleared back to `None` (no JSON null-vs-absent distinction implemented) - pass an
  empty string to blank `contents`.
- **doc_id was added to `read_markup`/`update_markup`/`delete_markup`/`save_document`**
  (the design's illustrative tables showed some of these keyed by `markup_id` alone) -
  `MarkupStore` is doc-scoped, so this is required, not optional.
- **Socket endpoint**: `$TMPDIR/redline-mcp.sock` (Unix) / `\\.\pipe\redline-mcp`
  (Windows) - a fixed, single-instance-assumption path both binaries compute
  independently with zero shared runtime state, since `redline-mcp` has no Tauri
  context to derive an app-scoped directory from.

**Windows named-pipe path: UNVERIFIED, named plainly.** This build/test environment is
macOS-only for this session - the Windows server (`rpc::run_windows`) and client
(`redline_mcp::connect` under `cfg(windows)`) branches were written from documented API
shapes but never compiled or run against a Windows target. They also fall short of the
design's explicit ACL requirement (§5: "a named pipe with an ACL restricted to the
current user's SID") - `ServerOptions` uses tokio's defaults rather than a custom
security descriptor, which needs `windows-sys` calls this session could not verify.
Flagged as a real follow-up (compile + fix + tighten the ACL on the Windows testbench),
not shipped-and-proven coverage. The Unix domain socket path (macOS/Linux) is fully
built and unit-tested.

**Verified this session** (all quoted from this session's own tool output): `cargo test
--workspace --all-targets` - 611 passing, 0 failed (7 pdf-diff-lib + 14 pdf-diff-tests +
586 redline_lib + 4 redline-mcp bin, plus 18 pre-existing corpus-gated `#[ignore]`
tests, unchanged); `cargo clippy --all-targets` - 0 warnings; `cargo build --bins` -
all three binaries (`redline`, `bench`, `redline-mcp`) link successfully.

**Not verified / out of reach in this environment, named honestly:**
- No live `cargo tauri dev` GUI session was run - the socket bridge, a real
  `redline-mcp` <-> app round trip, and the lock guard's effect on the actual GUI
  Properties panel are unexercised end-to-end. Same class of gap named in every prior
  docops PR in this repo's history (obs:3dw1ojc5vtw8cdklhlla, obs:t5g42nvkczmj5p12hhrt).
- The Windows named-pipe path, as stated above.
- Multi-document / multi-instance behaviour beyond what's noted above (open questions 2
  and 3 below remain genuinely open).

## Implementation notes (Phase 2a, 2026-09-03)

Phase 1 shipped ten tools that all require a `doc_id`, but left no way for a client to
obtain one - the orchestrator hit this live 2026-09-02, having to read a `doc_id` out of
the app's own log file (`observation:dtko8oxo8fooqt7qrt44`). Phase 2a adds four
document-lifecycle tools, owner-approved 2026-09-03 ("add open tools"), closing that
gap: `list_open_documents`, `open_document`, `close_document`, `get_active_document`.
Fourteen tools total.

**`list_open_documents`/`get_active_document` resolve open question 3 above** (whether
`list_markups` without a `doc_id` should mean "the active tab" or "every open tab"): the
answer taken is neither - `doc_id` stays required on every Wave 1/Wave 2 tool (no change
to their schemas), and these two new tools are how a client discovers a `doc_id` in the
first place. `list_open_documents` enumerates every open tab (doc_id/path/title/
page_count/is_active/dirty); `get_active_document` resolves to the one currently
focused. Both are needed - a client that already knows the file it wants uses
`list_open_documents` to find its doc_id (or dedups via `open_document`); a client
co-driving alongside a human uses `get_active_document` to follow whichever tab the
human is looking at.

**"Active tab" and "dirty" are two different plumbing problems, solved differently.**
Neither concept existed anywhere in the Rust backend before this phase - both are
Svelte-only in the existing multi-doc-tab feature (`DocTabStore.activeDocId`,
`MarkupStore.dirty` in `src/lib/doc-tabs.svelte.ts`/`markup-store.svelte.ts`).
- **`dirty` is derived entirely server-side**, with no new frontend plumbing: a
  `dirty: bool` field was added to `document::store::DocEntry`, set `true` by
  `MarkupStore::add`/`update`/`delete` (the same choke point both the GUI's own
  `add_markup`/`update_markup`/`delete_markup` commands and the MCP `create_markup`/
  `update_markup`/`delete_markup` tools already call through - so an MCP-driven edit and
  a GUI-driven edit both mark the doc dirty correctly, for free), and cleared by a new
  `MarkupStore::clear_dirty`, called from both `commands::document::save_inner` (covers
  `save_document`/`save_document_as`) and the end of `commands::document::apply_page_edit`
  (covers rotate/delete/reorder/insert page ops and, via `commands::docops`, flatten/
  optimize/redact - all of which flush the current markup state to disk exactly like a
  save does, via the same `write_markups`-then-atomic-rename pipeline).
  `MarkupStore::seed_loaded` (merging a document's own pre-existing on-disk annotations
  at open time) deliberately does NOT touch `dirty` - that is not an edit.
- **`is_active` genuinely requires a frontend push** - there is no way for the backend
  to infer which Svelte tab is focused from anything it already tracks. A new
  `AppState.active_doc: Mutex<Option<String>>` plus a `set_active_document` Tauri
  command is written by exactly one place in the frontend: a single `$effect` in
  `App.svelte` watching `tabStore.activeDocId`, added once rather than at each of the
  several call sites that change it (`addTab`/`switchTab`/`closeTab`, keyboard
  next/prev-tab) - see `set_active_document`'s Rust doc comment and the `$effect`'s own
  comment in `App.svelte` for why a single watcher was chosen over per-call-site pushes.
  **Named limitation**: closing a document via the MCP `close_document` tool has no
  channel to tell the frontend to refresh its own tab list or `activeDocId` - the
  frontend's tabs are unaware of an MCP-driven close. `close_document`'s Rust
  implementation and its RPC dispatch arm both clear `active_doc` when it matches the
  closed doc, so `get_active_document` never reports a doc that no longer exists, but a
  GUI tab for it can still be showing until the human interacts with it. This mirrors
  the design's own stated architecture cost (§2: "an MCP call against a closed app
  returns a clear error, never a silent fallback") extended to a subtler case the
  original design didn't anticipate (a *third* actor - the MCP client - changing tab
  state the GUI doesn't poll for).

**`open_document`'s already-open dedup** (design requirement, §3's illustrative sketch
predates this tool but the phrasing carries over: "returns the existing doc_id if that
path is already open") is implemented via a new `MarkupStore::find_by_path`, checked
before falling through to the real `commands::document::open_document` Tauri command -
the same function the Svelte frontend's file-open dialog and the `REDLINE_OPEN_PDF`
auto-open both call (no forked open logic, per §2). An MCP `path` must be absolute
(checked before touching the filesystem) - a relative path is refused with a
`path_not_absolute` structured error rather than being resolved against whatever
directory the GUI process happens to be running in.

**`close_document`'s dirty refusal** reads the same server-side `dirty` flag: refused
with a structured `document_dirty` error unless `discard_changes: true` is passed - "the
document was never silently dropped" (design §4's refusal-shape precedent for locked
markups, reused here for a different guard).

**Verified this session, live, against an isolated dev instance** (never the installed
release build or any pre-existing running `redline` process - see the constraint in the
dispatch prompt and the isolation method below): a full JSON-RPC round trip over the
real `redline-mcp` stdio protocol and the real Unix-socket bridge, against a scratch PDF
under a session-isolated `$TMPDIR` (the fixed `$TMPDIR/redline-mcp/mcp.sock` path is a
single-instance assumption per §2's own note - a second, deliberately isolated `TMPDIR`
was used for this dev instance rather than colliding with whatever `redline` process was
already running on the default one). Observed and quoted in the implementer's RETURN:
`tools/list` advertises all 14 tools with the schemas above; `list_open_documents`
correctly reported `is_active: true` for only the most-recently-focused tab across four
accumulated open docs (proving the `$effect` sync works end-to-end through a live
webview, not just in isolation); `open_document` on an already-open path returned
`already_open: true` with the existing doc_id; `create_markup` flipped `dirty: true` in
a following `list_open_documents` call; `close_document` correctly refused with
`document_dirty` while dirty, then succeeded once `save_document` had cleared it (and
the saved file was re-verified as a valid, larger PDF via `qpdf --check`); closing the
active doc correctly made `get_active_document` report `no_active_document`.
`cargo test --workspace --all-targets`: 645 passed (7 pdf-diff-lib + 14 pdf-diff-tests +
616 redline_lib + 8 redline-mcp bin), 0 failed among tests actually exercised, 5 ignored
(unchanged) - one pre-existing `redline-mcp` bin test
(`call_bridge_connection_failure_is_a_structured_tool_error`) failed in this same run
purely because a genuinely-running `redline` process (not this session's isolated
instance, which had already been torn down) occupied the real socket path; this is an
environmental collision with the test's unstated "nothing else is running" assumption,
reproduced identically by stashing every change in this phase and re-running against the
unmodified Phase 1 tip, not a regression from this work. `cargo clippy --all-targets`: 0
warnings. `cargo build --bins`: all binaries link. `npm run check` (svelte-check): 0
errors (23 pre-existing warnings, none in the two files this phase touched). `npm test`:
49 files / 820 tests passed.

**`cargo fmt --all -- --check` reports pre-existing, repo-wide drift unrelated to this
phase** - 223 `Diff in` blocks across the codebase, confirmed present identically on the
unmodified Phase 1 tip (`git stash` + re-run). Not fixed here - reformatting ~150
untouched files is out of this PR's scope; `git diff --stat` confirms only the 8 files
this phase intentionally touched changed.

## Implementation notes (Phase 2b, 2026-09-03)

Phase 2b exposes the rest of the app's existing command surface as MCP tools, per the
owner's "start 2b" direction (2026-09-03) - sixteen tools across search, takeoff, page
operations, compare, and docops, following the exact same wrap-don't-reimplement pattern
as Phase 1/2a: every tool is a thin pass-through in `rpc::dispatch::dispatch` to the
EXISTING `commands::*` Tauri command function, called via `AppHandle::state()`, no new
domain logic. Thirty tools total.

**Tools added** (param shapes in `rpc::tools`; dispatch arms in `rpc::dispatch`;
`tools/list` schemas in `redline_mcp.rs`'s `tool_defs()`):

| Tool | Wraps | Mutates | Persists |
|---|---|---|---|
| `search_document` | `commands::text::search_document` | no | n/a |
| `open_folder_index` | `commands::search::open_folder_index` | in-app search state only | on-disk index cache (app data dir) |
| `search_folder` | `commands::search::search_folder` | no | n/a |
| `folder_index_status` | `commands::search::folder_index_status` | no | n/a |
| `list_scales` | `commands::takeoff::list_scales` | no | n/a |
| `add_scale` | `commands::takeoff::add_scale` | yes | immediately, to sidecar |
| `delete_scale` | `commands::takeoff::delete_scale` | yes | immediately, to sidecar |
| `write_page_measure` | `commands::takeoff::write_page_measure` | yes | immediately, to the PDF file |
| `export_markup_list` | `commands::takeoff::export_markup_list` | no (writes a new file, doesn't touch the source) | new file at caller-supplied path |
| `rotate_page` | `commands::document::rotate_page` | yes | immediately, to the PDF file |
| `delete_page` | `commands::document::delete_page` | yes | immediately, to the PDF file |
| `reorder_pages` | `commands::document::reorder_pages` | yes | immediately, to the PDF file |
| `insert_blank_page` | `commands::document::insert_blank_page` | yes | immediately, to the PDF file |
| `compare_pages` | `commands::compare::compare_pages` | no | n/a |
| `redact_document` | `commands::docops::redact_document` | yes | immediately, to the PDF file |
| `save_document_as` | `commands::document::save_document_as` | no (writes a new file) | new file at caller-supplied path |

**Page ops and docops writes are NOT gated behind `save_document`, unlike markup
create/update/delete.** `rotate_page`/`delete_page`/`reorder_pages`/`insert_blank_page`/
`redact_document`/`write_page_measure` all share `commands::document::apply_page_edit` -
the same load-op-save(atomic temp+rename)-reload pipeline used since M4/M5 - which writes
the file on disk and reloads the render engine before the command even returns. There is
no in-memory staging for these, and no document-level lock concept exists anywhere in this
codebase to check before allowing one (only the markup-level `Locked`/`LockedContents` PDF
annotation flags from Phase 1's lock guard, which apply to individual annotations, not
page structure). Every one of these tool descriptions states this plainly rather than
implying a save step exists. Live-verified this session (see below): each of the four page
ops changed the target file's md5 hash immediately, with no `save_document` call in
between.

**`compare_pages` is the one tool that takes raw file paths (`path_a`/`path_b`), not a
`doc_id`.** The underlying Tauri command never touches `AppState`/`MarkupStore` - it runs
a standalone two-tier diff over two file paths and does not require either document to be
open in redline at all. Matching that 1:1 ("wrap, don't reimplement") was the more honest
choice than inventing a `doc_id`-based contract the real command doesn't have. Its MCP
response also omits `PageDiffResult::diff_png_b64` (a base64 PNG overlay, tens-to-hundreds
of KB per page) - a calling agent wants the numeric verdict (`changed_pct`,
`text_char_match`, etc.), not an embedded image in a JSON-in-text tool response.

**`export_markup_list` vs `export_markup_schedule` (wave 1): same writer, different path
contract, not different content.** Both ultimately call
`commands::takeoff::export_markup_list_to` and produce byte-identical output for the same
markup set. `export_markup_schedule` (Phase 1) generates its own path next to the source
document; `export_markup_list` (Phase 2b) requires the caller to supply an explicit `path`,
matching the underlying Tauri command exactly (it's driven by a GUI save dialog on the
frontend). Both are kept, per the owner's explicit "keep both, document the difference"
instruction - this is that documentation.

**`search_folder`/`folder_index_status` are root-checked, not silently mis-scoped.**
`AppState.folder_index` is a single `Mutex<Option<FolderIndex>>` - redline supports exactly
one active folder index at a time. `search_folder` refuses with a structured
`folder_index_root_mismatch` error (naming the requested and actual root) if no index is
open, or it's open for a different folder than the caller asked about, rather than silently
searching whatever happens to be active. `folder_index_status` instead returns the real
status plus a `matches_requested_root` boolean, since polling status is a read and the
caller can see the mismatch directly in the returned `folder_path`.

**`compare_pages` could NOT be live-verified - a real, pre-existing hang, named plainly
rather than papered over.** Every attempt (four, across two fresh dev-instance restarts,
including one against two completely untouched copies of the e2e fixture) hung
indefinitely inside `commands::compare::compare_pages` itself - the dev instance's log
shows `pdf_diff: PDFium loaded` and then nothing further, ever, for as long as 120s. Root
cause, inferred not confirmed: `crates/pdf-diff/src/lib.rs`'s `PdfDiffEngine::new()` calls
`Pdfium::bind_to_library`/`bind_to_system_library` to create its OWN, SEPARATE PDFium
binding, independent of `redline_lib::render::RenderEngine`'s binding - which was already
active on its own dedicated thread in the same process for every reproduction (a document
was open via the render engine in three of the four attempts; the fourth used fresh files
but the render engine's PDFium binding was still resident in-process from the earlier
`open_document` calls in that same session). This matches the exact class of hazard this
project's own judgment rules already name for PDFium ("PDFium global C state" - the reason
`REDLINE_BENCH_TESTS` tests must run serial) - a second concurrent binding to the same
underlying library, in the same process, is plausible to deadlock on a shared global lock
FPDFium's C API isn't documented as safe against. This is NOT caused by this PR's MCP
wrapper - the hang happens inside the existing `compare_pages` Tauri command before the
wrapper's own code (which only strips one field from the result afterward) ever runs, and
`pdf-diff`'s own isolated unit/integration tests (7 + 14, no render engine sharing the
process) pass cleanly in the same session. Flagged as a real, unverified gap for the owner
- `compare_pages` may need to run its diff on a dedicated thread that never shares a
process with an active `RenderEngine` PDFium binding, or the `pdf-diff` crate needs to
reuse the render engine's existing binding rather than creating its own. The MCP tool and
its schema ship as designed (matching the underlying command 1:1, per "wrap, don't
reimplement") but its live behavior is unverified pending that fix.

**Verified this session, live, against an isolated dev instance** (never the installed
release build or any pre-existing running `redline` process - the pre-existing process on
the default `$TMPDIR`, confirmed by `pgrep -x redline` before and after every step, was
never touched; each isolated instance ran under its own short-path `TMPDIR` and was killed
by its own PID at the end, located via `lsof` on that instance's own socket path, never a
broad process match). `search_document` found real text hits by rect; `open_folder_index`
+ `folder_index_status` + `search_folder` round-tripped correctly including the
`folder_index_root_mismatch` refusal on a mismatched root and a real hit
(`"snippet":"Redline WDIO E2E <b>fixture</b> ..."`) once queried with real fixture text
(a bare `"a"`/`"sample"` query returned no folder-search hits, consistent with Tantivy's
default English stopword/stemming behavior, not a wrapper defect - `search_document`
against the same text via PDFium's own search found `"a"` in four places, confirming the
underlying text IS there and the two search engines simply tokenize differently, which is
pre-existing, not something this PR changed). `list_scales`/`add_scale`/
`write_page_measure`/`export_markup_list`/`delete_scale` round-tripped correctly, and
`export_markup_list`'s output file was confirmed to exist on disk with real CSV headers.
`insert_blank_page`/`rotate_page`/`reorder_pages`/`delete_page` all changed the target
file's md5 immediately (before, `7117bad2...`; after all four ops, `b6decd99...`),
confirming the no-staging/no-save-step behavior claimed above. `redact_document` changed
its target file's md5 immediately (`e473e8b3...` -> `0f718911...`). `save_document_as`
wrote a distinct, valid, differently-sized PDF at the caller-supplied path while leaving
the source file's md5 unchanged. `compare_pages` did not complete - see above.

`cargo test --workspace --all-targets`: 660 passed (7 pdf-diff-lib + 14 pdf-diff-tests +
631 redline_lib + 8 redline-mcp bin), 0 failed among tests actually exercised, 18 ignored
(unchanged from Phase 2a's 5 plus 13 more `#[ignore]`d elsewhere in the workspace already
present pre-PR) - the one `redline-mcp` bin test failure
(`call_bridge_connection_failure_is_a_structured_tool_error`) is the SAME pre-existing
environmental collision Phase 2a already documented: a genuinely-running `redline` process
(this session's own default-`$TMPDIR` neighbor, PID confirmed via `pgrep -x redline` both
before this PR's changes and after, i.e. present regardless of this diff) answers the
test's connection attempt instead of refusing it. `cargo clippy --all-targets`: 0 warnings
(one `clippy::doc_lazy_continuation` warning was found and fixed during this session, in
this PR's own new doc comment on `ComparePagesParams`). `cargo fmt --all -- --check`:
this PR's own three touched files are fully clean; the pre-existing, repo-wide drift
Phase 2a already documented (223 `Diff in` blocks there) is unrelated and unchanged by
this PR - confirmed via `git stash` + re-run against the unmodified pre-PR tip (226 blocks,
consistent with Phase 2a's number within normal drift-count variance across sessions);
`git diff --stat` confirms only the four files this phase intentionally touched changed
(`CLAUDE.md`, `src-tauri/src/bin/redline_mcp.rs`, `src-tauri/src/rpc/dispatch.rs`,
`src-tauri/src/rpc/tools.rs`) plus this design doc.

**Not verified / out of reach in this environment, named honestly:** `compare_pages`'s
live behavior (see above - the tool exists and matches the design, but could not be
exercised end-to-end); the Windows named-pipe path (unchanged from Phase 1/2a - this
session was also macOS-only); no frontend (Svelte/`npm test`/`npm run check`) changes were
made or re-verified in this phase, since none of this phase's code touches the frontend.

### Fix round (2026-09-03, PR #99 review)

Three findings from the fresh-context PR review, all fixed on the same branch before merge:

1. **[HIGH] `redline-mcp` had no socket read/write timeout anywhere.** Its own loop is a
   single blocking thread (no tokio), so one stuck server-side call - `compare_pages`
   above is the known case - wedged the entire client process silently for every
   subsequent tool call too, not just that one. Fixed: `connect()` now sets
   `set_read_timeout`/`set_write_timeout` on the Unix socket (default 120s, override via
   `REDLINE_MCP_TIMEOUT_SECS`) before boxing the stream; a resulting `WouldBlock`/
   `TimedOut` I/O error is mapped to a structured `redline_timeout` tool error instead of
   the generic `read_failed`/`write_failed`, both distinguished by `socket_io_err_to_json`.
   `call_bridge`'s body was extracted into `call_bridge_over_stream<S: Read + Write>` so
   this is unit-testable against a synthetic `UnixStream::pair()` whose peer is held open
   but never written to - a genuine "socket that never replies," not a connection
   failure - and asserted to return the `redline_timeout` error within a bounded time
   rather than hanging the test suite itself. The Windows named-pipe client path does
   **not** get this timeout yet (`std::fs::File` has no equivalent method; would need
   `SetNamedPipeHandleState`/overlapped I/O via `windows-sys`) - named as a follow-up
   alongside the pipe path's existing UNVERIFIED status, not silently skipped.
2. **`compare_pages` pulled from the default tool list.** `tool_defs()` now delegates to
   `tool_defs_for(experimental: bool)`, which omits the `compare_pages` entry unless
   `REDLINE_MCP_EXPERIMENTAL=1` is set - so a client's `tools/list` no longer advertises a
   tool known to hang by default. `handle_tools_call` additionally refuses a direct
   `compare_pages` call by name (before ever touching the socket) when the gate is off,
   with a structured `experimental_tool_disabled` error naming the env var - belt and
   suspenders beyond the timeout fix above, since even a client-side-recovered hang can
   still leak a stuck app-side blocking thread from the double PDFium binding. This is a
   client-side-only gate (`redline_mcp.rs`); the server-side dispatch arm in
   `rpc/dispatch.rs` is untouched and still wraps the real command correctly for when the
   root cause is eventually fixed and the gate is lifted.
3. **[LOW] `save_document_as` now validates its path is absolute**, mirroring the
   `open_document` guard exactly (`path_not_absolute` structured error) - a relative path
   previously would have resolved against the Tauri process's cwd rather than the source
   document's directory.

Tracked as a mission-record next step: `fix pdf-diff second PDFium binding (reuse render
engine binding or dedicated process)` - the underlying `compare_pages` hang itself remains
unfixed; the fixes above make it recoverable and opt-in, not fixed at the root.

## Open questions for the owner (not resolved by this design)

1. **Does Wave 1 alone satisfy the "helpful" bar Martin described, or is mutation the
   actual point?** If the primary desired workflow is "AI edits markups for me," Wave 1
   is a smaller deliverable than that implies and should be scoped as a checkpoint, not
   the whole charter.
2. **Where does `redline-mcp` get distributed/launched from?** Same installer as
   `redline` itself (a second executable in the app bundle), or a separate lightweight
   package a Claude Code session installs independently? Affects packaging, not the
   architecture above.
3. **Multi-document sessions.** ~~The local RPC surface is sketched per-document
   (`doc_id`-scoped); redline supports tabbed multi-file sessions. Whether `list_markups`
   without a `doc_id` should mean "the active tab" or "every open tab" needs a decision
   before Wave 1's schema is frozen.~~ **Resolved by Phase 2a** (2026-09-03): neither -
   `doc_id` stays required everywhere it already was, and `list_open_documents`/
   `get_active_document` are the new tools that resolve one. See the Phase 2a
   implementation notes above.
