// ESLint 10 flat config (config shape per eslint-plugin-svelte 3.x README, "TypeScript project").
import svelteConfig from "./svelte.config.js";
import { defineConfig, globalIgnores } from "eslint/config";
import globals from "globals";
import js from "@eslint/js";
import ts from "typescript-eslint";
import svelte from "eslint-plugin-svelte";

export default defineConfig(
  globalIgnores(["dist/**", "node_modules/**", "src-tauri/**", "target/**", ".worktrees/**"]),
  js.configs.recommended,
  ts.configs.recommended,
  svelte.configs.recommended,
  {
    languageOptions: {
      parserOptions: { extraFileExtensions: [".svelte"] },
      globals: { ...globals.browser },
    },
  },
  {
    rules: {
      // Leading underscore marks an intentionally unused parameter/variable/caught error.
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
      // This codebase keeps Set/Map values inside $state and replaces them wholesale
      // (`x = new Set(x).add(y)`); they are never mutated in place, so a plain Set is
      // correct and SvelteSet would only add proxy overhead. The rule flags every `new Set`
      // regardless of use, so it is off for this reason only (all other svelte/* rules stay on).
      "svelte/prefer-svelte-reactivity": "off",
    },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts", "**/*.svelte.js"],
    languageOptions: { parserOptions: { parser: ts.parser, svelteConfig } },
  },
);
