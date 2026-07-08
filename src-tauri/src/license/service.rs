//! Client-side orchestration for activate/renew, parameterized over a
//! `LicenseClient` trait so the flow (call service -> persist -> re-evaluate)
//! is testable with a fake, without a live HTTP call - mirrors emittiv-staff's
//! `license-service.ts` (DbLike injection) on the producer side.

use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
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

/// Attempt a renew for an already-activated install. On success, persists the
/// fresh token. On `ClientError::Rejected` (offboarded) or any transport
/// failure (offline), the EXISTING stored token is left untouched - it keeps
/// gating on its own expiry, which is the grace window the spec calls for -
/// and the current (pre-renew) state is returned rather than surfacing the
/// renew failure as an app-blocking error.
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
        Err(e) => {
            log::info!(
                "license renew deferred (not fatal - existing token still valid until its own expiry): {e:?}"
            );
            gate::evaluate(Some(&stored.token), device_fingerprint, public_key, now)
        }
    }
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

    #[tokio::test]
    async fn renew_offboarded_keeps_existing_token() {
        let (sk, vk) = test_keypair(1);
        let dir = scratch_dir();
        // Still valid, but inside the renew window - the offboard refusal
        // must not destroy it; it keeps gating on its own expiry.
        let existing_token = token_for(&sk, "device-1", 2);
        let stored = StoredLicense {
            code: "CODE-1".to_string(),
            token: existing_token.clone(),
        };
        store::save(&dir, &stored).unwrap();

        let client = FakeClient {
            issue_result: Mutex::new(None),
            renew_result: Mutex::new(Some(Err(ClientError::Rejected("staff_not_active".to_string())))),
        };
        let state = renew(&client, &dir, &stored, "device-1", &vk, Utc::now()).await;
        assert!(state.is_valid(), "expected still-valid grace window, got {state:?}");

        let after = store::load(&dir).unwrap().expect("token untouched");
        assert_eq!(after.token, existing_token, "renew rejection must not overwrite the stored token");
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

    #[tokio::test]
    async fn renew_transport_failure_keeps_existing_token() {
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
        assert_eq!(store::load(&dir).unwrap().unwrap().token, existing_token);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
