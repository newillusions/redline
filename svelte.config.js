import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// Minimal config so svelte-check (and the vite-plugin-svelte) can resolve preprocessing.
// SPA mode (no SvelteKit) — Tauri serves the built dist/ directly.
export default {
  preprocess: vitePreprocess(),
};
