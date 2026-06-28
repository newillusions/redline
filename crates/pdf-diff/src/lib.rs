//! `pdf-diff` — headless two-tier PDF fidelity diff crate.
//!
//! Provides pixel-level and text-layer comparison of PDF page pairs.
//!
//! # No Tauri dependency
//! This crate contains ZERO Tauri code. It is designed to be consumed both by:
//! - redline M6 compare module (path dep inside the workspace)
//! - cad-export-api (Linux/axum, via Forgejo Cargo registry or git dep)
//!
//! Any attempt to add Tauri/webview deps here breaks the cad-export use-case.
//!
//! # Thread safety
//! [`PdfDiffEngine`] is `!Send + !Sync` because the underlying PDFium C library
//! uses process-global state that is NOT safe across threads. Rules:
//! - In synchronous code: use `PdfDiffEngine` directly on a single thread.
//! - In async (tokio) code: wrap with `tokio::task::spawn_blocking`.
//! - For Tauri commands: create one engine per compare call inside `spawn_blocking`.
//!
//! # Two-tier diff algorithm
//! Run text diff FIRST (tier 1) — it catches font-substitution, text reflow, and
//! field-value changes directly from the PDF text layer (Unicode-independent of
//! source font, so SHX/TTF substitution shows up as position/sequence change).
//! Then run pixel diff (tier 2) as a visual sanity check.
//!
//! # Usage example
//! ```no_run
//! use pdf_diff::{PdfDiffEngine, pixel_diff};
//! use std::path::Path;
//!
//! let mut engine = PdfDiffEngine::new().unwrap();
//! let doc_a = engine.open(Path::new("old.pdf")).unwrap();
//! let doc_b = engine.open(Path::new("new.pdf")).unwrap();
//!
//! // Tier 1: text-layer diff (fast, catches font substitution + text changes)
//! let text = engine.text_diff(&doc_a, &doc_b, 0, 0).unwrap();
//! if !text.char_sequence_match {
//!     println!("text changed on page 0");
//! }
//!
//! // Tier 2: pixel diff at 150 DPI (visual sanity check)
//! let img_a = engine.render_page_full(&doc_a, 0, 150.0).unwrap();
//! let img_b = engine.render_page_full(&doc_b, 0, 150.0).unwrap();
//! let px = pixel_diff(&img_a, &img_b, 5).unwrap();
//! println!("changed: {:.2}%, passed: {}", px.changed_pct, px.passed);
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, RgbImage};
use log::{info, warn};
use memmap2::Mmap;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// Result of a pixel-level comparison between two rendered page images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixelDiffResult {
    /// `true` when zero pixels differ beyond `tolerance` (images are identical
    /// within the antialiasing threshold — no visible difference).
    pub passed: bool,
    /// Percentage of pixels whose maximum channel delta exceeds `tolerance`
    /// (0.0 = identical, 100.0 = completely different).
    pub changed_pct: f32,
    /// Maximum per-pixel channel delta observed anywhere on the page (0–255).
    pub max_pixel_delta: u8,
    /// Diff-highlight image (same dimensions as inputs):
    /// - Changed pixels: red (R=220, G=30, B=30)
    /// - Unchanged pixels: 50% grey (average of the two pages)
    ///
    /// Omitted from Serde; encode to PNG externally for IPC transport.
    #[serde(skip)]
    pub diff_image: DynamicImage,
}

/// Result of a text-layer comparison between two PDF pages.
///
/// PDFium extracts Unicode text independently of the source font (SHX, TTF,
/// OTF all normalise to Unicode), so this catches font-substitution issues
/// that look identical to pixel diff but differ in the text layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDiffResult {
    /// `true` when the full character sequence matches between the two pages.
    /// `false` means text changed (added, removed, or reordered characters).
    pub char_sequence_match: bool,
    /// When `char_sequence_match` is `true`, per-character center-position
    /// deltas in PDF user-space points (y-up, origin bottom-left of page).
    /// Empty when sequences differ.
    pub position_deltas: Vec<CharDelta>,
}

