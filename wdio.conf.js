// Tier-2 real-app E2E harness (WebdriverIO + @wdio/tauri-service, embedded provider).
// Drives the ACTUAL compiled redline binary via real Tauri IPC/commands and a real
// webview - no mocked backend (the mocked-IPC harness is tools/gui-harness.mjs, a
// different tier). See docs/TESTING.md for setup, what this proves, and what it can't.
// Pattern + config copied verbatim from satchel-gui's wdio.conf.js (forge.mms.name/
// emittiv/satchel PR #25), adapted for redline's single-package (not sub-directory) layout.
//
// Requires, before running `npm run e2e`:
//   npm run e2e:build   (runs `tauri build --no-bundle --debug --config
//                        src-tauri/tauri.e2e.conf.json` - builds the frontend, then produces
//                        the debug binary this config points appBinaryPath at, with
//                        app.withGlobalTauri merged to true FOR THIS BUILD ONLY. The
//                        committed src-tauri/tauri.conf.json keeps withGlobalTauri false -
//                        flipping it unconditionally would ship the global __TAURI__ IPC
//                        surface in every production installer. See docs/TESTING.md.)
//
// REDLINE_OPEN_PDF: the embedded provider merges `services[].env` into the SPAWNED APP
// PROCESS's environment (verified against @wdio/tauri-service 1.3.0's
// dist/esm/index.js:1606-1632 - `spawnTauriApp(appBinaryPath, appArgs, { ...process.env,
// ...options.env })`), so setting it here drives the app's real pre-existing "§20 GUI
// smoke / floor-machine runbook" auto-open path (App.svelte's `autoOpenIfRequested()` ->
// `openFilePath()`, backed by the `auto_open_path` Tauri command reading this exact env
// var) - the real "File > Open" code path, minus the native OS file-picker dialog that
// WebDriver cannot drive. Auto-open only fires once the S2b license gate resolves
// valid/grace (App.svelte's `maybeInitializeAppContent`) - see docs/TESTING.md for what
// this harness could and couldn't prove on an unlicensed dev machine.
//
// REDLINE_LICENSE_API_URL: `src-tauri/src/license/client.rs`'s three-tier resolution only
// bakes a default (`REDLINE_LICENSE_API_URL_DEFAULT`) into RELEASE builds
// (.github/workflows/build-releases.yml sets it at build time); a debug build made by
// `npm run e2e:build` has neither the baked default nor the runtime env var, so any real
// `activate_license`/`renew_license` IPC call fails with `ClientError::NotConfigured`
// ("License service is not configured") before it ever reaches the network - confirmed by
// running this harness without this line and reading the resulting `.gate-error` text.
// Since `...process.env` is already merged in below the `services[].env` override, this is
// only a fallback for developers who haven't exported it themselves - but the value itself
// is a fixed, public hostname (not a secret), so pinning it here means every debug e2e run
// reaches the same real production license service `build-releases.yml` bakes into release
// binaries, with no per-machine setup step.
//
// PDFIUM_DYNAMIC_LIB_PATH - KNOWN GAP, deliberately NOT set here (see docs/TESTING.md "PDFium
// is not resolved..."): `tauri build --no-bundle` (what `npm run e2e:build` runs) never
// populates the resource dir PDFium normally loads from, so specs 2/3 (fixture render, markup
// placement) currently fail with a real `.error-banner` reading "PDFium init failed" rather
// than exercising the render path. Setting PDFIUM_DYNAMIC_LIB_PATH here to point at a manually
// copied `src-tauri/resources/libpdfium.dylib` DOES fix that error - but empirically
// (2026-08-28, isolated by toggling this one var across four otherwise-identical runs)
// introduces a WORSE regression: the app then never reaches ANY known state
// (`h1.gate-title`/`.empty-state`) within a 60-140s window under @wdio/tauri-service's
// embedded WebKit spawn, despite launching instantly (PDFium loads, webview keys, license
// check-in fires) when the identical binary + env is run directly from a Terminal `exec`
// outside WDIO. Root cause not yet isolated (suspected: some interaction between the
// freshly-resolved dylib load path and how the embedded provider's Node child_process spawn
// vs. a real TTY exec engages macOS's library-validation/Gatekeeper path - unconfirmed).
// Chasing it further was out of scope for the S2b activation work this file's other env vars
// support. Left named and reproducible rather than "fixed" with an env var that breaks
// something worse.
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
export const E2E_FIXTURE_PDF = join(__dirname, "e2e", "fixtures", "e2e-sample.pdf");
const REDLINE_LICENSE_API_URL =
  process.env.REDLINE_LICENSE_API_URL || "https://staff.emittiv.studio";

export const config = {
  runner: "local",

  specs: ["./e2e/specs/**/*.spec.js"],

  maxInstances: 1,
  maxInstancesPerCapability: 1,

  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath: "./target/debug/redline",
        driverProvider: "embedded",
        captureBackendLogs: true,
        captureFrontendLogs: true,
        startTimeout: 60000,
        commandTimeout: 30000,
        env: {
          REDLINE_OPEN_PDF: E2E_FIXTURE_PDF,
          REDLINE_LICENSE_API_URL,
          // PDFIUM_DYNAMIC_LIB_PATH intentionally NOT set - see the comment block above.
        },
      },
    ],
  ],

  capabilities: [
    {
      browserName: "tauri",
      "tauri:options": {
        application: "./target/debug/redline",
      },
    },
  ],

  logLevel: "warn",
  bail: 0,
  waitforTimeout: 15000,
  connectionRetryTimeout: 90000,
  connectionRetryCount: 3,

  framework: "mocha",
  mochaOpts: {
    ui: "bdd",
    // 60000 was the original value; raised for the S2b activation flow (spec 1's before()
    // hook), which needs headroom for a real production license-service round trip PLUS
    // @wdio/tauri-service's per-command window-focus check - it calls
    // `plugin:wdio|get_window_states` before every `$`/`elementClick`, which never resolves
    // against this app's single-window embedded WebKit session (caught internally, logged
    // as "Tauri core.invoke not available after 5s timeout", non-fatal) and adds ~5s of
    // latency to each such command. Several commands in sequence (waitUntil polling,
    // setValue, click, a second waitUntil) compounded past the old 60s default in practice.
    timeout: 180000,
  },

  reporters: ["spec"],
};
