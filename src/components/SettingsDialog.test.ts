// @vitest-environment jsdom
/**
 * SettingsDialog "License" section tests (Part B of the 2026-08-05 licensing
 * rework - "a way to see / edit / remove the license code from within the
 * app settings"). Pre-existing theme/tool/unit/author fields are untouched
 * by this change and are not re-tested here.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import SettingsDialog from "./SettingsDialog.svelte";
import type { AppSettings } from "$lib/settings";
import type { LicenseInfo } from "$lib/license";

const FAKE_SETTINGS: AppSettings = {
  theme: "dark",
  default_tool: null,
  measurement_unit: "m",
  author_name: "",
  last_window: null,
  recent_colors: [],
};

vi.mock("$lib/settings", () => ({
  loadSettings: vi.fn(() => Promise.resolve(FAKE_SETTINGS)),
  saveSettings: vi.fn(() => Promise.resolve()),
  withDefaults: (s: AppSettings) => s,
}));

const mockGetLicenseInfo = vi.fn();
const mockActivateLicense = vi.fn();
const mockDeactivateLicense = vi.fn();

vi.mock("$lib/license", () => ({
  getLicenseInfo: () => mockGetLicenseInfo(),
  activateLicense: (code: string) => mockActivateLicense(code),
  deactivateLicense: () => mockDeactivateLicense(),
}));

function validInfo(overrides: Partial<LicenseInfo> = {}): LicenseInfo {
  return {
    code: "ABCD-1234",
    device_fingerprint: "device-xyz",
    state: { state: "valid", staff_id: "staff:abc123", expires_at: "2099-01-01T00:00:00Z", days_remaining: 10 },
    ...overrides,
  };
}

describe("SettingsDialog - License section", () => {
  beforeEach(() => {
    mockGetLicenseInfo.mockReset().mockResolvedValue(validInfo());
    mockActivateLicense.mockReset();
    mockDeactivateLicense.mockReset();
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
  });

  it("shows the activation code and a human-readable active state", async () => {
    const { findByText, getByDisplayValue } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    await findByText(/active/i);
    expect(getByDisplayValue("ABCD-1234")).toBeTruthy();
  });

  it("copies the code to the clipboard", async () => {
    const { findByRole, getByRole } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    await findByRole("button", { name: /copy/i });
    await fireEvent.click(getByRole("button", { name: /copy/i }));
    await tick();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("ABCD-1234");
  });

  it("shows a grace-period description with a concrete day count", async () => {
    mockGetLicenseInfo.mockResolvedValue(
      validInfo({
        state: {
          state: "grace",
          staff_id: "staff:abc123",
          expired_at: "2026-08-01T00:00:00Z",
          grace_deadline: "2026-08-08T00:00:00Z",
          days_remaining: 4,
        },
      }),
    );
    const { findByText } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    expect(await findByText(/4 day/i)).toBeTruthy();
  });

  it("shows the revoked reason when the license was revoked", async () => {
    mockGetLicenseInfo.mockResolvedValue(
      validInfo({ code: "ABCD-1234", state: { state: "revoked", reason: "staff_not_active" } }),
    );
    const { findByText } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    expect(await findByText(/staff_not_active/)).toBeTruthy();
  });

  it("re-activates with a new code and reports it via onLicenseChanged", async () => {
    mockGetLicenseInfo.mockResolvedValueOnce(
      validInfo({ code: "OLD-CODE", state: { state: "expired" } }),
    );
    const fresh: LicenseInfo = validInfo({ code: "NEW-CODE" });
    mockActivateLicense.mockResolvedValue(fresh.state);
    mockGetLicenseInfo.mockResolvedValueOnce(fresh);

    const onLicenseChanged = vi.fn();
    const { findByPlaceholderText, getByRole, findByText } = render(SettingsDialog, {
      props: { onClose: vi.fn(), onLicenseChanged },
    });

    const codeInput = await findByPlaceholderText(/activation code/i);
    await fireEvent.input(codeInput, { target: { value: "NEW-CODE" } });
    await fireEvent.click(getByRole("button", { name: /reactivate|activate/i }));
    await waitFor(() => expect(mockActivateLicense).toHaveBeenCalledWith("NEW-CODE"));
    await waitFor(() => expect(onLicenseChanged).toHaveBeenCalledWith(fresh.state));
    expect(await findByText(/active/i)).toBeTruthy();
  });

  it("removing the license asks for confirmation before calling deactivateLicense", async () => {
    const { findByRole, getByRole, queryByRole } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    const removeButton = await findByRole("button", { name: /remove|deactivate/i });
    await fireEvent.click(removeButton);
    await tick();
    expect(queryByRole("dialog", { name: /remove/i })).toBeTruthy();
    expect(mockDeactivateLicense).not.toHaveBeenCalled();

    mockDeactivateLicense.mockResolvedValue({ state: "missing" });
    await fireEvent.click(getByRole("button", { name: /^yes$/i }));
    await waitFor(() => expect(mockDeactivateLicense).toHaveBeenCalledOnce());
  });

  it("confirming removal reports the missing state via onLicenseChanged", async () => {
    mockDeactivateLicense.mockResolvedValue({ state: "missing" });
    const onLicenseChanged = vi.fn();
    const { findByRole, getByRole } = render(SettingsDialog, {
      props: { onClose: vi.fn(), onLicenseChanged },
    });
    const removeButton = await findByRole("button", { name: /remove|deactivate/i });
    await fireEvent.click(removeButton);
    await tick();
    await fireEvent.click(getByRole("button", { name: /^yes$/i }));
    await waitFor(() => expect(onLicenseChanged).toHaveBeenCalledWith({ state: "missing" }));
  });

  it("deactivation works even when getLicenseInfo cannot be reached again afterward (offline-capable)", async () => {
    mockDeactivateLicense.mockResolvedValue({ state: "missing" });
    mockGetLicenseInfo.mockResolvedValueOnce(validInfo()).mockRejectedValueOnce(new Error("offline"));
    const { findByRole, getByRole, findByText } = render(SettingsDialog, { props: { onClose: vi.fn() } });
    const removeButton = await findByRole("button", { name: /remove|deactivate/i });
    await fireEvent.click(removeButton);
    await tick();
    await fireEvent.click(getByRole("button", { name: /^yes$/i }));
    // Even if the post-deactivate refresh fails, the section must fall back
    // to reflecting the known-good local result rather than erroring out.
    expect(await findByText(/not activated/i)).toBeTruthy();
  });
});
