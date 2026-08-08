//! Tauri IPC commands backing the About page's release-history + rollback
//! affordance. App-version display is entirely frontend-side
//! (`@tauri-apps/api/app`'s `getVersion()`) — the only backend surface
//! needed here is listing past releases and performing the actual rollback
//! install. See `updater_rollback` module doc for why a custom
//! commit-pinned updater endpoint is what makes a REAL downgrade possible
//! (not just a relabeled reinstall of latest).

use tauri_plugin_updater::UpdaterExt;

use crate::updater_rollback::{self, ReleaseInfo, DEFAULT_RELEASE_LIMIT};

#[tauri::command]
pub async fn list_available_releases() -> Result<Vec<ReleaseInfo>, String> {
    updater_rollback::list_releases(DEFAULT_RELEASE_LIMIT)
        .await
        .map_err(|e| e.to_string())
}

/// Roll back (or reinstall) to a specific past release. `manifest_url` MUST
/// be one returned by `list_available_releases` — it is commit-pinned, so
/// its signature is the real one the CI signing key produced for that
/// release. This never trusts an arbitrary caller-supplied URL beyond what
/// the updater plugin itself verifies via minisign against the app's baked
/// public key: a malicious/garbled URL just fails `check()`/`download_and_
/// install()` the same way a corrupt normal update would.
#[tauri::command]
pub async fn rollback_to_version(
    app_handle: tauri::AppHandle,
    manifest_url: String,
    target_version: String,
) -> Result<(), String> {
    let wanted_version = target_version.clone();
    let endpoint = reqwest::Url::parse(&manifest_url).map_err(|e| format!("invalid rollback manifest URL: {e}"))?;
    let updater = app_handle
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| format!("invalid rollback manifest URL: {e}"))?
        // The pinned manifest advertises exactly one version (the target
        // release). Accept it regardless of whether it's older, newer, or
        // equal to the running build — "available" here means "the caller
        // explicitly chose this exact version", not "newer than current".
        .version_comparator(move |_current, update| update.version.to_string() == wanted_version)
        .build()
        .map_err(|e| format!("failed to build rollback updater: {e}"))?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("rollback check failed: {e}"))?;

    match update {
        Some(update) => update
            .download_and_install(|_chunk_len, _total_len| {}, || {})
            .await
            .map_err(|e| format!("rollback download/install failed: {e}")),
        None => Err(format!(
            "no installable build found for v{target_version} - already running this exact version, or its pinned manifest could not be verified"
        )),
    }
}
