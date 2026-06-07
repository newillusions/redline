//! OCR module — Tesseract invisible-text layer generation (spec §4, §14).
//!
//! M4 scope: rasterize page → run Tesseract (via leptess) → write invisible text layer
//! so scanned plans become searchable. On-demand and batch modes.
//!
//! M1: stub only.
//!
//! Feature gate: enable with `--features ocr` (adds leptess + native tesseract dep).
//! See Cargo.toml [features].
