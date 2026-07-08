//! Persisted activation state: the activation `code` (needed to call the
//! renew endpoint again later) plus the last-issued `token`. Atomic write
//! (temp + rename), following the same pattern as `storage::settings`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLicense {
    pub code: String,
    pub token: String,
}

fn license_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("license").join("activation.json")
}

/// Load the stored activation, if any. Returns `Ok(None)` when never
/// activated, or when the file exists but is corrupt - gating falls back to
/// `Missing` rather than hard-failing the app on launch either way.
pub fn load(data_dir: &Path) -> io::Result<Option<StoredLicense>> {
    let path = license_file_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    Ok(serde_json::from_slice::<StoredLicense>(&bytes).ok())
}

/// Persist the activation atomically (temp + rename).
pub fn save(data_dir: &Path, license: &StoredLicense) -> io::Result<()> {
    let license_dir = data_dir.join("license");
    fs::create_dir_all(&license_dir)?;
    let dest = license_file_path(data_dir);
    let tmp = license_dir.join(format!(
        ".activation-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let json = serde_json::to_vec_pretty(license)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Err(e) = fs::write(&tmp, &json) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("redline-license-store-{}", Uuid::new_v4()))
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = scratch_dir();
        assert_eq!(load(&dir).unwrap(), None);
    }

    #[test]
    fn round_trips_saved_license() {
        let dir = scratch_dir();
        let license = StoredLicense {
            code: "ABCD-1234".to_string(),
            token: "payload.sig".to_string(),
        };
        save(&dir, &license).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(license));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_overwrites_prior_activation() {
        let dir = scratch_dir();
        save(
            &dir,
            &StoredLicense {
                code: "OLD".to_string(),
                token: "old.tok".to_string(),
            },
        )
        .unwrap();
        save(
            &dir,
            &StoredLicense {
                code: "NEW".to_string(),
                token: "new.tok".to_string(),
            },
        )
        .unwrap();
        let loaded = load(&dir).unwrap().unwrap();
        assert_eq!(loaded.code, "NEW");
        assert_eq!(loaded.token, "new.tok");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_treated_as_none() {
        let dir = scratch_dir();
        fs::create_dir_all(dir.join("license")).unwrap();
        fs::write(dir.join("license").join("activation.json"), b"not json").unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
