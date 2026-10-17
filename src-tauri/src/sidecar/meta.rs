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

/// Write `scales` as the scale records of the sidecar for `pdf_path`, keeping whatever else
/// that sidecar already holds (version history). Save As uses it to carry the calibration
/// to the new file.
pub fn save_scales(pdf_path: &Path, scales: &[ScaleRecord]) -> io::Result<()> {
    let mut meta = load_meta(pdf_path)?;
    meta.scales = scales.to_vec();
    save_meta(pdf_path, &meta)
}

/// What [`stage_scales`] replaced, so a later failure can put it back.
pub struct ScalesUndo {
    pdf_path: PathBuf,
    /// The previous `meta.json` bytes, or `None` if there was no sidecar file.
    previous: Option<Vec<u8>>,
}

impl ScalesUndo {
    /// Restore the sidecar as it was before [`stage_scales`] (best effort).
    pub fn undo(self) {
        let meta = sidecar_dir(&self.pdf_path).join("meta.json");
        match self.previous {
            Some(bytes) => {
                let _ = std::fs::write(&meta, bytes);
            }
            None => {
                let _ = std::fs::remove_file(&meta);
                let _ = std::fs::remove_dir(sidecar_dir(&self.pdf_path)); // only if empty
            }
        }
    }
}

/// [`save_scales`] that remembers what it replaced. Save As writes the calibration BEFORE the
/// new file is swapped in, so a sidecar failure aborts the save with nothing changed, and a
/// failed swap can [`ScalesUndo::undo`] it.
pub fn stage_scales(pdf_path: &Path, scales: &[ScaleRecord]) -> io::Result<ScalesUndo> {
    let previous = std::fs::read(sidecar_dir(pdf_path).join("meta.json")).ok();
    save_scales(pdf_path, scales)?;
    Ok(ScalesUndo { pdf_path: pdf_path.to_path_buf(), previous })
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

    #[test]
    fn save_scales_creates_the_destination_sidecar_and_keeps_its_history() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("a.pdf");
        let dest = dir.path().join("b.pdf");
        let scale = crate::takeoff::ScaleRecord::new(
            ScaleTarget::DocumentDefault,
            ScaleMethod::Preset,
            0.001,
            "m".into(),
            "1:1000".into(),
            2,
        );
        // A Save As over a file that already has history keeps that history.
        let mut existing = load_meta(&dest).unwrap();
        existing.next_version_seq = 7;
        save_meta(&dest, &existing).unwrap();

        save_scales(&dest, std::slice::from_ref(&scale)).unwrap();
        let got = load_meta(&dest).unwrap();
        assert_eq!(got.scales.len(), 1);
        assert_eq!(got.scales[0].id, scale.id);
        assert_eq!(got.next_version_seq, 7);
        assert!(load_meta(&src).unwrap().scales.is_empty(), "the source sidecar is untouched");
    }

    fn scale(label: &str) -> crate::takeoff::ScaleRecord {
        crate::takeoff::ScaleRecord::new(
            ScaleTarget::DocumentDefault,
            ScaleMethod::Preset,
            0.001,
            "m".into(),
            label.into(),
            2,
        )
    }

    #[test]
    fn stage_scales_fails_cleanly_when_the_sidecar_cannot_be_written() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("b.pdf");
        // A plain file where the sidecar directory must go.
        std::fs::write(sidecar_dir(&dest), b"in the way").unwrap();
        assert!(stage_scales(&dest, &[scale("1:100")]).is_err());
    }

    #[test]
    fn undoing_a_staged_sidecar_restores_what_was_there() {
        let dir = tempdir().unwrap();
        let fresh = dir.path().join("fresh.pdf");
        stage_scales(&fresh, &[scale("1:100")]).unwrap().undo();
        assert!(!sidecar_dir(&fresh).join("meta.json").exists(), "a new sidecar is removed again");

        let old = dir.path().join("old.pdf");
        save_scales(&old, &[scale("old")]).unwrap();
        stage_scales(&old, &[scale("new")]).unwrap().undo();
        assert_eq!(load_meta(&old).unwrap().scales[0].label, "old");
    }
}
