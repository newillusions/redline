/**
 * Unit tests for the S2b license IPC wrappers.
 *
 * Covers: invoke command/argument-key correctness (Tauri v2 maps JS
 * camelCase keys to Rust snake_case params - see ipc.test.ts's guard comment
 * for the incident this protects against), the `isLicensed` type guard, and
 * `renewLicenseIfDue`'s never-throws contract.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  getLicenseStatus,
  activateLicense,
  renewLicense,
  renewLicenseIfDue,
  isLicensed,
} from "./license";
import type { LicenseState } from "./license";

// @tauri-apps/api/core is globally mocked in src/tests/setup.ts.
const mockInvoke = vi.mocked(invoke);

describe("license invoke argument keys", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("license_status -> no args", async () => {
    mockInvoke.mockResolvedValue({ state: "missing" } as never);
    await getLicenseStatus();
    expect(mockInvoke).toHaveBeenCalledWith("license_status");
  });

  it("activate_license -> code (single-word key, no camelCase mapping risk)", async () => {
    mockInvoke.mockResolvedValue({ state: "missing" } as never);
    await activateLicense("ABCD-1234");
    expect(mockInvoke).toHaveBeenCalledWith("activate_license", { code: "ABCD-1234" });
  });

  it("renew_license -> no args", async () => {
    mockInvoke.mockResolvedValue({ state: "missing" } as never);
    await renewLicense();
    expect(mockInvoke).toHaveBeenCalledWith("renew_license");
  });
});

describe("isLicensed", () => {
  it("is true only for state: valid", () => {
    const valid: LicenseState = {
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 10,
      renew_due: false,
    };
    expect(isLicensed(valid)).toBe(true);
    expect(isLicensed({ state: "missing" })).toBe(false);
    expect(isLicensed({ state: "expired" })).toBe(false);
    expect(isLicensed({ state: "invalid", reason: "bad_signature" })).toBe(false);
    expect(isLicensed(null)).toBe(false);
  });
});

describe("renewLicenseIfDue", () => {
  it("does not call renew when state is not valid", async () => {
    mockInvoke.mockReset();
    const result = await renewLicenseIfDue({ state: "missing" });
    expect(result).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("does not call renew when valid but renew_due is false", async () => {
    mockInvoke.mockReset();
    const result = await renewLicenseIfDue({
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 10,
      renew_due: false,
    });
    expect(result).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("calls renew and returns the fresh state when renew_due is true", async () => {
    mockInvoke.mockReset();
    const fresh: LicenseState = {
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-02-01T00:00:00Z",
      days_remaining: 14,
      renew_due: false,
    };
    mockInvoke.mockResolvedValue(fresh as never);
    const result = await renewLicenseIfDue({
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2026-01-04T00:00:00Z",
      days_remaining: 2,
      renew_due: true,
    });
    expect(mockInvoke).toHaveBeenCalledWith("renew_license");
    expect(result).toEqual(fresh);
  });

  it("swallows a renew failure and returns null (existing token keeps gating)", async () => {
    mockInvoke.mockReset();
    mockInvoke.mockRejectedValue(new Error("offline"));
    const result = await renewLicenseIfDue({
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2026-01-04T00:00:00Z",
      days_remaining: 2,
      renew_due: true,
    });
    expect(result).toBeNull();
  });
});
