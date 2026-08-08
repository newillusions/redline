// @vitest-environment jsdom
/**
 * Regression test for the "update window shows the new version number
 * TWICE" report - the dialog previously rendered `Version {update.version}`
 * as its own label, then (when the manifest's `notes` field was the default
 * "Release v$VERSION" text every real release carries - see
 * .github/workflows/build-releases.yml's update-manifest job) the release
 * notes box repeated the SAME version number a second time, while the
 * CURRENT (installed) version - available on the Update object as
 * `currentVersion` - was never shown anywhere. The fix shows both: the
 * version being updated FROM and the version being updated TO, in one
 * distinct line.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/svelte";
import UpdateNotification from "./UpdateNotification.svelte";

const mockCheck = vi.fn();

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: (...args: unknown[]) => mockCheck(...args),
}));
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-log", () => ({
  info: vi.fn(),
  error: vi.fn(),
}));

function fakeUpdate(overrides: Partial<{ version: string; currentVersion: string; body: string | null }> = {}) {
  return {
    version: "0.3.14",
    currentVersion: "0.3.13",
    body: "Release v0.3.14",
    downloadAndInstall: vi.fn(),
    ...overrides,
  };
}

describe("UpdateNotification - current vs target version display", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockCheck.mockReset();
  });

  it("shows the CURRENT version and the TARGET version, not the target version twice", async () => {
    mockCheck.mockResolvedValue(fakeUpdate());
    const { findByText, queryAllByText } = render(UpdateNotification);

    await vi.advanceTimersByTimeAsync(3000);
    await waitFor(() => expect(mockCheck).toHaveBeenCalled());

    // The dialog-hint line must name BOTH versions, distinctly.
    await findByText((_, el) => el?.textContent === "v0.3.13 → v0.3.14");

    // The old bug: "0.3.14" (the new version) appeared as its own standalone
    // label AND again in the notes box, while "0.3.13" (current) never
    // appeared anywhere. Assert the current version is now visible at least
    // once, and the target version isn't duplicated as a bare "Version X"
    // label distinct from the from-to line and the notes prose.
    expect(await findByText(/0\.3\.13/)).toBeTruthy();
    const bareVersionLabels = queryAllByText(/^Version 0\.3\.14$/);
    expect(bareVersionLabels).toHaveLength(0);
  });
});
