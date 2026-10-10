/**
 * Plain-language wording for the activation gate (UX audit F7, 2026-10-10). The backend
 * reports machine reason codes (`device_mismatch`, `bad_signature`, `staff_not_active`...);
 * staff should never have to read those. Unknown codes fall back to a generic sentence.
 */
import type { LicenseState } from "./license";

// Only reason codes the backend can actually emit are mapped (gate.rs reason_str /
// "device_mismatch"; staff_not_active from the licence service rejection).
const REASONS: Record<string, string> = {
  device_mismatch: "This activation belongs to a different computer.",
  bad_signature: "The stored activation file has been changed or damaged.",
  malformed: "The stored activation file is unreadable.",
  expired: "The activation has expired.",
  staff_not_active: "Your staff account is no longer active.",
  already_claimed: "That activation code has already been claimed by another device.",
};

const GENERIC = "The activation could not be verified.";

/** Plain-English sentence for a backend reason code. */
export function reasonText(reason: string): string {
  return REASONS[reason] ?? GENERIC;
}

/** The hint paragraph under the gate headline. */
export function gateHint(state: LicenseState): string {
  switch (state.state) {
    case "invalid":
      if (state.reason === "device_mismatch") {
        return `${reasonText(state.reason)} Ask your administrator for a new activation code and quote the Device ID below.`;
      }
      return `${reasonText(state.reason)} Enter a new activation code, or ask your administrator for one.`;
    case "revoked":
      return `${reasonText(state.reason)} Your administrator can restore access. Quote the Device ID below.`;
    case "expired":
      return "Your licence has expired and could not be renewed automatically. Retry online, or enter a new activation code.";
    default:
      return "Enter the activation code from your administrator to unlock Redline on this computer.";
  }
}

/** Error line after the server refused an activation attempt. */
export function activationRefusedText(reason: string): string {
  return `Activation was refused. ${reasonText(reason)}`;
}

/** Strip stray whitespace from a pasted activation code. Case is left alone: the server's
 *  case rules for codes are not known to the client. */
export function normaliseCode(raw: string): string {
  return raw.replace(/\s+/g, "");
}

/**
 * Plain-language text for an error string thrown by the activation command. The Rust side
 * formats refusals as `Activation refused: <code>` and network failures as
 * `Could not reach the license service: <detail>`; neither should reach staff verbatim.
 */
export function activationErrorText(raw: string): string {
  const refused = /^Activation refused:\s*(.+)$/s.exec(raw);
  if (refused) return activationRefusedText(refused[1].trim());
  if (/^Could not reach the licen[sc]e service/i.test(raw)) {
    return "Could not reach the licence service. Check your internet connection and try again.";
  }
  if (/not configured/i.test(raw)) {
    return "The licence service is not set up on this computer. Contact your administrator.";
  }
  return raw;
}

/** Message after "Retry online" came back with a state that still blocks the app. */
export function retryOnlineFailureText(state: LicenseState): string {
  if (state.state === "revoked" || state.state === "invalid") {
    return `The licence service refused to renew this device. ${reasonText(state.reason)}`;
  }
  return "Could not renew online. Check your internet connection, or enter a new activation code.";
}
