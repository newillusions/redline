# Redline - Handover Notes

## Current Status (2026-08-31, dispatched session - v0.3.15 release + macOS install)

**Owner-authorized ("push the new redline build when it's ready, install it on the Mac").
Shipped v0.3.15 from main@29116fc0 - bundles #85 (crossviewer harness), #86 (search parity),
#87 (multicycle fidelity + CountSet.color fix), #88 (FreeText /C fix). Tag pushed to both
remotes, GitHub Actions build green (run 33401479243, ~15min), Forgejo release v0.3.15
published (4 assets), update.json live on GitHub main. Installed to
`/Applications/Redline.app` on mr-mac-mini (fresh install), version confirmed 0.3.15 via
Info.plist, process launched and stayed running.**

Gotcha for next release: only the git TAG needs pushing to `github` remote, not `main` -
`github/main` only ever receives the bot's `update.json` commits and diverges from
`origin/main` otherwise (attempting `git push github main` gets correctly rejected,
non-fast-forward - don't force it, just skip that step).

Gotcha: `gh auth setup-git` wires a correct per-URL `credential.https://github.com.helper`
into `~/.gitconfig`, but in this environment git did not invoke it when reading from the
file (git 2.50.1/Apple Git-155) - `git credential fill` failed with "could not read
Username... Device not configured" even though `gh auth git-credential get` worked
standalone and the config file was byte-clean. Workaround: pass the same helper via
`-c` on the command line (`git -c "credential.https://github.com.helper=!/opt/homebrew/bin/gh auth git-credential" push github <ref>`)
- this worked immediately. Root cause not isolated.

FreeText /C fix verification: GUI-level exercise of the installed app wasn't feasible
non-interactively (AppleScript/System-Events window targeting hung ~2min, likely an
unanswerable Accessibility permission prompt). Verified instead at the PDF-dictionary level
- 10/10 relevant Rust tests (`freetext_c_is_background_not_glyph_colour_for_callout_too`
and 9 others) pass against cd6ef2d, the exact commit tagged v0.3.15.

**Self-reported incident, contained same turn:** after the AppleScript hang, a fallback
full-screen `screencapture -x` (no window isolation) briefly exposed unrelated private
content (mail inbox subject lines/names, other app windows) alongside the Redline window.
Recognized immediately, deleted the file (confirmed gone), never reused as evidence, no
further capture attempts. Matches the known screen-capture privacy-incident class
(obs:nwf32d193g3h8d52xxy6) - next session doing any screenshot work on an owner-used Mac
should target the specific window via a non-Accessibility-dependent method (e.g. Quartz
window list) rather than falling back to full-screen.

KB: `observation:r261uz46h7uhwlj49soo`. Mission record narrative + `ns-freetext-c-fix` /
`ns-search-parity` / `ns-multicycle-fidelity` next-steps updated to reflect release.

---

## Current Status (2026-08-31, dispatched session - FreeText /C background-colour fix)

**Dispatched by team-lead on a live owner-witnessed defect (mr-desktop Acrobat, verbatim):
"when I selected and moved the comment from redline, the entire comment block filled with
blue and became unreadable." PR #88 open, mergeable, gates green, head
`f87274ce89939cf7c3d1bbea704d8db15ed802f7`, base `main@0c5261ffa1686a9d8c79072b85a25a837003535f`
- https://forge.mms.name/emittiv/redline/pulls/88. NOT merged (dispatch scope).**

Root cause (`markup/annotation.rs` `to_annotation_dict`): `/C` was written as
`appearance.color` (stroke/glyph) unconditionally for every subtype. Per ISO 32000-1
§12.5.6.6, `/C` on a FreeText annotation is the BACKGROUND colour (same role `/IC` plays
for Square/Circle), not stroke. redline's own renderer never showed the bug (reads
`color`/`fill` straight from the model, never from `/C`), but Acrobat regenerates the
appearance from the dictionary on any move/edit and painted the stroke colour as a solid
background.

