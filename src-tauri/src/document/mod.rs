//! Document module — PDF parse/model, open/save, page manipulation (spec §4).
//!
//! M1: minimal — just open/close wiring to the render engine.
//! M2+: page insertion/deletion/rotation/reorder/extract/crop/merge via PDFium + lopdf.
//! M4+: sets navigation, version hooks, local-first save.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Metadata returned when a document is opened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentInfo {
    /// Opaque handle used in all subsequent calls.
    pub doc_id: String,
    pub path: String,
    pub page_count: u32,
}

/// Generate a fresh doc_id.
pub fn new_doc_id() -> String {
    Uuid::new_v4().to_string()
}
