//! Client-side orchestration for activate/renew/info/deactivate, parameterized
//! over a `LicenseClient` trait so the flow (call service -> persist ->
//! re-evaluate) is testable with a fake, without a live HTTP call - mirrors
//! emittiv-staff's `license-service.ts` (DbLike injection) on the producer
//! side.
//!
//! Launch model (2026-08-05, Martin: "14 days as a default, with a 1 week
//! grace period? warnings when you open the app each time. but revoking the
//! license is an instant cancel as soon as the machine is online"):
//! `renew` is now the single online check-in call, made unconditionally on
//! every app launch (see the frontend's `license.ts::checkInIfActivated` -
//! previously this only fired when a `renew_due` window said so, which meant
//! revocation could take up to the token's own multi-day TTL to take effect).
//! Its three outcomes:
//!   - success -> persist the fresh token, evaluate it (ordinarily `Valid`).
//!   - `ClientError::Rejected` (revoked/offboarded) -> INSTANT hard stop:
//!     clear the stored token and return `Revoked` directly, never falling
//!     back to the old token's own expiry. This is deliberately NOT routed
//!     through `gate::evaluate` - revocation is a fact this install just
//!     learned online, not something derivable from the token bytes.
//!   - any other error (offline, DNS failure, timeout, `NotConfigured`) ->
//!     "unreachable" in every sense; the stored token is left untouched and
//!     `gate::evaluate` decides Valid/Grace/Expired from it, same as before.

use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::client::{self, ClientError};
use super::gate::{self, LicenseState};
use super::store::{self, StoredLicense};

#[async_trait::async_trait]
pub trait LicenseClient: Send + Sync {
    async fn issue(&self, code: &str, device_fingerprint: &str) -> Result<String, ClientError>;
    async fn renew(&self, code: &str, device_fingerprint: &str) -> Result<String, ClientError>;
}

/// The real network-backed implementation, used by the Tauri commands.
pub struct HttpLicenseClient;

#[async_trait::async_trait]
impl LicenseClient for HttpLicenseClient {
    async fn issue(&self, code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
        client::issue(code, device_fingerprint).await
    }
    async fn renew(&self, code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
        client::renew(code, device_fingerprint).await
    }
}

#[derive(Debug)]
pub enum ActivateError {
    Client(ClientError),
    Persist(String),
}

/// Activate a fresh install: call `issue`, persist the returned token, and
/// return the resulting gate state (expected `Valid` on success).
pub async fn activate(
    licenser: &dyn LicenseClient,
    data_dir: &Path,
    code: &str,
    device_fingerprint: &str,
    public_key: &VerifyingKey,
    now: DateTime<Utc>,
) -> Result<LicenseState, ActivateError> {
    let token = licenser
        .issue(code, device_fingerprint)
        .await
        .map_err(ActivateError::Client)?;
    store::save(
        data_dir,
        &StoredLicense {
            code: code.to_string(),
            token: token.clone(),
        },
    )
    .map_err(|e| ActivateError::Persist(e.to_string()))?;
    Ok(gate::evaluate(Some(&token), device_fingerprint, public_key, now))
}

/// The online check-in, called unconditionally on every app launch (see
/// module doc comment). On success, persists the fresh token. On rejection
/// (revoked/offboarded), clears the stored token immediately - see
/// `LicenseState::Revoked`'s doc comment. On any transport/config failure,
/// leaves the existing token untouched and falls back to offline evaluation
/// (which may itself now be `Valid`, `Grace`, or `Expired`).
pub async fn renew(
    licenser: &dyn LicenseClient,
    data_dir: &Path,
    stored: &StoredLicense,
    device_fingerprint: &str,
    public_key: &VerifyingKey,
    now: DateTime<Utc>,
) -> LicenseState {
    match licenser.renew(&stored.code, device_fingerprint).await {
        Ok(token) => {
            let fresh = StoredLicense {
                code: stored.code.clone(),
                token: token.clone(),
            };
            if let Err(e) = store::save(data_dir, &fresh) {
                log::warn!("license renew: persist failed, keeping prior token: {e}");
                return gate::evaluate(Some(&stored.token), device_fingerprint, public_key, now);
            }
            gate::evaluate(Some(&token), device_fingerprint, public_key, now)
        }
        Err(ClientError::Rejected(reason)) => {
            // Instant hard stop: the server told us this activation is no
            // longer good the moment we could reach it. Do not let the
            // machine keep running on the previously-cached token even if
            // that token's own signature/expiry still look fine.
            if let Err(e) = store::clear(data_dir) {
                log::warn!("license renew: revoked, but clearing local state failed: {e}");
            }
            LicenseState::Revoked { reason }
        }
        Err(e) => {
            log::info!(
                "license check-in deferred (unreachable - not fatal, falling back to the cached token): {e:?}"
            );
            gate::evaluate(Some(&stored.token), device_fingerprint, public_key, now)
        }
    }
}

