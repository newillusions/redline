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
/// **Replaces, never stacks**: any Redline OCR layer already on the page (identified
/// by its `OCR_FONT_NAME` reference, see `remove_ocr_layers`) is removed before the
/// new one is appended, so calling this twice on the same page leaves exactly one
/// layer. Files OCR'd by v0.3.24 and earlier, which stacked duplicates on every re-run,
/// are cleaned up the same way. If every new line is filtered out, the old layer is
/// left in place rather than deleted for nothing.
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

    remove_ocr_layers(doc, page_id)?;
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

/// Operators allowed between `Tf` and the closing `Q` of one Redline text object (the
/// shape `render_line_ops` emits: `q BT /RLOCRFont n Tf 3 Tr n Tz <Tm> (..) Tj ET Q`).
const LAYER_BODY_OPS: [&str; 6] = ["Tr", "Tz", "Tm", "Tj", "TJ", "ET"];

/// Op-index ranges of the Redline text objects in `ops`. A text object is recognised by
/// its exact shape and an EXACT `/RLOCRFont` name operand, never by a substring, so a
/// native stream that merely mentions the name, or a font called `/RLOCRFont2`, is not
/// mistaken for a layer.
fn layer_groups(ops: &[lopdf::content::Operation]) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i + 2 < ops.len() {
        let opens = ops[i].operator == "q"
            && ops[i + 1].operator == "BT"
            && ops[i + 2].operator == "Tf"
            && ops[i + 2]
                .operands
                .first()
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| n == OCR_FONT_NAME.as_bytes());
        if !opens {
            i += 1;
            continue;
        }
        let mut j = i + 3;
        while j < ops.len() && LAYER_BODY_OPS.contains(&ops[j].operator.as_str()) {
            j += 1;
        }
        if j < ops.len() && ops[j].operator == "Q" && ops[j - 1].operator == "ET" {
            groups.push(i..j + 1);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    groups
}

/// What a content stream holds of Redline's OCR layer.
struct LayerScan {
    /// The stream contains Redline text objects (or, for an undecodable stream, its marker).
    present: bool,
    /// The stream consists of nothing but Redline text objects, so it can be dropped whole.
    pure: bool,
    /// Decoded operations and the Redline text-object ranges within them; only set when
    /// the stream decoded cleanly and has no NUL bytes (NULs make lopdf stop parsing
    /// silently, so re-encoding such a stream would lose the content after them).
    editable: Option<(Vec<lopdf::content::Operation>, Vec<std::ops::Range<usize>>)>,
}

fn scan_layer_stream(stream: &Stream) -> LayerScan {
    let none = LayerScan {
        present: false,
        pure: false,
        editable: None,
    };
    let data = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let marker = format!("/{OCR_FONT_NAME}");
    let marker = marker.as_bytes();
    // Cheap pre-filter: decoding every content stream of a large drawing set is costly.
    if !data.windows(marker.len()).any(|w| w == marker) {
        return none;
    }
    let has_nul = data.contains(&0);
    let ops = match lopdf::content::Content::decode(&data) {
        Ok(c) => c.operations,
        // Unparseable but names our font: treat as a layer for the "already OCR'd"
        // probe, never as something to edit or delete.
        Err(_) => {
            return LayerScan {
                present: true,
                ..none
            }
        }
    };
    let groups = layer_groups(&ops);
    if has_nul {
        return LayerScan {
            present: true,
            ..none
        };
    }
    let covered: usize = groups.iter().map(|g| g.len()).sum();
    LayerScan {
        present: !groups.is_empty(),
        pure: !groups.is_empty() && covered == ops.len(),
        editable: (!groups.is_empty()).then_some((ops, groups)),
    }
}

fn scan_stream_id(doc: &Document, id: ObjectId) -> LayerScan {
    match doc.get_object(id) {
        Ok(Object::Stream(s)) => scan_layer_stream(s),
        _ => LayerScan {
            present: false,
            pure: false,
            editable: None,
        },
    }
}

