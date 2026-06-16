/**
 * Vitest global setup — loaded for ALL tests (both node + jsdom env).
 *
 * - Imports @testing-library/jest-dom matchers (only relevant in jsdom env,
 *   but harmless to import globally).
 * - Mocks @tauri-apps/api/core so any test that imports ipc.ts or a component
 *   which pulls in Tauri won't fail with "window.__TAURI__ not defined".
 * - Does NOT set up SurrealDB or unrelated mocks (redline doesn't use those).
 */
import "@testing-library/jest-dom";
import { vi, beforeEach, afterEach } from "vitest";

// Tauri v2 internals shim — needed so @tauri-apps/api/core can initialise in jsdom.
if (typeof window !== "undefined") {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: {
      transformCallback: vi.fn(),
      invoke: vi.fn().mockResolvedValue(null),
    },
    writable: true,
    configurable: true,
  });
}

// Mock Tauri core module — component tests mock $lib/ipc directly,
// but this safety net prevents any accidental real invoke() calls.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(null),
  transformCallback: vi.fn(),
}));

// Mock Tauri dialog plugin (not used by redline yet but imported transitively).
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
  save: vi.fn().mockResolvedValue(null),
  message: vi.fn().mockResolvedValue(null),
}));

beforeEach(() => {
  vi.clearAllMocks();
  if (typeof document !== "undefined" && document.body) {
    document.body.innerHTML = "";
  }
});

afterEach(() => {
  vi.clearAllMocks();
  if (typeof document !== "undefined" && document.body) {
    document.body.innerHTML = "";
  }
});
