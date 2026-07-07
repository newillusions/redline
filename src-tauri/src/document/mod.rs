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
    /// True if this open required a password - whether it was passed in
    /// explicitly, reused from the frontend's session cache, or auto-tried
    /// from the known-password store (`document::known_passwords`). The
    /// frontend can't otherwise tell an auto-tried-transparently open apart
    /// from a plain PDF, and needs to know for the "Save Unprotected Copy"
    /// button + the on-open save-unprotected-copy prompt.
    pub was_encrypted: bool,
}

/// Sentinel error strings returned by the `open_document` command when a PDF is
/// password-protected. Every other `open_document` failure keeps the existing
/// free-form `format!("{:#}", e)` message - these two are the only machine-checked
/// values, matching this codebase's `Result<T, String>` command convention (no
/// custom Tauri error-serialization type exists anywhere else in this crate).
/// The frontend (`src/lib/ipc.ts`) checks for these exact strings to decide
/// whether to show the password dialog vs a generic error banner - keep in sync.
pub const ERR_PASSWORD_REQUIRED: &str = "PASSWORD_REQUIRED";
pub const ERR_WRONG_PASSWORD: &str = "WRONG_PASSWORD";

pub mod annots;
pub mod known_passwords;
pub mod page_ops;
pub mod save;
pub mod store;

/// Generate a fresh doc_id.
pub fn new_doc_id() -> String {
    Uuid::new_v4().to_string()
}
