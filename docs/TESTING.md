# Testing

Per the workspace-wide Tauri UI testing research (`/Users/martin/dev-reports/2026-08-22-tauri-ui-testing.md`)
and the `satchel-gui` precedent that first adopted it (forge.mms.name/emittiv/satchel PR #25).
None of these replaces a real interactive click-through on the target hardware for
installer/SmartScreen/native-dialog chrome, or the still-owed §20 floor-machine run and G9
human Acrobat/Bluebeam visual check (see CLAUDE.md "Current phase").

## Tier 0/1 — Rust unit/integration tests + Svelte component tests

- `cargo test` (portable) / `REDLINE_BENCH_TESTS=1 cargo test --release -- --test-threads=1`
  (PDFium + corpus, serial - PDFium holds global C state).
- `npm test` (vitest) + `npm run check` (svelte-check) - component logic and store wiring
  against scripted/mocked IPC, no Tauri runtime.
- Ceiling: neither exercises the real Rust `#[tauri::command]` functions through real IPC, a
  real webview, or real OS dialogs/permissions.

## Tier 2.5 - Cross-viewer harness (Acrobat / Bluebeam on mr-desktop)

`tools/crossviewer/` automates the manual "open every staged PDF in Acrobat and Bluebeam and
look at it" pass. It matters because Acrobat and Revu REGENERATE markup appearances from the
annotation dictionary, whereas every engine in `tools/crossviewer-render-matrix.mjs` mostly
blits our stored `/AP` - so a page can look right there while the data model is wrong, which
is the shape of every real G9 defect so far.

```bash
node tools/crossviewer/run-crossviewer.mjs
```

Corpus is regenerated from the repo's own `#[ignore]`d emitters (24 PDFs: one per
`MarkupType`, an all-types page, the blank backdrop, and two Bluebeam-reference round-trips),
staged to mr-desktop, run there as Session-1 scheduled tasks because GUI apps cannot run in
the Session 0 context SSH lands in, then screened through a local vision model against
`tools/crossviewer/checklist.json`.

Proven on real hardware 2026-08-29: Session-1 invocation, Acrobat DC 26.1 over COM/IAC
(`AllTypes.pdf` -> `pages=1 annots=20`, matching all 20 fixtures), and the vision-review leg
end to end (`qwen3.8:27b`, ~51s/page, structured per-item verdicts). The vision review has
NOT yet been run against an Acrobat capture - see the open blocker below.

**2026-08-31: Fit Page + symmetric crop, closing the "Revu missing the lower-left cluster"
question from 2026-08-30.** Root cause was a viewport artifact, not a Bluebeam render
defect: `win/BluebeamGuiLeg.ps1` never sent a Fit Page command, so Revu opened at whatever
zoom it last remembered and the page ran on well past the bottom of the window - proven by
comparing the captured render against Revu's own page thumbnail, which already showed the
"missing" cluster. Fixed by sending `Ctrl+9` (Fit Page - confirmed against Bluebeam's own
Keyboard Shortcuts Guide; `Ctrl+0` is Fit *Width*, a different command) before every
capture. Both legs' renders are then cropped to just the page rectangle
(`tools/crossviewer/crop-to-page.mjs`, ray-cast from image centre with a debounce
calibrated against a real measured failure - see its module header) before vision-review or
any comparison runs, since both legs photograph the whole application window and a fair
comparison needs the same content on both sides. `tools/crossviewer/compare-alltypes.mjs`
adds a standing, no-model-call pixel check of the lower-left cluster specifically. Full
writeup: `tools/crossviewer/README.md` "Status as of 2026-08-31".

Captures are placed on a landscape display (mr-desktop has a portrait panel), defaulting to
`\\.\DISPLAY1`, the owner's review monitor. Displays must be enumerated from inside
Session 1 - over SSH `Screen.AllScreens` returns a single fake `WinDisc 1024x768`.

**Display sizes depend on DPI awareness.** The same three panels report 2560x1440 /
1440x2560 / 5120x1440 to a DPI-unaware process and 3840x2160 / 2160x3840 / 7680x2160 once
`SetProcessDPIAware` has been called - the machine runs at 150% scaling. An earlier note in
this file recorded the scaled figures and concluded "there is no 3840x2160 monitor"; that was
a DPI artifact, and DISPLAY1 is in fact a 4K panel. Screen capture addresses PHYSICAL pixels,
so the legs call `Enable-DpiAwareness` before reading any rectangle. Never self-test window
placement by creating a window - an unpumped WinForms window wedges Session-1 task launching
entirely; see the README.

