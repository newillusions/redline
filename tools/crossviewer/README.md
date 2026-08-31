# Cross-viewer harness

Automates the manual "open every staged PDF in Acrobat and Bluebeam on mr-desktop and look
at it" pass that the G9 Bluebeam-interop work has owed since `obs:nx5nqon8k8xrty2vljsz`.

## Why this exists

Redline's markups have to read correctly in the viewers the owner actually uses. The
existing checks do not cover that:

- `src-tauri/tests/bb_interop_conformance.rs` is the **primary, structural** check - it
  diffs our annotation dictionaries against genuine Bluebeam-authored ones. It says nothing
  about what a viewer draws.
- `tools/crossviewer-render-matrix.mjs` renders through pdfium / mutool / poppler / Chrome.
  All of those mostly **blit our stored `/AP` appearance stream**, so a page can look perfect
  there while the underlying data model is wrong - which is the exact shape of every real G9
  defect so far. Chrome is PDFium internally, so it is not even an independent engine.

Acrobat and Bluebeam **regenerate** markup appearances from the annotation dictionary. That
is why a render from them is worth more than four renders from the matrix, and why this
harness targets those two specifically.

## Layout

| Path | Runs on | What it does |
|---|---|---|
| `run-crossviewer.mjs` | Mac | Orchestrates everything: regenerate corpus, stage, run legs, pull results, vision-review, report |
| `checklist.json` | Mac | The visual checklist, phrased as questions the vision model answers |
| `vision-review.mjs` | Mac | Screens each render through a local vision model via llm-gate |
| `win/Register-CrossviewerTask.ps1` | mr-desktop | Registers the Session-1 scheduled tasks (one-shot, idempotent) |
| `win/AcrobatLeg.ps1` | mr-desktop | Drives Acrobat over COM/IAC: open, page count, annotation scan, PNG export |
| `win/BluebeamLeg.ps1` | mr-desktop | Bluebeam leg - currently a licence probe, see below |
| `win/CloseAcrobat.ps1` | mr-desktop | Recovery: close leftover documents and exit Acrobat, API-only |

The corpus is **not** stored here. It is regenerated from the repo's own `#[ignore]`d
emitters so it can never drift from what the code actually writes:

```
REDLINE_CROSSVIEWER_OUT=/tmp/redline-crossviewer \
  cargo test --lib -- --ignored --nocapture emit_crossviewer_corpus emit_bb_corpus_roundtrip
```

That yields 24 PDFs: one per `MarkupType` (20), an all-types page on a real project plan
backdrop, the synthetic blank backdrop, and redline-resaved round-trips of the two genuine
Bluebeam reference files. The two round-trips and the real backdrop require
`bench/corpus/bb-ref/` (gitignored, machine-local); without it you get 22 and the emitters
say so rather than failing silently.

## Running it

```bash
node tools/crossviewer/run-crossviewer.mjs
# useful subsets
node tools/crossviewer/run-crossviewer.mjs --skip-remote          # vision-review existing renders
node tools/crossviewer/run-crossviewer.mjs --skip-corpus --skip-vision
```

## Why Session-1 scheduled tasks

Acrobat and Revu are GUI applications. A command launched over SSH lands in Session 0, which
has no window station, so COM either fails outright or hangs. The workaround proven on this
workstation by cad-export's AutoCAD runner is a scheduled task registered against the
interactive user and started remotely.

Two settings are load-bearing:

- **`RunLevel Limited`, never `Highest`.** A GUI app runs at medium integrity; an elevated
  task cannot attach to its COM server.
- **A resolved `WindowsIdentity` string for `-UserId`.** A hand-built `DOMAIN\user` string
  fails registration with "No mapping between account names and security IDs was done".

The tasks run **Windows PowerShell 5.1**, not `pwsh`: on mr-desktop `pwsh` is an MSIX
app-execution alias with no invocable file path (`obs:ryzah0kwi09tjeg9ppf8`), and a scheduled
task needs a real executable. The leg scripts are 5.1-compatible for that reason.

## Status as of 2026-08-31: Fit Page + symmetric crop

**Closes the "Revu is missing the lower-left markup cluster" question left open on
2026-08-30 (HANDOVER.md) - it was a viewport artifact, not a Bluebeam interop defect.**
Proven by direct pixel measurement against that session's own real captures, not assumed:
Revu's own page thumbnail (visible in the render's left rail) already showed the missing
cluster; the main viewport did not, because `win/BluebeamGuiLeg.ps1` never sent a Fit Page
command and Revu opened at whatever zoom it last remembered - a small vertical scrollbar
thumb spanning ~30% of the track proved the page ran on well past the bottom of the window.

Two fixes, both required together:

1. **`win/BluebeamGuiLeg.ps1` now sends Fit Page (`Ctrl+9`) before every capture**
   (`Send-FitPage`, via `SendKeys` - there is no scripting-API route on this licence tier,
   see the module header). **`Ctrl+9`, not `Ctrl+0`** - confirmed against Bluebeam's own
   Keyboard Shortcuts Guide (`support.bluebeam.com/resources/pdfs/keyboard-shortcuts.pdf`,
   View section): Fit Page is `Ctrl+9`, Fit Width is `Ctrl+0` - a different command that
   would not have fixed this. `win/AcrobatLeg.ps1` already sent `ZoomTo(FitPage)` over COM;
   it needed no zoom change, only the crop below.