/// Positional shift for a single character between two pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharDelta {
    pub ch: char,
    /// x-shift in PDF user-space points (positive = rightward in page_b).
    pub delta_x: f32,
    /// y-shift in PDF user-space points (positive = upward in page_b).
    pub delta_y: f32,
}

/// A single character extracted from a page with its center position.
#[derive(Debug, Clone)]
pub struct CharInfo {
    pub ch: char,
    /// Center x in PDF user-space points (y-up, origin bottom-left of page).
    pub x: f32,
    /// Center y in PDF user-space points.
    pub y: f32,
}

/// Opaque handle to a document open inside a [`PdfDiffEngine`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocId(String);

// ---------------------------------------------------------------------------
// Internal document storage
// ---------------------------------------------------------------------------

/// An open PDF document with its PDFium handle.
///
/// # Drop order (SAFETY-critical)
/// `document` holds PDFium page data that calls back into the C library on drop.
/// `_backing` is the mmap that `document` borrows for large-file loads.
/// Drop order MUST be: `document` first, then `_backing`.
/// Struct fields drop in declaration order — do NOT reorder these fields.
struct OpenDoc {
    /// SAFETY: the `'static` lifetime is a transmute lie — the document borrows
    /// from the engine's `Pdfium` bindings. It is sound as long as all `OpenDoc`
    /// instances are dropped before the owning `PdfDiffEngine` (which holds `pdfium`
    /// as its LAST field, ensuring it drops last).
    document: PdfDocument<'static>,
    /// Backing mmap for files >= MMAP_THRESHOLD. MUST outlive `document`.
    _backing: Option<Mmap>,
    page_count: u32,
    #[allow(dead_code)]
    path: PathBuf,
}

// ---------------------------------------------------------------------------
// PdfDiffEngine
// ---------------------------------------------------------------------------

/// Headless PDF diff engine. `!Send + !Sync` — PDFium uses process-global C state.
///
/// # Drop order (SAFETY-critical)
/// `docs` MUST drop before `pdfium` — PdfDocument/PdfPage call back into the
/// PDFium library on drop, so the library must still be loaded at that point.
/// Fields drop in declaration order: `docs` is declared first. Do NOT reorder.
pub struct PdfDiffEngine {
    /// Open documents keyed by opaque ID. MUST drop before `pdfium`.
    docs: HashMap<String, OpenDoc>,
    /// PDFium bindings — owns the shared library. MUST be the LAST field.
    pdfium: Pdfium,
}

