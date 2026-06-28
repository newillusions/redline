//! Compare module — page-pair alignment + two-tier diff rendering (spec §4, §10).
//!
//! M6 / Phase 1.1: two-tier diff (text-layer + pixel) via the headless `pdf-diff`
//! workspace crate. The `pdf-diff` crate is Tauri-free and shared with `cad-export-api`
//! (Linux/axum). This module provides the Tauri-aware wrapper: it runs `PdfDiffEngine`
//! on a `spawn_blocking` thread (PDFium is `!Send + !Sync`) and returns serialisable
//! results to the webview via the `compare_pages` command in `commands/compare.rs`.
//!
//! Full M6 UX (color-channel overlay, viewport comparison panels, change-heatmap)
//! is deferred — tracked as follow-up. The shared crate is the M6 must-have; the
//! Tauri integration here provides a functional one-shot diff command.

use anyhow::Result;
use pdf_diff::{PdfDiffEngine, pixel_diff};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of a two-tier compare between one page from each of two PDF documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageDiffResult {
    // --- Tier 1: text-layer ---
    /// `true` when the full character sequence is identical on both pages.
    pub text_char_match: bool,
    /// Number of character position deltas (only meaningful when `text_char_match` is true).
    pub text_delta_count: usize,
    /// RMS of character position deltas in PDF points (0 when sequences differ).
    /// Useful for detecting subtle title-block coordinate shifts.
    pub text_rms_delta_pts: f32,

    // --- Tier 2: pixel ---
    /// `true` when zero pixels differ beyond `tolerance`.
    pub pixel_passed: bool,
    /// Percentage of pixels changed (0.0–100.0).
    pub changed_pct: f32,
    /// Maximum per-channel delta seen anywhere (0–255).
    pub max_pixel_delta: u8,
    /// PNG-encoded diff image as a base64 string, ready for `<img src="data:image/png;base64,...">`
    /// on the Svelte side. Changed pixels are red; unchanged pixels are 50% grey.
    pub diff_png_b64: String,

    // --- Meta ---
    /// DPI used for the pixel render.
    pub render_dpi: f32,
}

/// Run a two-tier diff between `page_a` of `path_a` and `page_b` of `path_b`.
///
/// MUST be called from a `tokio::task::spawn_blocking` closure because `PdfDiffEngine`
/// is `!Send + !Sync`. All PDFium operations complete before this function returns.
pub fn run_two_tier_diff(
    path_a: &Path,
    path_b: &Path,
    page_a: u32,
    page_b: u32,
    dpi: f32,
    pixel_tolerance: u8,
) -> Result<PageDiffResult> {
    let mut engine = PdfDiffEngine::new()?;
    let doc_a = engine.open(path_a)?;
    let doc_b = engine.open(path_b)?;

    // --- Tier 1: text-layer diff ---
    let text = engine.text_diff(&doc_a, &doc_b, page_a, page_b)?;

    let text_rms_delta_pts = if text.char_sequence_match && !text.position_deltas.is_empty() {
        let sum_sq: f64 = text.position_deltas.iter()
            .map(|d| (d.delta_x * d.delta_x + d.delta_y * d.delta_y) as f64)
            .sum();
        (sum_sq / text.position_deltas.len() as f64).sqrt() as f32
    } else {
        0.0
    };

    // --- Tier 2: pixel diff ---
    let img_a = engine.render_page_full(&doc_a, page_a, dpi)?;
    let img_b = engine.render_page_full(&doc_b, page_b, dpi)?;
    let px = pixel_diff(&img_a, &img_b, pixel_tolerance)?;

    // Encode diff image to PNG, then base64.
    let diff_png_b64 = encode_image_b64(&px.diff_image)?;

    Ok(PageDiffResult {
        text_char_match: text.char_sequence_match,
        text_delta_count: text.position_deltas.len(),
        text_rms_delta_pts,
        pixel_passed: px.passed,
        changed_pct: px.changed_pct,
        max_pixel_delta: px.max_pixel_delta,
        diff_png_b64,
        render_dpi: dpi,
    })
}

/// Encode a `DynamicImage` to a PNG base64 string for transport over Tauri IPC.
fn encode_image_b64(img: &image::DynamicImage) -> Result<String> {
    let mut png_bytes: Vec<u8> = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )?;
    Ok(base64_encode(&png_bytes))
}

/// Minimal base64 encoder — avoids pulling in the `base64` crate for this single use.
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 63) as usize] as char);
        out.push(CHARS[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[(n & 63) as usize] as char } else { '=' });
    }
    out
}