/// How many Redline OCR layers each page carries, keyed by 0-based page index; pages
/// with none are absent. Reads only the document structure and content-stream bytes,
/// so it works on scans whose NUL-padded streams defeat text extraction.
pub(crate) fn ocr_layer_counts(doc: &Document) -> std::collections::BTreeMap<u32, usize> {
    doc.get_pages()
        .into_iter()
        .filter_map(|(page_num_1based, page_id)| {
            let n = doc
                .get_page_contents(page_id)
                .into_iter()
                .filter(|id| scan_stream_id(doc, *id).present)
                .count();
            (n > 0).then(|| (page_num_1based - 1, n))
        })
        .collect()
}

/// Whether any page other than `page_id` lists content stream `id` in its `/Contents`.
fn referenced_by_other_page(doc: &Document, id: ObjectId, page_id: ObjectId) -> bool {
    doc.get_pages()
        .values()
        .any(|p| *p != page_id && doc.get_page_contents(*p).contains(&id))
}

/// Remove Redline's OCR layer from `page_id`, returning how many streams were affected.
///
/// Never deletes native page content:
/// * a stream made up purely of Redline text objects is dropped from the page's
///   `/Contents`, and the stream object is deleted only if no other page still lists it;
/// * a stream that mixes Redline text objects with other content (another tool merged
///   the layer into a native stream) has just those text objects cut out, when it decodes
///   cleanly, has no NUL bytes, re-encodes to the same remaining operations, and no
///   other page shares it;
/// * anything else that names our font is left exactly as it is.
fn remove_ocr_layers(doc: &mut Document, page_id: ObjectId) -> Result<usize> {
    let ids = doc.get_page_contents(page_id);
    let mut kept: Vec<ObjectId> = Vec::new();
    let mut dropped: Vec<ObjectId> = Vec::new();
    let mut stripped: Vec<(ObjectId, Vec<u8>)> = Vec::new();
    for id in ids {
        let scan = scan_stream_id(doc, id);
        match scan.editable {
            Some(_) if scan.pure => dropped.push(id),
            Some((ops, groups)) => {
                kept.push(id);
                if referenced_by_other_page(doc, id, page_id) {
                    continue;
                }
                let remaining: Vec<_> = ops
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !groups.iter().any(|g| g.contains(i)))
                    .map(|(_, op)| op.clone())
                    .collect();
                let expected = remaining.len();
                let Ok(bytes) = (lopdf::content::Content {
                    operations: remaining,
                })
                .encode() else {
                    continue;
                };
                // Re-encoding must round-trip to the same operations or we leave it be.
                if lopdf::content::Content::decode(&bytes)
                    .map(|c| c.operations.len() == expected)
                    .unwrap_or(false)
                {
                    stripped.push((id, bytes));
                }
            }
            None => kept.push(id),
        }
    }
    let affected = dropped.len() + stripped.len();
    if affected == 0 {
        return Ok(0);
    }
    if !dropped.is_empty() {
        doc.get_dictionary_mut(page_id)
            .context("page dict for OCR layer removal")?
            .set(
                "Contents",
                Object::Array(kept.iter().copied().map(Object::Reference).collect()),
            );
        for id in dropped {
            if !referenced_by_other_page(doc, id, page_id) {
                doc.objects.remove(&id);
            }
        }
    }
    for (id, bytes) in stripped {
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&id) {
            s.set_plain_content(bytes);
        }
    }
    Ok(affected)
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

    // --- re-run replaces the layer instead of stacking it -----------------------

    fn two_lines() -> Vec<OcrLine> {
        vec![
            line_at(
                "ROOM SCHEDULE FOR LEVEL THREE",
                Some(0.9),
                (50.0, 700.0, 300.0, 712.0),
            ),
            line_at(
                "AUDITORIUM CEILING PLAN",
                Some(0.9),
                (50.0, 650.0, 300.0, 662.0),
            ),
        ]
    }

    #[test]
    fn a_second_ocr_run_leaves_exactly_one_layer_per_page() {
        let (mut doc, _) = crate::search::indexer::test_support::nul_padded_scan_doc(2);
        let pages = vec![(0, two_lines()), (1, two_lines())];
        write_pages_text_layers(&mut doc, &pages, DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(
            ocr_layer_counts(&doc).values().copied().collect::<Vec<_>>(),
            vec![1, 1]
        );

        write_pages_text_layers(&mut doc, &pages, DEFAULT_MIN_CONFIDENCE).unwrap();
        let counts = ocr_layer_counts(&doc);
        assert_eq!(counts.len(), 2);
        assert!(
            counts.values().all(|&n| n == 1),
            "layers stacked: {counts:?}"
        );

        // The old layer objects are gone, not just unreferenced.
        let streams = doc
            .objects
            .values()
            .filter(|o| matches!(o, Object::Stream(s) if scan_layer_stream(s).present))
            .count();
        assert_eq!(streams, 2);
    }

    #[test]
    fn re_ocr_collapses_the_duplicate_layers_v0_3_24_stacked() {
        let (mut doc, pages) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        // Simulate the shipped bug: two layers appended by two un-guarded runs.
        let font_id = doc.add_object(helvetica_font_dict());
        for _ in 0..2 {
            let id = doc.add_object(Stream::new(
                dictionary! {},
                b"q BT /RLOCRFont 10 Tf 3 Tr (OLD LAYER TEXT) Tj ET Q".to_vec(),
            ));
            append_to_page_contents(&mut doc, pages[0], id).unwrap();
        }
        add_named_objects_to_page_resources(
            &mut doc,
            pages[0],
            b"Font",
            &[(OCR_FONT_NAME.to_string(), font_id)],
        )
        .unwrap();
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&2));

        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));
        let text = crate::search::indexer::extract_doc_text(&mut doc);
        assert!(text[0].1.contains("ROOM SCHEDULE"), "{:?}", text[0]);
        assert!(!text[0].1.contains("OLD LAYER"), "{:?}", text[0]);
    }

    #[test]
    fn a_rerun_whose_lines_are_all_filtered_keeps_the_existing_layer() {
        let (mut doc, _) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        let junk = vec![line_at("xx", Some(0.1), (50.0, 100.0, 80.0, 112.0))];
        write_pages_text_layers(&mut doc, &[(0, junk)], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));
        let text = crate::search::indexer::extract_doc_text(&mut doc);
        assert!(text[0].1.contains("ROOM SCHEDULE"));
    }

    #[test]
    fn a_nul_padded_scan_with_a_layer_is_detected_as_already_ocred() {
        // The owner's Konica shape: NUL-padded scanner stream first, layer second. Both
        // the layer probe and the (repaired) text probe must see the OCR.
        let (mut doc, _) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));
        let text = crate::search::indexer::extract_doc_text(&mut doc);
        assert!(
            text[0].1.chars().filter(|c| !c.is_whitespace()).count() >= 8,
            "text probe blind to the layer: {:?}",
            text[0]
        );
    }

    #[test]
    fn a_page_without_a_layer_is_not_reported() {
        let (doc, _) = crate::search::indexer::test_support::nul_padded_scan_doc(2);
        assert!(ocr_layer_counts(&doc).is_empty());
    }

    // --- never delete native content (review findings F1, F3) -------------------

    const NATIVE_OPS: &[u8] = b"q\n1 0 0 1 0 0 cm\n10 10 200 100 re\nf\nQ\n";
    const OLD_LAYER_OPS: &[u8] =
        b"q\nBT\n/RLOCRFont 12 Tf\n3 Tr\n100 Tz\n1 0 0 1 50 700 Tm\n(OLD LAYER TEXT) Tj\nET\nQ\n";

    fn set_stream(doc: &mut Document, id: ObjectId, bytes: Vec<u8>) {
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&id) {
            s.set_plain_content(bytes);
        }
    }

    fn stream_bytes(doc: &Document, id: ObjectId) -> Vec<u8> {
        match doc.get_object(id) {
            Ok(Object::Stream(s)) => s.decompressed_content().unwrap(),
            other => panic!("not a stream: {other:?}"),
        }
    }

    #[test]
    fn a_layer_merged_into_a_native_stream_is_cut_out_and_the_native_content_survives() {
        // Another tool concatenated our old layer into the page's native stream.
        let (mut doc, pages) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        let stream_id = doc.get_page_contents(pages[0])[0];
        let mut merged = NATIVE_OPS.to_vec();
        merged.extend_from_slice(OLD_LAYER_OPS);
        set_stream(&mut doc, stream_id, merged);
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));

        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();

        // The native stream still exists, still draws its rectangle, and the old text is gone.
        let native = stream_bytes(&doc, stream_id);
        let ops = lopdf::content::Content::decode(&native).unwrap().operations;
        let names: Vec<&str> = ops.iter().map(|o| o.operator.as_str()).collect();
        assert_eq!(
            names,
            vec!["q", "cm", "re", "f", "Q"],
            "native ops changed: {names:?}"
        );
        assert!(!String::from_utf8_lossy(&native).contains("OLD LAYER"));
        // Exactly one layer now: the freshly appended one.
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));
        assert!(doc.get_page_contents(pages[0]).contains(&stream_id));
        assert_eq!(doc.get_page_contents(pages[0]).len(), 2);
    }

    #[test]
    fn a_stream_that_only_mentions_the_font_name_is_not_a_layer_and_is_never_touched() {
        let (mut doc, pages) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        let stream_id = doc.get_page_contents(pages[0])[0];
        // Different font whose name merely starts with ours, plus ordinary drawing.
        let lookalike =
            b"q\nBT\n/RLOCRFont2 12 Tf\n(HELLO) Tj\nET\nQ\n10 10 20 20 re\nf\n".to_vec();
        set_stream(&mut doc, stream_id, lookalike.clone());
        assert!(ocr_layer_counts(&doc).is_empty());
        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(stream_bytes(&doc, stream_id), lookalike);
        assert!(doc.get_page_contents(pages[0]).contains(&stream_id));
    }

    #[test]
    fn a_nul_containing_stream_that_names_our_font_is_left_untouched() {
        // Undecodable-by-lopdf content must never be re-encoded (that would drop the
        // part after the NUL) or deleted; it still counts as an existing layer.
        let (mut doc, pages) = crate::search::indexer::test_support::nul_padded_scan_doc(1);
        let stream_id = doc.get_page_contents(pages[0])[0];
        let mut odd = NATIVE_OPS.to_vec();
        odd.push(0);
        odd.extend_from_slice(OLD_LAYER_OPS);
        set_stream(&mut doc, stream_id, odd.clone());
        assert_eq!(ocr_layer_counts(&doc).get(&0), Some(&1));
        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert_eq!(stream_bytes(&doc, stream_id), odd);
    }

    #[test]
    fn a_layer_shared_by_two_pages_is_not_deleted_out_from_under_the_other_page() {
        let (mut doc, pages) = crate::search::indexer::test_support::nul_padded_scan_doc(2);
        let shared = doc.add_object(Stream::new(dictionary! {}, OLD_LAYER_OPS.to_vec()));
        for page in &pages {
            append_to_page_contents(&mut doc, *page, shared).unwrap();
        }
        assert_eq!(
            ocr_layer_counts(&doc).values().copied().collect::<Vec<_>>(),
            vec![1, 1]
        );

        // Re-OCR page 0 only.
        write_pages_text_layers(&mut doc, &[(0, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();

        assert!(doc.objects.contains_key(&shared), "shared stream deleted");
        let page1 = doc.get_page_contents(pages[1]);
        assert!(page1.contains(&shared));
        assert!(
            page1.iter().all(|id| doc.objects.contains_key(id)),
            "page 1 holds a dangling /Contents reference"
        );
        assert!(!doc.get_page_contents(pages[0]).contains(&shared));

        // Re-OCR page 1 as well: now nobody references it and it is deleted.
        write_pages_text_layers(&mut doc, &[(1, two_lines())], DEFAULT_MIN_CONFIDENCE).unwrap();
        assert!(!doc.objects.contains_key(&shared));
        for page in &pages {
            assert!(doc
                .get_page_contents(*page)
                .iter()
                .all(|id| doc.objects.contains_key(id)));
        }
        assert_eq!(
            ocr_layer_counts(&doc).values().copied().collect::<Vec<_>>(),
            vec![1, 1]
        );
    }
}