impl PdfDiffEngine {
    /// Create a new engine.
    ///
    /// PDFium is loaded from:
    ///   1. `PDFIUM_DYNAMIC_LIB_PATH` environment variable (recommended).
    ///   2. System library search path as a fallback.
    ///
    /// Returns `Err` if PDFium cannot be loaded — this is a hard failure.
    pub fn new() -> Result<Self> {
        let bindings = if let Ok(p) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
            Pdfium::bind_to_library(p)
                .context("PDFium not found at PDFIUM_DYNAMIC_LIB_PATH")?
        } else {
            Pdfium::bind_to_system_library()
                .context("PDFium not found; set PDFIUM_DYNAMIC_LIB_PATH")?
        };
        info!("pdf-diff: PDFium loaded");
        Ok(Self {
            pdfium: Pdfium::new(bindings),
            docs: HashMap::new(),
        })
    }

    /// Open a PDF file and return an opaque [`DocId`] handle.
    ///
    /// Uses the same three-path strategy as redline's render engine:
    /// 1. Streaming `load_pdf_from_file` for files < 1.9 GiB.
    /// 2. Memory-mapped `load_pdf_from_byte_slice` for files >= 1.9 GiB.
    /// 3. lopdf normalize + reload for large files where page loads fail
    ///    (PDFium's signed 32-bit internal offset limit).
    pub fn open(&mut self, path: &Path) -> Result<DocId> {
        let id = unique_id(path);
        let file_len = std::fs::metadata(path)
            .with_context(|| format!("stat {:?}", path))?
            .len();

        let (doc, backing) = load_doc(&self.pdfium, path, file_len)?;
        let page_count = doc.pages().len() as u32;

        // Probe page 0. If it fails on a large file, run lopdf normalize.
        let probe_ok = doc.pages().get(0).is_ok();
        let (doc, backing, page_count) = if !probe_ok && file_len >= MMAP_THRESHOLD {
            warn!("pdf-diff: large PDF page load failed; normalizing via lopdf: {:?}", path);
            drop(doc);
            drop(backing);
            let norm_path = normalize_large_pdf(path)?;
            let norm_len = std::fs::metadata(&norm_path).map(|m| m.len()).unwrap_or(0);
            let (d, b) = load_doc(&self.pdfium, &norm_path, norm_len)?;
            let pc = d.pages().len() as u32;
            d.pages().get(0).context("normalized PDF still fails to load page 0 — possibly corrupt")?;
            (d, b, pc)
        } else {
            (doc, backing, page_count)
        };

        self.docs.insert(
            id.clone(),
            OpenDoc { document: doc, _backing: backing, page_count, path: path.to_path_buf() },
        );
        info!("pdf-diff: opened {:?} ({} pages) id={}", path, page_count, id);
        Ok(DocId(id))
    }

    /// Return the number of pages in an open document.
    pub fn page_count(&self, doc: &DocId) -> Option<u32> {
        self.docs.get(&doc.0).map(|d| d.page_count)
    }

    /// Close an open document and release its PDFium resources.
    pub fn close(&mut self, doc: DocId) {
        if self.docs.remove(&doc.0).is_some() {
            info!("pdf-diff: closed {}", doc.0);
        }
    }

    /// Render a complete page to a [`DynamicImage`] at the given DPI.
    ///
    /// `dpi = 150.0` gives good diff resolution without excessive memory.
    /// `dpi = 72.0` produces 1 px per PDF user-space point.
    ///
    /// The matrix path (M1.5 tile strategy from redline) is used with tile
    /// origin (0, 0), so the full page is rendered into a bitmap of
    /// `(page_w_pts * dpi/72) x (page_h_pts * dpi/72)` pixels.
    pub fn render_page_full(&mut self, doc: &DocId, page_idx: u32, dpi: f32) -> Result<DynamicImage> {
        let open_doc = self.docs.get_mut(&doc.0)
            .with_context(|| format!("unknown doc id: {}", doc.0))?;

        let page = open_doc.document.pages().get(page_idx as u16)
            .with_context(|| format!("page {} not found", page_idx))?;

        let scale = dpi / 72.0;
        let sz = page.page_size();
        let w = (sz.width().value * scale).round() as u32;
        let h = (sz.height().value * scale).round() as u32;

        anyhow::ensure!(w > 0 && h > 0, "page {} has zero dimensions", page_idx);

        // Full-page render: tile origin (0,0), pure scale — no translation offset.
        // pdfium-render's matrix path handles the PDF y-flip internally.
        let matrix = PdfMatrix::new(scale, 0.0, 0.0, scale, 0.0, 0.0);
        let config = PdfRenderConfig::new()
            .set_fixed_size(w as i32, h as i32)
            .render_form_data(false)
            .apply_matrix(matrix)
            .context("render matrix apply failed")?;

        let bitmap = page
            .render_with_config(&config)
            .context("PDFium render_with_config failed")?;

        Ok(bitmap.as_image())
    }

    /// Extract all characters with their center positions from a page.
    ///
    /// Skips control characters and characters with no Unicode representation.
    /// Positions are in PDF user-space points (y-up, origin bottom-left of page).
    pub fn extract_chars(&mut self, doc: &DocId, page_idx: u32) -> Result<Vec<CharInfo>> {
        let open_doc = self.docs.get_mut(&doc.0)
            .with_context(|| format!("unknown doc id: {}", doc.0))?;

        let page = open_doc.document.pages().get(page_idx as u16)
            .with_context(|| format!("page {} not found", page_idx))?;

        let page_text = page.text()
            .with_context(|| format!("text-layer load failed for page {}", page_idx))?;

        let mut chars = Vec::new();
        for ch in page_text.chars().iter() {
            let Some(c) = ch.unicode_char() else { continue };
            if c.is_control() { continue }
            // loose_bounds: full glyph box in PDF user-space (y-up).
            let Ok(bounds) = ch.loose_bounds() else { continue };
            let cx = (bounds.left().value + bounds.right().value) * 0.5;
            let cy = (bounds.bottom().value + bounds.top().value) * 0.5;
            chars.push(CharInfo { ch: c, x: cx, y: cy });
        }
        Ok(chars)
    }

    /// Two-tier text-layer diff between one page from each of two documents.
    ///
    /// Tier 1 (`char_sequence_match`): are the character sequences identical?
    /// Tier 2 (`position_deltas`): for matching sequences, how far did each
    /// character's center move between the two pages?
    ///
    /// **Run this BEFORE pixel diff.** Text diff is faster and catches font
    /// substitution, title-block field changes, and text reflow directly.
    pub fn text_diff(
        &mut self,
        doc_a: &DocId,
        doc_b: &DocId,
        page_a: u32,
        page_b: u32,
    ) -> Result<TextDiffResult> {
        let chars_a = self.extract_chars(doc_a, page_a)?;
        let chars_b = self.extract_chars(doc_b, page_b)?;

        let seq_a: String = chars_a.iter().map(|c| c.ch).collect();
        let seq_b: String = chars_b.iter().map(|c| c.ch).collect();
        let char_sequence_match = seq_a == seq_b;

        // Position deltas only when sequences match — otherwise the pairing is undefined.
        let position_deltas = if char_sequence_match {
            chars_a.iter().zip(chars_b.iter()).map(|(a, b)| CharDelta {
                ch: a.ch,
                delta_x: b.x - a.x,
                delta_y: b.y - a.y,
            }).collect()
        } else {
            Vec::new()
        };

        Ok(TextDiffResult { char_sequence_match, position_deltas })
    }
}

