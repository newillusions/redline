//! Tauri IPC commands for redline's S2b client entitlement gate.
//!
//! Thin wrappers only - the actual activate/renew orchestration (call the
//! license service -> persist -> re-evaluate) lives in `license::service`,
//! parameterized over `LicenseClient` so it's testable without a live HTTP
//! call. See `license/service.rs` doc comment.

use tauri::Manager;

use crate::license::gate::{self, LicenseState};
use crate::license::service::{self, HttpLicenseClient};
use crate::license::store;
use crate::license::token::parse_public_key_pem;
use crate::license::LICENSE_PUBLIC_KEY_PEM;

fn data_dir(app_handle: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))
}

async fn resolve_device_fingerprint(dir: std::path::PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::license::device::load_or_create(&dir))
        .await
        .map_err(|e| e.to_string())?
}

async fn resolve_stored_license(
    dir: std::path::PathBuf,
) -> Result<Option<store::StoredLicense>, String> {
    tokio::task::spawn_blocking(move || store::load(&dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

fn client_error_message(err: crate::license::client::ClientError) -> String {
    use crate::license::client::ClientError;
    match err {
        ClientError::NotConfigured => {
            "License service is not configured (REDLINE_LICENSE_API_URL unset) - contact the administrator".to_string()
        }
        ClientError::Rejected(reason) => format!("Activation refused: {reason}"),
        ClientError::Transport(msg) => format!("Could not reach the license service: {msg}"),
    }
}

/// Startup gate check: is there a valid, device-bound, unexpired token?
/// Called once on app launch, and again after `activate_license` succeeds.
#[tauri::command]
pub async fn license_status(app_handle: tauri::AppHandle) -> Result<LicenseState, String> {
    let dir = data_dir(&app_handle)?;
    let device_fingerprint = resolve_device_fingerprint(dir.clone()).await?;
    let stored = resolve_stored_license(dir).await?;
    let public_key = parse_public_key_pem(LICENSE_PUBLIC_KEY_PEM);
    Ok(gate::evaluate(
        stored.as_ref().map(|s| s.token.as_str()),
        &device_fingerprint,
        &public_key,
        chrono::Utc::now(),
    ))
}

/// Claim a token for a freshly entered activation code.
#[tauri::command]
pub async fn activate_license(app_handle: tauri::AppHandle, code: String) -> Result<LicenseState, String> {
    let dir = data_dir(&app_handle)?;
    let device_fingerprint = resolve_device_fingerprint(dir.clone()).await?;
    let public_key = parse_public_key_pem(LICENSE_PUBLIC_KEY_PEM);

    service::activate(
        &HttpLicenseClient,
        &dir,
        &code,
        &device_fingerprint,
        &public_key,
        chrono::Utc::now(),
    )
    .await
    .map_err(|e| match e {
        service::ActivateError::Client(ce) => client_error_message(ce),
        service::ActivateError::Persist(msg) => format!("Failed to save license: {msg}"),
    })
}

/// Attempt a renew (call when `license_status`/`activate_license` reported
/// `renew_due: true`). Never blocks the app: on an offline/rejected renew,
/// the existing token's own expiry keeps gating (the grace window).
#[tauri::command]
pub async fn renew_license(app_handle: tauri::AppHandle) -> Result<LicenseState, String> {
    let dir = data_dir(&app_handle)?;
    let device_fingerprint = resolve_device_fingerprint(dir.clone()).await?;
    let stored = resolve_stored_license(dir.clone()).await?;

    let Some(stored) = stored else {
        return Ok(LicenseState::Missing);
    };

    let public_key = parse_public_key_pem(LICENSE_PUBLIC_KEY_PEM);
    Ok(service::renew(
        &HttpLicenseClient,
        &dir,
        &stored,
        &device_fingerprint,
        &public_key,
        chrono::Utc::now(),
    )
    .await)
}
