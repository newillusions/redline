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
 *
 * Launch model (2026-08-05): the server is authoritative whenever reachable.
 * `checkInIfActivated` is meant to be called once per launch whenever there
 * IS a stored activation (state !== "missing"), regardless of what the
 * offline read says - a reachable server can revoke instantly (`revoked`) or
 * heal a stale/expired/invalid local read via a fresh renew. See
 * `src-tauri/src/license/service.rs`'s module doc comment for the full
 * rationale; this file only carries the frontend half.
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
}

/**
 * The stored token's `expires_at` has passed, but we're still within the
 * server's grace window (7 days by default) - the app keeps working, but the
 * caller must show a warning every launch (see `renderLicenseGraceWarning`
 * usage in App.svelte). Only reachable while offline: an online check-in
 * either renews (back to "valid") or is refused ("revoked").
 */
export interface LicenseGrace {
  state: "grace";
  staff_id: string;
  expired_at: string;
  grace_deadline: string;
  days_remaining: number;
}

export interface LicenseMissing {
  state: "missing";
}

export interface LicenseInvalid {
  state: "invalid";
  reason: string;
}

/**
 * The license service explicitly refused a reachable check-in (offboarded
 * staff, revoked activation) - an immediate hard stop, distinct from
 * "missing" so the activation gate can say why, not just that.
 */
export interface LicenseRevoked {
  state: "revoked";
  reason: string;
}

export interface LicenseExpired {
  state: "expired";
}

export type LicenseState =
  | LicenseValid
  | LicenseGrace
  | LicenseMissing
  | LicenseInvalid
  | LicenseRevoked
  | LicenseExpired;

/** Settings-panel display bundle - mirrors Rust `service::LicenseInfo`. */
export interface LicenseInfo {
  code: string | null;
  device_fingerprint: string;
  state: LicenseState;
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

export function isLicensed(state: LicenseState | null): state is LicenseValid {
  return state?.state === "valid";
}

/**
 * True for any state where the app should render its normal content rather
 * than the activation gate - "valid" outright, or "grace" (the app keeps
 * working through the offline grace window; the warning is a separate,
 * non-blocking surface).
 */
export function isUsable(state: LicenseState | null): state is LicenseValid | LicenseGrace {
  return state?.state === "valid" || state?.state === "grace";
}

// ---------------------------------------------------------------------------
// IPC wrappers
// ---------------------------------------------------------------------------

/** Startup gate check - offline verify of the stored token, if any. Fast,
 * no network call - used for the very first paint so launch is never
 * blocked on a network round trip. */
export async function getLicenseStatus(): Promise<LicenseState> {
  return invoke<LicenseState>("license_status");
}

/** Claim a token for a freshly entered activation code. */
export async function activateLicense(code: string): Promise<LicenseState> {
  return invoke<LicenseState>("activate_license", { code });
}

/** The online check-in. Never throws for an offline/unreachable server - see
 * the Rust `license::service::renew` doc comment (the existing token's own
 * expiry/grace window keeps gating). DOES resolve to `{state: "revoked"}`
 * when the server is reachable and explicitly refuses - that is the one
 * outcome callers must treat as an immediate hard stop, not a soft warning. */
export async function renewLicense(): Promise<LicenseState> {
  return invoke<LicenseState>("renew_license");
}

/** Settings-panel display bundle: code on file, device fingerprint, current
 * state. Local read only, no network call. */
export async function getLicenseInfo(): Promise<LicenseInfo> {
  return invoke<LicenseInfo>("license_info");
}

/** Remove the license from this device. Local-only (works offline), always
 * succeeds, idempotent. Resolves to `{state: "missing"}`. */
export async function deactivateLicense(): Promise<LicenseState> {
  return invoke<LicenseState>("deactivate_license");
}

/**
 * Fire-and-forget: attempt the online check-in whenever a stored activation
 * exists (`state.state !== "missing"`), regardless of what the offline read
 * says - a reachable server is authoritative and can revoke or heal a stale
 * read either way. Returns the updated state, or `null` when there's nothing
 * to check in (never activated) or the server was unreachable (the caller
 * keeps its current state either way).
 */
export async function checkInIfActivated(state: LicenseState): Promise<LicenseState | null> {
  if (state.state === "missing") return null;
  try {
    return await renewLicense();
  } catch {
    // Non-fatal by design - the current token keeps gating on its own
    // (possibly grace-extended) expiry.
    return null;
  }
}
