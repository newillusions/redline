//! Version snapshot on save (spec §15, §18).
//!
//! Every call to `save_version_snapshot` copies the pre-save PDF to
//! `.redline/history/<7-digit-seq>__<iso-utc>__<5-char-id>.pdf`
//! and appends a `VersionRecord` to `meta.json`.  After writing it
//! prunes the history directory so only the most recent `retain_n`
//! snapshots are kept (oldest deleted first).
//!
//! `restore_version` copies the named snapshot back over the live PDF
//! atomically (temp + rename), then removes the snapshot from the
//! history directory and the versions array in meta.json.
//!
//! ## Invariants enforced by tests
//!
//! - `save_version_snapshot`: snapshot file present; meta.json updated;
//!   sequence number increments; prune removes oldest when > retain_n.
//! - `list_versions`: returns records in newest-first order.
//! - `restore_version`: snapshot copied atomically; record removed from meta;
//!   snapshot file deleted; errors on unknown version_id.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::sidecar::{load_meta, save_meta, sidecar_dir, VersionRecord};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return the history directory inside the sidecar: `<file>.redline/history/`.
pub fn history_dir(pdf_path: &Path) -> PathBuf {
    sidecar_dir(pdf_path).join("history")
}

/// Save a version snapshot of `pdf_path` before it is overwritten.
///
/// - Creates the history directory if absent.
/// - Copies the current PDF into `history/<seq>__<ts>__<id>.pdf`.
/// - Appends the new `VersionRecord` to `meta.json`.
/// - Prunes oldest snapshots so at most `retain_n` remain.
///
/// Returns the created `VersionRecord`.
pub fn save_version_snapshot(
    pdf_path: &Path,
    label: Option<String>,
    retain_n: usize,
) -> io::Result<VersionRecord> {
    let history = history_dir(pdf_path);
    fs::create_dir_all(&history)?;

    // Load existing meta to get the monotonic sequence counter.
    let mut meta = load_meta(pdf_path).unwrap_or_default();

    // Increment counter before use so it's never reused even after pruning.
    meta.next_version_seq += 1;
    let seq = meta.next_version_seq;
    let ts = Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let id = short_id();
    let filename = format!("{seq:07}__{ts}__{id}.pdf");

    let dest = history.join(&filename);
    fs::copy(pdf_path, &dest)?;

    let record = VersionRecord {
        id: id.clone(),
        created_at: Utc::now().to_rfc3339(),
        label,
        filename: filename.clone(),
    };
    meta.versions.push(record.clone());
    save_meta(pdf_path, &meta)?;

    // Prune oldest snapshots beyond retain_n.
    prune_history(pdf_path, retain_n)?;

    Ok(record)
}

/// List version records for `pdf_path`, newest first.
pub fn list_versions(pdf_path: &Path) -> io::Result<Vec<VersionRecord>> {
    let meta = load_meta(pdf_path).unwrap_or_default();
    let mut records = meta.versions.clone();
    records.reverse();
    Ok(records)
}

/// Restore the snapshot identified by `version_id` back over the live PDF.
///
/// Steps (atomic):
/// 1. Locate the record in meta.json.
/// 2. Copy snapshot → `<pdf>.redline-restore-tmp` then rename onto the live path.
/// 3. Remove the snapshot file from history.
/// 4. Remove the record from meta.json and persist.
///
/// Errors if `version_id` is not found in meta.json.
pub fn restore_version(pdf_path: &Path, version_id: &str) -> io::Result<()> {
    let mut meta = load_meta(pdf_path)?;

    let pos = meta
        .versions
        .iter()
        .position(|r| r.id == version_id)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("version not found: {version_id}"),
            )
        })?;

    let record = meta.versions[pos].clone();
    let snapshot = history_dir(pdf_path).join(&record.filename);

    // Atomic restore: copy snapshot to a sibling temp, then rename onto live path.
    let parent = pdf_path
        .parent()
        .ok_or_else(|| io::Error::other("no parent dir"))?;
    let tmp = parent.join(format!(
        ".redline-restore-{}-{}.tmp",
        std::process::id(),
        short_id()
    ));
    let restore_result = (|| -> io::Result<()> {
        fs::copy(&snapshot, &tmp)?;
        fs::rename(&tmp, pdf_path)?;
        Ok(())
    })();
    if restore_result.is_err() {
        let _ = fs::remove_file(&tmp);
        return restore_result;
    }

    // Remove snapshot file and strip the record from meta.
    let _ = fs::remove_file(&snapshot);
    meta.versions.remove(pos);
    save_meta(pdf_path, &meta)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Generate a short 5-character alphanumeric ID for snapshot filenames.
fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Mix pid + nanos for a low-collision id without an extra dep.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    let val = (nanos as u64)
        .wrapping_mul(2654435761)
        .wrapping_add(pid as u64);
    let charset: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..5)
        .map(|i| {
            let idx = ((val >> (i * 5)) as usize) % charset.len();
            charset[idx] as char
        })
        .collect()
}

/// Delete oldest snapshot files (+ meta records) beyond `retain_n`.
fn prune_history(pdf_path: &Path, retain_n: usize) -> io::Result<()> {
    if retain_n == 0 {
        return Ok(());
    }
    let mut meta = load_meta(pdf_path).unwrap_or_default();
    if meta.versions.len() <= retain_n {
        return Ok(());
    }
    let to_delete = meta.versions.len() - retain_n;
    let history = history_dir(pdf_path);
    for record in meta.versions.drain(..to_delete) {
        let _ = fs::remove_file(history.join(&record.filename));
    }
    save_meta(pdf_path, &meta)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Create a minimal fake PDF at the given path so fs::copy has something to copy.
    fn make_fake_pdf(path: &Path) {
        fs::write(path, b"%PDF-1.4\n%%EOF\n").unwrap();
    }

    #[test]
    fn snapshot_creates_file_and_record() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let rec = save_version_snapshot(&pdf, None, 10).unwrap();

        // Snapshot file exists.
        let snap = history_dir(&pdf).join(&rec.filename);
        assert!(snap.exists(), "snapshot file must exist");

        // Meta record persisted.
        let meta = load_meta(&pdf).unwrap();
        assert_eq!(meta.versions.len(), 1);
        assert_eq!(meta.versions[0].id, rec.id);
    }

    #[test]
    fn snapshot_filename_includes_seq_and_id() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let rec = save_version_snapshot(&pdf, None, 10).unwrap();
        // Filename should start with "0000001__"
        assert!(
            rec.filename.starts_with("0000001__"),
            "first snapshot seq must be 0000001, got {}",
            rec.filename
        );
        assert!(rec.filename.ends_with(".pdf"), "must have .pdf extension");
    }

    #[test]
    fn sequence_increments_across_snapshots() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let r1 = save_version_snapshot(&pdf, None, 10).unwrap();
        let r2 = save_version_snapshot(&pdf, None, 10).unwrap();
        let r3 = save_version_snapshot(&pdf, None, 10).unwrap();

        assert!(r1.filename.starts_with("0000001__"), "got {}", r1.filename);
        assert!(r2.filename.starts_with("0000002__"), "got {}", r2.filename);
        assert!(r3.filename.starts_with("0000003__"), "got {}", r3.filename);
    }

    #[test]
    fn label_persisted_in_record() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let rec = save_version_snapshot(&pdf, Some("pre-issue".into()), 10).unwrap();
        assert_eq!(rec.label.as_deref(), Some("pre-issue"));

        let meta = load_meta(&pdf).unwrap();
        assert_eq!(meta.versions[0].label.as_deref(), Some("pre-issue"));
    }

    #[test]
    fn prune_keeps_only_retain_n_snapshots() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        for _ in 0..5 {
            save_version_snapshot(&pdf, None, 3).unwrap();
        }

        // After 5 saves with retain_n=3, only 3 snapshots remain.
        let meta = load_meta(&pdf).unwrap();
        assert_eq!(meta.versions.len(), 3, "prune should keep only 3");

        // The 3 remaining should be the newest (seq 3, 4, 5).
        assert!(
            meta.versions[0].filename.starts_with("0000003__"),
            "oldest kept must be seq 3, got {}",
            meta.versions[0].filename
        );
        assert!(
            meta.versions[2].filename.starts_with("0000005__"),
            "got {}",
            meta.versions[2].filename
        );
    }

    #[test]
    fn prune_removes_snapshot_files_on_disk() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let r1 = save_version_snapshot(&pdf, None, 2).unwrap();
        let _r2 = save_version_snapshot(&pdf, None, 2).unwrap();
        let r3 = save_version_snapshot(&pdf, None, 2).unwrap();

        // r1 should be pruned (only r2+r3 kept).
        let snap1 = history_dir(&pdf).join(&r1.filename);
        let snap3 = history_dir(&pdf).join(&r3.filename);
        assert!(!snap1.exists(), "pruned snapshot must be deleted from disk");
        assert!(snap3.exists(), "retained snapshot must still exist");
    }

    #[test]
    fn list_versions_newest_first() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        for _ in 0..3 {
            save_version_snapshot(&pdf, None, 10).unwrap();
        }

        let versions = list_versions(&pdf).unwrap();
        assert_eq!(versions.len(), 3);
        // Newest-first: seq 3 comes before seq 1.
        assert!(
            versions[0].filename.starts_with("0000003__"),
            "newest-first expected, got {}",
            versions[0].filename
        );
        assert!(
            versions[2].filename.starts_with("0000001__"),
            "got {}",
            versions[2].filename
        );
    }

    #[test]
    fn restore_version_replaces_pdf_atomically() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");

        // Write "version A" content.
        fs::write(&pdf, b"%PDF A").unwrap();
        let rec = save_version_snapshot(&pdf, None, 10).unwrap();

        // Overwrite live PDF with "version B".
        fs::write(&pdf, b"%PDF B").unwrap();

        // Restore back to the snapshot.
        restore_version(&pdf, &rec.id).unwrap();

        let content = fs::read(&pdf).unwrap();
        assert_eq!(content, b"%PDF A", "restore must put snapshot content back");
    }

    #[test]
    fn restore_removes_snapshot_file_and_record() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let rec = save_version_snapshot(&pdf, None, 10).unwrap();
        let snap = history_dir(&pdf).join(&rec.filename);
        assert!(snap.exists());

        restore_version(&pdf, &rec.id).unwrap();

        // Snapshot file deleted.
        assert!(!snap.exists(), "snapshot must be deleted after restore");
        // Record removed from meta.
        let meta = load_meta(&pdf).unwrap();
        assert!(
            meta.versions.is_empty(),
            "version record must be removed from meta"
        );
    }

    #[test]
    fn restore_unknown_id_returns_error() {
        let dir = tempdir().unwrap();
        let pdf = dir.path().join("plans.pdf");
        make_fake_pdf(&pdf);

        let result = restore_version(&pdf, "nope123");
        assert!(result.is_err(), "restoring unknown id must error");
    }

    // -----------------------------------------------------------------------
    // Versioning guard (backlog #9): snapshot/restore must not silently strip
    // or corrupt an encrypted PDF's password protection.
    //
    // Both `save_version_snapshot` and `restore_version` use `fs::copy` -
    // a raw byte copy, never an lopdf load->save re-serialization. That's
    // the property this test proves: an encrypted PDF's bytes (and therefore
    // its `/Encrypt` dict and the password needed to open it) survive a
    // snapshot + restore round-trip untouched, unlike `save_with_markups`
    // (document::save), which re-serializes via lopdf and therefore refuses
    // encrypted sources outright rather than risk silently stripping them.
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_and_restore_preserve_encrypted_pdf_password_state() {
        use crate::document::annots::tests::encrypted_one_page_doc;

        let dir = tempdir().unwrap();
        let pdf = dir.path().join("protected.pdf");

        let mut doc = encrypted_one_page_doc("redline-pw", "owner-pw");
        doc.save(&pdf).unwrap();
        let original_bytes = fs::read(&pdf).unwrap();

        // Snapshot the encrypted file, then overwrite the live path with
        // different (plain) content - simulating an edit made after the
        // snapshot was taken.
        let rec = save_version_snapshot(&pdf, None, 10).unwrap();
        fs::write(&pdf, b"%PDF overwritten, not encrypted").unwrap();

        // Restore the encrypted snapshot back over the live path.
        restore_version(&pdf, &rec.id).unwrap();

        // Byte-identical to the original encrypted file - fs::copy, not a
        // re-parse, so nothing about the /Encrypt dict could have changed.
        let restored_bytes = fs::read(&pdf).unwrap();
        assert_eq!(
            restored_bytes, original_bytes,
            "restore must byte-for-byte reproduce the encrypted snapshot"
        );

        // And it still actually decrypts with the original password via lopdf.
        let mut reloaded = lopdf::Document::load(&pdf).unwrap();
        assert!(
            reloaded.is_encrypted(),
            "restored file must still be encrypted"
        );
        reloaded
            .decrypt("redline-pw")
            .expect("restored file must decrypt with the original password");
    }
}
