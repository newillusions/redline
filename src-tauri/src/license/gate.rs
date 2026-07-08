//! Pure startup gating decision: combines the offline token verification
//! (`token.rs`) with device binding (does the token's device_fingerprint
//! match THIS install's persisted device id) and the renew-due window. No IO
//! here - callers (the license Tauri commands) load the stored token + device
//! id first and pass them in, which keeps this fully unit-testable.

use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::token::{verify_token, LicensePayload, VerifyFailureReason, VerifyResult};

/// Renew once the stored token has this much time or less left.
pub const RENEW_WINDOW_DAYS: i64 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LicenseState {
    /// A valid, device-matched, unexpired token is present.
    Valid {
        staff_id: String,
        expires_at: String,
        days_remaining: i64,
        /// True when within `RENEW_WINDOW_DAYS` of expiry - the caller should
        /// attempt a renew (best-effort; a failed renew here is not fatal,
        /// see `service::renew`).
        renew_due: bool,
    },
    /// No stored token/activation at all - first run, or never activated.
    Missing,
    /// A stored token failed offline verification: bad signature, malformed,
    /// or bound to a different device than this install.
    Invalid { reason: String },
    /// Signature and shape were fine but `expires_at` has passed. This is the
    /// grace-window boundary: an offboarded staff member's token keeps
    /// working right up to its own expiry, then the app locks here.
    Expired,
}

impl LicenseState {
    pub fn is_valid(&self) -> bool {
        matches!(self, LicenseState::Valid { .. })
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
            ..
        } => LicenseState::Expired,
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
    let expires_at = DateTime::parse_from_rfc3339(&payload.expires_at)
        .expect("verify_token already validated this parses")
        .with_timezone(&Utc);
    let remaining = expires_at - now;

    LicenseState::Valid {
        staff_id: payload.staff_id,
        expires_at: payload.expires_at,
        days_remaining: remaining.num_days(),
        renew_due: remaining <= Duration::days(RENEW_WINDOW_DAYS),
    }
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

    fn token_with(device_fingerprint: &str, expires_in_days: i64, signing_key: &SigningKey) -> String {
        let now = Utc::now();
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
        let token = token_with("device-1", 14, &sk);
        let state = evaluate(Some(&token), "device-1", &vk, Utc::now());
        assert!(state.is_valid());
        match state {
            LicenseState::Valid {
                renew_due,
                days_remaining,
                ..
            } => {
                assert!(!renew_due);
                assert!(days_remaining >= 13);
            }
            other => panic!("expected valid, got {other:?}"),
        }
    }

    #[test]
    fn renew_due_within_window() {
        let (sk, vk) = test_keypair(1);
        let token = token_with("device-1", 2, &sk); // within RENEW_WINDOW_DAYS (3)
        match evaluate(Some(&token), "device-1", &vk, Utc::now()) {
            LicenseState::Valid { renew_due, .. } => assert!(renew_due),
            other => panic!("expected valid+renew_due, got {other:?}"),
        }
    }

    #[test]
    fn expired_token_locks_out() {
        let (sk, vk) = test_keypair(1);
        let token = token_with("device-1", -1, &sk);
        assert_eq!(evaluate(Some(&token), "device-1", &vk, Utc::now()), LicenseState::Expired);
    }

    #[test]
    fn wrong_device_is_invalid_even_with_good_signature() {
        let (sk, vk) = test_keypair(1);
        // Validly signed and unexpired, but bound to a DIFFERENT device than
        // this install - e.g. a token file copied off another machine.
        let token = token_with("device-OTHER", 14, &sk);
        match evaluate(Some(&token), "device-1", &vk, Utc::now()) {
            LicenseState::Invalid { reason } => assert_eq!(reason, "device_mismatch"),
            other => panic!("expected device_mismatch, got {other:?}"),
        }
    }

    #[test]
    fn bad_signature_is_invalid() {
        let (_, vk) = test_keypair(1);
        let (wrong_sk, _) = test_keypair(4);
        let token = token_with("device-1", 14, &wrong_sk); // signed with the WRONG key
        match evaluate(Some(&token), "device-1", &vk, Utc::now()) {
            LicenseState::Invalid { reason } => assert_eq!(reason, "bad_signature"),
            other => panic!("expected bad_signature, got {other:?}"),
        }
    }
}
