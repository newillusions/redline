//! HTTP calls to the emittiv-staff license service (`/api/license/{issue,renew}`,
//! S2a). The service is not deployed yet - no live URL exists - so the base
//! URL is read from the `REDLINE_LICENSE_API_URL` env var at call time rather
//! than hardcoded, so wiring the real deployment in is a config change, not a
//! code change. Returns a clear error (never a guessed/fabricated URL) when
//! unset.

use serde::{Deserialize, Serialize};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
struct LicenseRequestBody<'a> {
    code: &'a str,
    device_fingerprint: &'a str,
}

#[derive(Debug, Deserialize)]
struct LicenseResponseBody {
    token: String,
}

#[derive(Debug, Deserialize)]
struct ErrorResponseBody {
    error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// `REDLINE_LICENSE_API_URL` is not configured - not yet wired to a live
    /// deploy (emittiv-staff has no Dockerfile / URL as of S2b).
    NotConfigured,
    /// The service responded with a rejection (403/404 body carries the gate
    /// reason, e.g. "staff_not_active", "already_claimed" - see
    /// emittiv-staff's activation-gate.ts).
    Rejected(String),
    /// Transport/parse failure (network down, bad JSON, unexpected status).
    Transport(String),
}

fn base_url() -> Result<String, ClientError> {
    std::env::var("REDLINE_LICENSE_API_URL").map_err(|_| ClientError::NotConfigured)
}

async fn post_license(path: &str, code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
    let base = base_url()?;
    let url = format!("{}/api/license/{}", base.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ClientError::Transport(e.to_string()))?;

    let response = client
        .post(&url)
        .json(&LicenseRequestBody {
            code,
            device_fingerprint,
        })
        .send()
        .await
        .map_err(|e| ClientError::Transport(e.to_string()))?;

    let status = response.status();
    if status.is_success() {
        let body: LicenseResponseBody = response
            .json()
            .await
            .map_err(|e| ClientError::Transport(format!("bad response body: {e}")))?;
        Ok(body.token)
    } else if status.as_u16() == 403 || status.as_u16() == 404 {
        let reason = response
            .json::<ErrorResponseBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_else(|_| "unknown".to_string());
        Err(ClientError::Rejected(reason))
    } else {
        Err(ClientError::Transport(format!("unexpected status {status}")))
    }
}

/// Claim a fresh token for an activation code (first claim, or idempotent
/// re-issue to the same device).
pub async fn issue(code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
    post_license("issue", code, device_fingerprint).await
}

/// Re-mint a token before it expires. Refuses (`Rejected`) if the staff
/// record has been offboarded since the last issue/renew - callers keep the
/// existing (still offline-valid) token until it actually expires; that
/// window is the grace period the S2b spec calls for.
pub async fn renew(code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
    post_license("renew", code, device_fingerprint).await
}
