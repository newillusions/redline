//! HTTP calls to the emittiv-staff license service (`/api/license/{issue,renew}`,
//! S2a). The base URL resolves in three tiers, checked in order: (1) the
//! `REDLINE_LICENSE_API_URL` runtime env var, so dev/test can always override;
//! (2) a compile-time default baked in via `REDLINE_LICENSE_API_URL_DEFAULT`
//! (set in `.github/workflows/build-releases.yml`'s Windows release build step),
//! so a released binary activates the entitlement gate with no user-set env var;
//! (3) `ClientError::NotConfigured` when neither is present (local/dev/CI builds
//! that don't bake the default and don't set the runtime override). Never a
//! guessed/fabricated URL.

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
    /// Neither the `REDLINE_LICENSE_API_URL` runtime env var nor a compile-time
    /// baked default (`REDLINE_LICENSE_API_URL_DEFAULT`) is present - see
    /// `resolve_base_url`. Expected in local/dev/CI test builds; should not occur
    /// in a release build once the build workflow bakes the default in.
    NotConfigured,
    /// The service responded with a rejection (403/404 body carries the gate
    /// reason, e.g. "staff_not_active", "already_claimed" - see
    /// emittiv-staff's activation-gate.ts).
    Rejected(String),
    /// Transport/parse failure (network down, bad JSON, unexpected status).
    Transport(String),
}

/// Compile-time baked default (set via the `REDLINE_LICENSE_API_URL_DEFAULT` build-time
/// env var - see `.github/workflows/build-releases.yml`). `None` in any build that
/// doesn't set it (local dev, CI test builds), so `base_url()` falls through to
/// `NotConfigured` there unless the runtime env var is set.
const BAKED_DEFAULT: Option<&str> = option_env!("REDLINE_LICENSE_API_URL_DEFAULT");

/// Pure three-tier resolution: `env_override` (the runtime `REDLINE_LICENSE_API_URL`
/// value, if set) wins over `baked` (the compile-time default), which wins over
/// `NotConfigured`. Both inputs are passed in rather than read internally so this is a
/// plain function of its arguments - a test-only injection seam, since `option_env!` is
/// fixed at THIS binary's compile time and can't be toggled per test, and mutating the
/// real process env var would race under Rust's parallel test runner.
fn resolve_base_url(env_override: Option<String>, baked: Option<&str>) -> Result<String, ClientError> {
    if let Some(v) = env_override {
        return Ok(v);
    }
    if let Some(v) = baked {
        return Ok(v.to_string());
    }
    Err(ClientError::NotConfigured)
}

fn base_url() -> Result<String, ClientError> {
    resolve_base_url(std::env::var("REDLINE_LICENSE_API_URL").ok(), BAKED_DEFAULT)
}

/// Join a base URL and an `/api/license/<path>` endpoint, tolerating a trailing slash
/// (or several) on `base` so both `https://host` and `https://host/` resolve to the same
/// endpoint.
fn license_url(base: &str, path: &str) -> String {
    format!("{}/api/license/{}", base.trim_end_matches('/'), path)
}

async fn post_license(path: &str, code: &str, device_fingerprint: &str) -> Result<String, ClientError> {
    let base = base_url()?;
    let url = license_url(&base, path);
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- resolve_base_url: three-tier resolution ---------------------------------

    #[test]
    fn env_override_wins_over_baked_default() {
        let got = resolve_base_url(
            Some("https://dev-override.example".to_string()),
            Some("https://staff.emittiv.studio"),
        );
        assert_eq!(got, Ok("https://dev-override.example".to_string()));
    }

    #[test]
    fn baked_default_used_when_env_unset() {
        let got = resolve_base_url(None, Some("https://staff.emittiv.studio"));
        assert_eq!(got, Ok("https://staff.emittiv.studio".to_string()));
    }

    #[test]
    fn env_override_used_when_no_baked_default() {
        // Local/dev build (no REDLINE_LICENSE_API_URL_DEFAULT baked in) still honours
        // an explicitly-set runtime env var.
        let got = resolve_base_url(Some("https://dev.example".to_string()), None);
        assert_eq!(got, Ok("https://dev.example".to_string()));
    }

    #[test]
    fn not_configured_when_neither_env_nor_baked_default_present() {
        let got = resolve_base_url(None, None);
        assert_eq!(got, Err(ClientError::NotConfigured));
    }

    #[test]
    fn base_url_falls_back_to_baked_default_in_a_release_build() {
        // BAKED_DEFAULT reflects THIS test binary's own compile-time env, which is
        // unset for a plain `cargo test` - so base_url() (env unset in this process)
        // resolves exactly like resolve_base_url(None, BAKED_DEFAULT). This asserts
        // the wiring between base_url() and resolve_base_url() is correct without
        // needing to fake a real release build's compile-time env.
        std::env::remove_var("REDLINE_LICENSE_API_URL");
        assert_eq!(base_url(), resolve_base_url(None, BAKED_DEFAULT));
    }

    // --- license_url: trailing-slash normalization --------------------------------

    #[test]
    fn license_url_joins_bare_base() {
        assert_eq!(
            license_url("https://staff.emittiv.studio", "issue"),
            "https://staff.emittiv.studio/api/license/issue"
        );
    }

    #[test]
    fn license_url_normalizes_a_single_trailing_slash() {
        assert_eq!(
            license_url("https://staff.emittiv.studio/", "issue"),
            "https://staff.emittiv.studio/api/license/issue"
        );
    }

    #[test]
    fn license_url_normalizes_multiple_trailing_slashes() {
        assert_eq!(
            license_url("https://staff.emittiv.studio///", "renew"),
            "https://staff.emittiv.studio/api/license/renew"
        );
    }

    #[test]
    fn license_url_bare_and_trailing_slash_bases_produce_the_same_url() {
        assert_eq!(
            license_url("https://staff.emittiv.studio", "renew"),
            license_url("https://staff.emittiv.studio/", "renew"),
        );
    }
}
