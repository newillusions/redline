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
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
export const E2E_FIXTURE_PDF = join(__dirname, "e2e", "fixtures", "e2e-sample.pdf");

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
        env: { REDLINE_OPEN_PDF: E2E_FIXTURE_PDF },
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
    timeout: 60000,
  },

  reporters: ["spec"],
};
