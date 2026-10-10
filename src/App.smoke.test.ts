// @vitest-environment jsdom
/**
 * App shell smoke test (UX audit, 2026-10-10): the toolbar groups, the narrow-window
 * overflow menu, and the hidden Apply Redactions item. App.svelte has no other test, so
 * this mounts it with the Tauri/IPC surface mocked out.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null), convertFileSrc: (p: string) => p }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: vi.fn(async () => () => {}) }),
}));
const onCloseRequested = vi.fn(async () => () => {});
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested, destroy: vi.fn(async () => {}) }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("$lib/license", async () => {
  const actual = await vi.importActual<typeof import("$lib/license")>("$lib/license");
  return {
    ...actual,
    getLicenseStatus: vi.fn(async () => ({ state: "valid", staff_id: "s", expires_at: "x", days_remaining: 30 })),
    checkInIfActivated: vi.fn(async () => null),
  };
});
vi.mock("$lib/ipc", async () => {
  const actual = await vi.importActual<typeof import("$lib/ipc")>("$lib/ipc");
  return new Proxy(actual, {
    get(target, prop) {
      const v = (target as Record<string | symbol, unknown>)[prop];
      return typeof v === "function" ? vi.fn(async () => []) : v;
    },
  });
});
vi.mock("$lib/settings", () => ({
  loadSettings: vi.fn(async () => ({ viewer_mode: "single", auto_ocr_on_open: false })),
  saveSettings: vi.fn(async () => {}),
}));
vi.mock("$lib/recent-docs", async () => {
  const actual = await vi.importActual<typeof import("$lib/recent-docs")>("$lib/recent-docs");
  return { ...actual, loadRecentDocs: vi.fn(async () => []), saveRecentDocs: vi.fn(async () => {}) };
});

import App from "./App.svelte";

function setWidth(w: number) {
  Object.defineProperty(window, "innerWidth", { value: w, configurable: true, writable: true });
}

beforeEach(() => {
  onCloseRequested.mockClear();
  setWidth(1440); // jsdom defaults to 1024, which is below the compact-toolbar breakpoint
});

describe("App toolbar", () => {
  it("registers the window close guard", async () => {
    render(App);
    await waitFor(() => expect(onCloseRequested).toHaveBeenCalledOnce());
  });

  it("the close guard lets the window close when no tab has unsaved mark-ups", async () => {
    render(App);
    await waitFor(() => expect(onCloseRequested).toHaveBeenCalled());
    const handler = (onCloseRequested.mock.calls[0] as unknown as [(e: { preventDefault: () => void }) => void])[0];
    const event = { preventDefault: vi.fn() };
    handler(event);
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("groups actions into File / Edit / Document / View on a wide window", async () => {
    const { findByRole, getByRole, getByTestId } = render(App);
    await findByRole("group", { name: "File" });
    expect(getByRole("group", { name: "Edit" })).toBeTruthy();
    expect(getByRole("group", { name: "View" })).toBeTruthy();
    expect(getByTestId("document-menu")).toBeTruthy();
    // The export action exists on the toolbar but is disabled with no document/mark-up.
    expect((getByTestId("export-markup-list") as HTMLButtonElement).disabled).toBe(true);
  });

  it("the two sidebar toggles are distinguishable (distinct accessible names)", async () => {
    const { findByRole } = render(App);
    const left = await findByRole("button", { name: "Toggle left sidebar" });
    const right = await findByRole("button", { name: "Toggle right sidebar" });
    expect(left).not.toBe(right);
  });

  it("hides Apply Redactions from the Document menu until a Redact tool exists", async () => {
    const { findByTestId, getByRole, queryByRole } = render(App);
    const menu = await findByTestId("document-menu");
    await fireEvent.click(menu.querySelector("button")!);
    await tick();
    expect(getByRole("menuitem", { name: /flatten annotations/i })).toBeTruthy();
    expect(getByRole("menuitem", { name: /optimise file size/i })).toBeTruthy();
    expect(queryByRole("menuitem", { name: /redactions/i })).toBeNull();
  });

  it("collapses into a More menu on a narrow window", async () => {
    setWidth(1000);
    const { findByTestId, queryByRole } = render(App);
    await findByTestId("more-menu");
    expect(queryByRole("group", { name: "View" })).toBeNull();
  });
});
