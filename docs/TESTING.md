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
