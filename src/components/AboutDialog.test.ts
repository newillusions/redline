// @vitest-environment jsdom
/**
 * AboutDialog tests - the owner-reported "no About page / can't find the
 * app version" gap, plus the update-check and dev-stage rollback
 * affordances it hosts.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor, within } from "@testing-library/svelte";
import AboutDialog from "./AboutDialog.svelte";
import type { ReleaseInfo } from "$lib/updater-rollback";

const mockGetVersion = vi.fn();
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => mockGetVersion(),
}));

const mockCheck = vi.fn();
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: () => mockCheck(),
}));

const mockRelaunch = vi.fn();
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: () => mockRelaunch(),
}));

const mockListReleases = vi.fn();
const mockRollback = vi.fn();
vi.mock("$lib/updater-rollback", () => ({
  listAvailableReleases: () => mockListReleases(),
  rollbackToVersion: (url: string, version: string) => mockRollback(url, version),
}));

function release(overrides: Partial<ReleaseInfo> = {}): ReleaseInfo {
  return {
    version: "0.3.12",
    pub_date: "2026-08-01T00:00:00Z",
    notes: "Release v0.3.12",
    manifest_url: "https://raw.githubusercontent.com/newillusions/redline/sha1/update.json",
    ...overrides,
  };
}

describe("AboutDialog", () => {
  beforeEach(() => {
    mockGetVersion.mockReset().mockResolvedValue("0.3.13");
    mockCheck.mockReset();
    mockRelaunch.mockReset();
    mockListReleases.mockReset().mockResolvedValue([release()]);
    mockRollback.mockReset().mockResolvedValue(undefined);
  });

  it("shows the live app version from getVersion(), not a hardcoded string", async () => {
    const { findByText } = render(AboutDialog, { props: { onClose: vi.fn() } });
    expect(await findByText("Version 0.3.13")).toBeTruthy();
  });

  it("Check for Updates reports up to date when check() resolves null", async () => {
    mockCheck.mockResolvedValue(null);
    const { findByRole, findByText } = render(AboutDialog, { props: { onClose: vi.fn() } });
    const btn = await findByRole("button", { name: /check for updates/i });
    await fireEvent.click(btn);
    expect(await findByText(/up to date/i)).toBeTruthy();
  });

  it("Check for Updates shows current -> target version when an update is found", async () => {
    mockCheck.mockResolvedValue({
      version: "0.3.14",
      currentVersion: "0.3.13",
      downloadAndInstall: vi.fn(),
    });
    const { findByRole, findByText } = render(AboutDialog, { props: { onClose: vi.fn() } });
    const btn = await findByRole("button", { name: /check for updates/i });
    await fireEvent.click(btn);
    expect(await findByText("v0.3.13 → v0.3.14 available.")).toBeTruthy();
  });

  it("lists past releases from listAvailableReleases()", async () => {
    mockListReleases.mockResolvedValue([release({ version: "0.3.12" }), release({ version: "0.3.11" })]);
    const { findAllByTestId } = render(AboutDialog, { props: { onClose: vi.fn() } });
    const items = await findAllByTestId("release-item");
    expect(items).toHaveLength(2);
  });

  it("rolling back asks for confirmation, then calls rollbackToVersion with the release's manifest URL", async () => {
    const { findByRole, getByRole } = render(AboutDialog, { props: { onClose: vi.fn() } });
    const rollbackBtn = await findByRole("button", { name: /roll back/i });
    await fireEvent.click(rollbackBtn);

    // ConfirmDialog should now be showing - scope to it so "Roll back" in the
    // list item behind it isn't matched too.
    const confirmDialog = await findByRole("dialog", { name: /roll back to v0\.3\.12\?/i });
    const confirmBtn = within(confirmDialog).getByRole("button", { name: /^roll back$/i });
    await fireEvent.click(confirmBtn);

    await waitFor(() =>
      expect(mockRollback).toHaveBeenCalledWith(
        "https://raw.githubusercontent.com/newillusions/redline/sha1/update.json",
        "0.3.12",
      ),
    );
    expect(getByRole("button", { name: /restart now/i })).toBeTruthy();
  });

  it("surfaces a rollback error instead of silently failing", async () => {
    mockRollback.mockRejectedValue(new Error("network unreachable"));
    const { findByRole, findByText } = render(AboutDialog, { props: { onClose: vi.fn() } });
    const rollbackBtn = await findByRole("button", { name: /roll back/i });
    await fireEvent.click(rollbackBtn);
    const confirmDialog = await findByRole("dialog", { name: /roll back to v0\.3\.12\?/i });
    const confirmBtn = within(confirmDialog).getByRole("button", { name: /^roll back$/i });
    await fireEvent.click(confirmBtn);
    expect(await findByText(/rollback failed: network unreachable/i)).toBeTruthy();
  });
});
