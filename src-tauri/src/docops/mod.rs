//! DocOps module — swappable flatten/optimize/redact trait (spec §4, §8).
//!
//! M5 scope: `DocOps` trait with v1 baseline implementation (lopdf-backed).
//! Pluggable backend: MuPDF (AGPL — quarantined here behind the trait) or Apryse Advanced
//! slots in without caller changes. See spec §8 and §16 on licensing.
//!
//! M1: stub only — trait definition scaffolded.

use anyhow::Result;

/// The swappable document-surgery backend (spec §8).
pub trait DocOps: Send + Sync {
    /// Flatten annotation appearance streams into page content.
    fn flatten(&self, pdf_bytes: &[u8]) -> Result<Vec<u8>>;

    /// Strip unused objects + recompress streams.
    /// Note: deep image downsampling is out of scope for the v1 free baseline.
    fn optimize(&self, pdf_bytes: &[u8], level: u8) -> Result<Vec<u8>>;

    /// Rasterize-the-region redaction (safe v1 floor — not a drawn black box).
    /// True vector redaction only via a mature engine behind this trait.
    fn redact(&self, pdf_bytes: &[u8], regions: &[RedactRegion]) -> Result<Vec<u8>>;
}

/// A page region to redact (PDF user space).
#[derive(Debug, Clone)]
pub struct RedactRegion {
    pub page_index: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
