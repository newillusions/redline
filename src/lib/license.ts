/**
 * S2b client entitlement - types + IPC wrappers.
 *
 * Lives in its own file (not `ipc.ts`) per the conflict-avoidance pattern in
 * `.claude/rules/judgment.md` - a workspace-level concern like entitlement
 * gating has no business sharing a hot file with the render/document IPC.
 *
 * Mirrors the Rust `LicenseState` enum in `src-tauri/src/license/gate.rs`
 * (serde `tag = "state"`, `rename_all = "snake_case"`) exactly - do not rename
 * fields without updating both sides.
 */
import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Types (mirror Rust LicenseState)
// ---------------------------------------------------------------------------

export interface LicenseValid {
  state: "valid";
  staff_id: string;
  expires_at: string;
  days_remaining: number;
  renew_due: boolean;
}

export interface LicenseMissing {
  state: "missing";
}

export interface LicenseInvalid {
  state: "invalid";
  reason: string;
}

export interface LicenseExpired {
  state: "expired";
}

export type LicenseState = LicenseValid | LicenseMissing | LicenseInvalid | LicenseExpired;

// ---------------------------------------------------------------------------
// Pure helper
// ---------------------------------------------------------------------------

export function isLicensed(state: LicenseState | null): state is LicenseValid {
  return state?.state === "valid";
}

// ---------------------------------------------------------------------------
// IPC wrappers
// ---------------------------------------------------------------------------

/** Startup gate check - offline verify of the stored token, if any. */
export async function getLicenseStatus(): Promise<LicenseState> {
  return invoke<LicenseState>("license_status");
}

/** Claim a token for a freshly entered activation code. */
export async function activateLicense(code: string): Promise<LicenseState> {
  return invoke<LicenseState>("activate_license", { code });
}

/** Attempt a renew. Never throws for an offline/rejected renew - see the Rust
 * `license::service::renew` doc comment (the existing token's own expiry is
 * the grace window). */
export async function renewLicense(): Promise<LicenseState> {
  return invoke<LicenseState>("renew_license");
}

/**
 * Fire-and-forget: call `renewLicense` only when `state.renew_due` is set,
 * and only ever move the caller's state forward on success/failure (never
 * throws). Returns the updated state, or `null` if no renew was attempted.
 */
export async function renewLicenseIfDue(state: LicenseState): Promise<LicenseState | null> {
  if (state.state !== "valid" || !state.renew_due) return null;
  try {
    return await renewLicense();
  } catch {
    // Non-fatal by design - the current token keeps gating on its own expiry.
    return null;
  }
}
