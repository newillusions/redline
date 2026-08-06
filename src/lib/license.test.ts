/**
 * Unit tests for the S2b license IPC wrappers.
 *
 * Covers: invoke command/argument-key correctness (Tauri v2 maps JS
 * camelCase keys to Rust snake_case params - see ipc.test.ts's guard comment
 * for the incident this protects against), the `isLicensed`/`isUsable` type
 * guards, and `checkInIfActivated`'s never-throws, always-check-in contract.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  getLicenseStatus,
  activateLicense,
  renewLicense,
  getLicenseInfo,
  deactivateLicense,
  checkInIfActivated,
  isLicensed,
  isUsable,
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

  it("license_info -> no args", async () => {
    mockInvoke.mockResolvedValue({ code: null, device_fingerprint: "d1", state: { state: "missing" } } as never);
    await getLicenseInfo();
    expect(mockInvoke).toHaveBeenCalledWith("license_info");
  });

  it("deactivate_license -> no args", async () => {
    mockInvoke.mockResolvedValue({ state: "missing" } as never);
    await deactivateLicense();
    expect(mockInvoke).toHaveBeenCalledWith("deactivate_license");
  });
});

describe("isLicensed", () => {
  it("is true only for state: valid", () => {
    const valid: LicenseState = {
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 10,
    };
    expect(isLicensed(valid)).toBe(true);
    expect(isLicensed({ state: "missing" })).toBe(false);
    expect(isLicensed({ state: "expired" })).toBe(false);
    expect(isLicensed({ state: "grace", staff_id: "staff:abc", expired_at: "x", grace_deadline: "y", days_remaining: 3 })).toBe(false);
    expect(isLicensed({ state: "revoked", reason: "staff_not_active" })).toBe(false);
    expect(isLicensed({ state: "invalid", reason: "bad_signature" })).toBe(false);
    expect(isLicensed(null)).toBe(false);
  });
});

describe("isUsable", () => {
  it("is true for valid and grace, false for everything else", () => {
    expect(isUsable({ state: "valid", staff_id: "staff:abc", expires_at: "x", days_remaining: 10 })).toBe(true);
    expect(
      isUsable({ state: "grace", staff_id: "staff:abc", expired_at: "x", grace_deadline: "y", days_remaining: 3 }),
    ).toBe(true);
    expect(isUsable({ state: "missing" })).toBe(false);
    expect(isUsable({ state: "expired" })).toBe(false);
    expect(isUsable({ state: "revoked", reason: "staff_not_active" })).toBe(false);
    expect(isUsable({ state: "invalid", reason: "device_mismatch" })).toBe(false);
    expect(isUsable(null)).toBe(false);
  });
});

describe("checkInIfActivated", () => {
  it("does not call renew when there's no stored activation", async () => {
    mockInvoke.mockReset();
    const result = await checkInIfActivated({ state: "missing" });
    expect(result).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("checks in even when the offline read was already valid (online is authoritative every launch)", async () => {
    mockInvoke.mockReset();
    const fresh: LicenseState = {
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-02-01T00:00:00Z",
      days_remaining: 14,
    };
    mockInvoke.mockResolvedValue(fresh as never);
    const result = await checkInIfActivated({
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-01-04T00:00:00Z",
      days_remaining: 10,
    });
    expect(mockInvoke).toHaveBeenCalledWith("renew_license");
    expect(result).toEqual(fresh);
  });

  it("checks in on an expired local read, which can come back revoked", async () => {
    mockInvoke.mockReset();
    const revoked: LicenseState = { state: "revoked", reason: "staff_not_active" };
    mockInvoke.mockResolvedValue(revoked as never);
    const result = await checkInIfActivated({ state: "expired" });
    expect(mockInvoke).toHaveBeenCalledWith("renew_license");
    expect(result).toEqual(revoked);
  });

  it("checks in on a grace-period local read, which can come back valid", async () => {
    mockInvoke.mockReset();
    const fresh: LicenseState = {
      state: "valid",
      staff_id: "staff:abc",
      expires_at: "2099-02-01T00:00:00Z",
      days_remaining: 14,
    };
    mockInvoke.mockResolvedValue(fresh as never);
    const result = await checkInIfActivated({
      state: "grace",
      staff_id: "staff:abc",
      expired_at: "2026-01-01T00:00:00Z",
      grace_deadline: "2026-01-08T00:00:00Z",
      days_remaining: 3,
    });
    expect(mockInvoke).toHaveBeenCalledWith("renew_license");
    expect(result).toEqual(fresh);
  });

  it("swallows an unreachable-server failure and returns null (existing token keeps gating)", async () => {
    mockInvoke.mockReset();
    mockInvoke.mockRejectedValue(new Error("offline"));
    const result = await checkInIfActivated({ state: "expired" });
    expect(result).toBeNull();
  });
});
