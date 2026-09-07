//! OCR Tauri commands (Phase 2c-ii) — the "OCR this document" toolbar action, the
//! no-text-layer detection the auto-OCR-on-open setting uses, and per-page progress
//! events. Builds on Phase 2c-i's writer (`ocr::writer`, PR #103) and Phase 2a/2b's
//! engine + bundling (`ocr` module, `ocr::OcrEngineHandle`).
//!
//! # Why `document_needs_ocr` needs no `ocr` feature
//!
//! It only reads text that's already there, via the same lopdf extraction
//! `search::indexer::extract_pdf_text` already uses for the Tantivy folder index — no
//! Tesseract involved. It is therefore unconditionally compiled and registered, so the
//! auto-OCR-on-open heuristic can always ask "does this document need OCR" even in a
//! build without the `ocr` feature compiled in (Tesseract just isn't bundled there).
//!
//! # Why `run_ocr_document` is a single unconditionally-registered command
//!
//! Tauri's `generate_handler!` list in `lib.rs` is a fixed, compile-time list — cleanly
//! branching an individual entry in/out by `#[cfg(feature = "ocr")]` would require
//! duplicating that entire list under two `#[cfg]` arms. Instead this command function is
//! always compiled and registered, but its real work lives in `run_ocr_document_impl`,
//! which is itself `#[cfg(feature = "ocr")]`-gated (referencing `crate::ocr::*`, only
//! compiled when the feature is on). A build without the feature returns a clear error at
//! the point of use instead of failing to build the whole app — the same posture
//! `lib.rs::resolve_tessdata_dir`'s doc comment already names as the intended Phase 2c
//! design ("the actual fail-loud behavior for an end user attempting to run OCR happens
//! at the point of use").
//!
//! # Threading
//!
//! Tesseract recognition is synchronous CPU work behind a non-`Send` C++ handle
//! (`leptess::LepTess`) — see `OcrEngineHandle::load`'s own doc comment on reusing one
//! instance across pages rather than reloading per call. It is therefore confined
//! entirely to ONE `tokio::task::spawn_blocking` closure (loaded once, used for every
//! page, dropped at the end of that closure) rather than crossing thread boundaries
//! mid-run — the same "dedicate blocking work to one place" idiom `RenderHandle` already
//! uses for PDFium (a whole dedicated OS thread) and `apply_page_edit` already uses for
//! lopdf save work (`spawn_blocking` per call).

use std::collections::HashSet;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::AppState;

/// Minimum non-whitespace character count across a sampled page's extracted text to call
/// it "has a text layer" — a single stray space/control character surviving a scan
/// artifact must not count as "already searchable". Shared between `document_needs_ocr`
/// (the auto-OCR-on-open heuristic) and `run_ocr_document`'s own already-has-text guard
/// (see `pages_needing_ocr`'s doc comment) so both agree on what "has text" means.
const MIN_TEXT_CHARS: usize = 8;

/// How many leading pages `document_needs_ocr` samples — enough to catch a scanned cover
/// sheet followed by native-text pages (or vice versa) without paying the extraction cost
/// of every page on a large drawing set purely to decide whether to *offer* OCR.
const SAMPLE_PAGES: usize = 3;

/// `true` if none of the first `SAMPLE_PAGES` pages have meaningful extractable text — the
/// no-text-layer test the auto-OCR-on-open setting uses to decide whether to trigger OCR
/// after a document opens (spec requirement 3). Pure logic, unit-tested directly; the
/// command wrapper below only supplies the real per-page text.
fn pages_lack_text(pages: &[(u64, String)], sample_pages: usize, min_chars: usize) -> bool {
    !pages
        .iter()
        .take(sample_pages)
        .any(|(_, text)| text.chars().filter(|c| !c.is_whitespace()).count() >= min_chars)
}

