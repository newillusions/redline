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

#[cfg(any(test, feature = "ocr"))]
use std::collections::HashSet;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::AppState;

/// Minimum non-whitespace character count on a page for its native text to count as a
/// real text layer rather than incidental print furniture.
///
/// A raster scan routinely carries a little genuine native text: the footer a
/// "Print to PDF" driver stamps ("Page 1 of 1" = 8 chars, which cleared the old floor of
/// 8 and made OCR refuse the page), a Bates number, a print date line ("Printed
/// 2026-10-21 Page 1 of 3" = 25 chars). Those are all well under 40. A page that is
/// actually searchable (a title block, a schedule, a paragraph, any CAD sheet) carries
/// far more than 40 characters, so a floor of 40 separates the two without needing
/// glyph geometry, which lopdf's text extraction does not expose. Trade-off: a native
/// page with under 40 characters in total is treated as scan-like and gets an invisible
/// OCR layer added over it, which is harmless, whereas the reverse mistake (a scan that
/// cannot be OCR'd) is a dead end. The explicit "OCR anyway" action (`force`) covers any
/// page this floor still skips.
///
/// Shared between `document_needs_ocr` (the auto-OCR-on-open heuristic) and
/// `run_ocr_document`'s already-has-text guard through [`has_real_text_layer`], so both
/// agree on what "has text" means.
const MIN_TEXT_CHARS: usize = 40;

/// `true` if `text` (one page's extracted text) is a real text layer, not incidental
/// footer/header/stamp text. The single definition both OCR guards use.
fn has_real_text_layer(text: &str) -> bool {
    text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_TEXT_CHARS
}

/// How many leading pages `document_needs_ocr` samples — enough to catch a scanned cover
/// sheet followed by native-text pages (or vice versa) without paying the extraction cost
/// of every page on a large drawing set purely to decide whether to *offer* OCR.
const SAMPLE_PAGES: usize = 3;

