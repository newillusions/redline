//! OCR text-layer writer (Phase 2c-i) — embeds recognized `OcrLine`s as an
//! **invisible, in-place** searchable text layer directly in the page
//! content stream, so the file that ships to the user (or sits in a folder
//! the Tantivy index scans) is itself searchable — no sidecar file, no
//! separate reader to wire into Bluebeam/Acrobat/our own search paths.
//!
//! # Design decision: in-place, not a sidecar (`-ocr.pdf`)
//!
//! The scoping doc's working filename (`-ocr.pdf`) suggested a sidecar
//! output file. This module does the opposite — it writes the text layer
//! into the SAME PDF bytes the caller already has — for one decisive
//! reason: every consumer that needs to "see" OCR text already reads the
//! document's own content stream directly, and none of them know a sidecar
//! convention:
//!
//! - Bluebeam Revu and Acrobat search the file a user actually opens. A
//!   `-ocr.pdf` living next to it is a different, undiscovered document as
//!   far as those apps are concerned — the entire point of Phase 2c is
//!   Bluebeam-parity searchability, which a sidecar does not deliver.
//! - `search::indexer::extract_pdf_text` (the Tantivy folder-index text
//!   source) calls `lopdf::Document::extract_text` on the file it finds by
//!   walking the folder — it has no sidecar-pairing logic, and adding one
//!   would need to be threaded through the watcher/rename/move handling
//!   too (`index_folder_blocking`'s `notify` integration).
//! - `render::RenderEngine::search_page` (in-app in-document search, M4 S3)
//!   opens exactly the `doc_id` the app already has loaded — again, no
//!   sidecar awareness anywhere in that path.
//!
//! A sidecar would need matching logic added in at least three independent
//! places (two different text-extraction libraries plus the file-open flow)
//! for a benefit (avoiding touching the original bytes) that doesn't matter
//! here: this operation is additive-only (see below) and never touches the
//! existing raster/content, so there is no revision-safety reason to prefer
//! a sidecar the way e.g. `docops::redact` might have.
//!
//! # How the invisible layer works (PDF spec, not vendor-specific)
//!
//! Each kept `OcrLine` becomes one `BT ... ET` text object appended to the
//! page's content stream (via `docops::append_to_page_contents`, the same
//! append-a-new-content-stream-object idiom `docops::redact_regions` already
//! uses):
//!
//! - **Text rendering mode 3** (`3 Tr`, PDF 32000-1:2008 §9.3.3, mode
//!   "Neither fill nor stroke text \[...\] (invisible)"): the glyphs are
//!   never painted, so nothing changes visually. This is the standard
//!   technique every "searchable scanned PDF" tool uses (Adobe's own OCR,
//!   `ocrmypdf`, Tesseract's own PDF renderer) — not something invented for
//!   this codebase.
//! - **A standard (non-embedded) Helvetica font** (`/Type1 /BaseFont
//!   /Helvetica /Encoding /WinAnsiEncoding`): every PDF-1.x-conformant
//!   viewer, Bluebeam and Acrobat included, has this built in — no font
//!   file to bundle, no licensing question, smaller output than embedding a
//!   real font just to keep glyphs invisible anyway.
//! - **Font size = the recognized line's box height**, and **horizontal
//!   scaling (`Tz`) chosen so the string's nominal Helvetica width matches
//!   the box's baseline length** (`corners_pdf`'s top-left→top-right edge).
//!   This uses a single flat average-glyph-width constant
//!   (`HELVETICA_AVG_WIDTH_EM`, documented at its definition), not a full
//!   per-glyph AFM metrics table — deliberately: the layer is invisible, so
//!   the only consequence of an approximate width is the size of the
//!   PDFium/Bluebeam search-hit highlight rectangle and a future
//!   text-selection drag box, not text-extraction correctness (extraction
//!   reads the string content, not glyph metrics) or PDF validity (`Tz` is
//!   clamped to a sane range regardless — see `compute_tz`). A precise
//!   per-glyph table is a reasonable follow-up if exact hit-box sizing ever
//!   becomes a real requirement; not attempted here to avoid asserting
//!   metrics data this session had no way to verify byte-exact.
//! - **Rotation follows the line's own baseline vector**, not just its
//!   axis-aligned `bbox_pdf` — `corners_pdf`'s top-left→top-right edge
//!   already encodes the CAD dimension string's true on-page orientation
//!   (Phase 2a's rotate-4x maps everything back into the ORIGINAL page's
//!   coordinate space; see `ocr::mod`'s module doc comment). Using it means
//!   a 90°-rotated dimension string gets an invisible text run that is
//!   itself rotated 90° at the right spot, not a wide horizontal box
//!   stamped over a tall narrow region.
//!
//! # Confidence / degenerate-line filtering
//!
//! `docs/ocr.md` already names this as owed: the rotate-4x merge keeps
//! every non-overlapping candidate across all 4 rotation passes, so 3 of
//! the 4 passes routinely contribute low-confidence noise fragments
//! alongside the one correct reading (9-30% measured precision — see the
//! module's benchmark numbers). `write_ocr_pdf` filters BEFORE embedding
//! (never after — an already-embedded noise string is exactly as
//! searchable as a correct one): lines below `min_confidence`, lines with
//! empty/whitespace-only text, and lines with a degenerate (near-zero-area)
//! box are all dropped. See `DEFAULT_MIN_CONFIDENCE` for the chosen default
//! and its rationale.
//!
//! # What this module does NOT do (named, not silently skipped)
//!
//! No UI, no "Run OCR" command wiring, no auto-trigger-on-open heuristic —
//! all explicitly Phase 2c-ii scope (owner-approved split, this phase ships
//! 2c-i: the writer itself, proven searchable both via PDFium in-document
//! search and via the Tantivy folder indexer's own `lopdf::extract_text`
//! path — see this module's tests and `tests/ocr_writer_e2e.rs`). No human
//! visual/search confirmation in real Bluebeam/Acrobat has been done this
//! session either — same "owed, not silently assumed" posture this repo
//! already uses for G9 (`.claude/rules/judgment.md`).

