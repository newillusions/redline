// @vitest-environment jsdom
/**
 * ActivationGate component tests.
 *
 * Covers the headline/hint shown for each blocking license state, including
 * the new "revoked" state (2026-08-05 launch-model rework) - a reachable
 * server explicitly refusing a check-in must read differently from a bare
 * "missing" (first run) or "expired" (offline-only lockout), so the user
 * knows this was a deliberate revocation, not an oversight.
 */
import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/svelte";
import ActivationGate from "./ActivationGate.svelte";
import type { LicenseState } from "$lib/license";

describe("ActivationGate", () => {
  it("shows the first-run headline for missing", () => {
    const { getByText } = render(ActivationGate, {
      props: { licenseState: { state: "missing" } as LicenseState, onActivated: vi.fn() },
    });
    expect(getByText(/activate redline/i)).toBeTruthy();
  });

  it("shows an expired-specific headline and hint", () => {
    const { getByText } = render(ActivationGate, {
      props: { licenseState: { state: "expired" } as LicenseState, onActivated: vi.fn() },
    });
    expect(getByText(/license expired/i)).toBeTruthy();
  });

  it("shows a device-mismatch-specific hint for invalid/device_mismatch", () => {
    const { getByText } = render(ActivationGate, {
      props: {
        licenseState: { state: "invalid", reason: "device_mismatch" } as LicenseState,
        onActivated: vi.fn(),
      },
    });
    expect(getByText(/different device/i)).toBeTruthy();
  });

  it("shows a revoked-specific headline and surfaces the server's reason", () => {
    const { getByRole, getByText } = render(ActivationGate, {
      props: {
        licenseState: { state: "revoked", reason: "staff_not_active" } as LicenseState,
        onActivated: vi.fn(),
      },
    });
    expect(getByRole("heading", { name: /revoked/i })).toBeTruthy();
    expect(getByText(/staff_not_active/)).toBeTruthy();
  });
});
