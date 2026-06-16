//! Minimal app-configured user identity (spec §6 / §12 g): a stable `user_id` (UUID)
//! plus an editable display name, generated on first run and persisted atomically.
//! S4 promotes this to the full user_id <-> display-name registry; the shape here is
//! forward-compatible (matches markup::UserRef).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: Uuid,
    pub display_name: String,
}

fn default_display_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "User".to_string())
}

/// Load `<dir>/identity.json`, generating + persisting one on first run. A corrupt or
/// unreadable file is replaced with a fresh identity (never hard-fails the app).
pub fn load_or_create(dir: &Path) -> Result<Identity, String> {
    let path = dir.join("identity.json");
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(id) = serde_json::from_slice::<Identity>(&bytes) {
            return Ok(id);
        }
    }
    let id = Identity {
        user_id: Uuid::new_v4(),
        display_name: default_display_name(),
    };
    fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(&id).map_err(|e| e.to_string())?;
    fs::write(&tmp, json).map_err(|e| format!("write identity: {e}"))?;
    // Clean up the staged file if the rename fails, so a later run never trips over an
    // orphaned tmp and the next attempt starts clean (mirrors the save pipeline).
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("rename identity: {e}"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_then_reuses_identity() {
        let dir = std::env::temp_dir().join(format!("redline-id-{}", Uuid::new_v4()));
        let first = load_or_create(&dir).expect("first run generates");
        let second = load_or_create(&dir).expect("second run reuses");
        assert_eq!(first, second, "identity is stable across runs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_replaced() {
        let dir = std::env::temp_dir().join(format!("redline-id-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("identity.json"), b"not json").unwrap();
        let id = load_or_create(&dir).expect("replaces corrupt file");
        assert!(!id.display_name.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