use anyhow::{Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::io::Cursor;

use super::OcrLine;
use crate::docops::{add_named_objects_to_page_resources, append_to_page_contents, pdf_num};

/// Lines below this confidence (0.0-1.0) are dropped before embedding.
///
/// Chosen from the measured per-fixture mean confidences in
/// `docs/ocr.md`/the benchmark table: correctly-recognized lines measured
/// 63-82% mean confidence; the "wrong orientation" noise fragments the
/// precision numbers are attributed to (`docs/ocr.md`'s worked examples:
/// `"fo)"`, `"(=)"`) are qualitatively low-confidence reads of text that
/// doesn't really exist at that orientation. 0.5 sits below every measured
/// correct-line mean and gives a real margin above where noise fragments
/// are expected to score, without a corpus run in THIS session to tune it
/// exactly — callers needing a different threshold (e.g. a future UI
/// "strict/lenient" setting, Phase 2c-ii scope) pass their own via
/// `min_confidence`.
pub const DEFAULT_MIN_CONFIDENCE: f32 = 0.5;

/// A flat average Helvetica glyph advance width, in 1/1000 em units.
///
/// Real Helvetica AFM widths range roughly 222 ("i"/"l"/"."/",") to 889
/// ("M"/"W"); a commonly-cited representative average for mixed English
/// text (digits and most lowercase letters sit near 556-600) is used here
/// rather than a full 95-entry per-glyph table, for the reason explained in
/// this module's doc comment: the layer is invisible, so width accuracy
/// only affects the approximate size of a search-hit/selection box, never
/// text-extraction correctness or PDF validity. `compute_tz` clamps its
/// result regardless, so even a materially-off average cannot produce
/// invalid content.
const HELVETICA_AVG_WIDTH_EM: f64 = 556.0;

/// Minimum line dimension (PDF points) below which a box is treated as
/// degenerate and dropped — guards against a zero-area or near-zero box
/// producing a division-by-zero in `compute_tz` or a font size of ~0.
const MIN_LINE_DIMENSION: f64 = 0.5;

/// The `/Resources/Font` entry name this writer's Helvetica font is
/// registered under. Deliberately an unusual, redline-prefixed name (not a
/// generic `F1`/`OCRF`/`TT0`-style short name a scanner or another tool
/// might already use) to make a collision with a pre-existing font on the
/// SAME page's shared `/Resources/Font` sub-dict unlikely — a collision
/// would silently overwrite that mapping for the whole page, changing the
/// font pre-existing (visible) text renders with. `add_named_objects_to_
/// page_resources` doesn't check for an existing entry under this name; an
/// unusual name is the mitigation, not a guarantee.
const OCR_FONT_NAME: &str = "RLOCRFont";

/// Embed `lines` as an invisible searchable text layer on `pages_lines`'
/// named pages of `pdf_bytes`, and return the modified PDF as new bytes.
///
/// `pages_lines` entries are `(page_index, lines)` with **0-based**
/// `page_index` (matching every other page-indexed API in this codebase,
/// e.g. `render::RenderEngine::render_page_full`) — NOT `lopdf`'s own
/// 1-based `get_pages()` numbering, which this function converts
/// internally. Pages not named in `pages_lines` are left untouched.
///
/// Lines are filtered by `write_page_text_layer` before embedding (see the
/// module doc comment) — `min_confidence` is forwarded unchanged.
pub fn write_ocr_pdf(
    pdf_bytes: &[u8],
    pages_lines: &[(u32, Vec<OcrLine>)],
    min_confidence: f32,
) -> Result<Vec<u8>> {
    let mut doc = Document::load_from(Cursor::new(pdf_bytes)).context("load PDF from bytes")?;
    write_pages_text_layers(&mut doc, pages_lines, min_confidence)?;

    let mut out: Vec<u8> = Vec::new();
    doc.save_to(&mut out).context("save OCR'd PDF to bytes")?;
    Ok(out)
}

/// Embed `lines` as an invisible searchable text layer on `pages_lines`' named pages of
/// an ALREADY-LOADED `doc`, without (re)loading or saving it — the shared core `write_ocr_pdf`
/// wraps with its own load/save-to-bytes pair.
///
/// `pub(crate)` (not `pub`) so `commands::ocr::run_ocr_document` (Phase 2c-ii) can call it
/// directly inside `commands::document::apply_page_edit`'s `&mut lopdf::Document` closure —
/// reusing the exact same page-lookup/font-sharing/per-page-write logic `write_ocr_pdf`
/// uses, rather than that command hand-rolling a second load/save-to-bytes round trip that
/// would either duplicate `apply_page_edit`'s own atomic-save/markup-preservation/render-
/// reload contract or bypass it outright (see that command's doc comment).
pub(crate) fn write_pages_text_layers(
    doc: &mut Document,
    pages_lines: &[(u32, Vec<OcrLine>)],
    min_confidence: f32,
) -> Result<()> {
    // lopdf's get_pages() is 1-based (BTreeMap<u32, ObjectId>) — matches the
    // same convention `docops::redact_regions` already relies on.
    let page_map = doc.get_pages();

    // Share ONE Helvetica font object across every page it's needed on —
    // it's a plain base-14 font dict with no per-page state, so an indirect
    // reference from each page's /Resources/Font is the natural shape (and
    // avoids adding a duplicate font object per page).
    let mut font_id: Option<ObjectId> = None;

    for (page_index, lines) in pages_lines {
        if lines.is_empty() {
            continue;
        }
        let page_num_1based = page_index + 1;
        let Some(page_id) = page_map.get(&page_num_1based).copied() else {
            // Named an out-of-range page — skip rather than error, matching
            // this codebase's general "don't abort a whole document over
            // one bad input" posture (e.g. indexer.rs's per-page skip).
            continue;
        };

        let fid = *font_id.get_or_insert_with(|| doc.add_object(helvetica_font_dict()));
        write_page_text_layer(doc, page_id, fid, lines, min_confidence)?;
    }

    Ok(())
}

/// Embed one page's worth of `lines` as invisible text, appending a new
/// content-stream object and registering `font_id` under `/Resources/Font`
/// (name `OCR_FONT_NAME`) if the page doesn't already reference it.
///
/// **Not idempotent**: calling this twice on the same page (or `write_ocr_pdf`
/// twice on the same document) appends a SECOND, duplicate invisible text
/// object with the same lines — duplicate search hits at the same location,
/// plus a second orphaned Helvetica font object per separate `write_ocr_pdf`
/// call (`font_id` is only shared across pages WITHIN one call). No guard
/// against this exists here; detecting/preventing re-OCR of an already-OCR'd
/// page is left as a Phase 2c-ii command-layer responsibility (e.g. a marker
/// checked before invoking this, once a "Run OCR" command exists to invoke
/// it more than once).
///
/// Public (not just `pub(crate)`) so a future Phase 2c-ii command can call
/// it directly per-page (e.g. re-OCR a single page without touching the
/// rest of the document) without going through the whole-document
/// `write_ocr_pdf` entry point.
pub fn write_page_text_layer(
    doc: &mut Document,
    page_id: ObjectId,
    font_id: ObjectId,
    lines: &[OcrLine],
    min_confidence: f32,
) -> Result<()> {
    let mut content = Vec::new();
    let mut any_written = false;

    for line in lines {
        let Some(op) = render_line_ops(line, min_confidence) else {
            continue;
        };
        content.extend_from_slice(&op);
        any_written = true;
    }

    if !any_written {
        return Ok(());
    }

    let content_id = doc.add_object(Stream::new(dictionary! {}, content));
    append_to_page_contents(doc, page_id, content_id)?;
    add_named_objects_to_page_resources(
        doc,
        page_id,
        b"Font",
        &[(OCR_FONT_NAME.to_string(), font_id)],
    )?;

    Ok(())
}

/// Build the `q BT ... ET Q` content-stream snippet for one line as raw
/// bytes, or `None` if the line is filtered out (low confidence, empty/
/// whitespace text, or a degenerate box).
///
/// Returns bytes, NOT a `String` — the `(...)` literal carries
/// `/WinAnsiEncoding` bytes from `winansi_encode`/`escape_pdf_literal`,
/// which for the 0xA0-0xFF Latin-1-supplement range (e.g. `°`, `é`, `£`) are
/// single bytes with the high bit set. Building this as a `String` and
/// pushing those bytes via `byte as char` would silently UTF-8-re-encode
/// each one to TWO bytes (`u8 as char` maps a byte's numeric value to that
/// Unicode *code point*, and `String` always stores UTF-8) — a real bug an
/// earlier version of this function had, caught in code review before merge
/// (see `escape_pdf_literal_preserves_single_byte_winansi_chars` and
/// `write_ocr_pdf_embeds_extractable_text_with_degree_sign` below for the
/// regression tests). The ASCII-only numeric/operator prelude and suffix are
/// still built via `format!` (safe — `pdf_num` and the literal PDF operator
/// tokens are pure ASCII) and converted to bytes with `.as_bytes()`; only
/// the escaped text literal itself is spliced in as raw bytes.
fn render_line_ops(line: &OcrLine, min_confidence: f32) -> Option<Vec<u8>> {
    // Confidence filter (see DEFAULT_MIN_CONFIDENCE doc comment). A `None`
    // confidence (the defensive "zero confidence-bearing words" case the
    // `OcrLine` doc comment names) is treated as failing the filter — never
    // embed a line this codebase's own OCR engine couldn't score at all.
    let conf = line.confidence?;
    if conf < min_confidence {
        return None;
    }

    let text = line.text.trim();
    if text.is_empty() {
        return None;
    }

    // Shape filter (see `looks_like_text`'s doc comment) — independent of
    // and in addition to the confidence filter above. Confidence alone
    // doesn't reliably separate real content from the symbol-heavy noise
    // fragments the rotate-4x merge's "wrong orientation" passes routinely
    // produce (`docs/ocr.md`'s "Measured numbers": 9-30% precision on lines
    // that already passed a confidence filter). Rejecting those here means
    // they never enter the invisible text layer at all, so no later text
    // extraction (Tantivy folder-index snippets, PDFium in-document search)
    // can surface them next to a real match.
    if !super::looks_like_text(text) {
        return None;
    }

    // corners_pdf: [top-left, top-right, bottom-right, bottom-left] (see
    // ocr::OcrLine's doc comment). Baseline runs along bottom-left→bottom-right
    // in a purely axis-aligned line, but the RELIABLE edge across every
    // rotation this module needs to handle is top-left→top-right (the
    // line's "reading direction" vector, which rotate-4x already maps back
    // into the original page's coordinate space for 90/180/270° lines).
    let [tl, tr, _br, bl] = line.corners_pdf;

    let dx = tr.0 - tl.0;
    let dy = tr.1 - tl.1;
    let baseline_len = dx.hypot(dy);

    let height_dx = bl.0 - tl.0;
    let height_dy = bl.1 - tl.1;
    let box_height = height_dx.hypot(height_dy);

    if baseline_len < MIN_LINE_DIMENSION || box_height < MIN_LINE_DIMENSION {
        return None;
    }

    let angle = dy.atan2(dx);
    let (sin_a, cos_a) = angle.sin_cos();

    // Text origin: PDF text is positioned by its BASELINE, which sits near
    // the bottom of the glyph box, not the box's geometric bottom edge
    // (ascenders/descenders exist) — bottom-left is a reasonable
    // approximation for an invisible layer with no visual fidelity
    // requirement (see module doc comment).
    let (tx, ty) = bl;

    let font_size = box_height;
    let tz = compute_tz(text, baseline_len, font_size);

    let escaped = escape_pdf_literal(&winansi_encode(text));

    let prelude = format!(
        "q\nBT\n/{font} {fs} Tf\n3 Tr\n{tz} Tz\n{a} {b} {c} {d} {e} {f} Tm\n(",
        font = OCR_FONT_NAME,
        fs = pdf_num(font_size),
        tz = pdf_num(tz),
        a = pdf_num(cos_a),
        b = pdf_num(sin_a),
        c = pdf_num(-sin_a),
        d = pdf_num(cos_a),
        e = pdf_num(tx),
        f = pdf_num(ty),
    );

    let mut ops = Vec::with_capacity(prelude.len() + escaped.len() + 16);
    ops.extend_from_slice(prelude.as_bytes()); // ASCII-only prelude: safe as bytes.
    ops.extend_from_slice(&escaped); // raw WinAnsi bytes — never through `char`/`String`.
    ops.extend_from_slice(b") Tj\nET\nQ\n");
    Some(ops)
}

/// Horizontal scaling (`Tz`, PDF percent units — 100 = unscaled) so the
/// string's nominal Helvetica width at `font_size` matches `target_width`
/// (the line's measured baseline length). Clamped to [10, 1000] so an
/// unusual input (a one-character line, a very long string crammed into a
/// short box) can never produce a zero/negative/absurd scale — the layer is
/// invisible, so a clamped-but-imprecise width is harmless; an invalid
/// content-stream number is not.
fn compute_tz(text: &str, target_width: f64, font_size: f64) -> f64 {
    let char_count = text.chars().count().max(1) as f64;
    let natural_width = char_count * (HELVETICA_AVG_WIDTH_EM / 1000.0) * font_size;
    if natural_width <= 0.0 {
        return 100.0;
    }
    (100.0 * target_width / natural_width).clamp(10.0, 1000.0)
}

/// Encode `text` as WinAnsiEncoding bytes for a PDF literal string.
///
/// WinAnsiEncoding matches Unicode/Latin-1 code points directly for ASCII
/// printable (0x20-0x7E) and the upper Latin-1 range (0xA0-0xFF) — see PDF
/// 32000-1:2008 Annex D.2. Codes below 0x20 (control characters — should
/// not appear in OCR output) and the 0x80-0x9F Windows-1252-specific block
/// (curly quotes, em-dash, etc. — occasionally emitted by Tesseract) fall
/// back to `?`, a documented, low-stakes approximation: this only affects
/// the invisible layer's exact character content for that rare glyph, not
/// extraction of the surrounding text or PDF validity.
fn winansi_encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            if (0x20..=0x7E).contains(&cp) || (0xA0..=0xFF).contains(&cp) {
                cp as u8
            } else {
                b'?'
            }
        })
        .collect()
}

