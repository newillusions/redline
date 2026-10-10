// @vitest-environment jsdom
/**
 * ActivationGate component tests.
 *
 * Covers the headline/hint shown for each blocking licence state (plain-language reasons,
 * never raw codes), the device id with Copy, auto-focus, normalised code entry and the
 * "Retry online" action for an expired licence (UX audit F7, 2026-10-10).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import ActivationGate from "./ActivationGate.svelte";
import type { LicenseState } from "$lib/license";

const mockActivate = vi.fn();
const mockRenew = vi.fn();
const mockInfo = vi.fn();

vi.mock("$lib/license", async () => {
  const actual = await vi.importActual<typeof import("$lib/license")>("$lib/license");
  return {
    ...actual,
    activateLicense: (...a: unknown[]) => mockActivate(...a),
    renewLicense: (...a: unknown[]) => mockRenew(...a),
    getLicenseInfo: (...a: unknown[]) => mockInfo(...a),
  };
});

const DEVICE = "dev-1234-abcd";

beforeEach(() => {
  mockActivate.mockReset();
  mockRenew.mockReset();
  mockInfo.mockReset();
  mockInfo.mockResolvedValue({ code: null, device_fingerprint: DEVICE, state: { state: "missing" } });
});

describe("ActivationGate", () => {
  it("shows the first-run headline for missing", () => {
    const { getByText } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    expect(getByText(/activate redline/i)).toBeTruthy();
  });

  it("shows an expired-specific headline in UK spelling", () => {
    const { getByText } = render(ActivationGate, {
      props: { licenseState: { state: "expired" } as LicenseState, onActivated: vi.fn() },
    });
    expect(getByText(/licence expired/i)).toBeTruthy();
  });

  it("explains a device mismatch in plain language and never shows the raw code", () => {
    const { getByText, container } = render(ActivationGate, {
      props: {
        licenseState: { state: "invalid", reason: "device_mismatch" } as LicenseState,
        onActivated: vi.fn(),
      },
    });
    expect(getByText(/different computer/i)).toBeTruthy();
    expect(container.textContent).not.toContain("device_mismatch");
  });

  it("maps a revoked reason code to plain language and hides the code", () => {
    const { getByRole, getByText, container } = render(ActivationGate, {
      props: {
        licenseState: { state: "revoked", reason: "staff_not_active" } as LicenseState,
        onActivated: vi.fn(),
      },
    });
    expect(getByRole("heading", { name: /revoked/i })).toBeTruthy();
    expect(getByText(/staff account is no longer active/i)).toBeTruthy();
    expect(container.textContent).not.toContain("staff_not_active");
  });

  it("falls back to a generic sentence for an unknown reason code", () => {
    const { container } = render(ActivationGate, {
      props: {
        licenseState: { state: "invalid", reason: "weird_new_code" } as LicenseState,
        onActivated: vi.fn(),
      },
    });
    expect(container.textContent).toContain("could not be verified");
    expect(container.textContent).not.toContain("weird_new_code");
  });

  it("auto-focuses the activation code input", async () => {
    const { getByLabelText } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    await tick();
    expect(document.activeElement).toBe(getByLabelText(/activation code/i));
  });

  it("shows this device's id with a Copy button that writes it to the clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { findByTestId, getByRole } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    const box = await findByTestId("device-id");
    expect(box.textContent).toContain(DEVICE);
    await fireEvent.click(getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(DEVICE);
  });

  it("trims whitespace out of a pasted code before activating", async () => {
    mockActivate.mockResolvedValue({ state: "valid", staff_id: "s", expires_at: "x", days_remaining: 30 });
    const onActivated = vi.fn();
    const { getByLabelText, getByRole } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated },
    });
    await fireEvent.input(getByLabelText(/activation code/i), { target: { value: "  AB12 CD34\n" } });
    await fireEvent.click(getByRole("button", { name: "Activate" }));
    await waitFor(() => expect(onActivated).toHaveBeenCalled());
    expect(mockActivate).toHaveBeenCalledWith("AB12CD34");
  });

  it("shows a plain-language error when activation is refused", async () => {
    mockActivate.mockResolvedValue({ state: "invalid", reason: "device_mismatch" });
    const { getByLabelText, getByRole, findByText } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/activation code/i), { target: { value: "ABC" } });
    await fireEvent.click(getByRole("button", { name: "Activate" }));
    expect(await findByText(/activation was refused/i)).toBeTruthy();
  });

  it("offers Retry online only when expired, and unlocks the app if the renewal works", async () => {
    mockRenew.mockResolvedValue({ state: "valid", staff_id: "s", expires_at: "x", days_remaining: 30 });
    const onActivated = vi.fn();
    const { getByTestId } = render(ActivationGate, {
      props: { licenseState: { state: "expired" } as LicenseState, onActivated },
    });
    await fireEvent.click(getByTestId("retry-online"));
    await waitFor(() => expect(onActivated).toHaveBeenCalled());
    expect(mockRenew).toHaveBeenCalledOnce();
  });

  it("explains a failed online retry and keeps the code entry available", async () => {
    mockRenew.mockResolvedValue({ state: "expired" });
    const { getByTestId, findByText } = render(ActivationGate, {
      props: { licenseState: { state: "expired" } as LicenseState, onActivated: vi.fn() },
    });
    await fireEvent.click(getByTestId("retry-online"));
    expect(await findByText(/could not renew online/i)).toBeTruthy();
  });

  it("does not show Retry online for a missing licence", () => {
    const { queryByTestId } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    expect(queryByTestId("retry-online")).toBeNull();
  });

  it("turns the thrown 'Activation refused: <code>' string into plain language", async () => {
    mockActivate.mockRejectedValue("Activation refused: already_claimed");
    const { getByLabelText, getByRole, findByText, container } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/activation code/i), { target: { value: "ABC" } });
    await fireEvent.click(getByRole("button", { name: "Activate" }));
    expect(await findByText(/already been claimed by another device/i)).toBeTruthy();
    expect(container.textContent).not.toContain("already_claimed");
  });

  it("turns a transport failure into a connection message", async () => {
    mockActivate.mockRejectedValue("Could not reach the license service: dns error");
    const { getByLabelText, getByRole, findByText, container } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    await fireEvent.input(getByLabelText(/activation code/i), { target: { value: "ABC" } });
    await fireEvent.click(getByRole("button", { name: "Activate" }));
    expect(await findByText(/check your internet connection/i)).toBeTruthy();
    expect(container.textContent).not.toContain("dns error");
  });

  it("Retry online: a server refusal is not described as a network problem", async () => {
    mockRenew.mockResolvedValue({ state: "revoked", reason: "staff_not_active" });
    const onActivated = vi.fn();
    const { getByTestId, findByText, container } = render(ActivationGate, {
      props: { licenseState: { state: "expired" } as LicenseState, onActivated },
    });
    await fireEvent.click(getByTestId("retry-online"));
    expect(await findByText(/refused to renew this device/i)).toBeTruthy();
    expect(container.textContent).not.toMatch(/internet connection/i);
    // The app is told the state changed so the gate headline can switch to "revoked".
    expect(onActivated).toHaveBeenCalledWith({ state: "revoked", reason: "staff_not_active" });
  });
});
