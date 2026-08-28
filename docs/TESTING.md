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

### The S2b activation gate blocks Tier 2 on an unlicensed device

`src/components/ActivationGate.svelte` blocks **all** app content - including
`REDLINE_OPEN_PDF`'s auto-open - until `license_status` resolves `"valid"` or `"grace"` for
the specific device running the binary (`App.svelte`'s `maybeInitializeAppContent` gates on
`isUsable(licenseState)`). There is no debug/test bypass for this in the codebase - it calls a
real production license service (`REDLINE_LICENSE_API_URL`, staff.emittiv.studio) to verify or
claim a device-bound token.

**As of the PR that added this harness, this dev Mac has never been activated** - no token
under `~/Library/Application Support/com.emittiv.redline/`, confirmed by that directory's
`recent-docs.json` predating the S2b gate's introduction (PR #49) entirely. Producing an
activation code is an owner-gated production action (consumes/creates a real device seat) and
was correctly not attempted by the agent that built this harness.

**What was proven, this PR:** `npm run e2e:build` compiles the debug binary with both wdio
plugins wired into `redline_lib`'s real `tauri::Builder` (`cargo build -p redline`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all green
with the two new deps present). `npm run e2e` launches the real compiled binary and
`@wdio/tauri-service` opens a genuine WebDriver session against it. Spec 1 passes for real,
asserting the app reaches the S2b `ActivationGate` (`h1.gate-title` = "Activate Redline") -
which is itself a real, meaningful regression signal: if the binary crashed, hung on
`about:blank`, or the license check itself broke, spec 1 would fail differently (timeout, or a
wrong heading text) rather than pass.

**What is NOT proven:** specs 2 and 3 (fixture opens and renders; a placed Rectangle persists
to `list_markups`) mark themselves `pending` via `this.skip()` when the gate is blocking -
never silently omitted, never reported as passed. Re-run `npm run e2e` from a session on a
licensed/grace device to get a real pass on those two. If they still fail there, the failure
is real and needs diagnosis from that state (start by checking `.empty-state` /
`canvas.tile-canvas` / `.viewport-root[data-doc-id]` in that order).

`data-doc-id` on `Viewport.svelte`'s `.viewport-root` is a one-line, purely additive
attribute added by this harness so spec 3 can address `list_markups` against the real open
document without inventing an untested global - it carries no behavior.

### Not wired into CI

Deliberately, matching `satchel-gui`: the Forgejo runner fleet (Primary/AI/NUC-worker, all
Unraid-hosted Linux containers) has no macOS runner, and the embedded provider's macOS support
is the point of this tier - a Linux CI leg would exercise `webkit2gtk-driver`/`xvfb`, not
WKWebView, and wouldn't validate what this tier is for.
