//! Storage module — local-first file + version management (spec §4, §9, §15, §18).
//!
//! M4 S2 scope: per-file `.redline/history/` version snapshots, meta.json `versions`
//! array, retained-N pruning, version list + restore.
//!
//! Key design notes (spec §18):
//! - Snapshot filenames: `<7-digit-seq>__<iso-utc>__<5-char-id>.pdf`
//! - meta.json / markups.json: atomic write (temp file + rename).
//! - Retained-N: default 10. Prune runs after every successful snapshot.
pub mod versioning;

pub use versioning::{history_dir, list_versions, restore_version, save_version_snapshot};