Fix: for `Text|Callout`, `/C` now carries `appearance.fill` (background) when set, omitted
when unset; glyph colour moved to a new private `/RLTextColor` key. Read side: 3 cases -
post-fix file (`/RLTextColor` present), pre-fix redline file (`/RLType` present, no
`/RLTextColor` - `/C` still read as glyph colour, self-heals next save), foreign file (no
`/RL*` markers - `/C` read as real background, glyph best-effort recovered from a foreign
`/DA`'s `rg` operator via new `color_from_da` helper). 10 new/updated tests; full lib suite
524/524 passing (incl. `fidelity_matrix`/`multicycle_fidelity`, unaffected); fmt+clippy
clean. Only file touched: `src-tauri/src/markup/annotation.rs` (+320/-8 vs origin/main).

`bb_interop_conformance.rs` (real-Bluebeam-corpus harness) NOT run - gated on
`bench/corpus/btx/`, machine-local/gitignored, absent in this environment.

KB: `observation:d487fa2eh4fyu712pd5x` (solution, project-scoped). Mission record's
`ns-freetext-c-fix` next-step marked done.

---

## Current Status (2026-08-31, dispatched session - multi-cycle markup fidelity harness + a real fix)

**Dispatched by team-lead to reproduce the owner's "big issue" verbatim - save/close/reopen,
edit a markup, save/close/reopen again shows inconsistencies - as a real disk-backed harness,
and fix whatever it caught. PR #87 open, mergeable, gates green, head
`d0c566c5e236458b8aad5cd96a319a45e245b853`, base `main@83d36afbc2b727e16e4a70f7afd87ef19bb7555c`
- https://forge.mms.name/emittiv/redline/pulls/87. NOT merged (dispatch scope).**

New test module `document::annots::tests::fidelity_matrix::multicycle_fidelity` (3 tests), all
through real `save_with_markups`/`load_markups_from` disk round trips (not the existing
in-memory single-cycle test): per-type gen1(create)/gen2(edit the reloaded value)/gen3(idempotent
re-save) for all 20 `MarkupType` variants; a combined-document case proving an edit to a SUBSET
of markups leaves the rest byte-stable across generations; and the existing single-cycle
orphaned-popup test extended across two real edit generations. Structural invariants re-checked
every generation: no duplicate managed annots, `/Rect == /AP/BBox`, and (main already has real
`/IRT`+`/RT`/`Group` PDF group linkage, design doc 2026-08-11 - discovered mid-session, see
below) a grouped follower's `/IRT` resolves to a currently-present head.

**Harness went red on first run, as expected - the owner has seen this bug.** Root cause:
`CountSet.color` (`markup/annotation.rs`) was never persisted on its own PDF key - it was
re-derived from `/C` (== `appearance.color`) on every read. Editing a `MeasurementCount`
marker's own stroke colour - an ordinary restyle, not a count-set edit - silently rewrote
`count_set.color` on that marker's next reload, and since `count_set.id` ties multiple markers
together, this could corrupt ANOTHER marker's set-colour without it ever being touched. Fixed:
persist via a dedicated `/RLCountSetColor` key, fall back to `/C` for pre-fix files.

**Method note - worktree/branch mismatch, worth reading before the next dispatch into this
repo:** the shared working directory at dispatch time was checked out on
`fix/bb-interop-datamodel-wave` (1 local unpushed commit ahead of its own origin, itself
diverged from `main`), not `main` as instructed. Built the harness there first; discovered
main already had substantial functionality (the `/IRT`/`/RT` group linkage above) absent from
that branch - the tell that the base was wrong. Recovered before pushing anything: restored the
shared working tree to its exact original state, built a fresh `git worktree add <path> -b
<branch> origin/main`, reapplied the change there (1 trivial EOF-only patch conflict), extended
the harness for main's own group feature, re-ran every gate against the correct base. **Check
which branch a dispatched session actually lands on before writing code** - `git log <cwd>
--oneline -3` vs the instructed base, not just trusting the checkout.

**Still open (owner-gated):** an in-GUI smoke test - apply a markup, save/close/reopen, edit,
save/close/reopen again - to confirm the reported symptom is gone. This fix addresses ONE
verified drift class (Count-marker colour); it is not proven to be the ONLY cause of the
owner's original report.

KB: `observation:9e5kk1bpwe6z5l0o5qbl` (pinned, project-scoped). Mission record's
`ns-multicycle-fidelity` next-step marked done; `current_focus` updated.

---

## Current Status (2026-08-30, dispatched session - cross-viewer harness: both legs proven, first-ever Revu render)

**Dispatched by team-lead to run the cross-viewer capture for real on mr-desktop (owner-idle
window, Martin: "mr-desktop idle"). PR #85 unchanged, head `81f1aaf10f8ee5911fec72718187bdb5781249ac`,
not merged (dispatch scope).**

Acrobat and Bluebeam Revu were already running as MARTIN'S OWN live processes when the run
started (he'd opened them himself to help with the test). Both harness scripts refuse/risk
touching a session they didn't start (Revu hard-refuses; Acrobat's command-line-launch mode
has a latent gap that could photograph whatever tab Martin currently had open - same failure
class as the 2026-08-29 Teams-photograph incident). Reported the blocker instead of pushing
through. Martin's direct reply: **"kill acrobat+revu"** - re-verified each of the 3 named PIDs
by process name immediately before killing, `Stop-Process` by PID only (never by name), machine
confirmed clean before either leg ran.

**Both legs then PASSED, first time in the same session:**
- **Acrobat** (command-line launch, `-LaunchViaCommandLine`): real page render, bright fraction
  0.5005, verified-on-top 5/5, painted in 2s, 274,200 b. COM/IAC still unavailable in this launch
  mode (`0x80080005 CO_E_SERVER_EXEC_FAILURE`) - matches the already-documented limitation, not new.
- **Bluebeam Revu GUI leg** - **first time this leg has ever completed successfully.** Launched,
  placed on DISPLAY1, render settled (6 polls), captured 170,607 b, closed itself gracefully
  (WM_CLOSE). Zero dialogs, zero errors.

**New finding, NOT yet root-caused:** the two renders disagree. Acrobat shows the full markup
set (2 Highlight bars + a lower-left cluster of Rectangle/Arrow/Text/StampDynamic). Revu shows
ONLY the 2 Highlight bars - the lower-left cluster is blank in Revu's main viewport, though
Revu's own page thumbnail hints the content exists. Not flagged as a script error
(`settled=true`, 0 dialogs). Could be a genuine Bluebeam-interop rendering defect (exactly what
G9 exists to catch) or a capture-viewport artifact - flagged for follow-up, not diagnosed.

Artifacts (both renders, both results.json, a comparison-summary.md) staged at
`/Volumes/base/clouds/oc/Personal/Sync/claude/redline-crossviewer-2026-08-30/`. All 7
`redline-crossviewer-*` scheduled tasks confirmed Disabled at end; machine confirmed clean
(0 Acrobat/AcroCEF/RdrCEF/Revu). Task arguments that were temporarily extended for both legs
were restored exactly (`RESTORE_OK: True` both times). KB observation:
`observation:6vlfocvtwpz58dov3fif`.

**Next step for a future session:** investigate the Revu render-completeness gap - is it a real
interop defect (crop + re-run vision-review on both renders would be the cheapest next probe)
or a viewport/zoom artifact of the automated capture.

---

## Current Status (2026-08-29, dispatched session - E2E DirectEval rewrite, eliminates the evaluate_js hang)

**Dispatched by team-lead to continue the E2E harness work below (PR #83), picking up from a
root-cause brief written by a prior session that was itself blocked by a mid-session TCC
filesystem-access revocation before it could implement anything. PR #84 open, mergeable, CI
green (test-rust 33s, test-frontend 26s), head `e8aa4762d5ce78e9efdb67a9a41d1968857411d6`, base
`main@ea26131f` - https://forge.mms.name/emittiv/redline/pulls/84. NOT merged (dispatch scope).**

**The evaluate_js hang (item 3 in the entry below) is FIXED.** Rewrote
`e2e/specs/app-launch.spec.js` to eliminate every WebdriverIO command that routes through
`tauri-plugin-wdio-webdriver`'s `evaluate_js` (`browser.$()`, `.isExisting()`, `.getText()`,
`.getAttribute()`, `.click()`, `.setValue()`, `.action("pointer", ...)`, sync `browser.execute()`)
and replaced them exclusively with `browser.tauri.execute()` (DirectEval, `callAsyncJavaScript`-
backed, with the crate's own 4-attempt reclaim-retry) - every element query/click/value-set now
runs as plain DOM ops inside DirectEval callbacks, and the rectangle-markup drag dispatches a
real `PointerEvent` down/move/up sequence directly on `svg.markup-overlay` from inside one
DirectEval call.

**Verified 4/4 consecutive `npm run e2e` runs against the UNMODIFIED `wdio.conf.js`**: spec 1
passes reliably in ~30s every time, zero hangs (was 100% hang rate at 60-140s on every attempt).
An ad-hoc check with `PDFIUM_DYNAMIC_LIB_PATH` set (not committed) proved the full interaction
rewrite works end-to-end - one clean run, all 3 specs green in 261ms including the real pointer
drag producing a persisted Rectangle markup via `list_markups` over real IPC - but 3 further such
runs hit a SEPARATE, genuinely intermittent PDFium-specific regression (`before()` never reaching
any known UI state). This DEFINITIVELY ANSWERS the open question in docs/TESTING.md's Round 3:
it is NOT the same evaluate_js bug (if it were, the DirectEval fix would have closed it the same
way it closed spec 1). Left unresolved, documented, out of this PR's scope.

**Specs 2/3 still fail on this Mac's default `npm run e2e`** (no PDFium bundled - the
pre-existing, separately-scoped `tauri build --no-bundle` resource gap named in the entry below)
- but now fail FAST and DETERMINISTICALLY on a real named error (`.error-banner` = "PDFium
dynamic library not found"), never on a hang. That is the actual deliverable here: the harness
tells the truth about what's broken instead of timing out uninformatively.

Full detail + evidence trail: `docs/TESTING.md` "Round 4". KB observation:
`observation:438zgj9xgz6d0rs3i9wl`. Mission record `next_step_elem ns-ui-test-harness` updated
to `done`.

Gates run: `cargo test --workspace` 545 passed, 0 failed. `cargo clippy --workspace --all-targets
-D warnings` clean. `npm test` 749 passed/46 files. `npm run check` 0 errors (23 pre-existing
warnings untouched).

---

## Current Status (2026-08-29, dispatched session - E2E crash fix + harness bridge, resumed after usage-limit kill)

**Dispatched by team-lead to resume the 2026-08-28 S2b activation E2E work. PR #83 open,
mergeable, head `fe7bf44`, base `main@0995984` - https://forge.mms.name/emittiv/redline/pulls/83.
NOT merged (dispatch scope).**

This dev Mac IS now activated (a real, owner-issued activation code was entered through the
real ActivationGate UI in the prior session; the token persists under
`~/Library/Application Support/com.emittiv.redline/license/activation.json`). Two real,
independent bugs were found and fixed on top of that; a third, deeper one was found and
documented but NOT fixed - `npm run e2e` still does not produce a real pass.

1. **Fixed a real SIGABRT crash.** Root-caused via `fern` 0.7.1's own source: its
   `backup_logging` panics if BOTH a log write AND its stderr fallback fail - which happened
   under `@wdio/tauri-service`'s piped-stdio spawn (stdout/stderr both EPIPE), and our own
   `panic_guard.rs` hook then re-triggered the SAME failure by calling `log::error!` again
   while already unwinding the first panic - a panic-while-panicking, unconditionally fatal in
   Rust. Fixed: wrapped the hook's reporting calls in `catch_unwind`
   (`run_without_escalating`). 5 new unit tests reproduce the double-panic shape directly.

2. **Fixed "Tauri core.invoke not available after 5s timeout"** blocking 100% of wdio
   commands. `@wdio/tauri-plugin` (the frontend package `@wdio/tauri-service`'s own docs
   require) was never installed or imported at all. Added as a devDependency, imported in
   `main.ts` gated on `import.meta.env.MODE === "e2e"` (verified dead-code-eliminated from the
   production bundle by content-hash). Added `wdio:default`/`core:window:default` capability
   permissions. Bumped Rust `tauri-plugin-log` 2.0.0-rc→2.9.0 to match a JS version the new
   package pulled in (real version-skew guard failure, not optional). Verified: zero
   "Failed to get window states" warnings post-fix (was 100%), real `get_window_states`
   responses in `~/Library/Logs/com.emittiv.redline/Redline.log`.

3. **NOT FIXED - real blocker remains, root-caused, documented in `docs/TESTING.md`.**
   `npm run e2e` still hangs identically (reproduced 4/4 consecutive runs - deterministic on
   this Mac). `browser.$()`/`isExisting()` routes through the third-party
   `tauri-plugin-wdio-webdriver` crate's `evaluate_js`, which uses plain
   `WKWebView.evaluateJavaScript(_:completionHandler:)` - its completion never fires under a
   background-spawned window (zero log activity for the full 60s wait, matching 2×30s script
   timeouts exactly). `browser.tauri.execute()` (DirectEval, `callAsyncJavaScript` +
   message-handler) DOES complete - proven by fix #2 above working. **Concrete next step**
   (not attempted this session): rewrite `e2e/specs/app-launch.spec.js`'s element queries to
   use DirectEval instead of `browser.$()`. Spec 3's real pointer-drag
   (`browser.action("pointer", ...)`) is a separate Actions-API endpoint, not audited.

Gates run: `cargo test -p redline` 515 passed (9 in `panic_guard::tests`), `cargo test
--workspace` all green, `cargo clippy --workspace --all-targets -D warnings` clean, `npm test`
749 passed/46 files, `npm run check` 0 errors (added `src/vite-env.d.ts`, was missing
entirely), `npm run e2e:build` compiles clean. KB observation: `observation:bfqepdfj9qulk023rv6y`.

---

## Current Status (2026-08-28, dispatched session - WDIO harness + grouped-markups recovery)

**Dispatched by team-lead, two deliverables, no merges (per dispatch scope).**

1. **Grouped/layered markups was NOT lost - it was already merged.** The 2026-08-21 recon
   note below ("stalled 10 days, environment access block") was stale. Verified: branch
   `fix/grouped-layered-markups` (commit `20d308f`) was merged to `main` via PR #79 (merge
   commit `5421489961d0902e015a24cff28fe8ffce1ff377b`) on **2026-08-11 itself** - the same
   day it was built. A duplicate PR #80 opened later found and self-closed on the same
   fact. The "not pushed" state was just a stale LOCAL clone (`git fetch` + fast-forward
   fixed it in seconds). Mission record `next_step_elem ns-grouped-layered-markups-2026-08-11`
   marked `done`.

2. **Tier-2 real-app WDIO E2E harness adopted** from `satchel-gui`'s PR #25 pattern
   (owner-authorised 2026-08-22). PR **#82 open**, mergeable, head `57a5d7f`, base
   `main@3b486d4` - https://forge.mms.name/emittiv/redline/pulls/82. NOT merged (orchestrator
   owns merge/deploy). Full battery green: `cargo test --workspace` 534/0/18-ignored,
   clippy 0, `npm test` 749/749, `npm run check` 0 errors. `npm run e2e` produced a REAL
   WebKit/macOS WebDriver session against the compiled binary (Session ID
   `dc138915-d84f-420c-8e60-830024c8bf40`).
   - **Important limitation, documented not hidden** (`docs/TESTING.md`): this dev Mac has
     never been activated against the real S2b production license service (no token under
     `~/Library/Application Support/com.emittiv.redline/`). Spec 1 (reaches a real terminal
     state) passes for real. Specs 2-3 (open fixture + render; place a Rectangle markup and
     verify it persists) correctly report `pending` via `this.skip()` - producing an
     activation code is a production action, not attempted. **Next session with a
     licensed/grace device: re-run `npm run e2e` to get a true pass on specs 2-3.**
   - New files: `wdio.conf.js`, `src-tauri/tauri.e2e.conf.json`, `e2e/specs/app-launch.spec.js`,
     `e2e/fixtures/e2e-sample.pdf`, `docs/TESTING.md`. One additive test hook:
     `data-doc-id` on `Viewport.svelte`'s `.viewport-root`.