/// `true` if none of the first `SAMPLE_PAGES` pages have meaningful extractable text — the
/// no-text-layer test the auto-OCR-on-open setting uses to decide whether to trigger OCR
/// after a document opens (spec requirement 3). Pure logic, unit-tested directly; the
/// command wrapper below only supplies the real per-page text.
fn pages_lack_text(pages: &[(u64, String)], sample_pages: usize) -> bool {
    !pages
        .iter()
        .take(sample_pages)
        .any(|(_, text)| has_real_text_layer(text))
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
        Ok(pages_lack_text(&pages, SAMPLE_PAGES))
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
#[cfg(any(test, feature = "ocr"))]
fn pages_needing_ocr(page_count: u32, existing_text_pages: &HashSet<u32>) -> Vec<u32> {
    (0..page_count)
        .filter(|p| !existing_text_pages.contains(p))
        .collect()
}

/// 0-based indices of pages that must NOT be OCR'd again: pages with a real text layer
/// (native content or a prior OCR pass, see [`has_real_text_layer`]) plus pages carrying a
/// Redline OCR layer even if that layer's text is too short or too unreadable to clear
/// `MIN_TEXT_CHARS`.
///
/// With `force` (the explicit "OCR anyway" action) native text no longer protects a page:
/// only Redline's own single layer does, so it is never stacked, while a scan with
/// incidental or unwanted native text is OCR'd regardless.
///
/// A page carrying MORE than one Redline layer is deliberately NOT in the set: files OCR'd
/// by v0.3.24 and earlier stacked a duplicate layer on every re-run, and re-OCR'ing such a
/// page makes the writer replace all of them with one fresh layer, which cleans the file.
///
/// `text_pages` is `extract_doc_text`'s 1-based `(page_num, text)` output; `layer_counts`
/// is `ocr::writer::ocr_layer_counts`'s 0-based `page -> layer count` map.
#[cfg(feature = "ocr")]
fn pages_already_ocred(
    text_pages: &[(u64, String)],
    layer_counts: &std::collections::BTreeMap<u32, usize>,
    force: bool,
) -> HashSet<u32> {
    let mut done: HashSet<u32> = if force {
        HashSet::new()
    } else {
        text_pages
            .iter()
            .filter(|(_, text)| has_real_text_layer(text))
            // extract_pdf_text's page_num is 1-based; this module's page indices are
            // 0-based throughout (matches render_page_full).
            .map(|(page_num_1based, _)| page_num_1based.saturating_sub(1) as u32)
            .collect()
    };
    done.extend(layer_counts.keys().copied());
    for (page, count) in layer_counts {
        if *count > 1 {
            done.remove(page);
        }
    }
    done
}

/// 0-based pages of `doc` that OCR will process: every page not protected by
/// [`pages_already_ocred`]. The whole of `run_ocr_document`'s page selection, split out so
/// it can be exercised against a real (in-memory) document.
#[cfg(feature = "ocr")]
fn pages_to_ocr_in_doc(doc: &mut lopdf::Document, page_count: u32, force: bool) -> Vec<u32> {
    // Layer detection reads stream bytes directly, so it still sees Redline's own layer
    // on scans whose NUL-padded streams defeat text extraction.
    let layer_counts = crate::ocr::writer::ocr_layer_counts(doc);
    let text_pages = crate::search::indexer::extract_doc_text(doc);
    pages_needing_ocr(
        page_count,
        &pages_already_ocred(&text_pages, &layer_counts, force),
    )
}

/// Per-page progress, emitted on the `"ocr-progress"` Tauri event as `run_ocr_document`
/// recognizes each page — the frontend listens via `@tauri-apps/api/event`'s `listen` to
/// drive a live per-page status line (spec requirement 2).
#[cfg(feature = "ocr")]
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
    /// Pages left alone because they already carry a real text layer or Redline's own OCR
    /// layer. Non-zero on a normal run is the cue to offer the "OCR anyway" action.
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
/// `force` (default false) is the explicit "OCR anyway" action: pages with native text are
/// OCR'd too. Pages carrying exactly one Redline OCR layer are still skipped, so a layer is
/// never stacked.
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
    force: Option<bool>,
) -> Result<OcrRunReport, String> {
    #[cfg(feature = "ocr")]
    {
        run_ocr_document_impl(app, state, doc_id, min_confidence, force.unwrap_or(false)).await
    }
    #[cfg(not(feature = "ocr"))]
    {
        let _ = (app, state, doc_id, min_confidence, force);
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
    force: bool,
) -> Result<OcrRunReport, String> {
    use tauri::Emitter as _;
    use tauri::Manager as _;

    use crate::ocr::writer::{write_pages_text_layers, DEFAULT_MIN_CONFIDENCE};
    use crate::ocr::{OcrEngineHandle, TessdataDir};

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

    let pages_to_ocr: Vec<u32> = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            // A scan failure must not block OCR entirely - treat it as "no existing text
            // known" (nothing skipped), matching `search::indexer`'s own per-page-skip,
            // don't-abort-the-whole-document posture rather than erroring the whole run
            // over a pre-check that isn't the actual operation.
            let Ok(mut doc) = lopdf::Document::load(&path) else {
                return (0..page_count).collect();
            };
            pages_to_ocr_in_doc(&mut doc, page_count, force)
        })
        .await
        .map_err(|e| e.to_string())?
    };
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
    // Resolved once at app startup (`lib.rs::resolve_tessdata_dir`, `app.manage`d there)
    // and threaded straight into `OcrEngineHandle::load` below instead of relying on
    // Tesseract's own `TESSDATA_PREFIX` env-var lookup — see `TessdataDir`'s doc comment
    // for why the env var alone is not sufficient on Windows (CRT `getenv` split, found
    // 2026-09-14). A missing managed state (should not happen outside tests, since
    // `setup` always manages it under the `ocr` feature) degrades to `None`, which still
    // lets Tesseract try its own standard lookup rather than failing outright.
    let tessdata = app.try_state::<TessdataDir>().map(|s| s.inner().clone());
    let tessdata_dir = tessdata.as_ref().and_then(|t| t.dir.clone());
    let tessdata_candidates_checked = tessdata
        .as_ref()
        .map(|t| t.candidates_checked.clone())
        .unwrap_or_default();
    let pages_lines: Vec<(u32, Vec<crate::ocr::OcrLine>)> =
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let mut engine = OcrEngineHandle::load(tessdata_dir.as_deref()).map_err(|e| {
                if tessdata_dir.is_none() && !tessdata_candidates_checked.is_empty() {
                    let checked = tessdata_candidates_checked
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "{e:#}\n\nno bundled tessdata directory was found at startup; \
                             candidates checked: {checked}"
                    )
                } else {
                    format!("{e:#}")
                }
            })?;
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

    const REAL_TEXT: &str = "ROOM 101 OFFICE PLAN LEVEL 2 SCALE 1 TO 100 DRAWN BY MR";

    // --- has_real_text_layer / pages_lack_text -----------------------------

    #[test]
    fn incidental_print_furniture_is_not_a_text_layer() {
        // The repro shape: a Print-to-PDF footer, plus the usual stamp variants.
        for footer in [
            "Page 1 of 1",
            "Page 12 of 140",
            "Printed 2026-10-21 Page 1 of 3",
            "BATES 000123",
        ] {
            assert!(
                !has_real_text_layer(footer),
                "{footer:?} must be incidental"
            );
        }
    }

    #[test]
    fn a_page_of_real_text_is_a_text_layer() {
        assert!(has_real_text_layer(REAL_TEXT));
    }

    #[test]
    fn pages_lack_text_true_when_all_sampled_pages_are_empty() {
        let pages = vec![
            (1, "".to_string()),
            (2, "   \n\t  ".to_string()),
            (3, "".to_string()),
        ];
        assert!(pages_lack_text(&pages, 3));
    }

    #[test]
    fn pages_lack_text_false_when_any_sampled_page_has_real_text() {
        let pages = vec![(1, "".to_string()), (2, REAL_TEXT.to_string())];
        assert!(!pages_lack_text(&pages, 3));
    }

    #[test]
    fn pages_lack_text_ignores_pages_beyond_the_sample_window() {
        // Page 3 has real text, but the sample window is only the first 2 pages.
        let pages = vec![
            (1, "".to_string()),
            (2, "".to_string()),
            (3, REAL_TEXT.to_string()),
        ];
        assert!(pages_lack_text(&pages, 2));
    }

    #[test]
    fn pages_lack_text_a_stray_whitespace_scan_artifact_does_not_count_as_text() {
        let pages = vec![(1, " \u{a0} \n ".to_string())];
        assert!(pages_lack_text(&pages, 3));
    }

    #[test]
    fn pages_lack_text_a_footer_only_scan_lacks_text() {
        // Regression: "Page 1 of 1" (8 chars) used to clear the floor and suppress OCR.
        let pages = vec![(1, "Page 1 of 1".to_string())];
        assert!(pages_lack_text(&pages, 3));
    }

    #[test]
    fn pages_lack_text_true_for_an_empty_document() {
        assert!(pages_lack_text(&[], 3));
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

    // --- pages_already_ocred ------------------------------------------------

    #[cfg(feature = "ocr")]
    mod already_ocred {
        use super::*;
        use std::collections::BTreeMap;

        #[test]
        fn a_page_with_text_is_done() {
            let text = vec![(1, REAL_TEXT.to_string()), (2, String::new())];
            let done = pages_already_ocred(&text, &BTreeMap::new(), false);
            assert_eq!(done, [0].into_iter().collect());
        }

        #[test]
        fn a_footer_only_page_is_not_done() {
            let text = vec![(1, "Page 1 of 1".to_string())];
            let done = pages_already_ocred(&text, &BTreeMap::new(), false);
            assert!(done.is_empty());
        }

        #[test]
        fn a_redline_layer_counts_even_when_text_extraction_found_nothing() {
            // The NUL-padded-scan case: lopdf extracted zero chars but the layer is there.
            let text = vec![(1, String::new()), (2, String::new())];
            let layers: BTreeMap<u32, usize> = [(0, 1), (1, 1)].into_iter().collect();
            let done = pages_already_ocred(&text, &layers, false);
            assert_eq!(done, [0, 1].into_iter().collect());
        }

        #[test]
        fn a_page_with_duplicate_layers_is_redone_so_the_writer_can_collapse_them() {
            let text = vec![(1, REAL_TEXT.to_string()), (2, REAL_TEXT.to_string())];
            let layers: BTreeMap<u32, usize> = [(0, 2), (1, 1)].into_iter().collect();
            let done = pages_already_ocred(&text, &layers, false);
            assert_eq!(done, [1].into_iter().collect());
        }

        #[test]
        fn force_ignores_native_text_but_never_stacks_on_a_single_redline_layer() {
            let text = vec![
                (1, REAL_TEXT.to_string()),
                (2, REAL_TEXT.to_string()),
                (3, REAL_TEXT.to_string()),
            ];
            // page 1: native text only; page 2: one Redline layer; page 3: duplicate layers.
            let layers: BTreeMap<u32, usize> = [(1, 1), (2, 2)].into_iter().collect();
            let done = pages_already_ocred(&text, &layers, true);
            assert_eq!(done, [1].into_iter().collect());
        }

        // --- pages_to_ocr_in_doc: the repro shape against a real document ---

        #[test]
        fn raster_scan_with_a_page_footer_is_selected_for_ocr() {
            let mut doc =
                crate::search::indexer::test_support::scan_with_text_doc(&["Page 1 of 1"]);
            // The footer must really be extractable, or this would pass vacuously.
            let extracted = crate::search::indexer::extract_doc_text(&mut doc);
            assert!(extracted[0].1.contains("Page 1 of 1"), "got {extracted:?}");
            assert_eq!(pages_to_ocr_in_doc(&mut doc, 1, false), vec![0]);
        }

        #[test]
        fn native_text_page_is_skipped_unless_forced() {
            let mut doc = crate::search::indexer::test_support::scan_with_text_doc(&[
                "Page 1 of 1",
                REAL_TEXT,
            ]);
            assert_eq!(pages_to_ocr_in_doc(&mut doc, 2, false), vec![0]);
            assert_eq!(pages_to_ocr_in_doc(&mut doc, 2, true), vec![0, 1]);
        }

        #[test]
        fn a_page_already_carrying_redlines_layer_is_skipped_even_when_forced() {
            let mut doc =
                crate::search::indexer::test_support::scan_with_text_doc(&["Page 1 of 1"]);
            let lines = vec![crate::ocr::OcrLine {
                text: "HELLO WORLD".to_string(),
                confidence: Some(0.95),
                corners_pdf: [(50.0, 712.0), (300.0, 712.0), (300.0, 700.0), (50.0, 700.0)],
                bbox_pdf: (50.0, 700.0, 300.0, 712.0),
            }];
            crate::ocr::writer::write_pages_text_layers(
                &mut doc,
                &[(0, lines)],
                crate::ocr::writer::DEFAULT_MIN_CONFIDENCE,
            )
            .unwrap();
            assert!(pages_to_ocr_in_doc(&mut doc, 1, false).is_empty());
            assert!(pages_to_ocr_in_doc(&mut doc, 1, true).is_empty());
        }

        #[test]
        fn the_committed_print_footer_scan_fixture_is_selected_for_ocr() {
            // tools/fixtures/ocr/f-scan-with-print-footer.pdf: raster page + native
            // "Page 1 of 1" footer, built by `gen-ocr-fixtures` (the repro shape).
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tools/fixtures/ocr/f-scan-with-print-footer.pdf");
            let mut doc = lopdf::Document::load(&path).unwrap();
            let extracted = crate::search::indexer::extract_doc_text(&mut doc);
            assert!(extracted[0].1.contains("Page 1 of 1"), "got {extracted:?}");
            assert!(pages_lack_text(&extracted, SAMPLE_PAGES));
            assert_eq!(pages_to_ocr_in_doc(&mut doc, 1, false), vec![0]);
        }
    }
}
