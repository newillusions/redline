//! Storage module — local-first file + version management (spec §4, §9, §15, §18).
//!
//! M4 scope: per-file `.redline/` sidecar (meta.json, audit.ndjson, markups.json,
//! history/), atomic writes (temp + rename — never in-place for sensitive state),
//! retained-N revision snapshots. Set definition (`<setname>.redlineset.json`).
//!
//! M1: stub only.
//!
//! Key design notes (spec §18):
//! - audit.ndjson: append-only (O_APPEND), grows monotonically — never rewrite.
//! - meta.json / markups.json: atomic write (temp file + rename).
//! - Records key on stable markup `id` (= PDF /NM) for external-edit reconciliation.
