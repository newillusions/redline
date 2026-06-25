//! Sidecar `meta.json` — schema version + scale records (spec §18).
//! Atomic write: write to temp file then rename, so crashes leave the old file intact.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::takeoff::ScaleRecord;

/// A single persisted version record inside `meta.json` (spec §18).
/// Defined here (alongside SidecarMeta) to avoid a circular dep with the storage module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VersionRecord {
    /// Short random identifier — used as the restore key.
    pub id: String,
    /// ISO-8601 UTC timestamp at the moment the snapshot was taken.
    pub created_at: String,
    /// Optional human-readable label ("pre-issue", "client v2", …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Snapshot filename (basename only — the history dir is implicit).
    pub filename: String,
}

/// Persisted sidecar metadata (subset of spec §18 — M3 adds scales; M4 S2 adds versions).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarMeta {
    pub schema_version: u32,
    #[serde(default)]
    pub scales: Vec<ScaleRecord>,
    /// Monotonically increasing counter for the NEXT version sequence number.
    /// Stored explicitly so pruning never causes a seq reuse.
    #[serde(default)]
    pub next_version_seq: u64,
    /// Version snapshot records, oldest-first. Added in M4 S2.
    #[serde(default)]
    pub versions: Vec<VersionRecord>,
}

/// Return the sidecar directory path for the given PDF path: `<file>.redline/`.
pub fn sidecar_dir(pdf_path: &Path) -> PathBuf {
    let stem = pdf_path.file_name().unwrap_or_default().to_string_lossy();
    let dir = pdf_path.parent().unwrap_or(Path::new("."));
    dir.join(format!("{stem}.redline"))
}

/// Load `meta.json` from the sidecar dir. Returns `SidecarMeta::default()` if absent.
pub fn load_meta(pdf_path: &Path) -> io::Result<SidecarMeta> {
    let path = sidecar_dir(pdf_path).join("meta.json");
    if !path.exists() {
        return Ok(SidecarMeta {
            schema_version: 1,
            scales: vec![],
            next_version_seq: 0,
            versions: vec![],
        });
    }
    let s = std::fs::read_to_string(&path)?;
    serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Persist `meta.json` atomically (temp file + rename).
pub fn save_meta(pdf_path: &Path, meta: &SidecarMeta) -> io::Result<()> {
    let dir = sidecar_dir(pdf_path);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join("meta.json");
    let tmp = dir.join("meta.json.tmp");
    let s = serde_json::to_string_pretty(meta)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, s)?;
    std::fs::rename(tmp, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::takeoff::{ScaleMethod, ScaleTarget};
    use tempfile::tempdir;

    #[test]
    fn round_trip_empty_meta() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        // No sidecar yet — load returns default
        let meta = load_meta(&pdf).unwrap();
        assert_eq!(meta.scales.len(), 0);
        assert_eq!(meta.schema_version, 1);
    }

    #[test]
    fn round_trip_with_scales() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        let mut meta = SidecarMeta {
            schema_version: 1,
            scales: vec![],
            next_version_seq: 0,
            versions: vec![],
        };
        meta.scales.push(crate::takeoff::ScaleRecord::new(
            ScaleTarget::DocumentDefault,
            ScaleMethod::Preset,
            0.001,
            "m".into(),
            "1:1000".into(),
            2,
        ));
        save_meta(&pdf, &meta).unwrap();
        let loaded = load_meta(&pdf).unwrap();
        assert_eq!(loaded.scales.len(), 1);
        assert!((loaded.scales[0].ratio - 0.001).abs() < 1e-9);
        assert_eq!(loaded.scales[0].label, "1:1000");
    }

    #[test]
    fn sidecar_dir_naming() {
        let pdf = Path::new("/work/plans-L3.pdf");
        let sd = sidecar_dir(pdf);
        assert_eq!(sd.file_name().unwrap(), "plans-L3.pdf.redline");
    }
}
