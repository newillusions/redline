import App from "./App.svelte";
import { mount } from "svelte";

// WDIO Tauri E2E harness (Tier-2 real-app tests, see wdio.conf.js + docs/TESTING.md).
// `@wdio/tauri-plugin`'s guest-js captures a reference to the real Tauri `core` object
// (as `window.__wdio_original_core__`) that `@wdio/tauri-service`'s embedded provider
// needs for every DirectEval command - without this import, `window.__wdio_original_core__`
// is never set and EVERY wdio command fails with "Tauri core.invoke not available after 5s
// timeout" regardless of whether the app itself is working (root-caused 2026-08-29: the app
// booted, licensed, and rendered correctly the whole time - this one missing import was the
// entire blocker).
//
// Gated on `import.meta.env.MODE === "e2e"`, NOT plain `import.meta.env.DEV` - `tauri build`
// (both `tauri:build`'s release build and `e2e:build`'s debug build) runs its frontend step
// via `beforeBuildCommand`, which is `vite build` in EITHER case; a bare `vite build` always
// resolves `MODE` to `"production"` (so `DEV` is always false) regardless of `--debug` on the
// tauri side - only `e2e:build`'s dedicated `--mode e2e` override (tauri.e2e.conf.json's
// `build.beforeBuildCommand`) makes this condition true. Vite statically replaces
// `import.meta.env.MODE` at build time, so the `production` build's `if` is dead code and
// `@wdio/tauri-plugin` never reaches that bundle - the intended mirror of the Rust-side
// `#[cfg(debug_assertions)]` gate on the matching wdio plugins in src-tauri/src/lib.rs.
if (import.meta.env.MODE === "e2e") {
  import("@wdio/tauri-plugin");
}

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