// ---------------------------------------------------------------------------
// Pixel diff (standalone — no PDFium, no engine required)
// ---------------------------------------------------------------------------

/// Compare two rendered page images pixel by pixel.
///
/// `tolerance`: per-channel delta that counts as "same" — handles antialiasing
/// and minor rendering non-determinism. Typical: 5 for AA, 0 for exact match.
///
/// `passed` is `true` when zero pixels differ beyond `tolerance` (images are
/// visually identical within the antialiasing threshold).
///
/// Returns `Err` when image dimensions differ — render both pages at the same
/// DPI with [`PdfDiffEngine::render_page_full`] to ensure matching sizes.
pub fn pixel_diff(img_a: &DynamicImage, img_b: &DynamicImage, tolerance: u8) -> Result<PixelDiffResult> {
    let (w, h) = img_a.dimensions();
    let (wb, hb) = img_b.dimensions();
    anyhow::ensure!(
        w == wb && h == hb,
        "image dimensions mismatch: {}x{} vs {}x{} — render both pages at the same DPI",
        w, h, wb, hb
    );

    let total = (w * h) as u64;
    let mut changed: u64 = 0;
    let mut max_delta: u8 = 0;

    // Diff image: grey (unchanged), red (changed).
    let mut diff_buf: Vec<u8> = vec![0u8; (w * h * 3) as usize];

    let rgb_a = img_a.to_rgb8();
    let rgb_b = img_b.to_rgb8();

    for y in 0..h {
        for x in 0..w {
            let pa = rgb_a.get_pixel(x, y);
            let pb = rgb_b.get_pixel(x, y);
            // Max channel delta across R, G, B.
            let d = pa.0.iter().zip(pb.0.iter())
                .map(|(&a, &b)| a.abs_diff(b))
                .max()
                .unwrap_or(0);
            if d > max_delta { max_delta = d; }
            let off = ((y * w + x) * 3) as usize;
            if d > tolerance {
                changed += 1;
                // Red highlight for changed pixels.
                diff_buf[off]     = 220;
                diff_buf[off + 1] = 30;
                diff_buf[off + 2] = 30;
            } else {
                // 50% grey blend for unchanged — makes surrounding context visible.
                let grey = ((pa.0[0] as u16 + pb.0[0] as u16) / 2) as u8;
                diff_buf[off]     = grey;
                diff_buf[off + 1] = grey;
                diff_buf[off + 2] = grey;
            }
        }
    }

    let changed_pct = if total == 0 {
        0.0
    } else {
        (changed as f64 / total as f64 * 100.0) as f32
    };

    let diff_image = DynamicImage::ImageRgb8(
        RgbImage::from_vec(w, h, diff_buf)
            .context("failed to construct diff image buffer (size mismatch?)")?
    );

    Ok(PixelDiffResult {
        passed: changed == 0,
        changed_pct,
        max_pixel_delta: max_delta,
        diff_image,
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Files at or above this size use mmap + FPDF_LoadMemDocument64 (64-bit clean)
/// instead of the streaming path (which has a signed 32-bit internal offset cap).
const MMAP_THRESHOLD: u64 = 1_900_000_000;

/// Load a PDF from `path`, choosing the load strategy by file size.
///
/// Returns `(PdfDocument<'static>, Option<Mmap>)`.
/// SAFETY: the `'static` is a transmute lie — the document borrows from `pdfium`
/// (for the streaming path) or from the returned `Mmap` (for the mmap path).
/// Callers must ensure the owning `Pdfium` and `Mmap` outlive the document.
fn load_doc(
    pdfium: &Pdfium,
    path: &Path,
    file_len: u64,
) -> Result<(PdfDocument<'static>, Option<Mmap>)> {
    if file_len >= MMAP_THRESHOLD {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open for mmap: {:?}", path))?;
        // SAFETY: file is read-only; mmap is kept alive in OpenDoc._backing.
        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("mmap: {:?}", path))?;
        let doc = pdfium
            .load_pdf_from_byte_slice(
                // SAFETY: byte slice borrows mmap, which lives in OpenDoc._backing
                // and therefore outlives the PdfDocument (both in the same OpenDoc,
                // with _backing declared after document — field drop order).
                unsafe { std::mem::transmute::<&[u8], &'static [u8]>(&mmap[..]) },
                None,
            )
            .with_context(|| format!("load large PDF via mmap: {:?}", path))?;
        let doc: PdfDocument<'static> = unsafe { std::mem::transmute(doc) };
        Ok((doc, Some(mmap)))
    } else {
        let doc = pdfium
            .load_pdf_from_file(path, None)
            .with_context(|| format!("open PDF: {:?}", path))?;
        // SAFETY: doc borrows pdfium (live for the engine's lifetime); streaming
        // reader is internal. 'static is tied to the engine lifetime via the
        // OpenDoc struct stored in PdfDiffEngine.docs.
        let doc: PdfDocument<'static> = unsafe { std::mem::transmute(doc) };
        Ok((doc, None))
    }
}

/// Normalize an oversized PDF via lopdf into a temp file.
/// One-time cost; see redline render/mod.rs for the full rationale.
fn normalize_large_pdf(path: &Path) -> Result<PathBuf> {
    let mut doc = lopdf::Document::load(path)
        .with_context(|| format!("lopdf load {:?}", path))?;
    let pruned = doc.prune_objects();
    if !pruned.is_empty() {
        info!("pdf-diff: normalize pruned {} unreferenced objects", pruned.len());
    }
    doc.compress();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "doc".into());
    let mut out = std::env::temp_dir();
    out.push(format!("pdf-diff-norm-{}-{}.pdf", stem, std::process::id()));
    doc.save(&out).with_context(|| format!("lopdf save to {:?}", out))?;
    Ok(out)
}

/// Generate a unique ID for an open document slot.
fn unique_id(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

// ---------------------------------------------------------------------------
// Unit tests (no PDFium or real PDFs required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn solid_image(r: u8, g: u8, b: u8, w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |_, _| Rgb([r, g, b])))
    }

    #[test]
    fn pixel_diff_identical_images_pass() {
        let img = solid_image(128, 128, 128, 100, 100);
        let result = pixel_diff(&img, &img, 0).unwrap();
        assert!(result.passed, "identical images must pass");
        assert_eq!(result.changed_pct, 0.0, "identical images: 0% changed");
        assert_eq!(result.max_pixel_delta, 0);
    }

    #[test]
    fn pixel_diff_different_images_fail() {
        let white = solid_image(255, 255, 255, 50, 50);
        let black = solid_image(0, 0, 0, 50, 50);
        let result = pixel_diff(&white, &black, 0).unwrap();
        assert!(!result.passed, "completely different images must fail");
        assert_eq!(result.changed_pct, 100.0);
        assert_eq!(result.max_pixel_delta, 255);
    }

    #[test]
    fn pixel_diff_tolerance_filters_small_delta() {
        // Image A: all pixels (100, 100, 100); Image B: all pixels (103, 103, 103).
        // Delta = 3. With tolerance=5 all pixels are "same"; with tolerance=2 all changed.
        let a = solid_image(100, 100, 100, 40, 40);
        let b = solid_image(103, 103, 103, 40, 40);
        let r5 = pixel_diff(&a, &b, 5).unwrap();
        assert!(r5.passed, "delta 3 <= tolerance 5 must pass");
        assert_eq!(r5.changed_pct, 0.0);

        let r2 = pixel_diff(&a, &b, 2).unwrap();
        assert!(!r2.passed, "delta 3 > tolerance 2 must fail");
        assert_eq!(r2.changed_pct, 100.0);
    }

    #[test]
    fn pixel_diff_dimension_mismatch_is_error() {
        let a = solid_image(0, 0, 0, 10, 10);
        let b = solid_image(0, 0, 0, 20, 20);
        assert!(pixel_diff(&a, &b, 0).is_err());
    }

    #[test]
    fn pixel_diff_diff_image_correct_size() {
        let a = solid_image(255, 0, 0, 30, 40);
        let b = solid_image(0, 255, 0, 30, 40);
        let r = pixel_diff(&a, &b, 0).unwrap();
        assert_eq!(r.diff_image.dimensions(), (30, 40));
    }

    #[test]
    fn text_diff_result_serde_round_trip() {
        let r = TextDiffResult {
            char_sequence_match: false,
            position_deltas: vec![CharDelta { ch: 'A', delta_x: 1.0, delta_y: -2.5 }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: TextDiffResult = serde_json::from_str(&json).unwrap();
        assert!(!back.char_sequence_match);
        assert_eq!(back.position_deltas.len(), 1);
        assert_eq!(back.position_deltas[0].ch, 'A');
    }

    #[test]
    fn engine_new_fails_gracefully_without_pdfium() {
        // Without PDFIUM_DYNAMIC_LIB_PATH and no system library, new() returns Err.
        // With a valid path it returns Ok. Both are acceptable; no panic.
        let _ = PdfDiffEngine::new(); // ok or err — never panics
    }
}