/// `true` if none of the first `SAMPLE_PAGES` pages of the open document `doc_id` have
/// meaningful extractable text. Reuses `search::indexer::extract_pdf_text` — the identical
/// lopdf extraction path the Tantivy folder indexer and `ocr::writer`'s own e2e proof both
/// already rely on — rather than a second, independent text-presence check.
///
/// # Errors
/// Returns an error string if `doc_id` is unknown or the file can't be parsed.
#[tauri::command]
pub async fn document_needs_ocr(
    state: State<'_, AppState>,
    doc_id: String,
) -> Result<bool, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    tokio::task::spawn_blocking(move || {
        let pages =
            crate::search::indexer::extract_pdf_text(&path).map_err(|e| format!("{e:#}"))?;
        Ok(pages_lack_text(&pages, SAMPLE_PAGES, MIN_TEXT_CHARS))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which 0-based page indices (of `page_count` total) still need OCR, given the set of
/// page indices already known to carry extractable text (native content or a prior OCR
/// pass). Pure logic, unit-tested directly.
///
/// Closes the idempotence gap `ocr::writer::write_page_text_layer`'s own doc comment names
/// as owed to this phase ("detecting/preventing re-OCR of an already-OCR'd page is left as
/// a Phase 2c-ii command-layer responsibility"): re-running OCR on a page that already has
/// text would append a second, overlapping invisible layer for no benefit.
fn pages_needing_ocr(page_count: u32, existing_text_pages: &HashSet<u32>) -> Vec<u32> {
    (0..page_count)
        .filter(|p| !existing_text_pages.contains(p))
        .collect()
}

/// Per-page progress, emitted on the `"ocr-progress"` Tauri event as `run_ocr_document`
/// recognizes each page — the frontend listens via `@tauri-apps/api/event`'s `listen` to
/// drive a live per-page status line (spec requirement 2).
#[derive(Debug, Clone, Serialize)]
pub struct OcrProgressEvent {
    pub doc_id: String,
    /// Absolute 0-based page index within the whole document.
    pub page_index: u32,
    /// How many pages of THIS run have finished recognition so far, including this one.
    pub pages_done: u32,
    /// How many pages THIS run is processing (already excludes pages skipped by
    /// `pages_needing_ocr` for already having text).
    pub pages_total: u32,
    pub lines_found: usize,
}

/// What `run_ocr_document` actually did, so the toolbar action can report real counts
/// instead of an unqualified "done" — the same "success that changed nothing must say so"
/// posture `commands::docops::OptimizeReport`/`flatten_document`'s returned count already
/// established for the other DocOps actions.
#[derive(Debug, Clone, Serialize)]
pub struct OcrRunReport {
    pub pages_total: u32,
    pub pages_ocred: u32,
    pub pages_skipped_existing_text: u32,
    pub pages_with_text_embedded: u32,
    pub lines_embedded: usize,
}

/// Run OCR over every page of the open document `doc_id` that doesn't already have
/// extractable text, embed the recognized lines as an invisible searchable text layer
/// (`ocr::writer`, Phase 2c-i), and reload the render engine — same atomic-save/markup-
/// preservation/reload contract every other DocOps action gets via `apply_page_edit`, so
/// in-app search finds the new text immediately with no reopen (spec requirement 4); the
/// folder full-text index picks it up via its existing file-watcher, unchanged by this
/// command.
///
/// `min_confidence` defaults to `ocr::writer::DEFAULT_MIN_CONFIDENCE` when omitted.
///
/// # Errors
/// Returns an error string if `doc_id` is unknown, OCR isn't compiled into this build, the
/// Tesseract engine fails to load, recognition fails on any page, or the atomic save
/// fails. On error, nothing is written — this is an all-or-nothing action, matching
/// flatten/optimize/redact.
#[tauri::command]
pub async fn run_ocr_document(
    app: AppHandle,
    state: State<'_, AppState>,
    doc_id: String,
    min_confidence: Option<f32>,
) -> Result<OcrRunReport, String> {
    #[cfg(feature = "ocr")]
    {
        run_ocr_document_impl(app, state, doc_id, min_confidence).await
    }
    #[cfg(not(feature = "ocr"))]
    {
        let _ = (app, state, doc_id, min_confidence);
        Err(
            "OCR is not available in this build — the `ocr` feature was not compiled into \
             this release."
                .to_string(),
        )
    }
}

#[cfg(feature = "ocr")]
async fn run_ocr_document_impl(
    app: AppHandle,
    state: State<'_, AppState>,
    doc_id: String,
    min_confidence: Option<f32>,
) -> Result<OcrRunReport, String> {
    use tauri::Emitter as _;

    use crate::ocr::writer::{write_pages_text_layers, DEFAULT_MIN_CONFIDENCE};
    use crate::ocr::OcrEngineHandle;

    // Matches `tests/ocr_writer_e2e.rs` / `ocr_benchmark.rs`'s own DPI convention — see
    // those files for why 300 DPI is the accuracy/latency-benchmarked operating point.
    const OCR_RENDER_DPI: f32 = 300.0;

    let min_confidence = min_confidence.unwrap_or(DEFAULT_MIN_CONFIDENCE);

    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let page_count = state
        .render
        .page_count(doc_id.clone())
        .await
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;

    let existing_text_pages: HashSet<u32> = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            // A scan failure must not block OCR entirely — treat it as "no existing text
            // known" (nothing skipped), matching `search::indexer`'s own per-page-skip,
            // don't-abort-the-whole-document posture rather than erroring the whole run
            // over a pre-check that isn't the actual operation.
            crate::search::indexer::extract_pdf_text(&path)
                .map(|pages| {
                    pages
                        .into_iter()
                        .filter(|(_, text)| {
                            text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_TEXT_CHARS
                        })
                        // extract_pdf_text's page_num is 1-based; this module's page
                        // indices are 0-based throughout (matches render_page_full).
                        .map(|(page_num_1based, _)| page_num_1based.saturating_sub(1) as u32)
                        .collect()
                })
                .unwrap_or_default()
        })
        .await
        .map_err(|e| e.to_string())?
    };

    let pages_to_ocr = pages_needing_ocr(page_count, &existing_text_pages);
    let pages_skipped_existing_text = page_count - pages_to_ocr.len() as u32;

    if pages_to_ocr.is_empty() {
        return Ok(OcrRunReport {
            pages_total: page_count,
            pages_ocred: 0,
            pages_skipped_existing_text,
            pages_with_text_embedded: 0,
            lines_embedded: 0,
        });
    }

    // Render every page that needs OCR up front on the render thread (PDFium is confined
    // there) — cheap relative to recognition and keeps the CPU-heavy Tesseract work
    // isolated to the one blocking-pool thread below.
    let mut rasters = Vec::with_capacity(pages_to_ocr.len());
    for &page_index in &pages_to_ocr {
        let raster = state
            .render
            .render_page_full(doc_id.clone(), page_index, OCR_RENDER_DPI)
            .await
            .map_err(|e| format!("render page {page_index}: {e:#}"))?;
        rasters.push((page_index, raster));
    }

    let pages_total_this_run = rasters.len() as u32;
    let app_for_progress = app.clone();
    let doc_id_for_progress = doc_id.clone();
    let pages_lines: Vec<(u32, Vec<crate::ocr::OcrLine>)> =
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let mut engine = OcrEngineHandle::load(None).map_err(|e| format!("{e:#}"))?;
            let mut out = Vec::with_capacity(rasters.len());
            for (done, (page_index, raster)) in rasters.into_iter().enumerate() {
                let lines = engine
                    .recognize_page(&raster)
                    .map_err(|e| format!("recognize page {page_index}: {e:#}"))?;
                // Progress is best-effort: a closed/gone webview must not fail the OCR run.
                let _ = app_for_progress.emit(
                    "ocr-progress",
                    OcrProgressEvent {
                        doc_id: doc_id_for_progress.clone(),
                        page_index,
                        pages_done: done as u32 + 1,
                        pages_total: pages_total_this_run,
                        lines_found: lines.len(),
                    },
                );
                out.push((page_index, lines));
            }
            Ok(out)
        })
        .await
        .map_err(|e| e.to_string())??;

    let lines_embedded: usize = pages_lines.iter().map(|(_, lines)| lines.len()).sum();
    let pages_with_text_embedded = pages_lines
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .count() as u32;

    super::document::apply_page_edit(&state, &doc_id, move |doc| {
        write_pages_text_layers(doc, &pages_lines, min_confidence)
    })
    .await?;

    Ok(OcrRunReport {
        pages_total: page_count,
        pages_ocred: pages_to_ocr.len() as u32,
        pages_skipped_existing_text,
        pages_with_text_embedded,
        lines_embedded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- pages_lack_text ---------------------------------------------------

    #[test]
    fn pages_lack_text_true_when_all_sampled_pages_are_empty() {
        let pages = vec![
            (1, "".to_string()),
            (2, "   \n\t  ".to_string()),
            (3, "".to_string()),
        ];
        assert!(pages_lack_text(&pages, 3, MIN_TEXT_CHARS));
    }

    #[test]
    fn pages_lack_text_false_when_any_sampled_page_has_real_text() {
        let pages = vec![(1, "".to_string()), (2, "ROOM 101 OFFICE PLAN".to_string())];
        assert!(!pages_lack_text(&pages, 3, MIN_TEXT_CHARS));
    }

    #[test]
    fn pages_lack_text_ignores_pages_beyond_the_sample_window() {
        // Page 3 has real text, but the sample window is only the first 2 pages.
        let pages = vec![
            (1, "".to_string()),
            (2, "".to_string()),
            (3, "ROOM 101 OFFICE PLAN".to_string()),
        ];
        assert!(pages_lack_text(&pages, 2, MIN_TEXT_CHARS));
    }

    #[test]
    fn pages_lack_text_a_stray_whitespace_scan_artifact_does_not_count_as_text() {
        let pages = vec![(1, " \u{a0} \n ".to_string())];
        assert!(pages_lack_text(&pages, 3, MIN_TEXT_CHARS));
    }

    #[test]
    fn pages_lack_text_below_min_chars_does_not_count_as_text() {
        // 3 non-whitespace chars, under the 8-char floor — a stray OCR/scan artifact,
        // not a real text layer.
        let pages = vec![(1, "1.2".to_string())];
        assert!(pages_lack_text(&pages, 3, MIN_TEXT_CHARS));
    }

    #[test]
    fn pages_lack_text_true_for_an_empty_document() {
        assert!(pages_lack_text(&[], 3, MIN_TEXT_CHARS));
    }

    // --- pages_needing_ocr --------------------------------------------------

    #[test]
    fn pages_needing_ocr_returns_all_pages_when_none_have_text() {
        let existing = HashSet::new();
        assert_eq!(pages_needing_ocr(4, &existing), vec![0, 1, 2, 3]);
    }

    #[test]
    fn pages_needing_ocr_skips_pages_already_having_text() {
        let existing: HashSet<u32> = [1, 3].into_iter().collect();
        assert_eq!(pages_needing_ocr(4, &existing), vec![0, 2]);
    }

    #[test]
    fn pages_needing_ocr_returns_empty_when_every_page_already_has_text() {
        let existing: HashSet<u32> = [0, 1, 2].into_iter().collect();
        assert!(pages_needing_ocr(3, &existing).is_empty());
    }

    #[test]
    fn pages_needing_ocr_returns_empty_for_a_zero_page_document() {
        let existing = HashSet::new();
        assert!(pages_needing_ocr(0, &existing).is_empty());
    }

    #[test]
    fn pages_needing_ocr_ignores_out_of_range_existing_indices() {
        // Defensive: an index beyond page_count (shouldn't happen given the real caller,
        // but the filter must not panic or otherwise misbehave on it).
        let existing: HashSet<u32> = [0, 99].into_iter().collect();
        assert_eq!(pages_needing_ocr(2, &existing), vec![1]);
    }
}
