//! Stable per-install device fingerprint for license binding.
//!
//! Deliberately separate from `identity.rs`'s user_id: that one is a display
//! identity (editable alongside display_name); this one binds a license to a
//! specific machine and must never change once a token has claimed it - the
//! license service treats a fingerprint change as "different device" and
//! refuses to renew (emittiv-staff's activation-gate.ts `device_mismatch`).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceId {
    device_fingerprint: String,
}

fn device_id_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("license").join("device_id.json")
}

/// Load `<data_dir>/license/device_id.json`, generating + persisting one on
/// first run. A corrupt or unreadable file is replaced with a fresh id (never
/// hard-fails the app) - mirrors `identity::load_or_create`.
pub fn load_or_create(data_dir: &Path) -> Result<String, String> {
    let path = device_id_path(data_dir);
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(id) = serde_json::from_slice::<DeviceId>(&bytes) {
            return Ok(id.device_fingerprint);
        }
    }

    let id = DeviceId {
        device_fingerprint: Uuid::new_v4().to_string(),
    };
    let license_dir = data_dir.join("license");
    fs::create_dir_all(&license_dir).map_err(|e| format!("create license dir: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(&id).map_err(|e| e.to_string())?;
    if let Err(e) = fs::write(&tmp, &json) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("write device id: {e}"));
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("rename device id: {e}"));
    }
    Ok(id.device_fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("redline-device-{}", Uuid::new_v4()))
    }

    #[test]
    fn generates_then_reuses_device_id() {
        let dir = scratch_dir();
        let first = load_or_create(&dir).expect("first run generates");
        let second = load_or_create(&dir).expect("second run reuses");
        assert_eq!(first, second, "device fingerprint is stable across runs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_replaced() {
        let dir = scratch_dir();
        fs::create_dir_all(dir.join("license")).unwrap();
        fs::write(dir.join("license").join("device_id.json"), b"not json").unwrap();
        let id = load_or_create(&dir).expect("replaces corrupt file");
        assert!(!id.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