### Rendering is by screen capture, and every capture must be proven

`saveAs(..., 'com.adobe.acrobat.png')` is dead as a render primitive: it never returns, with
Acrobat visible or hidden (two runs, 0 PNGs from 9 attempts). Pages are therefore captured
from Acrobat's own window. Three guards exist because each caught a real failure that looked
exactly like success:

- **Verify-on-top.** `SetForegroundWindow` is refused to a background process, so a scheduled
  task cannot raise a window that way. The first capture run produced a valid 415 KB PNG of
  the owner's Microsoft Teams window. Captures now force the window topmost via
  `SetWindowPos(HWND_TOPMOST)` and prove ownership by resolving five sample points through
  `WindowFromPoint` + `GetAncestor(GA_ROOT)`; an obstructed window is a hard failure.
  A blocker is logged by window class and process name only - never its title, which would put
  the owner's correspondent into a log file.
- **Page-visible check.** Window title, class, handle, rectangle and Z-order were all correct
  while the pane was blank, so no window property can settle this. `Test-PageVisible` samples
  the pixels and requires a bright fraction consistent with a page being displayed. A capture
  with no page in it is kept as `REJECTED-<name>.png` - never counted as a render.
- **Run control.** `Invoke-CrossviewerTask.ps1` is the only sanctioned way to start a leg: it
  enables exactly one task, enforces a hard timeout, disables the task again in a `finally`,
  and terminates only viewer processes whose start time is after the run began. `AcroCEF` is
  in that list - Acrobat renders its document pane in CEF children, and killing only the
  parent orphans them.

### Acrobat host preference: `bSDIMode` (owner-authorised config change, 2026-08-30)

The harness requires one change to the **owner's Acrobat install** on mr-desktop. It is
recorded here because it lives outside the repo and will not travel with a checkout.