/// Escape `\`, `(`, `)` for a PDF literal string per PDF 32000-1:2008 §7.3.4.2.
///
/// Returns raw `Vec<u8>`, deliberately NOT a `String` — `bytes` may already
/// contain single-byte WinAnsi codes >= 0x80 (see `winansi_encode`), and a
/// `String` can only hold valid UTF-8, which would force exactly the
/// silent re-encoding bug this function must NOT reintroduce (see
/// `render_line_ops`'s doc comment).
fn escape_pdf_literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'(' => out.extend_from_slice(b"\\("),
            b')' => out.extend_from_slice(b"\\)"),
            _ => out.push(b),
        }
    }
    out
}

/// A standard (non-embedded) Helvetica Type1 font dictionary — every
/// PDF-1.x-conformant viewer has this built in, so no font program needs
/// bundling just to draw invisible glyphs. See module doc comment.
fn helvetica_font_dict() -> Dictionary {
    dictionary! {
        "Type" => Object::Name(b"Font".to_vec()),
        "Subtype" => Object::Name(b"Type1".to_vec()),
        "BaseFont" => Object::Name(b"Helvetica".to_vec()),
        "Encoding" => Object::Name(b"WinAnsiEncoding".to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn line(
        text: &str,
        confidence: Option<f32>,
        tl: (f64, f64),
        tr: (f64, f64),
        bl: (f64, f64),
        br: (f64, f64),
    ) -> OcrLine {
        let min_x = [tl.0, tr.0, bl.0, br.0]
            .into_iter()
            .fold(f64::MAX, f64::min);
        let max_x = [tl.0, tr.0, bl.0, br.0]
            .into_iter()
            .fold(f64::MIN, f64::max);
        let min_y = [tl.1, tr.1, bl.1, br.1]
            .into_iter()
            .fold(f64::MAX, f64::min);
        let max_y = [tl.1, tr.1, bl.1, br.1]
            .into_iter()
            .fold(f64::MIN, f64::max);
        OcrLine {
            text: text.to_string(),
            confidence,
            corners_pdf: [tl, tr, br, bl],
            bbox_pdf: (min_x, min_y, max_x, max_y),
        }
    }

    /// A horizontal line: 100pt-wide box, baseline at y=100, 12pt tall.
    fn horizontal_line(text: &str, confidence: Option<f32>) -> OcrLine {
        line(
            text,
            confidence,
            (10.0, 112.0),  // top-left
            (110.0, 112.0), // top-right
            (10.0, 100.0),  // bottom-left
            (110.0, 100.0), // bottom-right
        )
    }

    /// A horizontal line at an explicit `(min_x, min_y, max_x, max_y)` box —
    /// convenience for tests that need to control page POSITION (not just
    /// text/confidence), e.g. the reading-order regression tests below.
    fn line_at(text: &str, confidence: Option<f32>, bbox: (f64, f64, f64, f64)) -> OcrLine {
        let (min_x, min_y, max_x, max_y) = bbox;
        line(
            text,
            confidence,
            (min_x, max_y),
            (max_x, max_y),
            (min_x, min_y),
            (max_x, min_y),
        )
    }

    fn one_page_pdf() -> Document {
        // Minimal single-page document, same shape the docops tests already
        // use (see docops::mod's own test helpers) — a Page with an empty
        // existing content stream so append behavior is exercised.
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, vec![]));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => Object::Reference(content_id),
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.max_id = doc.objects.keys().map(|(id, _)| *id).max().unwrap_or(0);
        doc
    }

    #[test]
    fn write_ocr_pdf_embeds_extractable_text() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };

        let lines = vec![horizontal_line("HELLO WORLD", Some(0.9))];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        // Same extraction path search::indexer::extract_pdf_text uses (lopdf's
        // own Document::extract_text) — proves Tantivy folder-index pickup
        // without needing PDFium at all.
        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(
            text.contains("HELLO WORLD"),
            "expected embedded text in extracted content, got: {text:?}"
        );
    }

    #[test]
    fn write_pages_text_layers_embeds_extractable_text_in_an_already_loaded_document() {
        // Exercises the in-memory-Document entry point `run_ocr_document` (Phase 2c-ii)
        // calls directly inside `apply_page_edit`'s closure, as opposed to `write_ocr_pdf`'s
        // own load-bytes -> write -> save-bytes wrapper around the same function.
        let mut doc = one_page_pdf();
        let lines = vec![horizontal_line("HELLO WORLD", Some(0.9))];

        write_pages_text_layers(&mut doc, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        let reopened = Document::load_from(Cursor::new(&out)).unwrap();
        let text = reopened.extract_text(&[1]).unwrap();
        assert!(
            text.contains("HELLO WORLD"),
            "expected embedded text in extracted content, got: {text:?}"
        );
    }

    #[test]
    fn write_pages_text_layers_leaves_document_untouched_when_all_pages_have_no_lines() {
        // The empty-lines-list skip (`if lines.is_empty() { continue; }`) must hold on the
        // in-memory entry point too — a page named with zero recognized lines (e.g. every
        // page was already skipped by `run_ocr_document`'s existing-text guard) must not
        // gain a font resource or content-stream object it never needed.
        let mut doc = one_page_pdf();
        let before_max_id = doc.max_id;

        write_pages_text_layers(&mut doc, &[(0, vec![])], DEFAULT_MIN_CONFIDENCE).unwrap();

        assert_eq!(
            doc.max_id, before_max_id,
            "no objects should have been added for an empty lines list"
        );
    }

    #[test]
    fn write_ocr_pdf_content_uses_invisible_render_mode() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let lines = vec![horizontal_line("SCALE 1:100", Some(0.9))];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let content_bytes = doc
            .get_and_decode_page_content(page_id)
            .unwrap()
            .encode()
            .unwrap();
        let content_str = String::from_utf8_lossy(&content_bytes);
        assert!(
            content_str.contains("3 Tr"),
            "expected invisible text-rendering mode operator, got: {content_str}"
        );
        assert!(
            content_str.contains("Tj"),
            "expected a Tj text-show operator"
        );
    }

    #[test]
    fn write_ocr_pdf_registers_font_resource() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let lines = vec![horizontal_line("FONT CHECK", Some(0.9))];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        let res = match page.get(b"Resources").unwrap() {
            Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
            Object::Dictionary(d) => d,
            _ => panic!("unexpected Resources shape"),
        };
        let font_dict = match res.get(b"Font").unwrap() {
            Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
            Object::Dictionary(d) => d,
            _ => panic!("unexpected Font shape"),
        };
        assert!(
            font_dict.has(OCR_FONT_NAME.as_bytes()),
            "expected /Font /{OCR_FONT_NAME} registered"
        );
    }

    #[test]
    fn low_confidence_line_is_dropped() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        // Below DEFAULT_MIN_CONFIDENCE (0.5).
        let lines = vec![horizontal_line("NOISE FRAGMENT", Some(0.2))];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap_or_default();
        assert!(
            !text.contains("NOISE FRAGMENT"),
            "low-confidence line should have been filtered out, got: {text:?}"
        );
    }

    #[test]
    fn symbol_heavy_noise_line_is_dropped_even_at_high_confidence() {
        // The owner-reported search-snippet bug (2026-09-08): a symbol-heavy
        // noise fragment shaped exactly like the real ones seen in the
        // fixtures' extracted text ("2 = S", "&z"). Confidence alone doesn't
        // catch this — set it well ABOVE DEFAULT_MIN_CONFIDENCE so only the
        // new shape filter (`looks_like_text`) can be why it's dropped.
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        for noise in ["2 = S", "&z"] {
            let lines = vec![horizontal_line(noise, Some(0.95))];
            let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();
            let doc = Document::load_from(Cursor::new(&out)).unwrap();
            let text = doc.extract_text(&[1]).unwrap_or_default();
            assert!(
                !text.contains(noise),
                "symbol-heavy noise {noise:?} should have been filtered out, got: {text:?}"
            );
        }
    }

    #[test]
    fn real_word_survives_next_to_filtered_noise_and_extracts_in_reading_order() {
        // End-to-end regression for the exact owner-reported symptom: a real
        // word ("DRAWN") plus a noise-shaped fragment ("2 = S") both make it
        // into `write_ocr_pdf`'s input in CONFIDENCE order (noise first,
        // exactly `merge_rotate4x_candidates`'s pre-fix output shape) but
        // with the noise positioned at the BOTTOM of the page and the real
        // word at the TOP — the opposite of their write order. Asserts (a)
        // the noise never reaches extracted text, and (b) the real word does
        // — i.e. even without a reading-order-aware caller, the shape filter
        // alone keeps this specific pair from ever being glued together in
        // extraction order, because the noise side of the pair is dropped
        // entirely.
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let top_of_page = line_at("DRAWN", Some(0.55), (10.0, 780.0, 60.0, 790.0));
        let bottom_of_page = line_at("2 = S", Some(0.95), (10.0, 100.0, 60.0, 110.0));
        // Confidence order: noise first, exactly what a pre-fix caller would
        // hand this function without the ocr::mod reading-order sort.
        let lines = vec![bottom_of_page, top_of_page];

        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();
        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap_or_default();

        assert!(
            text.contains("DRAWN"),
            "real word should survive, got: {text:?}"
        );
        assert!(
            !text.contains("2 = S"),
            "noise fragment should have been filtered out, got: {text:?}"
        );
    }

    #[test]
    fn none_confidence_line_is_dropped() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let lines = vec![horizontal_line("UNSCORED", None)];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap_or_default();
        assert!(!text.contains("UNSCORED"));
    }

    #[test]
    fn empty_text_line_is_dropped() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let lines = vec![horizontal_line("   ", Some(0.99))];
        // Should not panic and should not add a content stream at all.
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn degenerate_zero_area_line_is_dropped() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        // Zero-width box (top-left == top-right).
        let degenerate = line(
            "ZEROWIDTH",
            Some(0.9),
            (10.0, 112.0),
            (10.0, 112.0),
            (10.0, 100.0),
            (10.0, 100.0),
        );
        let out = write_ocr_pdf(&base, &[(0, vec![degenerate])], DEFAULT_MIN_CONFIDENCE).unwrap();
        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap_or_default();
        assert!(!text.contains("ZEROWIDTH"));
    }

    #[test]
    fn empty_pages_lines_is_a_noop_and_preserves_page_count() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let out = write_ocr_pdf(&base, &[], DEFAULT_MIN_CONFIDENCE).unwrap();
        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
    }

    #[test]
    fn out_of_range_page_index_is_skipped_not_an_error() {
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };
        let lines = vec![horizontal_line("SHOULD NOT CRASH", Some(0.9))];
        // page_index 5 doesn't exist on a 1-page document.
        let result = write_ocr_pdf(&base, &[(5, lines)], DEFAULT_MIN_CONFIDENCE);
        assert!(result.is_ok());
    }

    #[test]
    fn compute_tz_is_clamped_and_finite() {
        assert!((10.0..=1000.0).contains(&compute_tz("", 100.0, 12.0)));
        assert!((10.0..=1000.0).contains(&compute_tz("X", 0.0001, 12.0)));
        assert!((10.0..=1000.0).contains(&compute_tz(
            "A VERY LONG STRING OF MANY CHARACTERS INDEED",
            5.0,
            12.0
        )));
        assert!(compute_tz("NORMAL", 100.0, 12.0).is_finite());
    }

    #[test]
    fn winansi_encode_passes_through_ascii_and_substitutes_control_chars() {
        assert_eq!(winansi_encode("ABC 123"), b"ABC 123");
        assert_eq!(winansi_encode("\u{0007}bell"), b"?bell");
    }

    #[test]
    fn escape_pdf_literal_escapes_parens_and_backslash() {
        assert_eq!(escape_pdf_literal(b"a(b)c\\d"), b"a\\(b\\)c\\\\d".to_vec());
    }

    #[test]
    fn winansi_encode_preserves_latin1_supplement_as_single_bytes() {
        // 0xB0 = degree sign in WinAnsiEncoding, matching Unicode U+00B0's
        // code point directly (PDF 32000-1:2008 Annex D.2) - the exact
        // character class CAD/lighting text uses constantly ("45°", "3000°K").
        let encoded = winansi_encode("45°");
        assert_eq!(encoded, vec![b'4', b'5', 0xB0]);
    }

    #[test]
    fn escape_pdf_literal_preserves_single_byte_winansi_chars() {
        // Regression test for a code-review-caught bug (PR #103 fix round):
        // an earlier version round-tripped these bytes through `char`/
        // `String`, which silently UTF-8-re-encoded any byte >= 0x80 into
        // TWO bytes (0xB0 -> 0xC2 0xB0), corrupting the Tj literal and
        // breaking both lopdf::extract_text and PDFium search for any word
        // containing a Latin-1-supplement character. The escaped output
        // MUST be the exact same single input byte, never re-encoded.
        let encoded = winansi_encode("45°");
        let escaped = escape_pdf_literal(&encoded);
        assert_eq!(
            escaped,
            vec![b'4', b'5', 0xB0],
            "degree sign must survive as the single raw WinAnsi byte 0xB0, \
             not be re-encoded to the two-byte UTF-8 sequence 0xC2 0xB0"
        );
    }

    #[test]
    fn write_ocr_pdf_embeds_extractable_text_with_degree_sign() {
        // End-to-end (lopdf-only, no Tesseract/PDFium needed) proof that a
        // Latin-1-supplement character round-trips through the full
        // write_ocr_pdf -> content-stream -> lopdf::extract_text path
        // (the exact call search::indexer::extract_pdf_text makes for the
        // Tantivy folder index) without corruption. Synthetic OcrLine -
        // deliberately does not depend on a real OCR engine correctly
        // recognizing a degree sign, which is a separate (Tesseract
        // accuracy) question from whether THIS WRITER correctly embeds one
        // it was given.
        let base = {
            let mut d = one_page_pdf();
            let mut out = Vec::new();
            d.save_to(&mut out).unwrap();
            out
        };

        let lines = vec![horizontal_line("SCALE 45° ANGLE", Some(0.9))];
        let out = write_ocr_pdf(&base, &[(0, lines)], DEFAULT_MIN_CONFIDENCE).unwrap();

        let doc = Document::load_from(Cursor::new(&out)).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(
            text.contains('°'),
            "expected the extracted text to contain the degree sign, got: {text:?}"
        );
        assert!(
            text.to_uppercase().contains("SCALE") && text.to_uppercase().contains("ANGLE"),
            "expected the surrounding ASCII text to survive too, got: {text:?}"
        );
    }

    #[test]
    fn rotated_line_produces_non_identity_text_matrix() {
        // A 90-degree-rotated line (top-left -> top-right runs straight UP,
        // matching how rotate-4x maps a vertical CAD dimension string back
        // into the original page's coordinate space).
        let vertical = line(
            "DIM 3650 MM",
            Some(0.9),
            (50.0, 10.0),  // top-left
            (50.0, 110.0), // top-right (baseline runs upward: dx=0, dy=100)
            (38.0, 10.0),  // bottom-left
            (38.0, 110.0), // bottom-right
        );
        let ops =
            render_line_ops(&vertical, DEFAULT_MIN_CONFIDENCE).expect("should not be filtered");
        // For a 90-degree rotation, cos(angle)=0, sin(angle)=1 -> Tm reads
        // "0 1 -1 0 ...". pdf_num(0.0) == "0", pdf_num(1.0) == "1",
        // pdf_num(-1.0) == "-1" (see docops::pdf_num's own tests).
        let ops_str = String::from_utf8_lossy(&ops);
        assert!(
            ops_str.contains("0 1 -1 0 "),
            "expected a 90-degree rotation text matrix, got: {ops_str}"
        );
    }
}