2. **`crop-to-page.mjs` (new) crops every render down to just its page rectangle** before
   anything compares the two legs. Both legs photograph the whole application window - a
   comparison without this is Acrobat's dark chrome and Comments panel against Revu's own
   different chrome, not the pages. Method: cast rays outward from the image's own centre
   (already known to be on the page) toward each edge and stop on a debounced run of
   non-bright pixels - deliberately NOT a global brightness-majority scan, because Acrobat's
   Comments panel is ALSO bright and sits right next to the page. Calibrated against this
   harness's own real 2026-08-30 captures, including a bug the first calibration pass
   produced and caught the same way: a too-short debounce read straight through a page-wide
   Highlight bar and called it the page's edge, silently cropping into real page content -
   see the module's own header for the measured fix. Wired into `run-crossviewer.mjs` as
   step 4.5, ahead of vision-review (which now reviews cropped renders) and step 5.5 below.
3. **`compare-alltypes.mjs` (new)** - a mechanical, no-model pixel check of the specific
   question this harness's first live run raised: does the lower-left cluster
   (Rectangle/Arrow/Text/StampDynamic) show up in both viewers' cropped `AllTypes.pdf`
   render? Measured against the real 2026-08-30 pre-fix captures: Acrobat's lower-left
   region was 25-41% non-white depending on exact sample box, Revu's was an exact, repeated
   0% - not "small", genuinely absent from the visible frame. Stays in the harness
   permanently (not a one-off diagnostic) as the standing proof this stays fixed.

**Live-run proof, same day.** Ran the full pass against mr-desktop (existing 24-PDF corpus,
`--skip-corpus`). Revu's `AllTypes.pdf` result: `fit_page_sent: true`, `settled: true`, no
error - and the cropped render (`out/cropped/bluebeam-gui/AllTypes.png`) visually shows the
COMPLETE markup set: both Highlight bars AND the lower-left Rectangle/Arrow/Text/StampDynamic
cluster, matching Acrobat's known-good 2026-08-30 render. Confirms the fix directly, not just
by inference from the code change.