| | |
|---|---|
| Key | `HKCU\Software\Adobe\Adobe Acrobat\DC\AVGeneral` |
| Value | `bSDIMode`, `REG_DWORD` |
| Before | **absent** (Acrobat's default: tabbed viewing) |
| After | `1` |
| Meaning | `1` = Single Document Interface: a PDF opens in its **own document window**. `0`/absent = documents open as tabs in one frame. |
| Backup | `H:\redline-crossviewer\backup-acrobat-prefs-<timestamp>.reg` (full `AVGeneral` key export, taken immediately before the write) |
| Rollback | Delete the `bSDIMode` value - it did not exist before - or re-import the `.reg`. Acrobat must be closed for either to stick. |

Why this preference and not a "hide the home screen" one: there is no such preference on this
install. The live `HKCU` Acrobat tree was dumped for every value name matching
`Home|Tab|Welcome|Startup|FirstView|OpenIn` and contains no `bShowHomeScreen`,
`bDisableHomeScreen`, `bSuppressHomeScreen` or `bOpenInNewTab`. `bSDIMode` attacks the
observed mechanism instead: the document was **always open** - COM reported `pages=1
annots=20` against a live `AVPageView` - it simply was not the *selected tab*, because the
frame stayed on Home. With SDI there is no shared frame and no Home tab to lose to.

Verification caveat, stated rather than glossed: Adobe's own ETK preference reference
(`adobe.com/devnet-docs/acrobatetk/.../AVGeneral.html`, `FeatureLockDown.html`) timed out on
four separate 60 s fetch attempts and direct `curl` is blocked from the dev sandbox, so the
key was confirmed against two independent Adobe Support Community threads that agree exactly
on path, name, type and semantics - **not** read off Adobe's documentation page. Treat it as
corroborated, not officially cited.

Set it only with Acrobat fully closed (`Acrobat`, `AcroCEF`, `RdrCEF` all at 0); Acrobat
rewrites `AVGeneral` from memory on exit and will discard a write made while it is running.

### RESOLVED - Acrobat renders. IAC's `AVDoc.Open` never creates a document window

Superseded the "viewer never paints" blocker on 2026-08-30. The pane was not failing to
paint; **there was no document window at all.**

With `bSDIMode=1` set and `AllTypes.pdf` open - COM reporting `pages=1 annots=20`,
`GetAVPageView()` returning a live view - the leg enumerated every top-level window owned by
every Acrobat process and found exactly **one**:

```
candidate hwnd=9769156 pid=58576 3840x2064 class=AcrobatSDIWindow title='Adobe Acrobat (64-bit)'
```

That is the empty application shell. No document window existed, so no amount of
`BringToFront`, tab selection, zoom or waiting could ever have produced a page. Every
window property was correct because the window we were photographing was real - just not
the document's.

Launching Acrobat with the file as a **command-line argument** creates one immediately:

```
candidate hwnd=8066690 pid=67144 3840x2064 class=AcrobatSDIWindow title='AllTypes.pdf - Adobe Acrobat (64-bit)'
...
waiting for page paint: bright fraction 0.5005
page painted after 2s
captured page 1/1 -> AllTypes.png (281858 bytes, settled=True, frames=4, bright=0.5005)
```

First non-zero bright fraction in the project's history. `-LaunchViaCommandLine` on
`AcrobatLeg.ps1` selects this path.

#### The remaining blocker: the two launch paths are mutually exclusive

Once Acrobat is running from a command-line launch, `New-Object -ComObject AcroExch.App`
fails with `0x80080005 CO_E_SERVER_EXEC_FAILURE` - the running instance does not serve
automation. So today the harness can have **either** the annotation scan (IAC, no picture)
**or** the picture (command line, no annotation scan). Under `-LaunchViaCommandLine` COM is
best-effort: when it is unavailable the leg still places, proves and photographs the real
document window, and reports `pages`/`annots` as null rather than inventing them.

The likely cause is Acrobat's **Protected Mode** sandbox (`bProtectedMode=1` is set on this
install; Adobe's guidance is that IAC/OLE requires it off). That was **deliberately not
changed** - disabling a sandbox on the owner's daily workstation is a security decision, not
a harness convenience. It needs an explicit owner call. Until then, a two-phase run (IAC
pass for annotation counts, close, command-line pass for renders) is the obvious workaround
and is **not yet built**.

#### Two smaller findings from the same session

- Acrobat raises a modal **`Scanned Page Alert`** dialog (class `#32770`, 744x240) over the
  document on every command-line open of `AllTypes.pdf`. It did not block this capture, but
  it sits mid-frame and will contaminate any vision review of this fixture.
- Acrobat's Comments panel counts **19** comments where the IAC annotation scan counts
  **20**. Unexplained; worth pinning down before either number is treated as authoritative.

#### First vision review: FAIL, with a large caveat

`vision-review.mjs` on the render returned `overall: fail` (`qwen3.8:27b`): `page-renders`,
`markups-visible` and `geometry-on-page` pass; `markups-not-empty-boxes`, `text-legible` and
`measurements-legible` fail, on "overlapping, illegible annotations and empty placeholder
boxes in the bottom-left corner".

Do not read that as a proven redline defect. Two confounders have to be removed first:
(1) the capture is the **whole application window** - left tool rail, right Comments panel,
title bar - not the page rectangle, so the model is reviewing chrome it was never given a
checklist for; cropping to the page before review is the obvious fix and is not yet done.
(2) `AllTypes.pdf` is a synthetic every-annotation-type fixture whose markups genuinely do
overlap in one corner by construction. The harness's own rule applies: a FAIL is a prompt to
look at the named page, not proof of a defect.

Bluebeam's Script Engine remains gated behind a higher subscription tier - `ScriptEngine.exe`
exits `-4` with "This feature requires a maximum subscription level" for any invocation.
`BluebeamGuiLeg.ps1` (GUI automation via the same capture helper) is committed but **has not
been run**.

Ceiling: the vision review is a screening layer, not an oracle - a FAIL is a prompt to look
at the named page, not proof of a defect.

## Tier 1.5 — Mocked-IPC frontend harness (`tools/gui-harness.mjs`)

Loads the real Vite dev app in headless Chromium with a `window.__TAURI_INTERNALS__` shim
returning synthetic tiles/docs - scripts zoom/pan/page-nav/tool interactions and screenshots
the result. Verifies frontend render-loop + interaction behaviour without a Tauri runtime.
Run: `npm run gui:harness` (needs `npm run dev` serving `:1421` first).

## Tier 2 — Real-app E2E (WebdriverIO + `@wdio/tauri-service`)

Drives the **actual compiled `redline` binary** - real Tauri commands over real IPC, a real
webview, no mocked backend. Uses `@wdio/tauri-service`'s embedded provider
(`tauri-plugin-wdio-webdriver` + `tauri-plugin-wdio`, both gated `#[cfg(debug_assertions)]` in
`src-tauri/src/lib.rs` - never linked into a release build's active codepath), which runs the
WebDriver server inside the app process itself. No CrabNebula subscription, no external
`tauri-driver`/`msedgedriver` needed on macOS. Pattern, config, and versions copied verbatim
from `satchel-gui`'s harness (PR #25) - see that repo's `docs/TESTING.md` for the original
`about:blank`/TCC investigation this pattern grew out of.

### Running it

```bash
npm install
npm run e2e:build   # tauri build --no-bundle --debug --config src-tauri/tauri.e2e.conf.json
                     # runs the frontend build (beforeBuildCommand) then produces
                     # target/debug/redline, with app.withGlobalTauri merged to true FOR
                     # THIS BUILD ONLY - see "withGlobalTauri" below for why the override
                     # exists and why it must never be the committed default.
npm run e2e         # wdio run ./wdio.conf.js
```

`tauri::generate_context!()` bakes `frontendDist`'s contents into the binary at Rust *compile*
time, so re-run `npm run e2e:build` after any frontend OR Rust change before `npm run e2e`, or
the running app will serve stale content.

Spec files live in `e2e/specs/*.spec.js` (outside `tsconfig.json`'s `include`, which is scoped
to `src/**` - `svelte-check` never touches them). `wdio.conf.js` (repo root) points
`appBinaryPath`/`application` at `./target/debug/redline` - this repo's package.json and
Cargo workspace both live at the repo root (unlike satchel-gui's sub-directory layout), so no
`../` is needed.

### `withGlobalTauri`

Getting a genuine WDIO session past the webview's default IPC surface requires
`app.withGlobalTauri: true` - `@wdio/tauri-service`'s window-tracking JS calls
`window.__TAURI__.core.invoke(...)` directly rather than falling back to the ESM
`@tauri-apps/api` bridge the rest of this app's own code uses.

**This is NOT set in the committed `src-tauri/tauri.conf.json`** (it stays `false`, matching
the shipped default). `.github/workflows/build-releases.yml` builds production installers
straight from that file - flipping the base config would ship the full `window.__TAURI__` IPC
surface to every installer permanently, which is exactly the exposure Tauri's own hardening
guidance keeps this flag off for by default. Instead, the override lives in
`src-tauri/tauri.e2e.conf.json` and is merged in only by `npm run e2e:build` via Tauri CLI's
`--config` flag (JSON Merge Patch). A plain `cargo tauri build`/`cargo build -p redline` (every
other build path in this repo, incl. CI and real releases) sees `withGlobalTauri: false`,
unchanged.

### Fixture opening — no native dialogs

Spec 2 opens `e2e/fixtures/e2e-sample.pdf` (a 1-page, ~1.2 KB fixture, generated with
PyMuPDF - regenerate with the snippet below if it ever needs to change) through the app's
**pre-existing** `REDLINE_OPEN_PDF` auto-open path (`commands::diag::auto_open_path` /
`App.svelte`'s `autoOpenIfRequested()`, already documented in CLAUDE.md's `cargo tauri dev`
command line as the "§20 GUI smoke / floor-machine runbook" mechanism). `wdio.conf.js` passes
it via the embedded provider's `services[].env` option, which merges into the **spawned app
process's** environment (`@wdio/tauri-service` 1.3.0, `dist/esm/index.js` — the
`spawnTauriApp(appBinaryPath, appArgs, { ...process.env, ...options.env })` call). This drives
the exact same code path a real "File > Open" would (`openFilePath()`), minus the native OS
file-picker dialog, which WebDriver cannot reach (it runs outside the webview).

```python
import fitz  # pymupdf
doc = fitz.open()
page = doc.new_page(width=612, height=792)  # US Letter
page.insert_text((72, 72), "Redline WDIO E2E fixture", fontsize=18)
page.insert_text((72, 100), "Single page, used only by the Tier-2 real-app WebdriverIO harness.", fontsize=10)
page.draw_rect(fitz.Rect(72, 140, 300, 260), color=(0, 0, 0), width=1)
doc.save("e2e/fixtures/e2e-sample.pdf")
```

### The S2b activation gate, and this dev Mac's real activation

`src/components/ActivationGate.svelte` blocks **all** app content - including
`REDLINE_OPEN_PDF`'s auto-open - until `license_status` resolves `"valid"` or `"grace"` for
the specific device running the binary (`App.svelte`'s `maybeInitializeAppContent` gates on
`isUsable(licenseState)`). There is no debug/test bypass for this in the codebase - it calls a
real production license service (`REDLINE_LICENSE_API_URL`, staff.emittiv.studio) to verify or
claim a device-bound token.

**As of 2026-08-28, this dev Mac IS activated.** `e2e/specs/app-launch.spec.js`'s `before()`
hook enters a real, owner-issued activation code through the **actual `ActivationGate` DOM**
(fills `#activation-code`, clicks `.gate-submit`) when three things are all true: the gate is
showing, `REDLINE_E2E_ACTIVATION_CODE` is set in the shell environment `npm run e2e` is
launched from, and the app isn't already licensed. This is a real `activate_license` Tauri
IPC call against the real production service - not a test-only hook, not a mock. On success
the resulting token is written by the app's own `store::save` to
`~/Library/Application Support/com.emittiv.redline/license/activation.json`, the same path
and mechanism a human activating through the real UI would produce. **Re-licensing note:**
because that token now exists and persists across runs, `npm run e2e` on this Mac no longer
exercises the activation code-entry path at all - it goes straight to `before()`'s
"already licensed" branch. To re-exercise the activation UI path itself, delete
`~/Library/Application Support/com.emittiv.redline/license/activation.json` first (this
does NOT free the license seat server-side - it only clears this machine's local copy).

`REDLINE_LICENSE_API_URL` also has to be set in the **spawned app process's** environment for
any of this to work at all in a debug e2e build - `wdio.conf.js` pins it to
`https://staff.emittiv.studio` (same value `.github/workflows/build-releases.yml` bakes into
release binaries) so no per-machine setup step is needed; see the comment in `wdio.conf.js`
for why a debug build has neither the runtime env var nor the compile-time default without it.

### 2026-08-29 update: activation crash fixed; a real harness-plumbing gap found and fixed; the evaluate_js hang eliminated via a DirectEval rewrite (spec 1 now reliably passes)

Two rounds of work happened between the paragraph above (2026-08-28's "spec 1 passes
reliably") and now. The 2026-08-28 claim is **no longer accurate as written** - re-running
`npm run e2e` today reproduces a full hang, not a pass, for a completely different reason than
the PDFium one below. Read this section, not the paragraph above, for current status.

**Round 1 - a real crash, root-caused and fixed.** The prior session's final `npm run e2e` run
crashed the binary outright (SIGABRT), leaving the suite mid-hang rather than at a clean pass.
Root cause, confirmed by reading `fern` 0.7.1's own source
(`fern-0.7.1/src/log_impl.rs:883-908`, vendored at
`~/.cargo/registry/src/index.crates.io-.../fern-0.7.1`): `fern::log_impl::backup_logging`
tries to report a failed log write to stderr, and **panics** if that stderr write also fails.
Under `@wdio/tauri-service`'s piped-stdio child-process spawn, a stdout write can EPIPE; when
its stderr fallback ALSO EPIPEs, fern panics once - which our own panic hook
(`src-tauri/src/panic_guard.rs`) caught and, in its old form, reported by calling `log::error!`
again, re-entering the SAME broken `fern::Dispatch` and triggering the identical failure a
second time. A panic raised while a thread is already panicking is unconditionally fatal in
Rust (`process::abort`) regardless of the `panic` profile setting - that is the SIGABRT seen in
`~/Library/Logs/DiagnosticReports/redline-2026-08-28-163301.ips`. Fixed by wrapping both of the
hook's own reporting attempts (`log::error!` and `eprintln!`) in `catch_unwind` via a new
`run_without_escalating` helper, so a nested failure while stdio is broken is merely
unreported, never a second, process-ending panic. Five new unit tests in `panic_guard.rs`
cover this directly, including one that reproduces the exact "Drop-during-unwind triggers a
second panic" shape and asserts the process survives. `cargo test -p redline` (515 lib tests,
including all 9 in `panic_guard::tests`), `cargo clippy --workspace --all-targets -D warnings`,
and `cargo test --workspace` all green with the fix in place. This was NOT "create the log
directory" (the originally-suspected cause) - `fern`'s `finish_logging` loop has no error
containment across outputs, so a File target would not have prevented the abort either; the
fix had to be in how the panic hook itself reports, not in what it logs to.

**Round 2 - "Tauri core.invoke not available after 5s timeout", fixed.** With the crash fixed,
every `npm run e2e` command failed identically and immediately with
`@wdio/tauri-service`'s `ensureActiveWindowFocus` warning
`Failed to get window states: Error: Tauri core.invoke not available after 5s timeout`, on
every single poll for the whole test duration (not intermittent - 100% of attempts). Traced to
the exact line in `@wdio/tauri-service`'s bundled `dist/esm/index.js` that throws this
(`window.__wdio_original_core__` never getting set) and from there to `@wdio/tauri-plugin`'s
guest-js, which is what captures that reference - **the frontend package was never installed
or imported**. `@wdio/tauri-service`'s own setup docs (`node_modules/@wdio/tauri-service/docs/
quick-start.md` and `plugin-setup.md`, "Tauri Plugin Setup") name three required pieces for
this to work; only one was previously in place:
1. `withGlobalTauri: true` - already present, but only in `src-tauri/tauri.e2e.conf.json`
   (correctly e2e-scoped, not touching the release `tauri.conf.json`).
2. `wdio:default` permission in `src-tauri/capabilities/default.json` - **missing**, added.
3. `import '@wdio/tauri-plugin'` in the frontend entry point - **missing entirely**; the
   package was not even an npm dependency. Added as a new devDependency and imported in
   `src/main.ts`, gated on `import.meta.env.MODE === "e2e"` (NOT plain `import.meta.env.DEV` -
   both `tauri:build`'s release build and `e2e:build`'s debug build invoke the same
   `beforeBuildCommand`, a bare `vite build`, which always resolves `MODE` to `"production"`
   regardless of the Tauri side's `--debug` flag; only `tauri.e2e.conf.json`'s new
   `build.beforeBuildCommand: "vite build --mode e2e"` override makes the `"e2e"` mode - and
   therefore the gate - true). Verified both directions by grepping the built bundles: the
   e2e-mode bundle contains `__wdio_original_core__` (from `@wdio/tauri-plugin`'s guest-js);
   the plain `npm run build` production bundle - same content hash as before this change,
   `index-CiKNimLK.js` - contains neither the string nor a changed chunk, so the import is
   fully dead-code-eliminated out of anything a user would ever run. Added `core:window:default`
   to the same capability at the same time (also named as required in the setup docs, also
   inert in a release build since `tauri_plugin_wdio`/`tauri_plugin_wdio_webdriver` are never
   `.plugin()`-registered outside `#[cfg(debug_assertions)]` in `src-tauri/src/lib.rs`).
   Installing `@wdio/tauri-plugin@1.3.0` also surfaced a real, separate version-skew guard
   failure - it depends on `@tauri-apps/plugin-log@2.9.0`, which npm's resolver hoisted over
   the project's existing `@tauri-apps/plugin-log@^2.8.0`, moving the JS package out of sync
   with the Rust `tauri-plugin-log` crate (still on a stale `"2.0.0-rc"` requirement, resolving
   to 2.8.0). `tauri build` refuses to proceed on a JS/Rust plugin version mismatch
   (`Found version mismatched Tauri packages`) - fixed by bumping `Cargo.toml`'s
   `tauri-plugin-log` to `"2.9.0"` and re-resolving `Cargo.lock`
   (`cargo update -p tauri-plugin-log --precise 2.9.0`); `cargo build`/`test`/`clippy` all still
   green afterward. **Result, verified**: re-running `npm run e2e` afterward shows **zero**
   `Failed to get window states` warnings (down from 100% of poll attempts), and
   `~/Library/Logs/com.emittiv.redline/Redline.log` shows real
   `[tauri_plugin_wdio::commands][DEBUG] [get_window_states] main: title='Redline',
   visible=true, focused=true` responses - this specific, structural harness-setup gap is
   closed.

**Round 3 - a deeper hang, root-caused but NOT fixed in this PR; specs 1-3 do NOT currently
pass.** Even with rounds 1 and 2 fixed, `npm run e2e` still times out identically on every
attempt (reproduced 4/4 consecutive runs on this Mac, so this is deterministic here, not
one-off WebKit flakiness) with the same `before()`-hook error as before either fix:
`neither the S2b ActivationGate (h1.gate-title) nor the empty-state document shell
(.empty-state) appeared`. The evidence points to a genuine limitation inside the
**third-party** `tauri-plugin-wdio-webdriver` 1.3.0 crate, not to anything in this repo's own
`src-tauri` code:
- `browser.$(selector).isExisting()` (what the failing `before()` wait uses) round-trips
  through the crate's standard WebDriver `POST /session/:id/element` route
  (`src/server/handlers/element.rs::find`), whose default `find_element` trait method
  (`src/platform/executor.rs:206`) calls `self.evaluate_js(...)`. On macOS
  (`src/platform/macos.rs:204`), `evaluate_js` uses the plain
  `WKWebView.evaluateJavaScript(_:completionHandler:)` API, bounded by a
  `tokio::time::timeout` of `Timeouts::default().script_ms` = **30 000 ms**
  (`src/webdriver/session.rs:26-40`).
- `~/Library/Logs/com.emittiv.redline/Redline.log` shows **zero** log lines of any kind during
  the entire ~60s `before()` wait (only `axum::serve` TCP-accept traces, no
  `tauri_plugin_wdio::commands` activity) - consistent with the `evaluateJavaScript`
  completion handler never firing at all, twice (60 000 ms / 30 000 ms per attempt = 2), which
  is exactly the observed total wall-clock time before failure.
- By contrast, `browser.tauri.execute(...)` (DirectEval, used for `get_window_states` above,
  and the mechanism Round 2 fixed) uses a **different** WKWebView API -
  `callAsyncJavaScript` plus an out-of-band `WKScriptMessageHandler` post
  (`macos.rs::execute_direct_eval`, `src/platform/macos.rs:365-420`) - and that path
  demonstrably DOES complete (proof: the real `get_window_states` responses quoted in Round 2).
  The crate's own code comments already document `callAsyncJavaScript`'s completion being
  intermittently reclaimed by WebKit on macOS 26.4+ (linking
  `https://github.com/webdriverio/desktop-mobile/issues/540`) and ship dedicated
  retry/reclaim-recovery logic for exactly that API - `evaluate_js`'s plain
  `evaluateJavaScript:completionHandler:` path has no equivalent recovery, and (on this Mac,
  macOS 26.6.2) appears not to complete at all when the app is launched as a background/
  non-interactive child process, rather than merely "intermittently reclaimed".
- **Not attempted in this PR**: rewriting `e2e/specs/app-launch.spec.js` to route its element
  queries through `browser.tauri.execute(() => ...)` (DirectEval, proven working) instead of
  `browser.$()`/`isExisting()`/`waitForDisplayed()` (evaluate_js, proven hanging) would very
  plausibly unblock spec 1, but spec 3's real pointer-drag drawing
  (`browser.action("pointer", ...)`) is a separate WebDriver Actions-API endpoint whose
  implementation was not audited here, and a full rewrite of this file's interaction model is
  a materially larger, riskier change than this PR's scope (activation + the crash fix) - it is
  flagged here as the concrete, well-evidenced next step rather than attempted speculatively
  against a session budget that has already been interrupted once by a usage limit.

**Bottom line at the end of the 2026-08-29 (earlier) session**: the crash is fixed and proven
(unit tests, quoted above). The "core.invoke not available" harness-setup gap is fixed and
proven (zero warnings, real `get_window_states` responses, quoted above). Specs 1-3 still did
**not** produce a real pass on this Mac as of that point - the blocker was narrowly the
`evaluate_js`/`find_element` hang described in Round 3, not the crash, not licensing, and not
(as far as the crash-fixed, that session's evidence showed) the PDFium gap described below,
which was never re-reached that session because `before()` never got past its own wait.

**Round 4 - the DirectEval rewrite, done, and it eliminates the hang.** `e2e/specs/app-launch.spec.js`
was rewritten to remove every standard WebdriverIO element/action command
(`browser.$()`, `.isExisting()`, `.getText()`, `.getAttribute()`, `.click()`, `.setValue()`,
`.action("pointer", ...)`, and the sync `browser.execute()`) and replace them with
`browser.tauri.execute()` calls - the plugin's DirectEval primitive (`callAsyncJavaScript` on
macOS, proven working since Round 2's `get_window_states`), used in preference to
`browser.executeAsync()` because it additionally carries the crate's own reclaim-retry logic for
a documented cold-webview WebKit flake (`node_modules/@wdio/tauri-service`'s `execute()`, a
4-attempt retry loop keyed on the *opaque* `"A JavaScript exception occurred"` error only - a
real script error still fails immediately). `browser.tauri.execute((tauri, ...args) => { ... })`
runs the callback as a real async IIFE inside the actual page (confirmed by reading
`wrapScriptForDirectEval` in `@wdio/tauri-service`'s bundled source: `await (script)(__wdio_tauri,
...__wdio_args)`), so it has full `document`/`window` access, not just Tauri's `core.invoke` -
every element query, click, and value-set in the spec now happens via plain DOM APIs
(`document.querySelector`, `.click()`, the native `HTMLInputElement.prototype.value` setter +
a dispatched `input` event) inside these callbacks, and the rectangle-markup drag dispatches a
real `PointerEvent` down/move.../up sequence directly on `svg.markup-overlay` from inside one
`browser.tauri.execute()` call (coordinates from the element's own `getBoundingClientRect()`,
computed in-page). `browser.waitUntil()` still wraps these for polling - it's plain Node-side
control flow, not a WebDriver command, so it was never part of the hang.

**Verified, this session, 4/4 consecutive `npm run e2e` runs against the as-shipped
`wdio.conf.js` (no config changes needed for this fix)**: spec 1 ("launches and reaches a real
terminal UI state") passes reliably in ~30s every time - zero hangs, zero
`ensureActiveWindowFocus` "Tauri core.invoke not available" warnings. This is the concrete
proof the `evaluate_js` hang class described in Round 3 is closed: the exact same `before()`
wait that timed out at 60-140s on every attempt before this rewrite now resolves in well under
a second of actual polling. The rectangle-drag `PointerEvent` sequence in spec 3 was also
proven end-to-end in an ad-hoc run with PDFium available (see below) - `setPointerCapture`
inside `Viewport.svelte`'s real `onOverlayPointerDown` handler accepted the synthetic,
in-page-dispatched pointer session without incident, and the drag produced a real persisted
`Rectangle` markup confirmed via `list_markups` over real IPC.

Specs 2 and 3 still fail on this Mac's *default* `npm run e2e` (no PDFium bundled - see below) -
but they now fail **fast and deterministically on a real, named, pre-existing error** (a real
`.error-banner` reading "PDFium dynamic library not found", 4/4 runs, ~30s each), never on a
hang. That distinction is the actual deliverable of this round: the harness now tells the truth
about what's broken instead of timing out uninformatively.

### PDFium in the e2e debug build (2026-08-28 finding; RE-TESTED 2026-08-29 under the fixed harness - the regression is real and still open)

The previous session's runs, before either the crash or the harness-setup gap above were
found, got far enough to reach specs 2/3 and observe:

```
REDLINE E2E DIAG: canvas not displayed after 20s - .error-banner exists=true
text=PDFium init failed: PDFium dynamic library not found. Set PDFIUM_DYNAMIC_LIB_PATH or
place libpdfium alongside the binary. See bench/README.md for setup steps. ...
```

`tauri build --no-bundle` (what `npm run e2e:build` runs) never populates the resource
directory `src-tauri/src/lib.rs`'s `resolve_pdfium_path` looks in, because resource-copying is
part of the bundling step `--no-bundle` explicitly skips - this is a pre-existing, licensing-
unrelated gap in the e2e debug-build path. Setting `PDFIUM_DYNAMIC_LIB_PATH` to a manually-copied
`src-tauri/resources/libpdfium.dylib` DOES clear that specific error - confirmed again this
session, one clean 3/3-passing run in 261ms with it set, all three specs green including the
real pointer-drag.

**But it is flaky, and Round 3's open question is now answered: it is NOT simply the same
`evaluate_js` bug fixed above.** Four consecutive runs with `PDFIUM_DYNAMIC_LIB_PATH` set (this
session, against the *already-fixed* DirectEval harness): 1 pass, 3 failures - and the 3
failures reproduce the exact same "worse regression" the 2026-08-28 finding described, `before()`
timing out at the full 60s with **neither** `h1.gate-title` nor `.empty-state` ever appearing,
i.e. the webview never rendering any real content at all. If this were downstream of the
`evaluate_js` hang, the DirectEval rewrite above would have closed it the same way it closed
spec 1 (4/4 clean, zero flakiness) - it did not. This is a **separate, still-open, genuinely
intermittent regression** in how the embedded provider's WebKit spawn interacts with a
PDFium-loading binary specifically, not resolved by this PR and out of this PR's scope (which
was the WebDriver interaction-model rewrite). Deliberately left unset in `wdio.conf.js` as
before, with this finding recorded rather than "fixed" with an override that fails 3 times out
of 4. Follow-up (still untried): `tauri build --debug` (no `--no-bundle`) for the e2e binary,
which should populate the resource dir the normal way and may sidestep
`PDFIUM_DYNAMIC_LIB_PATH` (and this regression) entirely - not attempted here since it changes
what `e2e:build` produces, beyond this PR's scope.

`data-doc-id` on `Viewport.svelte`'s `.viewport-root` is a one-line, purely additive
attribute added by this harness so spec 3 can address `list_markups` against the real open
document without inventing an untested global - it carries no behavior.

### Not wired into CI

Deliberately, matching `satchel-gui`: the Forgejo runner fleet (Primary/AI/NUC-worker, all
Unraid-hosted Linux containers) has no macOS runner, and the embedded provider's macOS support
is the point of this tier - a Linux CI leg would exercise `webkit2gtk-driver`/`xvfb`, not
WKWebView, and wouldn't validate what this tier is for.
