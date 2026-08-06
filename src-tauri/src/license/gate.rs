//! Pure startup gating decision: combines the offline token verification
//! (`token.rs`) with device binding (does the token's device_fingerprint
//! match THIS install's persisted device id) and a post-expiry grace window.
//! No IO here - callers (the license Tauri commands) load the stored token +
//! device id first and pass them in, which keeps this fully unit-testable.
//!
//! Revocation is NOT decided here - an offline-verified token has no way to
//! know it was revoked server-side. `service::renew`'s online check-in is
//! authoritative for that (see its doc comment); this module only ever
//! produces `Revoked` in the sense of re-evaluating a token that was cleared
//! by that path (which then reads back as `Missing`, same as any other
//! never-activated install). `LicenseState::Revoked` itself is constructed
//! directly by `service::renew`.

use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::token::{verify_token, LicensePayload, VerifyFailureReason, VerifyResult};

/// Once an offline-verified token's `expires_at` has passed, the app keeps
/// working (with a warning shown on every launch - see `service::renew`'s
/// callers) for this many additional days before hard-locking. This is the
/// offline-tolerance window; it is now decoupled from the token's own TTL
/// (previously the two were the same knob, which made a long-lived token
/// simultaneously give unlimited offline use AND unlimited revocation lag -
/// see the module doc comment on why revocation is handled online instead).
pub const GRACE_WINDOW_DAYS: i64 = 7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LicenseState {
    /// A valid, device-matched, unexpired token is present.
    Valid {
        staff_id: String,
        expires_at: String,
        days_remaining: i64,
    },
    /// The stored token's `expires_at` has passed, but we're still within
    /// `GRACE_WINDOW_DAYS` of it - the app keeps working, but the caller must
    /// surface a warning on every launch (see `service::renew` doc comment).
    /// Only reachable while offline: an online check-in either renews (back
    /// to `Valid`) or is refused (`Revoked`) - it never leaves a token to
    /// coast through grace, that's purely the "haven't been able to reach the
    /// server" path.
    Grace {
        staff_id: String,
        expired_at: String,
        /// `expired_at` + `GRACE_WINDOW_DAYS`, RFC3339 - the hard deadline.
        grace_deadline: String,
        days_remaining: i64,
    },
    /// No stored token/activation at all - first run, never activated, or
    /// removed via the settings panel's "remove license from this device".
    Missing,
    /// A stored token failed offline verification: bad signature, malformed,
    /// or bound to a different device than this install.
    Invalid { reason: String },
    /// The license service explicitly refused to renew this activation on a
    /// reachable check-in (offboarded staff, revoked activation, etc) - an
    /// immediate hard stop, distinct from `Missing`/`Expired` so the UI can
    /// say why, not just that. `service::renew` clears the stored token the
    /// moment this is produced; re-evaluating afterward reads back as
    /// `Missing`.
    Revoked { reason: String },
    /// Verification failed on expiry AND the grace window has also passed
    /// (or the expired token was never valid on this device to begin with -
    /// see `grace_or_expired`'s doc comment).
    Expired,
}

impl LicenseState {
    pub fn is_valid(&self) -> bool {
        matches!(self, LicenseState::Valid { .. })
    }

    /// True for any state where the app should render its normal content
    /// rather than the activation gate - `Valid` outright, or `Grace` (the
    /// app keeps working through the offline grace window, just with a
    /// warning surfaced elsewhere).
    pub fn is_usable(&self) -> bool {
        matches!(self, LicenseState::Valid { .. } | LicenseState::Grace { .. })
    }
}

/// Evaluate gating for a stored token string (if any) against the baked
/// public key, this device's fingerprint, and wall-clock `now`.
pub fn evaluate(
    stored_token: Option<&str>,
    device_fingerprint: &str,
    public_key: &VerifyingKey,
    now: DateTime<Utc>,
) -> LicenseState {
    let Some(token) = stored_token.filter(|t| !t.is_empty()) else {
        return LicenseState::Missing;
    };

    match verify_token(token, public_key, now) {
        VerifyResult::Valid(payload) => gate_valid_payload(payload, device_fingerprint, now),
        VerifyResult::Invalid {
            reason: VerifyFailureReason::Expired,
            payload,
        } => grace_or_expired(payload, device_fingerprint, now),
        VerifyResult::Invalid { reason, .. } => LicenseState::Invalid {
            reason: reason_str(reason),
        },
    }
}

fn gate_valid_payload(
    payload: LicensePayload,
    device_fingerprint: &str,
    now: DateTime<Utc>,
) -> LicenseState {
    if payload.device_fingerprint != device_fingerprint {
        return LicenseState::Invalid {
            reason: "device_mismatch".to_string(),
        };
    }

    // expires_at already validated as a parseable RFC3339 instant by verify_token.
    let expires_at = parse_rfc3339(&payload.expires_at);
    let remaining = expires_at - now;

    LicenseState::Valid {
        staff_id: payload.staff_id,
        expires_at: payload.expires_at,
        days_remaining: remaining.num_days(),
    }
}

/// A token whose signature/shape are fine but whose `expires_at` has passed.
/// Grants the offline grace window only when the token was actually issued
/// for THIS device - a token copied from a different install should never
/// extend grace here, the same reasoning `gate_valid_payload` applies to an
/// unexpired token's device check.
fn grace_or_expired(
    payload: Option<LicensePayload>,
    device_fingerprint: &str,
    now: DateTime<Utc>,
) -> LicenseState {
    let Some(payload) = payload else {
        return LicenseState::Expired;
    };
    if payload.device_fingerprint != device_fingerprint {
        return LicenseState::Expired;
    }

    let expired_at = parse_rfc3339(&payload.expires_at);
    let grace_deadline = expired_at + Duration::days(GRACE_WINDOW_DAYS);
    if now > grace_deadline {
        return LicenseState::Expired;
    }

    LicenseState::Grace {
        staff_id: payload.staff_id,
        expired_at: payload.expires_at,
        grace_deadline: grace_deadline.to_rfc3339(),
        days_remaining: (grace_deadline - now).num_days(),
    }
}

fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("verify_token already validated this parses")
        .with_timezone(&Utc)
}

fn reason_str(reason: VerifyFailureReason) -> String {
    match reason {
        VerifyFailureReason::Malformed => "malformed".to_string(),
        VerifyFailureReason::BadSignature => "bad_signature".to_string(),
        VerifyFailureReason::Expired => "expired".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::token::{mint_token_for_tests, LicensePayload};
    use ed25519_dalek::SigningKey;

    fn test_keypair(seed: u8) -> (SigningKey, VerifyingKey) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let verifying_key = VerifyingKey::from(&signing_key);
        (signing_key, verifying_key)
    }

    /// `now` is threaded through explicitly (rather than each call site doing
    /// its own `Utc::now()`) so a test can mint a token and evaluate it
    /// against the SAME instant - the grace-window tests below sit on exact
    /// day boundaries, where two independently-taken `Utc::now()` calls a few
    /// microseconds apart are enough to truncate `num_days()` across the
    /// boundary and flake.
    fn token_with(
        now: DateTime<Utc>,
        device_fingerprint: &str,
        expires_in_days: i64,
        signing_key: &SigningKey,
    ) -> String {
        let payload = LicensePayload {
            staff_id: "staff:abc123".to_string(),
            app: "redline".to_string(),
            device_fingerprint: device_fingerprint.to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + Duration::days(expires_in_days)).to_rfc3339(),
        };
        mint_token_for_tests(signing_key, &payload)
    }

    #[test]
    fn missing_when_no_token() {
        let (_, vk) = test_keypair(1);
        assert_eq!(evaluate(None, "device-1", &vk, Utc::now()), LicenseState::Missing);
        assert_eq!(evaluate(Some(""), "device-1", &vk, Utc::now()), LicenseState::Missing);
    }

    #[test]
    fn valid_when_token_matches_device_and_unexpired() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        let token = token_with(now, "device-1", 14, &sk);
        let state = evaluate(Some(&token), "device-1", &vk, now);
        assert!(state.is_valid());
        assert!(state.is_usable());
        match state {
            LicenseState::Valid { days_remaining, .. } => {
                assert!(days_remaining >= 13);
            }
            other => panic!("expected valid, got {other:?}"),
        }
    }

    #[test]
    fn wrong_device_is_invalid_even_with_good_signature() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        // Validly signed and unexpired, but bound to a DIFFERENT device than
        // this install - e.g. a token file copied off another machine.
        let token = token_with(now, "device-OTHER", 14, &sk);
        match evaluate(Some(&token), "device-1", &vk, now) {
            LicenseState::Invalid { reason } => assert_eq!(reason, "device_mismatch"),
            other => panic!("expected device_mismatch, got {other:?}"),
        }
    }

    #[test]
    fn bad_signature_is_invalid() {
        let (_, vk) = test_keypair(1);
        let (wrong_sk, _) = test_keypair(4);
        let now = Utc::now();
        let token = token_with(now, "device-1", 14, &wrong_sk); // signed with the WRONG key
        match evaluate(Some(&token), "device-1", &vk, now) {
            LicenseState::Invalid { reason } => assert_eq!(reason, "bad_signature"),
            other => panic!("expected bad_signature, got {other:?}"),
        }
    }

    // --- grace window --------------------------------------------------------

    #[test]
    fn just_expired_token_is_in_grace_not_expired() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        let token = token_with(now, "device-1", -1, &sk); // expired 1 day ago
        let state = evaluate(Some(&token), "device-1", &vk, now);
        assert!(state.is_usable());
        assert!(!state.is_valid(), "grace is usable but not the same as Valid");
        match state {
            LicenseState::Grace {
                days_remaining,
                staff_id,
                ..
            } => {
                assert_eq!(staff_id, "staff:abc123");
                // GRACE_WINDOW_DAYS(7) - 1 day already elapsed = 6 remaining.
                assert_eq!(days_remaining, 6);
            }
            other => panic!("expected grace, got {other:?}"),
        }
    }

    #[test]
    fn expired_exactly_at_grace_window_boundary_is_still_in_grace() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        let token = token_with(now, "device-1", -GRACE_WINDOW_DAYS, &sk);
        match evaluate(Some(&token), "device-1", &vk, now) {
            LicenseState::Grace { days_remaining, .. } => assert_eq!(days_remaining, 0),
            other => panic!("expected grace at the boundary, got {other:?}"),
        }
    }

    #[test]
    fn expired_past_grace_window_locks_out() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        let token = token_with(now, "device-1", -(GRACE_WINDOW_DAYS + 1), &sk);
        assert_eq!(evaluate(Some(&token), "device-1", &vk, now), LicenseState::Expired);
    }

    #[test]
    fn expired_token_for_a_different_device_gets_no_grace() {
        let (sk, vk) = test_keypair(1);
        let now = Utc::now();
        // Expired AND bound to another device - must not extend grace here.
        let token = token_with(now, "device-OTHER", -1, &sk);
        assert_eq!(evaluate(Some(&token), "device-1", &vk, now), LicenseState::Expired);
    }
}