`compare-alltypes.mjs` itself reported `NOT FOUND` for Acrobat this run, not a mismatch - a
**separate, new** Acrobat capture failure hit `AllTypes.pdf` specifically this run
(`render_error: "no page captured"`, `bright fraction 0` - a fully black capture, a different
failure shape than the documented "Home screen" blank-pane class). 16/24 other Acrobat files
captured fine in the same run, so this is not a wholesale regression of the Acrobat leg -
worth a follow-up session, not chased further here (out of this fix's scope, and the run
ended early - see below). Also observed: Revu did not self-close within its 45s timeout after
finishing all 24 tabs (`"Revu did not close within 45s; pid ... left running deliberately"`,
per the leg's own never-force-kill design) - plausibly just more tabs to wind down than any
prior run tested with, not investigated further.

**Run stopped mid-verification, not by choice.** The owner began actively using mr-desktop
partway through (looked at the very Acrobat window this run had left open) partway through
report generation. All 7 `redline-crossviewer-*` scheduled tasks were re-disabled immediately
on that signal; Acrobat/Revu windows were deliberately left untouched per the harness's own
"never force-close a viewer" rule, doubly so once they might be the owner's own live session.
A full clean end-to-end run (all 24 files through both legs, zero leftover processes) is
still owed.

## Status as of 2026-08-29

**Proven working on real hardware:**

- Session-1 task registration and remote invocation.
- Acrobat DC 26.1 over COM/IAC: opens the file, reports page count, and runs Acrobat's own
  annotation scan. On `AllTypes.pdf` it reported `pages=3 annots=20`, matching all 20 markup
  fixtures - real evidence Acrobat parses every markup type we emit.
- The vision-review leg end to end against llm-gate (`qwen3.8:27b`, `think=false`), ~51s per
  page, returning structured per-checklist-item verdicts. On a real `AllTypes.pdf` render it
  correctly judged the stamp to contain artwork rather than being an empty placeholder box -
  the exact defect class it is meant to catch.

**Two things a PowerShell caller must know about Acrobat's JSObject**, both cost real time
here:

1. Every JSObject method call fails with `Value does not fall within the expected range.`
   under normal late binding. It is a raw IDispatch; you must go through
   `[System.__ComObject].InvokeMember(...)` with an explicit `InvokeMethod` binding. That is
   what `Invoke-Jso` in `AcrobatLeg.ps1` exists for.
2. Under `app.Hide()` the PNG export via `saveAs` **never returns** - open and annotation
   scan both succeed, then it hangs indefinitely with the process still `Responding` and no
   dialog on screen. Acrobat is therefore left visible on purpose.

**Not yet working:**

- **PNG export is unverified.** The hidden-window hang above wedged Acrobat in a state where
  the document could not be closed even via `AVDoc.Close` (`CloseAcrobat.ps1` reports
  `closed [0] AllTypes.pdf` and then `remaining: 1`). Those Acrobat processes were later
  cleared, but by then the Session-1 task environment had been wedged by the placement
  self-test described above, so the batch still has not run. The visible-mode fix plus the
  in-use guard remains the change that has never had a clean run.
- **Window placement is unverified.** The code is written and wired into `AcrobatLeg.ps1`,
  but `SetWindowPos`/`GetWindowRect` have not been confirmed to move a real window - see the
  self-test section above for why no isolated test exists.
- **Bluebeam is blocked by licence tier, not by code.** Revu 21 is installed and ships
  `ScriptEngine.exe` 21.10.0.19316, but any invocation - including `/?` - exits `-4` with:

  > This feature requires a maximum subscription level. Please upgrade to access advanced
  > scripting capabilities

  `Bluebeam.Exporter.exe` was probed as an alternative and exits `-1` with no usable CLI.
  `win/BluebeamLeg.ps1` records this in the run report rather than silently covering only
  Acrobat. Options to unblock are documented in that script's `.NOTES`.

## Which monitor captures land on

mr-desktop has three monitors and one of them is **portrait**. A landscape drawing sheet
reviewed on a portrait panel is scaled down hard, so captures must land on a landscape
display. `win/Displays.ps1` handles selection and placement; `win/ProbeDisplays.ps1` reports
the layout.

**Measured in Session 1 on 2026-08-29:**

| Device | Resolution | Orientation | |
|---|---|---|---|
| `\\.\DISPLAY1` | 2560x1440 | landscape | |
| `\\.\DISPLAY2` | 1440x2560 | **portrait** | never captured on |
| `\\.\DISPLAY3` | 5120x1440 | landscape | primary, largest — selected |

**There is no 3840x2160 monitor on this machine.** The "4K" figure comes from
`Win32_VideoController`'s `VideoModeDescription`, which reports the Session 0 pseudo-mode
rather than any attached panel. Selection therefore falls through to "largest landscape" and
records exactly that in its `reason` field, which the run report prints. The preferred
resolution stays a parameter so the intent survives if a real 4K panel is attached later, and
`-TargetDevice` pins a specific monitor. Selection **refuses** to fall back to a portrait
panel under any circumstance.

**Enumerate displays only from inside Session 1.** Over SSH, `Screen.AllScreens` returns a
single fake `WinDisc 1024x768` device — the Session 0 pseudo-display. Verified both ways on
2026-08-29. Treat one 1024x768 "WinDisc" as proof you are in the wrong session, never as a
monitor layout; `Get-CrossviewerDisplays` flags it as `looks_like_session0` and `AcrobatLeg`
refuses to continue.

### Do not create windows to self-test placement

Two attempts at a placement self-test both went wrong and the second wedged the machine, so
`ProbeDisplays.ps1` deliberately has none:

- `notepad.exe` on Windows 11 is an MSIX stub whose launcher process exits immediately, so
  `Start-Process -PassThru` returns an object whose `MainWindowHandle` is null forever.
- A WinForms `Form` has a real handle, but with no message pump it never processes
  `WM_CLOSE`, so the PowerShell host cannot exit. `Stop-ScheduledTask` and
  `CloseMainWindow()` both failed to clear it.

**Separately, Session-1 task launching went bad while that host was stuck**: a trivial task
that only wrote a string to a file — no `Add-Type`, COM or display code — also hung, with its
process sitting at 0% CPU producing nothing. Two candidate causes, and this was NOT
disambiguated:

1. the stuck unpumped-window host wedging the session's window station, or
2. **contention between concurrent Session-1 tasks** — a parallel agent was driving Revu GUI
   tasks in the same interactive session at the same time.

Cause 2 is at least as likely as cause 1 and is the cheaper thing to rule out first.
**Serialise Session-1 work**: the interactive session is a single shared resource and two
harness legs must not drive it at once. Recovery from the wedged state needed the stuck host
terminated.

`Move-WindowToDisplay` is therefore exercised for real by `AcrobatLeg.ps1` against Acrobat's
own window, and the display it used is recorded per-file in `acrobat-results.json`. It has
**not** been independently verified in isolation.

## Guard against disturbing a real session

`AcrobatLeg.ps1` refuses to run if Acrobat already has documents open, because IAC attaches
to an **already-running** Acrobat rather than starting a private one - so a batch run could
churn, and `App.Exit()` could close, documents a human has open on this shared workstation.
cad-export learned the same lesson expensively with AutoCAD (`obs:8ji54fchnw06p7w9iwe6`).

Neither leg ever force-terminates a viewer. Documents close via `AVDoc.Close` and the
application via `App.Exit`; a process that survives is reported, not killed.

## Interpreting the vision review

It is a **screening** layer. A vision model can miss a subtle defect and can occasionally
invent one, so a FAIL is a prompt to open the named page - never on its own grounds for
asserting the app is broken. The authoritative checks remain the structural conformance test
and the Acrobat leg's own open/page/annotation-count report.
