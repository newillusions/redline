import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolve } from "path";

export default defineConfig({
  plugins: [
    svelte({
      hot: !process.env.VITEST,
      compilerOptions: {
        hydratable: false,
        dev: true,
      },
      emitCss: false,
      onwarn: (warning, handler) => {
        if (process.env.VITEST) return;
        handler(warning);
      },
    }),
  ],
  define: {
    "process.env.VITEST": JSON.stringify(process.env.VITEST || "true"),
  },
  ssr: {
    noExternal: process.env.VITEST ? [] : undefined,
  },
  test: {
    globals: true,
    // Default env: node (fast, for existing pure logic tests).
    // Component/interaction tests opt in per-file via:
    //   // @vitest-environment jsdom
    environment: "node",
    setupFiles: ["./src/tests/setup.ts"],
    include: ["src/**/*.{test,spec}.{js,ts}"],
  },
  resolve: {
    alias: {
      $lib: resolve(__dirname, "./src/lib"),
    },
    conditions: process.env.VITEST ? ["browser"] : undefined,
  },
});
