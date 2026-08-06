# Redline - Handover Notes

## Current Status

**main is still at `f695610a` (PR #63) - PR #67 is OPEN, CI green, NOT yet merged
(orchestrator owns merge/deploy per dispatch scope). Branch
`fix/docops-reseed-and-btx-subtype-guard`, head `39ee537c7b1c44a750e10b0bc2d0b94bfb428927`,
https://forge.mms.name/emittiv/redline/pulls/67. Dispatched to investigate two live
reports: "flatten and optimise don't seem to do anything" and ".btx file import issues."**

**Flatten/optimize root cause (real bug, not cosmetic): MarkupStore (frontend) was never
resynced after flatten_document/optimize_document/redact_document succeeded on the
backend. The PDF was correctly rewritten, but the frontend kept showing the same
markups as live/selectable (visually unchanged), and a LATER save would have
resurrected the flattened annotations by re-writing the stale in-memory list - not
just cosmetically silent, an actual data-integrity bug. Fixed via
`src/lib/docops-handlers.ts::runDocOpAndReseed` wired into all three DocOps handlers
in App.svelte. The "IPC level-arg-fails-to-bind" hypothesis was investigated and
REFUTED (new casing tests in ipc.test.ts prove `optimizeDocument()`'s default level
correctly resolves to 2). Backend now reports what happened instead of `Ok(())`:
`flatten_document` returns the flattened count, `optimize_document` returns an
`OptimizeReport` (bytes_before/bytes_after) - surfaced via a new `docOpsStatus`
banner in App.svelte.**

**.btx fix: `toolchest::btx::import_item` was missing the same `MARKUP_SUBTYPES`
allowlist guard that `document::annots::read_markups` applies before calling
`Markup::from_annotation_dict` - a Raw payload with an unmodeled subtype (Underline,
StrikeOut, Widget, Popup, ...) silently imported as a bogus "Text" tool with no skip,
no error. Fixed by exposing `MARKUP_SUBTYPES`/`subtype()` as `pub(crate)` and applying
the guard in `import_item`.**

**.btx NAMED, NOT FIXED - broader than first scoped: Martin corrected the diagnosis
same-day - "stamps are part of the issue, but several more as well... naming is one,
but also a number of items are incomplete or not the same as the original bb tool."
The confirmed Stamp-appearance-loss gap (import always produces `stamp: None`,
discarding the source graphic - see
`tests::stamp_import_currently_drops_the_appearance_asset_named_gap_not_fixed_this_pass`)
is ONE instance of this wider IMPORT-FIDELITY class, not the whole story. BLOCKED on
real .btx sample files - Martin has been asked to drop them into `bench/corpus/btx/`
(none present in this worktree). Do not start the naming/property-fidelity work
without real samples to diff against - see obs:cxu0x9pn1czhjici5huh for the full
analysis and the precondition. Mission record next_step `ns-btx-import-fidelity`.**

**Tests: cargo test --lib 425/425 (416 baseline + 9 new). cargo clippy --all-targets
0 warnings (incidentally cleared the one pre-existing redundant_closure warning in
commands/docops.rs). npm test 705/705 (694 baseline + 11 new). npm run check 0 errors.
npm run build clean. NOT done: live GUI re-verify in a real `cargo tauri dev` session
(automated tests only this pass).**

**Previous status (wave-3, superseded above but kept for context): main was at
`f695610a` (PR #63, squash-merged 2026-08-05; was `88ba31a` post-wave-2, vector snap
v1 + OCR descope, PR #62). Wave-3 shipped the three fixes from the wave-2b
GUI validation pass (obs:us5j4ne1r5byjzle8u23): undo/redo now has a real keyboard +
toolbar UI surface, ErrorBanner no longer occludes the toolbar-right panel-toggle
buttons, and the tools/gui-harness.mjs license-mock fix (left uncommitted by the
validation session) is now committed. A mid-flight CI failure (Docker build context
didn't include tools/fixtures/) was root-caused and fixed in the same PR before merge
- see Last Session for the full incident. Detail: obs:p2rl32rnjhpq2dygkcz5.**

**Previous status (wave-2, superseded above but kept for context): main was at `8d84754c`
(post-v0.3.6) before wave-2's PR. Wave-2 resolved two
spec-vs-reality gaps flagged by the 2026-08-04 portfolio review: geometry/snap
(BUILT a real v1) and OCR (DESCOPED, formally). See Last Session below for detail and
`.claude/rules/judgment.md` for the carried-forward gotchas both touch. One residual
RUSTSEC finding, NOT fixable from this repo: rkyv 0.7.46 (RUSTSEC-2026-0235) is an
unactivated optional feature of a transitive dep (rust_decimal, pulled in via
tauri-plugin-log -> byte-unit) - confirmed non-exploitable (`cargo tree -i rkyv
--target all` = zero reachable packages), blocked on an upstream rust_decimal release
compatible with rkyv 0.8. Detail: `obs:w3mm0ublu2xu1t8y8tyv`.**

## Last Session

**Date**: 2026-08-06 (dispatched by the orchestrator - investigate "flatten and
optimise don't seem to do anything" + ".btx file import issues")

**Summary**: See Current Status above for the full detail (root causes, fixes, named-
not-fixed gap). PR #67 open, CI green, not yet merged. Two independent, real bugs
fixed (frontend markup-store staleness after docops ops; a `.btx` unsupported-subtype
silent-miscoercion). One hypothesis investigated and refuted (IPC level-arg binding).
One broader gap named and correctly NOT attempted blind (`.btx` import fidelity vs
Bluebeam originals - naming/property drift - blocked on real sample files Martin was
asked to provide in `bench/corpus/btx/`).

### Previous session (2026-08-05, wave-3, dispatched by the orchestrator - ship the fixes from the
GUI validation pass)

**Summary**: All three items from the validation pass (obs:us5j4ne1r5byjzle8u23), one PR.

1. **Undo/redo UI surface** (was fully implemented in `MarkupStore.undo()`/`redo()` with
   zero UI/keyboard surface - Finding 1 of the validation pass). Keyboard: Cmd/Ctrl+Z ->
   undo, Cmd/Ctrl+Shift+Z and Cmd/Ctrl+Y -> redo, wired into `App.svelte`'s existing
   `handleKeydown` (same location as the Cmd/Ctrl+S / Ctrl+Tab bindings). The dispatch-
   and-guard logic is a new pure module, `src/lib/keyboard-shortcuts.ts`
   (`resolveUndoRedoShortcut` + `isEditableTarget`) - extracted rather than inlined
   because App.svelte has no test file of its own (license gate + Tauri IPC make it
   expensive to mount in vitest), so this is the same conflict-avoidance/testability
   pattern as `recent-docs.ts`/`license.ts`/`patchStatus`. The guard matters concretely:
   the Text/Callout inline `<textarea>` (Viewport.svelte) needs the browser's own
   field-level undo, not the app-level one - `isEditableTarget` returns true for
   INPUT/TEXTAREA/contentEditable targets and the resolver returns `null` for those, so
   `handleKeydown` never calls `preventDefault()` there.
   Toolbar surface: new `UndoRedoControls.svelte` (mirrors `ToolChestPanel`'s
   `markupStore` prop pattern - a separate leaf component specifically so it's directly
   testable with @testing-library/svelte, the same reason App.svelte's buttons aren't
   inlined here), mounted in the toolbar-left group right after "Save As…".
   **Real bug found and fixed en route, not cosmetic**: `MarkupStore.canUndo`/`canRedo`
   read `History`'s undo/redo stacks, which are plain (non-`$state`) arrays on a plain
   class (`markup-commands.ts`, not a `.svelte.ts` file, so runes aren't available
   there) - a component binding a button's `disabled` to `store.canUndo` would never
   re-render on push/pop, since Svelte's reactivity never saw a `$state` change. Added
   `historyVersion = $state(0)` to `MarkupStore`, bumped on every history mutation
   (create/update/delete/undo/redo/applyBatch/deleteSelected/seed); `canUndo`/`canRedo`
   read it (`void this.historyVersion`) purely to register the dependency before
   delegating to `history.canUndo`/`canRedo`. Without this the toolbar buttons would
   have shipped with a stuck disabled/enabled state - exactly the class of bug a mock-IPC
   harness pass can't catch (it never round-trips through Svelte's real reactivity graph
   the way a mounted component test does).
2. **ErrorBanner occlusion fix** (Finding 3 of the validation pass). `.error-banner-stack`
   moved from `top: var(--space-3)` (12px - inside the 40px toolbar) to
   `top: calc(var(--toolbar-height) + var(--space-3))` (below it entirely). Visibility
   unchanged, occlusion gone - no interactive control sits under the banner anymore.
   Not screenshot-verified in this session (no live `cargo tauri dev`/harness run
   performed here - see "Not done" below); the fix is geometry-only (fully-below the
   fixed-height toolbar) so a visual regression is unlikely, but flagging per the
   verification-gate standard rather than claiming a screenshot that wasn't taken.
3. **Committed the gui-harness.mjs mock-IPC fix** left uncommitted by the validation
   session (9 missing mock handlers incl. `license_status`, whose absence broke the
   documented harness procedure since the 2026-07-08 S2b gate - PR #49). Own commit,
   dev-tooling only, no app code.
4. **`.btx` fixture** - NOT skipped, was trivial: `tools/fixtures/sample-tool-set.btx`
   is the exact `PLAIN_ITEM` shape from `btx.rs`'s own test suite (a known-good fixture,
   not guessed), plus a leading XML comment for provenance. New Rust test
   (`checked_in_sample_tool_set_fixture_imports_via_import_btx_bytes`, uses
   `include_str!`) ties the checked-in file to the parser so it can't silently drift.
   **Not done** (named, not silently skipped): the fixture is not yet wired into
   `tools/gui-harness.mjs` itself - the harness has no file-dialog mock at all today (it
   only drives zoom/pan/page-nav), so actually exercising the click-through Tool Chest
   import UI is separate, larger scope than "add a fixture". This just removes the
   blocker for whoever does that next.

Tests: `npm test` 694/694 (694 = 673 baseline + 21 new: 15 `keyboard-shortcuts.test.ts`,
6 `UndoRedoControls.test.ts` - 5 direct + reactivity implied by the disabled-state
assertions). `npm run check` 0 errors (19 pre-existing a11y/legacy-syntax warnings,
none in touched files). `cargo test --lib` 416/416 (415 baseline + 1 new fixture test).
`cargo clippy --all-targets` 1 pre-existing unrelated warning (`commands/docops.rs`
redundant closure, present on `main` before this branch). `npm run lint`/`npm run
format` could not run in this session - `eslint`/`prettier` binaries are absent from
`node_modules/.bin` (confirmed pre-existing via `git stash` + rerun, not caused by this
branch) - flagging as an environment gap, not a code issue.

**Owed / not done this session**: live GUI re-verify of both UI fixes in a real
`cargo tauri dev` or harness session (button click -> visible undo, error banner no
longer overlapping the toolbar buttons) - this session's verification was automated
tests only, no screenshots taken. Also still owed from wave-2b: the vector-snap ring
indicator live check and the `.btx` import UI drive-through (see item 4 above).

**CI incident mid-flight (fixed same PR)**: the first push (`83cb51a`) went CI-red -
`test-rust` failed to compile with `couldn't read .../tools/fixtures/sample-tool-set.btx:
No such file or directory` from the `include_str!` in `btx.rs`. Root cause:
`.forgejo/Dockerfile.test-rust`'s build context only `COPY`s `Cargo.toml`/`Cargo.lock`/
`src-tauri`/`crates` into the image - `tools/` was never in it, even though the fixture
was genuinely committed to git (checked `git ls-tree` before assuming otherwise). Fixed
by switching the test to a runtime `std::fs::read` gated on file existence (mirrors
`render::tests::corpus()`'s existing `CARGO_MANIFEST_DIR`-relative pattern one directory
up in this repo) plus a narrow `COPY tools/fixtures tools/fixtures` added to the
Dockerfile so CI actually exercises the fixture instead of permanently skipping it.
Verified both the present-file and skip-gated paths locally before repushing. CI green on
`ba3225a` (run #166), then squash-merged as `f695610a`.

**PR merged**: https://forge.mms.name/emittiv/redline/pulls/63, squash `f695610a4c67d385c3ae60dcb6ee8aa991184496`.

### Previous session (2026-08-05, wave-2b GUI validation pass, dispatched by the
orchestrator - "have you tested the redline markups? And all the other tools?")

**Summary**: Exercised all 22 toolbar tools + supporting UI (properties panel, workflow-
status dropdown, Tool Chest, Takeoff/Quantities panel, Flatten/Optimize/Apply Redactions
buttons, Compare panel) via scripted Playwright driving the real Vite dev app with a mock
Tauri IPC layer - the same technique as `tools/gui-harness.mjs`, extended to drive each
tool individually rather than just the render-loop smoke. 27/29 checks passed with
screenshot + overlay-DOM-delta evidence (40 screenshots captured this session, in the
dispatching session's scratchpad, not committed to the repo).

**Found and fixed (uncommitted - `git diff -- tools/gui-harness.mjs`)**: the repo's own
documented GUI harness had been silently broken since the 2026-07-08 S2b license gate
(PR #49) landed - its mock IPC had no `license_status` handler, so `getLicenseStatus()`
resolved to `null`, `App.svelte` threw on `null.state`, and `.viewport-root` never
mounted. Added `license_status` + 8 other missing mock handlers
(`list_scales`/`get_page_snap_targets`/`list_tool_sets`/`recent_tools`/
`folder_index_status`/`load_recent_docs`/`save_recent_docs`/`check_file_exists`) so the
documented procedure works again. **Recommend committing this as a real fix** - it is
not app code, only the dev-tool mock.

**Two real product defects found** (not harness artifacts):
1. `MarkupStore.undo()`/`redo()` (src/lib/markup-store.svelte.ts:159-161) are fully
   implemented with a working command-history stack (unit-tested) but have **zero UI or
   keyboard surface** anywhere in the frontend - no button, no menu item, no Cmd/Ctrl+Z
   binding in `App.svelte`'s `handleKeydown`. Confirmed via `grep -rn "\.undo(\|\.redo("`
   (zero call sites outside the store + its own test) and empirically (drew 24 markups,
   Ctrl+Z/Ctrl+Shift+Z produced zero overlay change). A user cannot undo a markup today.
2. `ErrorBanner.svelte`'s `.error-banner-stack` (`position: fixed; top/right: var(--space-3);
   z-index: 2000`) sits exactly on top of the toolbar-right panel-toggle buttons (left
   panel / right panel / markups-list / settings - the last four icons in the toolbar
   header). Any uncaught error occludes and blocks clicks on those buttons until every
   stacked error is manually dismissed.

**Not independently verifiable this pass** (named, not silently skipped): real text
selection (mock has no PDFium text layer), the vector-snap v1 ring indicator (a second
targeted mock attempt with a synthetic `SnapTarget` was inconclusive - `.snap-indicator`
never appeared across a coordinate sweep; not concluded as a bug, needs a human GUI pass
- already independently tracked as owed), `.btx` Tool Set import (no fixture file in the
repo), drag-and-drop file open (`getCurrentWebview()` needs the real Tauri webview API).

Full detail + method notes (incl. `SnapTarget`'s actual wire shape `{point:{x,y},kind}`
for anyone else mocking `get_page_snap_targets`): `obs:us5j4ne1r5byjzle8u23`.

Dev server started/stopped cleanly via targeted `lsof -ti tcp:1421 | xargs kill -TERM`
(never `pkill`); no cargo/Rust build was triggered (frontend-only Vite dev session); no
scratch files left in the repo beyond the one harness fix.

### Previous session (2026-08-04, wave-2, dispatched by the orchestrator - "resolve the two
spec-vs-reality gaps found in the portfolio review, with authority to decide on
engineering evidence")

**Summary**: Two independent decisions, both made on evidence rather than punted.

**1. Geometry/snap - BUILT a real v1** (`geometry::extract_page_geometry(page_index)`
was a dead M2-era stub with the wrong signature - it could never work standalone,
since PDFium access must happen on the render thread, per this repo's own
`RenderCmd`/`RenderHandle` architecture). Split the fix the same way the existing
text-selection quad helpers already do: `geometry::build_snap_index()` is pure math
(sub-paths in, `Endpoint`+`Midpoint` `SnapTarget`s out via the existing `rstar` RTree -
no PDFium dependency, unit-tested without a binary); `RenderEngine::page_snap_index()`
(new, `render/mod.rs`) is the PDFium-touching half - walks `page.objects().iter()`,
filters path objects, reads their *transformed* segments (composes Form XObject CTMs
correctly, the exact trap the old stub's doc comment warned about), and caches the
result per (open doc, page) in a new `OpenDoc.snap_cache` field. New
`RenderCmd::PageSnapIndex` + `RenderHandle::page_snap_index()` follow the identical
plumbing pattern as `SearchPage`/`CharIndexAtPoint`. New Tauri command
`get_page_snap_targets` (`commands/geometry.rs`, own file per this repo's
concurrent-branch convention) returns the whole page's targets in one call - the
frontend does nearest-neighbour lookup client-side (`src/lib/snap.ts`,
`findNearestSnap`) so a drag gesture never pays an IPC round-trip per pointer-move.
Wired into `Viewport.svelte`'s `clientToPdf()` - the single choke point every
point-capture flow already calls through (drag-draw start, multi-click vertices,
calibration clicks) - gated by a new `SNAP_ELIGIBLE_TOOLS` set (calibrate +
Line/Arrow/Polyline/Polygon/Cloud + all Measurement* tools; deliberately excludes
select/pan/text-select/rect-drag/text/stamp tools) so every non-eligible tool/gesture
is completely unaffected (`snapTargets` stays empty, `clientToPdf` is a pure
passthrough). Added a screen-space ring indicator (`.snap-indicator`, design-token
styled) so snapping is visible feedback, not a silent cursor jump.

DELIBERATELY NOT built in v1: `Intersection` and `ArcCenter` snap kinds (the enum
variants exist, unpopulated). Documented at length in the `geometry` module doc
comment - `Intersection` needs a spatially-accelerated pairwise check, not a naive
O(n²) pass, because this app's own target use case (very large construction plan
sets, e.g. the §20 "dense A0" corpus) routinely produces thousands of path segments
per page; `ArcCenter` needs Bézier-arc-fitting heuristics PDFium doesn't expose
control points for. Both are real, scoped v2 work, not silently dropped.

Tests: 7 new pure `geometry::tests::build_snap_index_*` cases (open/closed/degenerate
sub-paths, multi-subpath independence, empty/single-point edge cases, a
`nearest_snap` round-trip) - all run in plain `cargo test --lib`, no PDFium needed.
3 new PDFium-gated integration tests in `render::tests` (`page_snap_index_*`) build a
synthetic fixture via a REAL lopdf content stream (`m`/`l`/`h`/`S` operators drawing
an open line + closed triangle, same "build a tiny doc, save, open via RenderEngine"
pattern as the existing double-render-bug test) and prove extraction end-to-end,
caching (`Arc::ptr_eq` on repeat calls), and the unknown-doc-id error path - these
self-skip without `PDFIUM_DYNAMIC_LIB_PATH` (CI), confirmed passing under
`PDFIUM_DYNAMIC_LIB_PATH=... cargo test --release --lib -- --test-threads=1` (the
documented invocation - plain debug-mode `cargo test` with the PDFium env set hits a
PRE-EXISTING (confirmed via `git stash`, not introduced this session) PDFium
thread-safe-binding panic when many `RenderEngine::new()` calls happen in one debug
test binary; `--release` is the only mode this repo's own PDFium tests are meant to
run in together). 11 new `src/lib/snap.test.ts` cases (cache identity/per-page
isolation/in-flight dedup/invalidation, `findNearestSnap` boundary cases). Fixed one
real bug found by the frontend suite: `getPageSnapTargets` originally chained
`.catch()` directly on `invoke()`'s return value, which threw when a test-mocked
`invoke` returned `undefined` synchronously (`Viewport.interaction.test.ts`'s
`afterEach(() => vi.restoreAllMocks())` resets `vi.fn()`-based mocks to no
implementation, unlike `vi.clearAllMocks()`) - wrapped in `Promise.resolve(...)` so a
non-promise return can't crash an uncaught async effect.

Docs corrected: `CLAUDE.md`'s Architecture module list already described geometry's
PURPOSE generically enough to still be true once built - no false claim there to
fix. No architecture-doc correction was needed for this half (the OCR half below did
need one).

**2. OCR - DESCOPED, formally** (was never actually built despite M4/"shipped"
framing implying otherwise). `ocr/mod.rs` was a 9-line stub, `leptess` and the `ocr`
Cargo feature were both commented out since M1, and per `decision:tntyyjau94smf6r6jitq`
(2026-06-25) OCR-via-leptess was DECIDED for M4 but the decision was never executed -
nobody had previously flagged that gap. Verified rather than assumed the "bounded
effort" bias the dispatch suggested: `.forgejo/workflows/ci.yml` (Linux, Forgejo) has
no `tesseract-ocr`/`libtesseract-dev`/`libleptonica-dev` apt step, so `--features ocr`
wouldn't even compile in CI today; `.github/workflows/build-releases.yml`'s
`build-macos` job has no `brew install tesseract` step; `build-windows` has NO
vcpkg/tesseract bootstrap at all (the hard platform for `leptess` - static/dynamic
linking + `VCPKG_ROOT`/`TESSDATA_PREFIX` wiring, unverified in this repo); and
`eng.traineddata` (~12MB tessdata) isn't staged in `tauri.bundle.resources` anywhere.
All three release-pipeline legs need work before OCR could ship, and this dispatch's
hard constraints (no releases/tags/deploys) meant I couldn't verify a working
Windows/macOS release build even if I wrote the Rust code - so BUILDING it now would
have meant shipping an unverified critical platform leg, which the verification-gate
standard doesn't allow. Removed the stub as dead scaffolding rather than leaving it
(`pub mod ocr` + `src/ocr/` deleted from `lib.rs`/the tree) and documented the real
gap in three places so it's discoverable rather than silently re-appearing as a
"shipped" claim: `CLAUDE.md` Tech Stack line, a new "Deferred: OCR" entry under Key
Decisions (the blocker list + what re-enabling needs, in order, cheapest-first), and
the Build Order + "Current phase" lines (which previously implied M4's OCR item had
shipped). Also annotated `docs/bluebeam-alternative-v1-spec.md`'s OCR bullet
(§14/§219) with the same status note, and left an inline comment in `Cargo.toml`
above the still-commented `leptess` line with the identical blocker list, so the
detail lives next to the code as well as in CLAUDE.md.

Verified: `cargo test --lib` 415/415 (18 new: 7 pure `geometry` + 3 PDFium-gated
`render` + 8 existing takeoff/geometry unaffected), `cargo test --release --lib --
--test-threads=1` under `PDFIUM_DYNAMIC_LIB_PATH` also 415/415 (0 failed - confirms
the debug-mode PDFium panic bucket is pre-existing and unrelated), `cargo clippy
--all-targets` 1 pre-existing unrelated warning (`commands/docops.rs` redundant
closure, present on `main` before this branch), `npm run check` 0 errors, `npm test`
673/673 (11 new in `src/lib/snap.test.ts`).

**Not touched**: `gate.rs`/`token.rs`/`store.rs` (license), `docops`, `takeoff` -
scope was geometry + OCR only per dispatch.

### Previous session (2026-08-04, wave-1 PR-C resume, dispatched by the orchestrator)
**Summary**: Resumed a stopped wave-1 dispatch. PR-A/PR-B were already merged (#59,
#60) by a prior agent run. Completed PR-C: found an unpushed local branch
(`feat/toolpalette-measurements-status-crashguard`) already had the four measurement
tools' draw-tool wiring done (model/IPC/render support predated it - only entry points
were missing); added the markup workflow-status dropdown in `PropertiesPanel`
(`patchStatus` existed as a pure function with zero UI wiring until now), a Rust panic
hook (`src-tauri/src/panic_guard.rs`) routing any-thread panics through the `log` crate
into the existing file log, a frontend uncaught-error/unhandledrejection surface
(`src/lib/error-surface.ts` + `src/components/ErrorBanner.svelte`, mounted outside the
license gate in `App.svelte`), and the macOS `REDLINE_LICENSE_API_URL_DEFAULT` build-
workflow fix. Verified: `cargo test --lib` 406/406 (6 new), `npm test` 662/662 (13
new), `npm run check` 0 errors, `cargo clippy` 1 pre-existing unrelated warning. CI
green (Forgejo run/action_run 2998, both `test-rust` + `test-frontend` success). **PR
#61 opened + merged** (squash `8d84754c`):
`https://forge.mms.name/emittiv/redline/pulls/61`. Method note: `cargo fmt -p redline
-- <file>` does NOT restrict to that file - it reformats the whole package; caught via
`git status` before committing, reverted with `git checkout --`. Use `rustfmt <file>`
directly for a single-file format in a workspace member.

### Previous session (2026-07-15, RUSTSEC dependency sweep, dispatched by the orchestrator)
**Summary**: `cargo audit` baseline found 4 vulnerabilities. Fixed
`crossbeam-epoch` 0.9.18 -> 0.9.20 (RUSTSEC-2026-0204, invalid pointer deref in
`fmt::Pointer`) via `cargo update -p crossbeam-epoch` - transitive via rayon,
resolved within the existing `^0.9` range, Cargo.lock only, no code changes.
Attempted `lopdf` 0.36 -> `>=0.42` (RUSTSEC-2026-0187, high 7.5) via
`cargo update -p lopdf --precise 0.42.0` - failed immediately, pinned outside the
`^0.36` range declared in `src-tauri/Cargo.toml` + `crates/pdf-diff/Cargo.toml`.
**Deferred**: lopdf is a direct dep with this repo's own documented 0.36 API
landmines (`.as_float()` vs `.as_f32()`, see Key Gotchas below) - a 6-minor-version
jump needs its own scoped migration PR + test pass, not a forced bump alongside an
unrelated sweep. `quick-xml` 0.39 -> `>=0.41` (RUSTSEC-2026-0195/0194) also
deferred - pinned transitively by the Tauri/wry/tao chain, fixing it means forcing
a Tauri major. `cargo build --workspace` clean, `cargo test --workspace` 400
passed/0 failed/2 ignored. **PR #58 opened**, CI green (Forgejo run #155, commit
`221f7ac8`): `https://forge.mms.name/emittiv/redline/pulls/58`, branch
`fix/dep-security-2026-07`. **Since merged as `ce849c7`** (status corrected in this
reconciliation pass - was still open when this session ended).
**Also found**: an orphaned uncommitted `.claude/HANDOVER.md` edit (documenting the
PR #57 happy-dom/playwright session + PR #54 double-render-bug fix, neither of
which had made it into this file) was sitting on the already-merged
`fix/security-deps-happydom-playwright` branch. Stashed rather than discarded or
committed into the security PR (out of that PR's scope) - `git stash list` on this
repo has it (`"pre-existing HANDOVER edit, unrelated to RUSTSEC dep fix"`). Next
session should pop it and fold its content in, then drop the stash entry.

### Previous session (2026-07-11, PR #53, dispatched by the orchestrator - follow-up to PR #52)
**Summary**: Baked a compile-time default for the S2b license API base URL so a released
Windows build activates the entitlement gate with no user-set `REDLINE_LICENSE_API_URL`
env var. `resolve_base_url` (new, `src-tauri/src/license/client.rs`) checks three tiers:
runtime env var (unchanged, always wins - keeps dev/test override) -> compile-time
`option_env!("REDLINE_LICENSE_API_URL_DEFAULT")` -> `NotConfigured` as before. The
function takes both values as plain arguments rather than reading them internally, so
it's a pure, unit-testable function - avoids racing on the real process env var under
Rust's parallel test runner. Wired `REDLINE_LICENSE_API_URL_DEFAULT:
https://staff.emittiv.studio` into **only the Windows job** of
`.github/workflows/build-releases.yml` (`build-windows` -> "Build Tauri app" env block),
matching the dispatch scope ("Windows machines"); the macOS job (`build-macos`) was NOT
touched, so a macOS release build still needs the runtime env var - flagged as a possible
follow-up if macOS should get the same treatment. Also extracted the base+path join into
`license_url()` and added regression tests for its trailing-slash handling (was already
correct - `trim_end_matches('/')` - no behavior change there, just test coverage that
didn't exist before). Verified the `option_env!` wiring end-to-end with a temporary
scratch test (confirmed `Some(url)` vs `None` with/without the build-time env var set,
then removed it - not part of the diff). Verified: `cargo test --lib` 390 passed/1
pre-existing ignored (9 new), `cargo clippy --all-targets` 0 new warnings (same
pre-existing `redundant_closure` in `commands/docops.rs`). No frontend files touched.
**PR #53 squash-merged** as `db11e2b`: `https://forge.mms.name/emittiv/redline/pulls/53`,
branch `fix/license-url-default` (source head `4b869876ff84bafd690077881223774a4150ebec`).
**Not touched** (per dispatch constraints): `gate.rs`, `token.rs`, `store.rs` - only URL
resolution + workflow env.
**Owed**: now that PR #53 has merged, once the orchestrator clears the Authelia bypass
rule + activation-code creation, cut the `v0.3.2` tag (see Next Steps below - this
supersedes the older `v0.2.0` tag-push item, which predates PR #52/#53 and the current
version).

### Previous session (2026-07-11, PR #52, dispatched by the orchestrator - Martin
reported markups "changing a bit" after saving and reopening a file)

**Date**: 2026-07-11 (PR #52, dispatched by the orchestrator - Martin reported markups
"changing a bit" after saving and reopening a file)
**Summary**: Built a full-type-matrix round-trip fidelity test harness
(`document::annots::tests::fidelity_matrix`, new in `src-tauri/src/document/annots.rs`):
one non-default-valued `Markup` per all 20 `MarkupType` variants, written into a real
in-memory PDF via `write_markups`, reread via `read_markups`, checked field-by-field
(epsilon for expected f32 `/Real` rounding, exact everywhere else), then written a SECOND
time to confirm idempotence. Verified the harness catches real regressions (reverted the
fix, reran, it failed immediately on the Line-truncation bug below). Found and fixed two
real drift bugs:
1. **`Markup::measurement` was hardcoded to `None` on every read** (`from_annotation_dict`)
   - every `MeasurementLength/Perimeter/Area/Volume/Count/Angle/Radius` markup silently
   lost its entire quantity payload (`raw_measure`, `unit`, `computed_quantity`, `depth`,
   `count_value`, `custom_columns`) on save -> reopen. This is the most likely cause of
   Martin's report for anyone using takeoff/measurement tools. Fixed via a new private
   `/RLMeasure` JSON-blob key (mirrors the existing `/RLType` tag pattern).
2. **Polyline geometry on a Line-subtype markup was truncated to its first 2 points on
   write** (`to_annotation_dict` only ever wrote the standard 2-point `/L` key for
   Line/Arrow/MeasurementLength/MeasurementRadius). Any additional vertex was silently
   dropped. Fixed by also writing `/Vertices` with the full point list when there are
   more than 2 points (`geometry_from_dict` already preferred `/Vertices` over `/L` on
   read, so no read-side change was needed). Not reachable via the current UI
   (`MeasurementLength` is always drag-drawn as exactly 2 points today) but a real,
   silent data-loss bug in the general write/read path.
Also persisted the reserved `workflow.assignee`/`workflow.thread` fields (previously
reset to empty on every reopen) via a new `/RLWorkflowExtra` JSON-blob key - same class
of bug, no v1 UI surfaces them yet but they're real fields that shouldn't silently reset.
No visual-only (`/AP`-rendering) drift was found - everything flagged was stored-data
drift in the annotation dictionary. Verified: `cargo test` 381 passed/1 pre-existing
ignored (1 new harness test), `cargo clippy --all-targets` 0 new warnings (1 pre-existing
`redundant_closure` in `commands/docops.rs`, confirmed present on `main` before this
branch). No frontend files touched - `Markup`'s IPC-visible shape is unchanged; this is
entirely a Rust PDF-annotation-dictionary mapping fix.
**PR #52 opened** (not merged - orchestrator owns merge/deploy per dispatch scope):
`https://forge.mms.name/emittiv/redline/pulls/52`, branch
`fix/markup-roundtrip-fidelity`, head `a6f62d457463c1f90ac61b34e5d600579f727ca2`.
**Owed**: live re-verify in the real app once merged - place a MeasurementLength/Area/etc
markup with real quantities, save, close and reopen the file (not just re-focus the
window), confirm the takeoff panel still shows the quantity. The automated harness proves
the PDF bytes round-trip correctly; a human GUI pass on the actual reported symptom is
still the final word.

### Previous session (2026-07-08, PR #50, dispatched by the orchestrator - 4-bug live-use batch)
**Summary**: Root-caused and fixed 3 of 4 reported live-use defects, all traced to one
ordering bug in `apply_page_edit` (`commands/document.rs`): it called
`write_markups(doc, markups)` AFTER `op(doc)`, not before. This defeated
`flatten_document` (baked+removed annotations, then write_markups immediately re-added
fresh live copies - "Flatten does nothing"), `optimize_document` (compressed all streams,
then write_markups added brand-new uncompressed appearance streams on top - "Optimize does
nothing"), and caused the reported "background artifact on move" bug (`store.flush()`
only drains to the in-memory Rust mirror, never writes the file; flatten's `Document::load`
still read the stale on-disk position, baking a permanent ghost at the old spot while
write_markups added a correctly-positioned live annotation). **Fix**: reorder to
write_markups first, then op - also the correct order for the existing page-restructuring
ops (rotate/delete/reorder/insert always needed op to see the CURRENT markup state, not a
stale one). Extracted `apply_edit_and_save` as a pure function (no Tauri `State`/render
engine) for direct file round-trip testing; confirmed each new test fails against the old
ordering and passes against the fix (temporarily reverted, re-verified, restored).
4th defect (highlight "not using text selection") is NOT a regression - `selectText` (I-beam)
tool -> Enter -> `commitTextSelectionHighlight` is fully wired with real PDFium text-range
selection, already tested. It's a discoverability gap: a separate freeform-rectangle
"Highlight" tool sat 5 slots away from "Select Text" in the toolbar, and Acrobat/Bluebeam
users expect "Highlight" itself to snap to text. Fixed via toolbar reorder (Select Text now
immediately after Highlight) + tooltip clarification only - zero behavior change to either
tool. Verified: `cargo test` 380 passed/1 pre-existing ignored (5 new), `cargo clippy
--all-targets` 0 new warnings, `npm run check` 0 errors, `npm test` 634 passed/34 files
(1 new). PR #50 squash-merged as `02a4e5d8decacc003815a9232ccd1616dffb8cd4`, CI green
(run #129).
**Not touched**: release/auto-updater/minisign manifest (out of scope per dispatch).
**Owed**: live re-test of Flatten/Optimize/Redact buttons + move-then-flatten in the real
app - the GUI harness (`tools/gui-harness.mjs`) mocks Tauri IPC with no
flatten/optimize/redact handlers, so it can't exercise this backend PDF-persistence bug;
file round-trip tests are the correct automated verification but a human GUI pass is still
the final word.

### Previous session (2026-07-08, PR #49)
**Summary**: Implemented S2b - redline gates on a valid, device-bound Ed25519 token from
the emittiv-staff license service (S2a). New `src-tauri/src/license/` module: `token.rs`
offline-verifies the compact `<payload>.<signature>` token, mirroring emittiv-staff's
`verifyToken` field-for-field (Ed25519 signs the raw base64url payload segment, not the
re-serialized JSON); `gate.rs` layers device-binding + a 3-day renew-due window on top,
pure/IO-free; `device.rs`/`store.rs` persist the per-install device fingerprint and
last-issued token (atomic temp+rename writes, same pattern as `storage::settings.rs`);
`client.rs`/`service.rs` split the network issue/renew calls from orchestration (mirrors
emittiv-staff's `license-service.ts` DbLike injection) so activate/renew are unit-testable
against a fake `LicenseClient`. Three new Tauri commands (`license_status`,
`activate_license`, `renew_license`). Frontend: `ActivationGate.svelte` blocks the whole
app shell until `license_status` reports valid; `App.svelte`'s onMount side effects
(recent docs, auto-open, drag-drop) now run only after the gate passes. An offboarded
staff record refuses renewal, but the already-issued token keeps gating on its own expiry
- that window is the intended grace period. Verified: `cargo test` 375 passed/1
pre-existing ignored (23 new license tests), `cargo clippy --all-targets` 0 new warnings,
`npm run check` 0 errors, `npm test` 633 passed/34 files (8 new). PR #49 merged as
`de1f8c20040eb06a297e60f5c647b92c8af28e02`, CI green (run #126).
**Deferred**: `REDLINE_LICENSE_API_URL` is unconfigured - emittiv-staff's license service
(S2a) has no deployment yet (no Dockerfile/URL). Live e2e (real activation code -> real
Tauri build) is owed once it deploys.

### Previous session (2026-07-08, PR #48)
**Summary**: Tool Chest v0.3.1 polish - true PNG-backed stamp appearance (real `/AP /N`
Image XObject + SMask, not box+label), dynamic stamp local-timezone dates, a
`StampPromptDialog` for `PromptedText` fields, drag-to-reorder in the Tool Chest panel.
`appearance::build_ap_stream` split into a pure `build_ap_stream` + `finish_ap_stream` so
the `Document`-owning caller resolves auxiliary Image XObjects into real indirect refs.
Verified: `cargo test -p redline` 352 passed/1 ignored, `cargo test -p pdf-diff` 7 passed,
`cargo clippy` 0 new warnings, `npm run check` 0 errors, `npm test` 625 passed (33 files).
Live GUI confirmation (stamp rendering in Acrobat/Bluebeam, prompt-dialog flow, drag feel)
still owed to a human session. Detail: `obs:e1tujicl7p4uck906rxa`.

## Next Steps

**Immediate (2026-08-06, as of 2026-08-06)**:
1. Merge PR #67 once orchestrator schedules it (CI already green).
2. Live re-verify PR #67 in a real `cargo tauri dev` session: Flatten a doc with real
   markups, confirm they disappear from the markup list/become unselectable and the
   new success banner reports a count; Optimize and confirm the before/after
   file-size banner; confirm a later save does NOT resurrect a flattened markup
   (as of 2026-08-06).
3. BLOCKED - `.btx` import fidelity (naming + property drift vs Bluebeam originals,
   Martin-confirmed broader than the stamp gap): wait for real `.btx` samples in
   `bench/corpus/btx/`, then diff `import_btx_bytes()`'s output field-by-field against
   the real Bluebeam Tool Chest for the same tools. Do not guess at Bluebeam's wire
   format without samples (as of 2026-08-06).

**Immediate (wave-2, historical)**: none from wave-2. Live-verify item added: place a measurement or draw
a Line/Arrow/Polyline near an existing vector line in a real (non-scanned) PDF and
confirm the cursor snaps to its endpoint/midpoint with the ring indicator showing -
the automated tests prove the extraction+plumbing is correct, but a human GUI pass on
the actual interaction feel (tolerance, snap "stickiness") is still the final word,
same class of gap as every other GUI-affecting change in this repo's history.
Follow-up worth scoping later (not blocking, not requested this dispatch):
`Intersection`/`ArcCenter` snap kinds (v2, needs a spatially-accelerated pairwise
check + Bézier-arc-fitting respectively - see the `geometry` module doc comment), and
re-enabling OCR (see "Deferred: OCR" under Key Decisions in CLAUDE.md for the
ordered blocker list - Linux CI apt step is the cheapest first step).

**Previously immediate, now historical**: the wave-1 3-PR plan (docs, security deps, UX unlock) is
complete, all merged to main. The one remaining RUSTSEC finding (rkyv, see Current
Status) is not actionable from this repo. New live-verify items from this session:
place a Perimeter/Volume/Angle/Radius measurement, confirm the workflow-status
dropdown in the Properties panel persists status across save/reopen (round-trips via
`/RLWorkflowExtra`, PR #52), and confirm a forced JS error surfaces the crash-guard
banner in a real `cargo tauri dev` session (only unit/component-tested so far, not
GUI-verified).

**Stale below this line** (dated 2026-07-11, predates PRs #54-#61 - v0.3.2 release
sequencing described here is superseded; main is already past v0.3.6 per Current
Status above). Kept for the still-owed live-verification items, which remain valid
regardless of version number.

0. **PR #52 (markup round-trip fidelity fix)** [DONE - merged `de0d9fd`]. Still owed,
   live-verify: place a
   MeasurementLength/Area/Perimeter/Volume/Count/Angle/Radius markup with real
   quantities, save, fully close and reopen the file, confirm the takeoff panel still
   shows the correct quantity (not reset/blank). Also worth a spot-check: draw a Line or
   Arrow, save/reopen, confirm it didn't change shape.
1. **Live-verify PR #50's docops/highlight fixes**: click Flatten, Optimize, and Apply
   Redactions in the real app on a document with markups (including one moved but not
   explicitly saved before flattening) and confirm the visible fix; confirm the toolbar
   now shows Select Text immediately after Highlight.
2. **Live-verify PR #48's Tool Chest polish**: a placed PNG stamp actually renders its
   graphic in Acrobat/Bluebeam (not a box+label), the local-tz date/time on a dynamic
   stamp, the `PromptedText` dialog end-to-end, and drag-reorder feel in the Tool Chest
   panel.
3. **S2b live e2e**: once emittiv-staff's license service is deployed, set
   `REDLINE_LICENSE_API_URL` and run the activation flow (code -> issue -> gate -> renew)
   through a real Tauri build.

Before the first tagged Windows/macOS release:

4. **Orchestrator: generate the redline minisign keypair** (`tauri signer generate`) and
   replace the placeholder `pubkey` in `src-tauri/tauri.conf.json` (currently decodes to an
   "untrusted comment: PLACEHOLDER..." block - clearly non-functional by design).
5. **Orchestrator: create GitHub mirror repo** `newillusions/redline` and add secrets
   `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `GITEA_TOKEN` (same
   names/convention as e-fees).
6. **Push tag `v0.2.0`** (current `Cargo.toml`/`package.json` version) once 4-5 are done,
   to trigger and verify the first release build end-to-end (especially the Windows leg,
   which this session could not run locally).
7. **Open decision, not yet made**: CLAUDE.md says "macOS (universal)" but PR #33 ships
   e-fees-style separate aarch64/x86_64 artifacts, not a combined universal binary - needs
   an explicit call from Martin/orchestrator.
8. **§20 definitive floor-machine run** (16 GB RAM, Windows + macOS) - the formal M1
   Go/No-Go, still owed, blocked on hardware access. Procedure: `bench/RUNBOOK-S20.md`.
9. **G9 human visual check** - sample regenerated 2026-07-10 (dispatched task, read-only:
   `cd src-tauri && cargo test g9_emit_sample -- --ignored --nocapture`, test passed,
   artifact `/tmp/redline-g9-sample.pdf`, 2915 bytes, handed to orchestrator scratchpad for
   Martin). Still owed: actually open it in Acrobat AND Bluebeam and confirm font +
   annotation-group interop - that visual check itself is owner-gated and unchanged by this
   session. Owed since M2.
10. **Project direction beyond polish** (pause / registration fast-follow / next milestone)
    is an owner-gated decision on Martin's business backlog - not yet made, don't infer one.

## Housekeeping flagged for the orchestrator

- `.claude/worktrees/` still has untracked, uncommitted agent-worktree directories (flagged
  2026-07-02, still present 2026-07-08). Not part of this task; flagging again for cleanup
  since it's untracked disk clutter in the repo root.

## Key Context

| Item | Value |
|------|-------|
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| Main branch | `main` @ `8d84754c` (M1-M6 + Phase 1.1 + Windows-dist infra + Tool Chest polish + S2b + docops/highlight bugfix batch + markup round-trip fidelity fix + license URL default (both platforms) + measurement tools + workflow status UI + crash-guard, all merged) |
| KB mission record | `project:q8gm8dv3k7smld12rm25` (stage: stabilizing, health: on_track) |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| Judgment rules | `.claude/rules/judgment.md` (2026-07-02 - incident/decision distillation) |
| PR #48 | `https://forge.mms.name/emittiv/redline/pulls/48` (Tool Chest v0.3.1 polish - merged `7f4a36b`) |
| PR #49 | `https://forge.mms.name/emittiv/redline/pulls/49` (S2b client entitlement - merged `de1f8c2`) |
| PR #50 | `https://forge.mms.name/emittiv/redline/pulls/50` (docops write-markups-ordering + highlight discoverability fix - merged `02a4e5d`) |
| PR #52 | `https://forge.mms.name/emittiv/redline/pulls/52` (markup save/reopen round-trip fidelity fix - MERGED `de0d9fd`) |
| PR #53 | `https://forge.mms.name/emittiv/redline/pulls/53` (license API URL compile-time default for Windows release builds - MERGED `db11e2b`) |
| PR #57 | `https://forge.mms.name/emittiv/redline/pulls/57` (happy-dom + playwright security bump - MERGED `610ba66`) |
| PR #58 | `https://forge.mms.name/emittiv/redline/pulls/58` (crossbeam-epoch RUSTSEC-2026-0204 fix - MERGED `ce849c7`) |
| PR #59 | `https://forge.mms.name/emittiv/redline/pulls/59` (docs/HANDOVER reconciliation through PR#58 - MERGED `941c1e0`) |
| PR #60 | `https://forge.mms.name/emittiv/redline/pulls/60` (lopdf 0.36->0.44 + quick-xml 0.39->0.41, RUSTSEC-2026-0187/0194/0195 - MERGED `ced6a4a`) |
| PR #61 | `https://forge.mms.name/emittiv/redline/pulls/61` (measurement tools + markup status UI + crash-guard + macOS license-URL fix - MERGED `8d84754c`) |
| S2b license contract | `emittiv-staff/src/lib/server/license.ts` (authoritative token shape - do not change without a hub message) |

## Key Gotchas (carry forward)

- **Any docops command that changes the file's annotation state (flatten/optimize/
  redact) MUST reseed `MarkupStore` afterward** (PR #67, 2026-08-06) - `App.svelte`'s
  handlers now go through `src/lib/docops-handlers.ts::runDocOpAndReseed`
  (flush -> op -> `loadMarkups` -> `store.seed`). Do NOT call `flattenDocument`/
  `optimizeDocument`/`redactDocument` directly without a reseed after - the store is
  the frontend's SoT and nothing else re-syncs it; a stale store both looks unchanged
  AND resurrects flattened annotations on the next save.
- **`import_item` (toolchest/btx.rs) must apply the same `MARKUP_SUBTYPES` allowlist
  guard as `document::annots::read_markups`** before calling
  `Markup::from_annotation_dict` (PR #67) - that function's `_ => Text` fallback is
  only safe when the caller pre-filters; `MARKUP_SUBTYPES`/`subtype()` are now
  `pub(crate)` in `document/annots.rs` specifically so `btx.rs` can share them rather
  than re-deriving the list.
- **`.btx` Stamp import currently discards the source appearance** (`stamp: None`
  always, PR #67 named-not-fixed) AND the wider import-fidelity gap Martin flagged
  (naming/property drift vs Bluebeam originals) is BLOCKED on real `.btx` samples in
  `bench/corpus/btx/` - do not attempt either without real samples to diff against,
  see obs:cxu0x9pn1czhjici5huh.
- **`FolderIndex::alive()`** = `Arc::strong_count(&self.inner) > 1` - background watcher thread exits within ~1s of AppState replacing the index
- **Background indexer uses `std::thread::spawn`** (not tokio) - watcher loop is indefinitely blocking, must NOT consume tokio's blocking thread pool
- **Tantivy `Document` trait must be imported** for `to_json()` to be in scope: `use tantivy::{Document, ...};`
- **Svelte store is in-session SoT**; Rust store is a mirror + save buffer. `flush()` throws on undrained mirror queue.
- **lopdf reals: read with `as_float()`, NEVER `as_f32()`** - see `.claude/rules/judgment.md` for the full incident (integer-valued reals silently corrupt on save→reopen)
- **lopdf borrow checker pattern**: immutable read phase (collect owned structs) then mutable write phase - avoids aliasing on `&mut Document`
- **lopdf `Stream::compress()` threshold**: only applies Deflate when `compressed.len() + 19 < original.len()` - streams shorter than ~50 bytes typically don't compress
- **M5 flatten v1 limitation**: only handles indirect /AP /N appearance streams; inline /AP /N streams (rare) are preserved as-is
- **M5 optimize v1 limitation**: no deep image downsampling (spec §8 - deferred to pluggable engine)
- **Recent docs IPC**: lives in `src/lib/recent-docs.ts` (NOT `ipc.ts`) - intentional conflict-avoidance pattern, see judgment.md
- **License IPC**: lives in `src/lib/license.ts` (NOT `ipc.ts`) - same conflict-avoidance pattern
- Overlay `pointer-events` toggles via `isCreateTool()`; Hand tool pans, creation tools capture on SVG overlay
- §5 precision invariant: overlay maps PDF user space → screen every render (never reads raster)
- **`next_version_seq`** in `SidecarMeta` is monotonic - increment BEFORE deriving seq; don't revert to `versions.len()+1` (breaks after prune)
- PDFium 2 GiB limit, global C state, `RenderEngine` drop order - unchanged from M1
- **`appearance::build_ap_stream` is Document-free/pure** - it returns an `ApBuild` (bbox +
  content + resources + any auxiliary `StampImageXObject`s, unresolved). Only
  `annots::write_markups` calls `finish_ap_stream` after resolving those into real indirect
  objects (PDF streams must be indirect - spec 7.3.8). Don't add a Document param back onto
  `build_ap_stream` itself - that's what makes it test-friendly.
- **License public key parsing**: the baked `LICENSE_PUBLIC_KEY_PEM` is Ed25519 SPKI-DER;
  `token::parse_public_key_pem` strips a fixed 12-byte RFC 8410 prefix rather than pulling
  in an ASN.1 crate - do not "simplify" this into a generic X.509 parser, the fixed-prefix
  trick only works because it's specifically an Ed25519 SPKI key.
- **`apply_page_edit`/`apply_edit_and_save`** (`commands/document.rs`): writes markups
  into the loaded doc BEFORE running `op` (flatten/optimize/redact/rotate/delete/reorder/
  insert), never after - see the `apply_edit_and_save` doc comment. `op` always needs the
  CURRENT markup state, and nothing may run after it that could re-add/regenerate what it
  just baked or compressed. This was reversed until PR #50 (2026-07-08) - the bug and its
  full root-cause chain are documented there and in `obs:mwen68znlue4jfrzewxb`.
- Tests: `npm run test` (vitest, mixed node+jsdom). Rust: `cargo test` from `src-tauri/` (not project root)
- **`Markup::measurement` and `workflow.assignee`/`workflow.thread` now round-trip
  through the PDF** via private JSON-blob keys `/RLMeasure` and `/RLWorkflowExtra`
  (`markup/annotation.rs`, PR #52) - do not reintroduce a hardcoded `None`/empty on
  `from_annotation_dict`, that's exactly the bug PR #52 fixed.
- **A "Line"-subtype markup's `/L` key only ever holds 2 points** (PDF spec constraint).
  `to_annotation_dict` now ALSO writes `/Vertices` with the full point list whenever a
  Line/Arrow/MeasurementLength/MeasurementRadius geometry has more than 2 points, so
  redline's own reread (`geometry_from_dict` checks `/Vertices` before `/L`) recovers
  everything losslessly. Don't remove this without re-checking
  `document::annots::tests::fidelity_matrix`.
- **License API base URL resolves in 3 tiers** (`license/client.rs::resolve_base_url`,
  PR #53): runtime `REDLINE_LICENSE_API_URL` env var wins > compile-time
  `REDLINE_LICENSE_API_URL_DEFAULT` (baked via `option_env!`) > `NotConfigured`. Both
  `build-windows` and `build-macos` jobs of `.github/workflows/build-releases.yml` now
  set this (PR #53 shipped Windows only; PR #61, 2026-08-04, closed the macOS gap) -
  a release build on either platform activates the S2b gate with no runtime env var.
- **Rust panic hook + frontend crash-guard** (PR #61, 2026-08-04):
  `src-tauri/src/panic_guard.rs::install_panic_hook()` (called first thing in
  `lib.rs::run()`, before the render thread spawns) routes any-thread panics through
  `log::error!` into the same persistent file log the auto-updater uses -
  `eprintln!`s too, doesn't change unwind/abort behavior. Frontend mirror:
  `src/lib/error-surface.ts` (`window.onerror`/`unhandledrejection` -> formatted
  string) + `src/components/ErrorBanner.svelte` (dismissible banner, logs via
  `@tauri-apps/plugin-log`, mounted in `App.svelte` OUTSIDE the license-gate `{#if}`
  so it's live even during license checks).
- **`Markup.workflow.status` now has a UI surface** (`PropertiesPanel.svelte`
  "Workflow" section, PR #61): dropdown over `patchStatus()`
  (`markup-properties.ts`) - the model function existed since an earlier PR but had
  zero UI wiring until this one. Assignee/thread remain unedited (no UI yet).
- **rkyv 0.7.46 (RUSTSEC-2026-0235) is a known, non-exploitable Cargo.lock entry** -
  do not try to "fix" it with a routine `cargo update`. It's an optional feature of
  `rust_decimal` (transitive via `tauri-plugin-log` -> `byte-unit`), never activated
  anywhere in this repo's own `Cargo.toml` files (`cargo tree -i rkyv --target all` =
  0 reachable packages). `cargo update -p rkyv --precise 0.8.17` fails: `rust_decimal`
  1.42.0 requires `rkyv ^0.7.46` for that optional feature and no compatible release
  exists yet. Blocked on upstream `rust_decimal`, not actionable here.
- **Vector snap (spec §5, v1, wave-2)**: `geometry::build_snap_index` is the pure
  half (subpaths in, `SnapTarget`s out - `Endpoint`/`Midpoint` only, v2 owes
  `Intersection`/`ArcCenter`); `RenderEngine::page_snap_index` is the PDFium half
  and is the ONLY place allowed to touch `page.objects()` for this feature (PDFium
  access must stay on the render thread). Frontend point-capture snapping is wired
  through ONE choke point, `Viewport.svelte`'s `clientToPdf()`, gated by
  `SNAP_ELIGIBLE_TOOLS` - do not add snapping logic at individual `localPdf`/
  `localPdfFromMouse` call sites, it belongs in `clientToPdf` so every tool that
  should NOT snap (select/pan/text-select/stamp/text) stays correctly unaffected
  by construction rather than by an per-call-site exclusion list.
- **PDFium tests in plain `cargo test --lib` (debug, no `--release`) can panic when
  many run together in one binary** - confirmed PRE-EXISTING on `main` via
  `git stash` before wave-2's snap tests existed (11 failures on main alone, debug
  mode). This repo's own documented invocation
  (`REDLINE_BENCH_TESTS=1 cargo test --release -- --test-threads=1`, see CLAUDE.md
  Commands) is not just a speed suggestion - it's required for the PDFium tests to
  pass reliably together. Plain `cargo test --lib` with no `PDFIUM_DYNAMIC_LIB_PATH`
  set (the CI path) is unaffected - every PDFium test self-skips via that env check.

---
*Updated: 2026-08-06 (PR #67: docops markup-store reseed fix + .btx unsupported-subtype guard; broader .btx import-fidelity gap named, blocked on real samples)*