/// Display bundle for the settings-panel "License" section: the activation
/// code on file (so the user can see/copy what's registered, or decide
/// whether to re-enter it), this device's fingerprint, and the current gate
/// state. Pure, no IO - `stored`/`device_fingerprint` are loaded by the
/// caller the same way `license_status` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LicenseInfo {
    pub code: Option<String>,
    pub device_fingerprint: String,
    pub state: LicenseState,
}

pub fn info(
    stored: Option<&StoredLicense>,
    device_fingerprint: &str,
    public_key: &VerifyingKey,
    now: DateTime<Utc>,
) -> LicenseInfo {
    LicenseInfo {
        code: stored.map(|s| s.code.clone()),
        device_fingerprint: device_fingerprint.to_string(),
        state: gate::evaluate(stored.map(|s| s.token.as_str()), device_fingerprint, public_key, now),
    }
}

/// Settings-panel "remove license from this device" - local-only, no server
/// call, and must work offline (a user must always be able to deactivate
/// even with no network). Idempotent: removing an already-missing activation
/// succeeds.
///
/// SEAM FOR A FUTURE SERVER RELEASE CALL (deliberately not built here - see
/// the redline dispatch's HARD CONSTRAINTS): local removal today leaves the
/// device's claim active server-side (emittiv-staff still counts this device
/// against the activation). A future `POST /api/license/release
/// {code, device_fingerprint}` on the license service, mirroring
/// `issue`/`renew`'s shape, would let the server free that device slot. The
/// client-side contract this seam expects: fire it AFTER the local `clear`
/// (so a user can always deactivate locally regardless of network), best-
/// effort / non-blocking (a failed or unreachable release must never re-add
/// the local activation or block the UI - same "local truth wins offline"
/// posture as `renew`'s transport-failure path), and log-only on failure.
/// `stored.code` + `device_fingerprint` are exactly the two fields `renew`
/// already threads through, so wiring it in later is additive, not a
/// reshape.
pub fn deactivate(data_dir: &Path) -> std::io::Result<()> {
    store::clear(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::token::{mint_token_for_tests, LicensePayload};
    use ed25519_dalek::SigningKey;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn test_keypair(seed: u8) -> (SigningKey, VerifyingKey) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let verifying_key = VerifyingKey::from(&signing_key);
        (signing_key, verifying_key)
    }

    fn token_for(signing_key: &SigningKey, device_fingerprint: &str, expires_in_days: i64) -> String {
        let now = Utc::now();
        let payload = LicensePayload {
            staff_id: "staff:abc123".to_string(),
            app: "redline".to_string(),
            device_fingerprint: device_fingerprint.to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::days(expires_in_days)).to_rfc3339(),
        };
        mint_token_for_tests(signing_key, &payload)
    }

    fn scratch_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("redline-license-service-{}", Uuid::new_v4()))
    }

    /// Test double for `LicenseClient`: each field is consumed exactly once by
    /// the corresponding call, so an unexpected extra call panics loudly
    /// rather than silently returning a stale canned response.
    struct FakeClient {
        issue_result: Mutex<Option<Result<String, ClientError>>>,
        renew_result: Mutex<Option<Result<String, ClientError>>>,
    }

    #[async_trait::async_trait]
    impl LicenseClient for FakeClient {
        async fn issue(&self, _code: &str, _device_fingerprint: &str) -> Result<String, ClientError> {
            self.issue_result
                .lock()
                .unwrap()
                .take()
                .expect("issue() called more times than expected")
        }
        async fn renew(&self, _code: &str, _device_fingerprint: &str) -> Result<String, ClientError> {
            self.renew_result
                .lock()
                .unwrap()
                .take()
                .expect("renew() called more times than expected")
        }
    }

    #[tokio::test]
    async fn activate_persists_token_and_returns_valid() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        let token = token_for(&sk, "device-1", 14);
        let client = FakeClient {
            issue_result: Mutex::new(Some(Ok(token))),
            renew_result: Mutex::new(None),
        };

        let state = activate(&client, &dir, "CODE-1", "device-1", &vk, Utc::now())
            .await
            .expect("activate succeeds");
        assert!(state.is_valid());

        let stored = store::load(&dir).unwrap().expect("persisted");
        assert_eq!(stored.code, "CODE-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn activate_rejected_does_not_persist() {
        let (_, vk) = test_keypair(1);
        let dir = scratch_dir();
        let client = FakeClient {
            issue_result: Mutex::new(Some(Err(ClientError::Rejected("staff_not_active".to_string())))),
            renew_result: Mutex::new(None),
        };

        let result = activate(&client, &dir, "CODE-1", "device-1", &vk, Utc::now()).await;
        assert!(matches!(result, Err(ActivateError::Client(ClientError::Rejected(_)))));
        assert_eq!(store::load(&dir).unwrap(), None, "a rejected issue must not write a token");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- renew: revoked is an instant hard stop, transport failure falls back ---

    #[tokio::test]
    async fn renew_rejected_clears_the_token_and_returns_revoked_instantly() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        // Still well within its own expiry - the point is that a reachable
        // revocation must NOT let this ride out to that expiry anymore.
        let existing_token = token_for(&sk, "device-1", 10);
        let stored = StoredLicense {
            code: "CODE-1".to_string(),
            token: existing_token,
        };
        store::save(&dir, &stored).unwrap();

        let client = FakeClient {
            issue_result: Mutex::new(None),
            renew_result: Mutex::new(Some(Err(ClientError::Rejected("staff_not_active".to_string())))),
        };
        let state = renew(&client, &dir, &stored, "device-1", &vk, Utc::now()).await;
        match state {
            LicenseState::Revoked { reason } => assert_eq!(reason, "staff_not_active"),
            other => panic!("expected Revoked, got {other:?}"),
        }
        assert_eq!(
            store::load(&dir).unwrap(),
            None,
            "a revoked check-in must clear the stored activation, not just report it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn renew_transport_failure_keeps_existing_token_and_falls_back_to_gate() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        let existing_token = token_for(&sk, "device-1", 10);
        let stored = StoredLicense {
            code: "CODE-1".to_string(),
            token: existing_token.clone(),
        };
        store::save(&dir, &stored).unwrap();

        let client = FakeClient {
            issue_result: Mutex::new(None),
            renew_result: Mutex::new(Some(Err(ClientError::Transport("offline".to_string())))),
        };
        let state = renew(&client, &dir, &stored, "device-1", &vk, Utc::now()).await;
        assert!(state.is_valid());
        assert_eq!(
            store::load(&dir).unwrap().unwrap().token,
            existing_token,
            "an unreachable server must never touch the cached token"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn renew_transport_failure_on_an_expired_token_falls_back_into_grace() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        // Expired 1 day ago - offline, this should read as Grace, not Expired
        // outright and not Revoked (we never heard from the server at all).
        let existing_token = token_for(&sk, "device-1", -1);
        let stored = StoredLicense {
            code: "CODE-1".to_string(),
            token: existing_token,
        };
        store::save(&dir, &stored).unwrap();

        let client = FakeClient {
            issue_result: Mutex::new(None),
            renew_result: Mutex::new(Some(Err(ClientError::Transport("offline".to_string())))),
        };
        let state = renew(&client, &dir, &stored, "device-1", &vk, Utc::now()).await;
        assert!(matches!(state, LicenseState::Grace { .. }), "expected Grace, got {state:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn renew_success_persists_fresh_token() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        let old_token = token_for(&sk, "device-1", 1);
        let stored = StoredLicense {
            code: "CODE-1".to_string(),
            token: old_token,
        };
        store::save(&dir, &stored).unwrap();

        let fresh_token = token_for(&sk, "device-1", 14);
        let client = FakeClient {
            issue_result: Mutex::new(None),
            renew_result: Mutex::new(Some(Ok(fresh_token.clone()))),
        };
        let state = renew(&client, &dir, &stored, "device-1", &vk, Utc::now()).await;
        assert!(state.is_valid());

        let after = store::load(&dir).unwrap().unwrap();
        assert_eq!(after.token, fresh_token);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- info: settings-panel display bundle (pure, no IO) ------------------

    #[test]
    fn info_missing_when_never_activated() {
        let (_, vk) = test_keypair(1);
        let got = info(None, "device-1", &vk, Utc::now());
        assert_eq!(got.code, None);
        assert_eq!(got.device_fingerprint, "device-1");
        assert_eq!(got.state, LicenseState::Missing);
    }

    #[test]
    fn info_exposes_the_activation_code_and_valid_state() {
        let (sk, vk) = test_keypair(1);
        let token = token_for(&sk, "device-1", 14);
        let stored = StoredLicense {
            code: "ABCD-1234".to_string(),
            token,
        };
        let got = info(Some(&stored), "device-1", &vk, Utc::now());
        assert_eq!(got.code, Some("ABCD-1234".to_string()));
        assert_eq!(got.device_fingerprint, "device-1");
        assert!(got.state.is_valid());
    }

    #[test]
    fn info_still_exposes_the_code_when_the_token_is_invalid() {
        // A device-mismatched (or otherwise invalid) stored token should still
        // let the settings panel show WHICH code is on file, so the user can
        // decide whether to re-enter it - not just a bare "invalid" with no
        // context.
        let (sk, vk) = test_keypair(1);
        let token = token_for(&sk, "device-OTHER", 14);
        let stored = StoredLicense {
            code: "ABCD-1234".to_string(),
            token,
        };
        let got = info(Some(&stored), "device-1", &vk, Utc::now());
        assert_eq!(got.code, Some("ABCD-1234".to_string()));
        assert!(matches!(got.state, LicenseState::Invalid { .. }));
    }

    // --- deactivate: local-only removal (no server call) --------------------

    #[test]
    fn deactivate_clears_a_stored_activation() {
        let dir = scratch_dir();
        store::save(
            &dir,
            &StoredLicense {
                code: "ABCD-1234".to_string(),
                token: "payload.sig".to_string(),
            },
        )
        .unwrap();

        deactivate(&dir).expect("deactivate succeeds");
        assert_eq!(store::load(&dir).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deactivate_when_nothing_was_ever_activated_is_ok() {
        let dir = scratch_dir();
        deactivate(&dir).expect("deactivate on an empty install is a no-op success");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
