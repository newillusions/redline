//! Search module — Tantivy folder/library full-text index (spec §4, §14).
//!
//! M4 scope: persistent Tantivy index over extracted + OCR'd text; incremental
//! re-index via file watcher (notify crate); query → file / page / snippet results.
//!
//! M1: stub only.
//!
//! Feature gate: enable with `--features search` (adds tantivy dep).
