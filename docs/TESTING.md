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

### 2026-08-29 update: activation crash fixed; a real harness-plumbing gap found and fixed; a deeper upstream hang remains open

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

**Bottom line for whoever reads this next**: the crash is fixed and proven (unit tests, quoted
above). The "core.invoke not available" harness-setup gap is fixed and proven (zero warnings,
real `get_window_states` responses, quoted above). Specs 1-3 still do **not** produce a real
pass on this Mac as of this PR - the blocker is now narrowly the `evaluate_js`/`find_element`
hang described in Round 3, not the crash, not licensing, and not (as far as the crash-fixed,
this-session evidence shows) the PDFium gap described below, which was never re-reached this
session because `before()` never gets past its own wait.

### PDFium in the e2e debug build (2026-08-28 finding, unconfirmed re-reachable this session)

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
unrelated gap in the e2e debug-build path. Setting `PDFIUM_DYNAMIC_LIB_PATH` in
`wdio.conf.js`'s spawned-app `env` to a manually-copied `src-tauri/resources/libpdfium.dylib`
DOES clear that specific error - but was found (2026-08-28, isolated across four otherwise-
identical runs by toggling only that one var) to introduce a WORSE regression: with it set,
the app never reaches ANY known UI state (`h1.gate-title` / `.empty-state`) within a 60-140s
window under `@wdio/tauri-service`'s embedded WebKit spawn, despite the identical binary + env
launching and reaching a working webview in under a second when run directly via a Terminal
`exec` outside WDIO. **Given Round 3 above, this may well be the SAME underlying `evaluate_js`
hang rather than a distinct PDFIUM_DYNAMIC_LIB_PATH-specific regression** - both produce an
identical symptom ("never reaches any known UI state" within the same time order of
magnitude) - but this was not re-tested this session since spec 1 never got that far.
Deliberately left unset with the regression named in `wdio.conf.js`'s own comment, rather than
"fixed" with an override that trades one real failure for a worse one. Follow-up (untried): try
`tauri build --debug` (no `--no-bundle`) for the e2e binary, which should populate the
resource dir the normal way and may sidestep the whole PDFIUM_DYNAMIC_LIB_PATH question -
not attempted here since it changes what `e2e:build` produces beyond this PR's scope.

`data-doc-id` on `Viewport.svelte`'s `.viewport-root` is a one-line, purely additive
attribute added by this harness so spec 3 can address `list_markups` against the real open
document without inventing an untested global - it carries no behavior.

### Not wired into CI

Deliberately, matching `satchel-gui`: the Forgejo runner fleet (Primary/AI/NUC-worker, all
Unraid-hosted Linux containers) has no macOS runner, and the embedded provider's macOS support
is the point of this tier - a Linux CI leg would exercise `webkit2gtk-driver`/`xvfb`, not
WKWebView, and wouldn't validate what this tier is for.
