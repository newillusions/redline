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