---

## Current Status (2026-08-21, recon run - read-only, superseded above for the grouped-markups item)

**First `/recon` run for redline, dispatched by the orchestrator.** No code changes - this
was a read-only competitive/currency study. Full report:
`/Users/martin/dev-reports/2026-08-21-recon-redline.md`; KB observation
`observation:8jugmmbiqp4rkp61z3ey`.

**Verdict: fundamentals solid.** M1-M6 shipped, 0 open PRs (confirmed via `list_pull_requests`),
main at `26807f4` - PR #78 already closed the BB-interop harness's first-run findings (`/F`,
`/DA`, `/RC`, `/BE`, `/OC` all now preserved on round-trip), so the mission record's
`current_focus` (still describing those as open) was stale and has been refreshed.

**Two proposals added to the mission record's `next_steps` (both `dispatch_state: proposed`,
NOT yet authorized/dispatched):**
1. `recon-2026-08-pdfium-upgrade` - move off `pdfium-render` 0.8.28 (currently 2 minors behind;
   0.9.x fixes the lifetime/threading issues behind this project's own documented SIGSEGV and
   debug-mode-panic traps, and 0.8.32+ added native Form XObject support redline hand-rolled a
   workaround for). Medium effort.
2. `recon-2026-08-ai-diff-summary` - scope an AI-assisted summary on top of the existing M6
   Compare panel (Bluebeam's new 2026 Max tier sells exactly this as its headline feature).
   Effort unknown - needs scoping before any build.

**Explicitly considered and dropped** (see report for full reasoning): real-time
collaboration/cloud sync (already a deliberate v1 non-goal, spec §2) and AI-driven takeoff
automation (wrong job - redline's spec scopes takeoff as supporting, not primary; that's a GC
estimating problem, not this project's).

**Process finding, not a recon proposal (already in-flight, just stalled):**
`fix/grouped-layered-markups` (the 33/77-corpus grouped-annotation gap) was designed and built
2026-08-11 with a full green test battery, but was never pushed to an open PR - blocked
mid-session on environment access, and has sat unshipped for 10 days. Mission record still
shows `ns-grouped-layered-markups-2026-08-11` as `dispatched`. Worth the orchestrator checking
whether the environment block has cleared.

---

## Current Status (previous, 2026-08-11 - PR #77/#78 wave, superseded above where noted)

**PR #77 (self-service Bluebeam-interop validation harness, new dispatch 2026-08-11)
open, mergeable, head `df4c727e83f6637e2f39fbe95dcbe8f381c71396`, based on
main@23469e9 (post-v0.3.14), https://forge.mms.name/emittiv/redline/pulls/77. CI green
(run #212, success). NOT yet merged (orchestrator owns merge - no deploy needed, this
is dev/test tooling only, zero app code touched). Owner directive verbatim (G9
reopened): "there are still significant issues with how our markups read in
bluebeam... you need to find a way of testing and validating that yourself... in
browsers and stirling pdf maybe."**

**Structural conformance harness (primary check),
`src-tauri/tests/bb_interop_conformance.rs`: diffs redline's write-side
annotation-dictionary output against GENUINE Bluebeam-authored PDF annotation
dictionaries, extracted independently (own hex+zlib decoder, not btx.rs's pub(crate)
one) from the real `.btx` corpus (4 files/77 items). Round-trips each golden dict
through the real production path (`Markup::from_annotation_dict` ->
`to_annotation_dict`) and diffs key sets per subtype. Deliberately the primary check,
not a render comparison - Bluebeam regenerates annotation appearances from the
dictionary on edit, so a generic viewer blitting the stored `/AP` can look fine while
the data model is wrong (the exact shape of every real G9 fix to date).**

**Calibrated: correctly detects the known grouped-`<Child>` class (33/77 items,
architecturally unsupported) and the known 4-stamp stock-artwork gap
(`objects.get(&root_id)` unresolvable, the "MR Init"/"MR Sig" class from PR #76);
confirms the 2026-08-08 UID-naming fix still holds (0/11 raw UID names).**

**First-run findings, not previously enumerated - candidates for the owner's
"significant issues" complaint, not yet fixed: every subtype's round-trip drops `/F`
(annotation flags - Print/NoZoom/Locked); FreeText drops `/DA` (default appearance
string - font/size/color) on 22/22 golden items - redline uses custom
RLFontFamily/RLFontSize instead of standard `/DA`; FreeText/Circle drop `/RC` (rich
text) on a majority of items that carry it; 4/7 golden Polygon items carry `/BE`
(cloud border effect) that never survives the round-trip, suggesting some
BB-authored cloud-style Polygons are misclassified as plain Polygon on read; `/OC`
(optional content/layers) dropped everywhere. Full per-subtype breakdown via
`cargo test --test bb_interop_conformance -- --ignored --nocapture`.**

**Render matrix (secondary check), `tools/render-matrix.mjs` (`npm run
render:matrix`): poppler + macOS Quartz + real Chrome (Playwright's bundled headless
Chromium has NO PDF viewer at all under automation - confirmed empirically, "Download
is starting" regardless of CDP settings; falls back to `channel: "chrome"`, which
works). mutool/ImageMagick not installed on this machine, named as a gap not silently
skipped. Verified end-to-end against `bench/corpus/bb-ref/markup-test-original.pdf`.**

**Tool evaluation (deliverable 3, posted as a PR #77 comment): researched Stirling
PDF, qpdf, Ghostscript, Okular, LibreOffice, PDFtk for appearance-REGENERATING
Bluebeam-proxy behavior on MARKUP annotations (not AcroForm fields, which is what
qpdf `--generate-appearances`/Ghostscript `/NeedAppearances` actually cover - a
different, easily-confused mechanism). Conclusion: no verified open-source
substitute exists. Recommendation: deploy nothing new - the structural harness above
already sidesteps the problem by comparing against Bluebeam's own real output
instead of needing a proxy tool.**

**G9 stays OPEN pending the owner's capture pass (author a representative markup set,
open in Bluebeam, note what reads wrong) plus triage of this harness's first-run
findings above.**

Verified: `cargo test --all-targets` 492 lib passed (486 baseline unaffected) + new
harness's 2 tests pass under `--ignored --nocapture`; `cargo clippy --all-targets` 0
warnings; `npm test` 742/742; `npm run check` 0 errors (23 pre-existing warnings, none
in touched files).

### Previous session (2026-08-08, PR #76, now merged as part of v0.3.14, kept for context)

**PR #76 (Form XObject stamp rendering - final piece of the stamp saga, new dispatch
2026-08-08) open, mergeable, head `9b82be4485996ce7f6e18031ff962399ca506dae`, based on
main@710d45b3 (post-PR #75), https://forge.mms.name/emittiv/redline/pulls/76. NOT yet
merged (orchestrator owns merge/deploy). Closes the owner's "still no joy with the
stamps" report for the stamps he actually uses: all 11 real production stamps are
`StampAsset::BluebeamFormXObject` and rendered as empty boxes everywhere before this.**

**Design choice (justified in the PR body, per dispatch steer to weigh a raster
alternative against a full interpreter): rasterize the Form XObject via PDFium rather
than write a custom content-stream-to-SVG interpreter.** `build_isolated_form_xobject_pdf`
(new, `document/annots.rs`) wraps the Form XObject in a minimal single-page PDF
(MediaBox = the XObject's own `/BBox`, content stream `q 1 0 0 1 0 0 cm /Fx0 Do Q`),
reusing `splice_bb_form_xobject` (refactored out of `write_markups`'s existing inline
logic for reuse). New `RenderEngine::rasterize_pdf_bytes` (`render/mod.rs`) renders it
through the app's existing single-instance PDFium render thread to a transparent PNG
(`RenderCmd::RasterizePdfBytes`, follows the `PageSnapIndex` plumbing pattern exactly).
Chosen over a custom interpreter because it gets nested XObjects, transforms, and text/
font handling for free from PDFium's own engine rather than reimplementing all of it.

**Zero frontend changes needed.** `Viewport.svelte`'s `stampAssetOf`/`createPlacedMarkup`
already copy `tool.stamp.asset` verbatim onto placed markups, and PR #73's existing
`"stamp-image"` render case already handles `PngBase64` - so wiring rasterization in at
`.btx` import time (new `rasterize_form_xobject_stamps_in_place`, called from the
`import_btx` command before tools are stored) was sufficient. Trade-off named in the PR
body: this is eager/at-import-time, not a lazy frontend cache, so re-export writes the
rasterized PNG rather than the original vector Form XObject - simpler, at the cost of
losing that save-fidelity.

**Two real, pre-existing bugs found and fixed en route (neither introduced by this PR):**
1. **`/Length` mismatch in real Bluebeam `.btx` data.** `parse_pdf_object_bytes` (`btx.rs`)
   produced an `Object::Dictionary` instead of `Object::Stream` whenever a real corpus
   item's declared `/Length` didn't match the actual stream body length - a LATENT bug
   that also silently affected the already-shipped `write_markups` save-time splicing,
   not just this new rasterization path. Root-caused via a debug dump of the raw bytes
   for a real failing corpus item ("emittiv stamp crop"). Fixed with new
   `fix_stream_length()`: scans for the true `stream`/`endstream` byte boundary and
   rewrites the `/Length` token to match before parsing.
2. **Stock/library stamps have no embeddable artwork at all** (e.g. "MR Init", "MR Sig").
   Their `/AP`'s `root_id` is absent from the item's own `<Resources>` block because
   `/TmpBRXFile` points at Bluebeam's *bundled* stamps library, not a user file -
   Bluebeam doesn't embed built-in library-stamp artwork in `.btx` exports. This is a
   genuine data-availability gap, not a bug. Handled by returning `None`/a named,
   logged skip rather than a silent blank box.

**Result: 7/11 real corpus stamps now rasterize with actual non-blank artwork**
(verified against real `bench/corpus/btx/*.btx` files through real PDFium, not
mocked); the remaining 4 are the stock-stamp gap above, named not silently degraded.

**Not done, named in the PR body:** read-side Form XObject recovery from an already-
*opened* PDF (extending PR #73's `recover_stamp_asset` beyond the `.btx`-import path) -
lower priority since all 11 real corpus stamps are sourced via `.btx` import, not
pre-existing PDF annotations.

Tests: `cargo test --all-targets` 486/486 lib (478 baseline + 8 new) + 21 pdf-diff, 0
failed. `cargo clippy --all-targets` 0 warnings. Both new PDFium-gated tests
(`rasterize_pdf_bytes_renders_real_content_not_a_blank_transparent_image`,
`rasterize_pdf_bytes_a_real_corpus_stamp_renders_non_blank_artwork`) confirmed passing
for real (not just skipped) via `PDFIUM_DYNAMIC_LIB_PATH=.../libpdfium.dylib cargo test
--lib -- --test-threads=1`. `npm test` 733/733 (0 new - frontend untouched, gates run
for regression confirmation only), `npm run check` 0 errors, `npm run build` clean.

**Mission-record hygiene note (flagged for the orchestrator, not fixed here - out of
this dispatch's scope):** the redline `next_steps` list has grown to 62 entries with
`kb_status_update` returning lint warnings on several ("reads like code-level churn");
worth a dedicated pruning pass per the live-project-ledger altitude rule.

### Previous session (2026-08-08, PR #75, now the base for PR #76, kept for context)

**PR #75 (About page + updater UX, new dispatch 2026-08-08) open, mergeable, head
`d250ce320352f9a54481a2e0fbea4a8b810a0358`, based on main@f021cc7 (post-PR #74),
https://forge.mms.name/emittiv/redline/pulls/75. NOT yet merged (orchestrator owns
merge/deploy). Owner reported three UX gaps: no About page, update dialog showing the
new version number twice, wanted a rollback-to-previous-release option "at least during
dev stages".**

1. **About page - built.** New `AboutDialog.svelte`, opened via a new toolbar icon button
   (ⓘ) next to Settings. Shows the live app version (`@tauri-apps/api/app`'s
   `getVersion()`, not hardcoded), a static update-channel line, a manual "Check for
   Updates" action (reuses `@tauri-apps/plugin-updater`), and the release-history/
   rollback list below.
2. **Double-version bug - root-caused and fixed.**
   `UpdateNotification.svelte:118-121` rendered `Version {update.version}` as its own
   label, then the release-notes box (defaulting to `"Release v$VERSION"` per
   `build-releases.yml`'s `update-manifest` job - every real release's manifest carries
   exactly this text) repeated the SAME version a second time. `update.currentVersion`
   was already on the `Update` object the whole time, only ever used in a log line, never
   rendered. Fixed: dialog now shows `v{currentVersion} -> v{version}` as one distinct
   line. Regression test (`UpdateNotification.test.ts`) confirmed red against the
   pre-fix component via `git stash`, then green.
3. **Real rollback mechanism - built, not faked.** `update.json` is overwritten to
   advertise only the latest release on every deploy, but the SAME CI job
   (`update-manifest` in `build-releases.yml`) also commits it to GitHub `main` on every
   release as a plain commit (never force-pushed) - so each past release's exact,
   CI-signed manifest stays reachable forever at a commit-pinned
   `https://raw.githubusercontent.com/newillusions/redline/<sha>/update.json`. New
   `src-tauri/src/updater_rollback.rs` lists past releases from that commit history (two
   GitHub REST calls per release: commits API then contents API; `ReleaseSource` trait
   for test injection, mirrors `license::service::LicenseClient`; dedup by version,
   tolerant of individual bad/missing entries). New `rollback_to_version` Tauri command
   points a custom `updater_builder().endpoints([...])` at the pinned manifest with a
   permissive `version_comparator` (exact-match the requested version) and calls the
   REAL `check()`/`download_and_install()` - minisign verification is exactly as real as
   a normal update, since it's driven by CI's actual signing key output for that release.
   Confirmed via context7 (Tauri v2.8.0+ docs, resolved crate version 2.10.1) that the
   Rust builder's `endpoints()`/`version_comparator()` is what makes an arbitrary
   historical rollback possible - the JS-level `check()` only has `allowDowngrades`,
   which alone can't pick a SPECIFIC old version since the default manifest only ever
   holds latest. **Shipped ungated** (visible in every build, not restricted to
   `cargo tauri dev`) - judgment call: "dev stages" read as the product's current
   internal/pre-1.0 lifecycle (no external users yet), not a compile-time debug flag.
   Flagged in the PR body as worth revisiting once redline ships externally.

Tests: `cargo test --all-targets` 484/484 (478 baseline + 6 new `updater_rollback` cases,
all unit-tested via a fake `ReleaseSource`, no live network calls in the suite).
`cargo clippy --all-targets` 0 warnings. `npm test` 742/742 (733 baseline + 9 new: 1
UpdateNotification regression, 6 AboutDialog, 2 updater-rollback IPC casing). `npm run
check` 0 errors (23 warnings, same pre-existing a11y-pattern class as
SettingsDialog/UpdateNotification's own dialogs - one new reactivity warning was caught
and fixed during development: `foundUpdate` needed `$state()`, not a plain `let`, since
it's read directly in the template). `npm run build` clean.

**PR #73 MERGED** (`b05bc73fcd3e1fc13472de7b3ea491b2b6a75028`) - see "Previous session
(PR #73)" below for the full 4-class writeup (nudge-Callout anchor collapse, stamp
artwork read recovery, text-rotation investigated-and-confirmed-already-fixed).

**PR #74 (btx toolset import fidelity, same dispatch, owner scope addition mid-session)
open, mergeable, head `3b14de6b291e4eaa30e72d221a24d4af551629bf`, based on main@b05bc73
(post-PR #73), https://forge.mms.name/emittiv/redline/pulls/74. NOT yet merged
(orchestrator owns merge/deploy). Owner: "btx toolset import is still not fully fixed
despite v0.3.13's fidelity work" - (a) some tools still show UID names, (b) many tools
don't match Bluebeam's format.**

1. **UID names - FIXED.** Characterized against the real 77-item/4-file production
   corpus (`bench/corpus/btx/`, gitignored): `/Subj` is absent on EXACTLY the Stamp-type
   items (7/77, 100% of the naming-fallback population) - the existing fallback
   (`/Subj` -> opaque `<Name>` UID) had nothing better than the raw UID e.g.
   `"XXEOVOCUQESTKIRL"`. Found via a diagnostic dump: every one of those 7 items
   carries a private `/TmpBRXFile` literal-string path (the source file Bluebeam built
   the custom stamp from, e.g. `"D:\...\Stamps\MR Init.pdf"`) - its basename minus
   extension (`"MR Init"`) is a real name. New `stamp_source_file_basename` second-tier
   fallback: `/Subj` -> `/TmpBRXFile` basename -> opaque UID floor. Measured: closes
   7/7 real UID-name cases (now `"emittiv stamp crop"`, `"MR Init"`, `"MR Sig"`), zero
   regression to the 70 items that already had `/Subj`.
2. **Format mismatches - dominant cause found, NAMED NOT FIXED (too large for this
   dispatch).** 100% of real Stamp items (11/11) resolve to
   `StampAsset::BluebeamFormXObject`. Neither the live canvas placement preview
   (`markupToSvg`) nor PR #73's read-side recovery (`recover_stamp_asset`) handles that
   asset kind - both only handle `PngBase64`. Every placed Bluebeam-native stamp
   therefore renders as an empty box, BOTH in the live editing canvas right after
   import/placement AND after save/reopen. This is a strong, evidenced candidate for
   the MAJORITY of "format mismatch" reports - every real Stamp tool in the owner's own
   production toolsets currently shows as a plain outline box, not its actual artwork,
   anywhere in the live app. PROPER FIX needs interpreting a Form XObject's own content
   stream (nested Image/Form XObjects, transform matrices) into renderable content -
   effectively a small PDF-content-stream-to-SVG/raster renderer. Deliberately NOT
   attempted here - follow-up PR territory.
3. **Ruled out**: a geometry/appearance sanity sweep across the other 66 non-stamp real
   corpus tools found no anomalies worth chasing beyond 8 Text/FreeText items with
   `line_weight=0`, which reflects genuinely-authored borderless text tools in the
   source data, not an import defect.

Tests: `cargo test --all-targets` 478 lib passed (471 baseline + 7 new). `cargo clippy
--all-targets` 0 warnings. (npm gates unaffected, no frontend files touched.)

**Method note - PR merge race**: PR #73 was merged by the orchestrator WHILE this PR
#74 commit was still being pushed to the same branch. The second push landed on an
already-merged/closed PR, and a naive new-PR-from-the-same-branch showed 7 files
changed (everything from the already-merged PR) instead of just the 1 new file. Fixed
via `git fetch && git rebase origin/main` (cleanly skipped the already-applied commit)
+ `git push --force-with-lease`, re-verified via `list_pr_files` showing exactly 1
file. Lesson: when resuming work on a branch across dispatch turns, always fetch and
check whether the branch's earlier PR merged before opening/reusing a PR on it.

### Previous session (2026-08-08, PR #73, now merged, kept for context)

**PR #73 (Bluebeam-interop follow-up fixes, dispatched by the orchestrator - owner
documented 4 fresh-corpus failure classes: original-render diffs, additional nudge-file
failures, stamps rendering as empty boxes, text rotated 90 degrees), squash-merged as
`b05bc73fcd3e1fc13472de7b3ea491b2b6a75028`,
https://forge.mms.name/emittiv/redline/pulls/73. Two real bugs found and fixed, one
investigated-and-confirmed-already-
fixed, one subsumed:
1. **Nudge-Callout anchor collapse (real bug, fixed)**: Bluebeam Revu's move/nudge
   operation strips `/CL` entirely from Callout FreeText annotations (spec-legal -
   `/CL` is optional per ISO 32000-1 12.5.6.6) while leaving redline's own
   `/RLGeom = "poly"` tag and a still-valid `/Rect`/`/RLRect` in place.
   `geometry_from_dict`'s `"poly"` branch (`markup/annotation.rs`) fell through to an
   EMPTY `Polyline` when Vertices/CL/L were all absent, and `markupToSvg`'s Callout
   branch (`markup-render.ts`) reads that empty array's "last point" via
   `?? {x:0,y:0}` - anchoring the whole markup at the PDF page's own origin corner.
   Every affected Callout in the real corpus (`Comment`/`e-callout`/`Cloud+`) stacked
   on top of each other there. Fixed: fall back to a degenerate 2-point line anchored
   at the annotation's own `/RLRect`/`/Rect` min corner when the leader-line keys are
   all absent.
2. **Stamps render as empty boxes (two real bugs, fixed, NEITHER Bluebeam-interop-
   specific)**: `Markup::from_annotation_dict` (`annotation.rs:787`, pre-existing)
   unconditionally set `stamp_asset: None` on EVERY read - redline's own placed stamps
   lose their artwork the moment a file is saved and reopened, not just foreign
   Bluebeam ones. `markupToSvg` also had ZERO render case for Stamp/StampDynamic
   artwork at all. Fixed both: new `document::annots::recover_stamp_asset` reads the
   AP's own Image XObject (+ optional `/SMask` alpha) back into a
   `StampAsset::PngBase64`; new `"stamp-image"` `SvgShape` + `<image>` element in
   `Viewport.svelte` renders it. Scoped to the Image-XObject case only - a
   Bluebeam-native Form XObject stamp recovered from an OPENED PDF (as opposed to
   `.btx` import, which already works) is a further gap, NAMED NOT FIXED. The corpus
   file's own stamps have their real artwork already stripped by the owner (debug
   red-outline box, deliberately, "so the empty boxes are visible") so this fix is
   proven via a synthetic round-trip test, not against the corpus file itself.
3. **Text markups rotated 90 degrees - INVESTIGATED, DOES NOT REPRODUCE on current
   main (v0.3.13)**. Verified three independent ways: a Rust test reading the actual
   corpus file's `read_markups()` output (correct wide/oriented display-space
   geometry); an isolated vitest unit test of `markupToSvg` fed that exact geometry
   (correctly wide, correctly positioned box); and a REAL BROWSER RENDER (Vite dev app
   + Playwright + mock Tauri IPC seeded with the real corpus geometry from the first
   check) whose screenshot matches the Bluebeam Revu reference screenshot almost
   exactly. PR #70/#72's rotation/MediaBox-origin fix (2026-08-06) is working
   correctly. No code change made for this class.
4. **Rendering differences on the original (non-nudged) open** - subsumed by items 2/3
   above; no separate defect found once those were addressed (confirmed via the same
   browser-render verification against the ORIGINAL, non-nudged corpus file).

Method note worth keeping: static/unit-level analysis of the rotation math (item 3)
checked out correct on paper, yet the owner's screenshot showed it broken - only a
REAL BROWSER RENDER (Vite dev + Playwright + mock Tauri IPC seeded with real
backend-derived data, not synthetic fixtures) resolved the contradiction. This matches
this project's own `judgment.md` precedent ("six M1 render-loop bugs were all
invisible to headless tests, only surfaced on a real `cargo tauri dev` session") -
worth reaching for this technique earlier next time static analysis and a screenshot
disagree.

Tests: `cargo test --all-targets` 471 lib passed (469 baseline + 2 new:
`callout_missing_cl_falls_back_to_rect_anchor_not_page_origin`,
`read_markups_recovers_a_png_stamp_asset_from_its_own_ap_image_xobject`) + 21 pdf-diff,
0 failed. `npm test` 733/733 (4 new stamp-image render cases). `npm run check` 0
errors. `cargo clippy --all-targets` 0 warnings. `npm run build` clean. Each fix has
red-then-green TDD evidence (temporarily reverted, confirmed failing, restored,
confirmed passing).

### Previous session (2026-08-06, PR #71, now the base for PR #73, kept for context)

**PR #71 (raw-storage fallback for tiny images + Bluebeam reference sanity-check,
follow-up to PR #69) open, CI running, mergeable, NOT yet merged. Branch
`fix/optimize-raw-fallback`, head `5365302bf93ac3995a5296c0bf75882973587006`,
https://forge.mms.name/emittiv/redline/pulls/71, based on current main (post PR #70 +
v0.3.12 bump). IMPORTANT PROCESS NOTE: PR #69's SECOND commit (the raw-fallback fix +
annotation-fidelity test, pushed as `97765e1`) was NEVER actually merged despite
Forgejo's PR #69 object showing `head.sha: 97765e1` after close - the orchestrator's
merge (37a13d78, 22:09:36) happened 9 minutes BEFORE that commit was pushed (22:18:49),
so the merge commit's actual parent only contains PR #69's first commit (`b05f4c7`).
Verified by reading the merged file content directly off origin/main (the raw-fallback
code was absent). Remediated by cherry-picking `97765e1` onto a fresh branch from
current main and opening it as PR #71 instead of assuming the closed PR #69 already
had it. Takeaway for future sessions: a closed/merged PR's `head.sha` reflects the
BRANCH'S CURRENT TIP, not necessarily what was actually merged - verify by reading the
merge commit's real file content, never trust the API field alone when a push happens
close to a merge. Owner supplied a real Bluebeam Revu "Reduce File Size" A/B reference
pair (bench/corpus/bb-ref/, gitignored) to sanity-check the image encoding choices;
findings + a concrete "Optimize never moves annotations" proof (40/40 stable across all
3 presets, re-verified against PR #70's Rect/BBox fix too) are in PR #71's body and
obs:7w6wpwu8k2f2xumytan4.**

### Previous session (2026-08-06, PR #70 - now merged, kept for context)

**main is at `37a13d78` (PR #69 image-aware Optimize merged) - PR #70 (markup
coordinate interop fix, this session) is open, CI green (run 3075, both test-rust and
test-frontend success, verified directly against the Forgejo DB for the exact head
sha), mergeable, NOT yet merged (orchestrator owns merge/deploy). Branch
`fix/annotation-rect-bbox-fit`, head `bba2f7645615a7cae876874e22c7cf340c23e1cb`,
https://forge.mms.name/emittiv/redline/pulls/70. Owner report: "the markups from
redline show up in different locations in bluebeam now" - renders correctly in redline,
wrong in Bluebeam. Two independent, empirically-proven bugs, NOT a single regression:
(1) `/AP /BBox` padded larger than `/Rect` for every markup type except Text/Callout/
MeasurementCount - per ISO 32000-1 12.5.5 a strict reader fits the padded BBox into the
tighter Rect, shrinking the appearance toward its own centre (proven with PDFium's own
annotation renderer); `/Rect` is now always sourced from `appearance::ap_bbox` for
every type. (2) SEPARATE finding: redline captures markup geometry in PDFium's
rotation- and MediaBox-origin-relative "display" space, but `/Rect` must be the PDF's
TRUE absolute default user space; `document/annots.rs` now converts display<->true
space only at the write/read serialization boundary, verified against real PDFium
rendering for all 4 rotations + an offset MediaBox origin. Investigated and RULED OUT:
CropBox != MediaBox does not actually cause a position bug (same absolute coordinate
system regardless of CropBox windowing). Both root causes have existed since v0.2.4 (
`/AP` generation) and v0.3.5 (a PARTIAL fix, Text/Callout/Count only) respectively -
not a regression from PR #68/#69. Full detail: obs:xjevyc2hvploz7msy9pq, PR #70 body.
Tests: 470 passed/0 failed/3 ignored (baseline 465/0/3, 5 new tests + 2 stale tests
updated), `cargo clippy --all-targets` 0 warnings, `cargo fmt --check` clean on touched
files (workspace-wide drift on untouched lines confirmed pre-existing, not introduced).**

### Previous session (2026-08-06, PR #69, now merged, kept for context)

**main was at `479aa8e8` (PR #68 merged) before PR #69 (image-aware Optimize) merged.
Branch `fix/optimize-image-compression`, head
`b05f4c70f65da2b447a09adb9e57aaf64ee32776`,
https://forge.mms.name/emittiv/redline/pulls/69. Fixes the owner report "reduced by 9kb
or something insanely silly" - `optimize_document`'s old baseline never touched raster
images (89.6% of a real 110MB corpus file's bytes), only pruned objects and
Deflate-compressed already-uncompressed streams. New `docops::image_ops` module
(Bluebeam-style High/Balanced/Small compression/quality preset) downsamples + recompresses
eligible images against a strict safety bar (skips ImageMask, explicit Mask, custom
Decode, non-Gray/RGB colorspaces, non-8bpc raw, JPX/CCITT/JBIG2). Measured on real
corpus: c1-typical 110.5MB -> 53.6MB (51.5%) at Balanced, up to 60.6% at Small. Render
fidelity verified via real PDFium + visual before/after PNG comparison. Full detail:
obs:t5g42nvkczmj5p12hhrt, PR #69 body.**

### Previous session (2026-08-06, PR #67/#68 - now merged, kept for context)

**main was at `f695610a` (PR #63) before PR #67 (docops reseed + btx subtype guard) and
PR #68 (btx import fidelity) both merged. PR #68: branch `fix/btx-fidelity`, head
`6fc966cd70bcf26eadf01850997edab2f8698fcd`. Real Bluebeam `.btx` samples finally
arrived (bench/corpus/btx/, gitignored per repo policy - "NEVER commit", 18.7MB) and
closed the blocker PR #67 left open ("naming is one, but also a number of items are
incomplete... not the same as the original bb tool" - Martin, same day as PR #67).**

**Fixed against 4 real files / 77 real items - the first real samples this importer was
ever checked against: (1) tool NAMING now prefers the annotation's own `/Subj` over the
opaque, Bluebeam-internal `<Name>` id (7/77 items fall back to `<Name>` when `/Subj` is
absent); (2) tool ORDERING now sorts by `<Index>` - real exports do NOT store items in
Bluebeam's authored display order; (3) STAMP ARTWORK - 11/11 real Stamp items now
resolve to `StampAsset::BluebeamFormXObject` (Bluebeam references artwork via
`/AP<</N/BBObjPtr_<id>>>`, resolved against sibling `<Resources>` blocks forming a small
object graph), spliced into a genuine indirect Form XObject appearance at placement time
(`document::annots::write_markups`) instead of the old always-`None` box+label fallback;
(4) `/BSIColumnData` now read from inside the `<Raw>` PDF dict, not a nonexistent XML
element. NAMED, NOT FIXED: `<Child>` (a second, paired annotation - shape + attached
label, or callout + leader) appears on 33/77 (43%) of real items - `Tool` is
architecturally 1:1 with a single Markup, so this needs a data-model change beyond the
importer's scope. Full detail + fidelity table: obs:ullyvzs86ncoa70itfdi, PR #68 body.
Tests: 465/465 passing (444 lib incl. 21 new + 7 + 14 pdf-diff crate), clippy 0
warnings. `cargo fmt --check` fails identically on `origin/main` pre-branch (confirmed
via git stash) - pre-existing workspace-wide drift, not introduced here.**

### Previous session (2026-08-06, PR #67, dispatched by the orchestrator - investigate
"flatten and optimise don't seem to do anything" and ".btx file import issues")

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

**Date**: 2026-08-11 (dispatched by the orchestrator - owner directive on the
reopened G9 gate: "you need to find a way of testing and validating that yourself...
in browsers and stirling pdf maybe")

**Summary**: See Current Status above for full detail. PR #77 open, CI green, not yet
merged. Built the self-service BB-interop validation harness: a structural
annotation-dictionary conformance test against the real `.btx` corpus (primary check
- catches data-model gaps a screenshot can't), a multi-renderer screenshot matrix
(secondary check), and a research-only tool evaluation (no viable open-source
Bluebeam appearance-regeneration proxy found for markup annotations - qpdf/
Ghostscript's similar-sounding mechanisms are AcroForm-field-only). First run of the
new harness already surfaces new candidate defects (missing `/F`/`/DA`/`/RC`, a
`/BE`-vs-Cloud classification gap) beyond the known grouped-Child residual - not
fixed this dispatch, flagged for the next fix wave once triaged.

### Previous session (2026-08-06, PR #70, dispatched by the orchestrator - owner report "the markups from
redline show up in different locations in bluebeam now")

**Summary**: See "Previous session (2026-08-08, PR #76...)" above for full detail on
the interim sessions. PR #70 open, CI green, not yet merged. Two independent root
causes found and fixed (both proven with PDFium's own annotation renderer, not just
derived): the `/AP /BBox`-vs-`/Rect` fit mismatch (the dominant, always-happens-on-
every-save cause), and a rotation/MediaBox-origin coordinate-frame mismatch (found
during this investigation). CropBox != MediaBox was investigated and ruled out as a
real bug.

### Previous session (2026-08-06, PR #67/#68/#69, dispatched by the orchestrator - investigate "flatten and
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
-1. Merge PR #70 (markup coordinate interop fix - Bluebeam positions) once orchestrator
   schedules it (CI green, run 3075). Live re-verify once merged: place a Rectangle/
   Line/Arrow/Cloud/Highlight/Ink markup, save, open the file in real Bluebeam Revu,
   confirm it renders in the same place and at the same size as in redline (as of
   2026-08-06).
0. Merge PR #69 (image-aware Optimize) once orchestrator schedules it (CI green, run #189).
   Live re-verify: Optimize a doc with real raster content at each of the three quality
   presets, confirm the toolbar select + completion banner's image breakdown, and
   spot-check a recompressed page still looks correct at normal zoom (as of 2026-08-06).
1. PR #67 and PR #68 are now MERGED (main at `479aa8e8`) - the live re-verify items below
   are still owed against the merged code, not superseded by the merge.
2. Live re-verify PR #67 in a real `cargo tauri dev` session: Flatten a doc with real
   markups, confirm they disappear from the markup list/become unselectable and the
   new success banner reports a count; Optimize and confirm the before/after
   file-size banner; confirm a later save does NOT resurrect a flattened markup
   (as of 2026-08-06).
3. Live re-verify PR #68: import one of the real `.btx` files
   (`bench/corpus/btx/` locally, gitignored) via the Tool Chest UI, confirm tool names
   match Bluebeam's own labels and a Stamp tool's placed appearance shows real artwork,
   not a box+label (as of 2026-08-06).
4. FOLLOW-UP, not started: `<Child>` grouped-annotation support (43% of real items) -
   needs a `Tool` data-model change (optional linked/secondary markup) plus
   placement-time support for two annotations at once. Scoping/design decision owed
   before implementation (as of 2026-08-06).

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
| PR #67 | `https://forge.mms.name/emittiv/redline/pulls/67` (docops markup-store reseed fix + btx unsupported-subtype guard - OPEN, CI green) |
| PR #67 | `https://forge.mms.name/emittiv/redline/pulls/67` (docops reseed + btx subtype guard - MERGED `88861cc`) |
| PR #68 | `https://forge.mms.name/emittiv/redline/pulls/68` (btx import fidelity vs real Bluebeam samples - MERGED `479aa8e8`) |
| PR #69 | `https://forge.mms.name/emittiv/redline/pulls/69` (image-aware Optimize compression preset - MERGED `37a13d78`) |
| PR #70 | `https://forge.mms.name/emittiv/redline/pulls/70` (markup coordinate interop fix for Bluebeam - OPEN, CI green) |
| PR #69 | `https://forge.mms.name/emittiv/redline/pulls/69` (image-aware Optimize, compression/quality preset - OPEN, CI green run #189, head `b05f4c70`) |
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
- **`.btx` import fidelity FIXED against real samples (PR #68, 2026-08-06)**: tool
  naming prefers `/Subj` over the opaque `<Name>` id; tools sort by `<Index>`; Stamp
  artwork resolves via `/AP<</N/BBObjPtr_<id>>>` + sibling `<Resources>` blocks into
  `StampAsset::BluebeamFormXObject`, spliced into a real indirect Form XObject by
  `document::annots::write_markups` (never redraw a `BluebeamFormXObject` asset - its
  whole point is being the AS-AUTHORED bytes; see `resolve_bb_objptr_refs`'s doc comment
  for why the replacement needs a LEADING SPACE - PDF names are self-delimiting, `5 0 R`
  with no separator produces an invalid merged token). STILL NAMED, NOT FIXED: `<Child>`
  grouped/paired annotations (43% of real items) need a `Tool` data-model change beyond
  this importer's scope. Full detail: obs:ullyvzs86ncoa70itfdi, PR #68.
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
- **`docops::image_ops` (PR #69, 2026-08-06)**: `optimize_document`'s `image_preset`
  param is additive to `level` - do not conflate the two axes. lopdf 0.44's
  `Stream::decompressed_content()`/`get_plain_content()` only implement
  FlateDecode/LZWDecode/ASCII85Decode internally (returns `Error::Unimplemented` for
  DCTDecode/CCITT/JPX/JBIG2) - read DCTDecode bytes directly off `Stream.content`
  instead, and for the `[FlateDecode DCTDecode]` double-wrap seen in real corpus files,
  manually inflate via `flate2::read::ZlibDecoder` before JPEG-decoding (calling
  `get_plain_content()` on that filter combo fails partway through). Placement-size
  detection (for DPI-based downsampling) walks page + one level of Form XObject content
  streams via `lopdf::content::Content::decode` tracking the CTM - do not add a second
  level of nesting without a real corpus case that needs it, it adds real complexity.

---
*Updated: 2026-08-06 (PR #69: image-aware Optimize - real JPEG downsample+recompress fixes the "reduced by 9kb" report; PR #67/#68 now confirmed merged to main)*

## Cross-viewer harness — Acrobat renders (2026-08-30)

**Status: SOLVED, with one owner-gated blocker.** Acrobat produced its first real page render
under the harness. Commit `81f1aaf` on `feat/crossviewer-automated-harness` (PR #85, not merged,
CI not checked).

Root cause: **IAC's `AVDoc.Open` never creates a document window.** With the doc open and COM
reporting `pages=1 annots=20`, enumerating every top-level Acrobat window found exactly one —
the empty shell `'Adobe Acrobat (64-bit)'`. Nothing existed that could paint. Launching Acrobat
with the file as a command-line argument creates `'AllTypes.pdf - Adobe Acrobat (64-bit)'`, which
paints in 2s: `bright=0.5005`, `AllTypes.png` 281,858 b, verify-on-top 5/5.

**Owner-authorised host pref (outside the repo):** `HKCU\Software\Adobe\Adobe Acrobat\DC\AVGeneral`
→ `bSDIMode` REG_DWORD **absent → 1**. Backup `H:\redline-crossviewer\backup-acrobat-prefs-20260830-051402.reg`;
rollback = delete the value. It removed the Home tab strip but was NOT sufficient on its own.

**Blocker (needs Martin):** IAC and command-line launch are mutually exclusive —
`AcroExch.App` fails `0x80080005` once Acrobat runs normally. So annotation counts OR renders,
not both. Likely Protected Mode (`bProtectedMode=1`); disabling that sandbox on his daily
workstation was deliberately not done.

### Next Steps
1. Decide Protected Mode, or build the decision-free two-phase run: IAC pass for counts → close → command-line pass for renders (as of 2026-08-30)
2. Crop captures to the page rectangle before vision review — the current capture is the whole app window (tool rail + Comments panel), which confounded the first review into a FAIL (as of 2026-08-30)
3. Explain the count discrepancy: Acrobat Comments panel says 19, IAC annotation scan says 20 (as of 2026-08-30)
4. Handle the modal `Scanned Page Alert` dialog Acrobat raises on every command-line open of AllTypes.pdf (as of 2026-08-30)
5. Prune the mission record — 71 next_steps, API is emitting code-churn lint warnings (as of 2026-08-30)

*Batch of 24 not started. mr-desktop left clean: Acrobat 0, AcroCEF 0, RdrCEF 0, Revu 0, all tasks Disabled, task args restored.*

---
*Updated: 2026-08-30*

## Cross-viewer harness — full clean run BLOCKED, mr-desktop wedged (2026-08-31 ~18:12-18:35)

A subsequent run on 2026-08-31 (report generated 11:59:53Z, before this entry) got 14/24
Acrobat renders + 24/24 Revu (Fit Page/crop from PR #85 confirmed working live) but was left
running - Acrobat with 16 open document windows plus one unidentified modal dialog (class
`#32770`, 496x170), Revu alive but with 0 enumerable windows. A follow-up dispatch to run
the "owed full clean" pass (post PR #85 + #88) found this state on arrival and could NOT
clear it: `CloseAcrobat.ps1` hung twice (90s and 150s timeouts) with `GetNumAVDocs()` never
returning - consistent with the modal dialog blocking Acrobat's COM thread, a different
failure shape than the documented IAC-vs-command-line 0x80080005 mutual exclusivity above.
This also blocks restaging the corpus (`scp` fails on files Acrobat has open).

New recovery script added (uncommitted, in worktree
`.claude/worktrees/crossviewer-capture-run/`): `tools/crossviewer/win/CloseRevu.ps1` (WM_CLOSE
companion to `CloseAcrobat.ps1`) — ran clean but found 0 windows to close on Revu, so it
could not recover Revu either. `DiagWindows.ps1` (read-only window dump) added alongside it.

**PR #88 (FreeText `/C` fix) confirmed merged and present** (`63ad31e`, ancestor of HEAD
`29116fc`); fresh 22-file corpus regenerated reflecting it (`Callout.pdf`/`Text.pdf` grew
vs the stale Aug-29 corpus, matching the new `/RLTextColor` key) — but no viewer render
exists yet to visually confirm the fix, since that's exactly what's blocked.

**Needs Martin**: hands-on look at mr-desktop (dismiss the unknown dialog, close Acrobat's
16 tabs, restart Revu) or explicit authorization to force-terminate named PIDs (Acrobat
58484/52968/5692-orphan + 6 AcroCEF children, Revu 54320). Full detail:
`observation:oezc33hr7nuvvxk6nb26`. Corpus + scripts already staged on mr-desktop; all
`redline-crossviewer-*` tasks confirmed left Disabled.

---
*Updated: 2026-08-31*
