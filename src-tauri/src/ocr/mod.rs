//! OCR (Phase 2a) — wraps Tesseract 5 (via the `leptess` crate) over a
//! whole-page raster from `render::RenderEngine::render_page_full`, with a
//! rotate-4x-and-merge pass to recover rotated/vertical CAD dimension
//! strings.
//!
//! Feature-gated (`ocr`, off by default): this phase ships the engine
//! wrapper + rotate-4x strategy + the fixture corpus (`tools/fixtures/ocr/`,
//! unchanged from Phase 1) + the accuracy/latency benchmark
//! (`tests/ocr_benchmark.rs`) that gates Phase 2b/2c. No `-ocr.pdf`
//! invisible-text writer, no UI, no auto-trigger yet — see the scoping doc
//! (dev-reports/2026-09-02-redline-auto-ocr-scoping.md) §B/§C for that
//! follow-on design.
//!
//! # Why Tesseract, and why this module owns rotation
//! Phase 1 chose `ocrs` (pure-Rust, ONNX-via-`rten`) specifically because its
//! detection model returns a per-line `RotatedRect`, needing no rotate+merge
//! pass. A 2026-09-02 bake-off (dev-reports/2026-09-02-redline-ocr-bakeoff.md,
//! observation:lr0cwsixkpbzei7vthon) found that engine's actual recognition
//! accuracy insufficient (30% overall recall even with a rotation fix,
//! measured against the SAME corpus this module is benchmarked against) and
//! Tesseract 5 via `leptess` decisively better (93%/98% recall,
//! baseline/rotate-4x). Tesseract's own page-segmentation only reads
//! horizontal-ish text at one fixed page orientation — it has no per-region
//! rotation detection — so THIS module rotates the whole-page raster at
//! 0/90/180/270 degrees, runs Tesseract at each, maps every recognized line's
//! bounding box back to the ORIGINAL page's pixel space, and merges the four
//! passes' results by discarding lower-confidence duplicates of the same
//! region (non-max suppression by IoU). See `recognize_page` below.
//!
//! # Tessdata
//! `leptess::LepTess::new(data_path, lang)` needs Tesseract's English
//! trained-data file (`eng.traineddata`) findable either at `data_path` (when
//! `Some`) or via Tesseract's own standard lookup (`TESSDATA_PREFIX` env var,
//! or its compiled-in default) when `data_path` is `None`. `OcrEngineHandle`
//! passes through whatever `tessdata_dir` its caller supplies (`None` covers
//! both a system install, e.g. Homebrew on macOS or `tesseract-ocr-eng` on
//! Debian/Ubuntu, and an explicit `TESSDATA_PREFIX`). Bundling `eng.
//! traineddata` into the shipped macOS/Windows app (so end users need no
//! system Tesseract install) is Phase 2b scope — see `docs/ocr.md`.
//!
//! # Coordinate mapping
//! Each recognized line's bounding box comes from Tesseract in the ROTATED
//! raster's pixel space (origin top-left, y down, axis-aligned since it is a
//! multiple-of-90-degrees rotation). `rotate::inverse_map` undoes exactly the
//! rotation that pass applied, landing back in the ORIGINAL raster's pixel
//! space; from there the same PDF-user-space conversion Phase 1 used
//! (`pixel_to_pdf`, per `render::PageRaster`'s doc comment on the one-flip
//! inverse formula) applies unchanged. A vertical CAD dimension string's
//! bounding box therefore comes out correctly tall-and-narrow in PDF space
//! even though Tesseract itself never saw a rotated page — the WHOLE PAGE was
//! rotated upright for that pass, not the string.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use image::ImageFormat;

use crate::render::PageRaster;

/// Invisible searchable text-layer writer (Phase 2c-i) — see its own module
/// doc comment for the in-place-vs-sidecar decision and design.
pub mod writer;

/// A single OCR-recognized text line, in PDF user-space coordinates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrLine {
    /// Recognized text (Tesseract's English recognition model — see the
    /// module doc comment on tessdata / language scope).
    pub text: String,
    /// Per-line confidence, 0.0-1.0. Tesseract reports per-WORD confidence
    /// (0-100) via its TSV output; this is the mean of the member words'
    /// confidences for the line, normalized to 0.0-1.0. `None` only if a
    /// line somehow has zero confidence-bearing words (defensive — not
    /// observed in practice).
    pub confidence: Option<f32>,
    /// The line's bounding box in PDF user-space points, as 4 corners
    /// (top-left, top-right, bottom-right, bottom-left of the ROTATED-pass's
    /// axis-aligned box, mapped through that pass's inverse rotation). For a
    /// line recognized at the 0-degree pass these are the usual axis-aligned
    /// rect corners; for a line recovered via a 90/180/270-degree pass they
    /// trace the box's true position/extent on the UNROTATED page — e.g. a
    /// vertical CAD dimension string comes out as a tall, narrow quad, not a
    /// wide horizontal one, even though Tesseract read it after the page was
    /// rotated upright.
    pub corners_pdf: [(f64, f64); 4],
    /// Axis-aligned bounding box of `corners_pdf`, as (min_x, min_y, max_x,
    /// max_y) — a convenience for callers that don't need the exact quad,
    /// and the field this module's rotate-4x merge overlap-checks against.
    pub bbox_pdf: (f64, f64, f64, f64),
}

/// A loaded Tesseract engine, ready to run against page rasters. Construction
/// (`load`) is the expensive step (Tesseract `Init`, loads the language
/// model); reuse one instance across pages/documents rather than reloading
/// per call — `recognize_page` reuses it across all 4 rotate-4x passes
/// internally already.
pub struct OcrEngineHandle {
    lt: leptess::LepTess,
}

impl OcrEngineHandle {
    /// Load Tesseract for English recognition. `tessdata_dir`, when `Some`,
    /// is passed straight to `leptess::LepTess::new` as the explicit
    /// tessdata search path; `None` defers to Tesseract's own standard
    /// lookup (`TESSDATA_PREFIX` env var or compiled-in default — see the
    /// module doc comment).
    pub fn load(tessdata_dir: Option<&Path>) -> Result<Self> {
        let data_path = tessdata_dir
            .map(|p| {
                p.to_str()
                    .with_context(|| format!("tessdata_dir {:?} is not valid UTF-8", p))
            })
            .transpose()?;
        let lt = leptess::LepTess::new(data_path, "eng").map_err(|e| {
            anyhow::anyhow!(
                "leptess::LepTess::new failed ({e}) — is tesseract-ocr-eng / the eng \
                 traineddata installed, or TESSDATA_PREFIX set? tessdata_dir arg: {:?}",
                tessdata_dir
            )
        })?;
        Ok(Self { lt })
    }

    /// Run OCR over a whole-page raster using the rotate-4x strategy: OCR
    /// the raster at 0/90/180/270-degree rotations, map every recognized
    /// line's box back to the raster's own (unrotated) pixel space, then
    /// merge the four passes by dropping lower-confidence duplicates of the
    /// same region (see `merge_rotate4x_candidates`). This is the intended
    /// entry point for production use — see the module doc comment for why
    /// a single 0-degree pass alone misses vertical/rotated CAD text
    /// entirely.
    pub fn recognize_page(&mut self, raster: &PageRaster) -> Result<Vec<OcrLine>> {
        let mut candidates = Vec::new();
        for rotation in Rotation::ALL {
            candidates.extend(
                self.recognize_pass(raster, rotation)
                    .with_context(|| format!("recognize_pass failed at {rotation:?}"))?,
            );
        }
        let mut lines = merge_rotate4x_candidates(candidates);
        // `merge_rotate4x_candidates` returns its survivors in the
        // confidence-descending order its own NMS pass needs internally —
        // that is an implementation detail of the merge, not a meaningful
        // order for a page's text. Re-order into reading order here so
        // EVERY consumer (the invisible-text-layer writer, and therefore
        // anything that later extracts this page's text — lopdf's
        // `extract_text` for the Tantivy folder index, PDFium's own text
        // search) sees lines top-to-bottom/left-to-right, the same way a
        // human reads the page. Without this, a real word and an unrelated
        // noise fragment from a completely different part of the page can
        // land right next to each other in extracted text purely because
        // they happened to score similarly — see `sort_by_reading_order`'s
        // own doc comment and
        // `reading_order_recovers_spatial_order_from_confidence_ordered_merge_output`
        // below for the concrete owner-reported symptom this fixes.
        sort_by_reading_order(&mut lines);
        Ok(lines)
    }

    /// Run OCR over the raster at a single rotation, returning lines already
    /// mapped back into the ORIGINAL raster's PDF-user-space coordinates.
    /// Exposed at `pub(crate)` visibility for the benchmark/tests; normal
    /// callers should use `recognize_page`.
    fn recognize_pass(&mut self, raster: &PageRaster, rotation: Rotation) -> Result<Vec<OcrLine>> {
        let png = rotated_png_bytes(raster, rotation)
            .with_context(|| format!("rotated_png_bytes failed at {rotation:?}"))?;
        self.lt
            .set_image_from_mem(&png)
            .map_err(|e| anyhow::anyhow!("leptess set_image_from_mem failed: {e}"))?;
        // Suppresses Tesseract's "Warning: Invalid resolution 0 dpi" (the
        // synthetic PNG carries no DPI metadata) and gives Tesseract's
        // internal heuristics the real value instead of a guess.
        self.lt
            .set_source_resolution(raster.dpi.round().max(1.0) as i32);

        // get_tsv_text(1): the `page` argument only labels the `page_num`
        // column in the output (Tesseract's own convention is 1-based page
        // numbering in TSV/hOCR); it does not affect recognition. Calling it
        // (rather than get_utf8_text) triggers recognition internally (per
        // leptess's own `set_image` doc comment) AND gives per-word
        // confidence + bounding boxes in one pass — no need for a second,
        // slower per-line re-recognition loop.
        let tsv = self
            .lt
            .get_tsv_text(1)
            .map_err(|e| anyhow::anyhow!("leptess get_tsv_text failed: {e:?}"))?;

        let words = parse_tsv_words(&tsv);
        let lines = group_words_into_lines(words);

        let mut out = Vec::with_capacity(lines.len());
        for line in lines {
            // Corners of the line's axis-aligned box in the ROTATED raster's
            // pixel space, in a fixed winding order (top-left, top-right,
            // bottom-right, bottom-left).
            let rotated_corners = [
                (line.left, line.top),
                (line.left + line.width, line.top),
                (line.left + line.width, line.top + line.height),
                (line.left, line.top + line.height),
            ];
            let mut corners_pdf = [(0.0_f64, 0.0_f64); 4];
            for (dst, (rx, ry)) in corners_pdf.iter_mut().zip(rotated_corners) {
                let (ox, oy) = rotation.inverse_map(rx, ry, raster.width_px, raster.height_px);
                *dst = pixel_to_pdf(ox, oy, raster);
            }
            let bbox_pdf = bounding_box(&corners_pdf);
            out.push(OcrLine {
                text: line.text,
                // TsvLine::confidence is 0-100 (Tesseract's own scale);
                // OcrLine::confidence is documented as 0.0-1.0.
                confidence: Some((line.confidence / 100.0).clamp(0.0, 1.0)),
                corners_pdf,
                bbox_pdf,
            });
        }
        Ok(out)
    }
}

/// A whole-page rotation applied before handing the raster to Tesseract.
/// Matches `image::imageops::rotate{90,180,270}`'s own convention: all three
/// rotate the image content CLOCKWISE by the named number of degrees
/// (verified against the `image` 0.25 crate's own docs this session — see
/// the RETURN payload's References).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rotation {
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    const ALL: [Rotation; 4] = [
        Rotation::Deg0,
        Rotation::Deg90,
        Rotation::Deg180,
        Rotation::Deg270,
    ];

    /// Map one point `(x, y)` in THIS rotation's rotated-image pixel space
    /// (continuous coordinates, origin top-left, y down — same convention
    /// `pixel_to_pdf` uses) back to the point in the ORIGINAL (unrotated)
    /// raster's pixel space that produced it. `orig_w`/`orig_h` are the
    /// original raster's `width_px`/`height_px`.
    ///
    /// Derivation (continuous boundary coordinates, not discrete pixel
    /// indices — consistent with how `pixel_to_pdf` already treats a pixel
    /// coordinate as a real number ranging 0..width_px / 0..height_px):
    /// rotating an image `rotate90` (90° clockwise) maps original point
    /// `(x, y)` to rotated point `(x', y') = (orig_h - y, x)`; solving for
    /// `(x, y)` given `(x', y')` gives the `Deg90` arm below. The `Deg180`
    /// and `Deg270` arms follow the same derive-forward-then-invert method.
    /// Each arm's self-consistency (forward-then-inverse recovers the input)
    /// is checked directly in this module's unit tests.
    fn inverse_map(self, x: f64, y: f64, orig_w: u32, orig_h: u32) -> (f64, f64) {
        let w = f64::from(orig_w);
        let h = f64::from(orig_h);
        match self {
            Rotation::Deg0 => (x, y),
            Rotation::Deg90 => (y, h - x),
            Rotation::Deg180 => (w - x, h - y),
            Rotation::Deg270 => (w - y, x),
        }
    }
}

/// Rotate `raster`'s RGB buffer by `rotation` and PNG-encode the result.
/// `leptess`'s high-level API only accepts image FILES or encoded image
/// BYTES (`set_image`/`set_image_from_mem`, backed by leptonica's
/// format-sniffing `pixReadMem` — verified against leptess v0.14.0's own
/// source this session, no raw-pixel-buffer constructor exists), so a raw
/// `PageRaster.rgb` buffer must be encoded before Tesseract can see it, at
/// every rotation pass including the 0-degree one.
fn rotated_png_bytes(raster: &PageRaster, rotation: Rotation) -> Result<Vec<u8>> {
    let img: image::RgbImage =
        image::RgbImage::from_raw(raster.width_px, raster.height_px, raster.rgb.clone()).context(
            "image::RgbImage::from_raw failed — PageRaster.rgb length must equal \
                 width_px * height_px * 3",
        )?;
    let rotated = match rotation {
        Rotation::Deg0 => img,
        Rotation::Deg90 => image::imageops::rotate90(&img),
        Rotation::Deg180 => image::imageops::rotate180(&img),
        Rotation::Deg270 => image::imageops::rotate270(&img),
    };
    let mut png_bytes = Vec::new();
    rotated
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .context("failed to PNG-encode rotated raster for leptess")?;
    Ok(png_bytes)
}

/// One Tesseract TSV word-level (level=5) row, the fields this module needs.
struct TsvWord {
    block_num: i64,
    par_num: i64,
    line_num: i64,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    /// 0-100 (Tesseract's own scale); negative values ("-1", meaning "not
    /// computed at this level") are filtered out by `parse_tsv_words` before
    /// this struct is constructed for word rows — word rows carry a real
    /// confidence in every Tesseract version this crate targets.
    conf: f32,
    text: String,
}

/// One merged text line: words sharing the same (block, paragraph, line)
/// grouping, joined into text with a unioned bounding box and averaged
/// confidence.
struct TsvLine {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    /// Mean of the member words' confidences, 0-100 scale (matches
    /// `TsvWord::conf`'s scale — normalized to 0.0-1.0 by the caller when
    /// building `OcrLine::confidence`).
    confidence: f32,
    text: String,
}

/// Parse Tesseract's TSV output (`LepTess::get_tsv_text`) into word-level
/// rows only (`level == 5`), dropping the page/block/paragraph/line
/// structural rows (`level` 1-4, whose `conf` column is always `-1`) and any
/// blank/whitespace-only word text.
///
/// Column layout verified empirically against a real `tesseract … stdout
/// tsv` run this session (Tesseract 5.5.3): `level  page_num  block_num
/// par_num  line_num  word_num  left  top  width  height  conf  text` — a
/// tab-separated header row followed by one row per detected component at
/// every level. Not guessed from memory; see the RETURN payload's
/// References for the exact captured output this parser was written against.
fn parse_tsv_words(tsv: &str) -> Vec<TsvWord> {
    const WORD_LEVEL: &str = "5";
    let mut out = Vec::new();
    for (i, line) in tsv.lines().enumerate() {
        if i == 0 {
            continue; // header row
        }
        // splitn(12, ..): the `text` column is last and may itself be empty
        // or (defensively) contain no further tabs in practice, but this
        // guards against a pathological word ever containing one.
        let cols: Vec<&str> = line.splitn(12, '\t').collect();
        if cols.len() < 12 || cols[0] != WORD_LEVEL {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let parsed = (|| -> Option<TsvWord> {
            Some(TsvWord {
                block_num: cols[2].parse().ok()?,
                par_num: cols[3].parse().ok()?,
                line_num: cols[4].parse().ok()?,
                left: cols[6].parse().ok()?,
                top: cols[7].parse().ok()?,
                width: cols[8].parse().ok()?,
                height: cols[9].parse().ok()?,
                conf: cols[10].parse().ok()?,
                text: text.to_string(),
            })
        })();
        if let Some(word) = parsed {
            out.push(word);
        }
    }
    out
}

/// Group consecutive words sharing the same (block_num, par_num, line_num)
/// into `TsvLine`s. Tesseract's TSV emits rows in structural traversal order
/// (block -> paragraph -> line -> word), so a line's words are always
/// contiguous — no need for a hash-map grouping that would lose the
/// original reading order.
fn group_words_into_lines(words: Vec<TsvWord>) -> Vec<TsvLine> {
    let mut groups: Vec<(i64, i64, i64, Vec<TsvWord>)> = Vec::new();
    for word in words {
        let key = (word.block_num, word.par_num, word.line_num);
        match groups.last_mut() {
            Some(last) if (last.0, last.1, last.2) == key => last.3.push(word),
            _ => groups.push((key.0, key.1, key.2, vec![word])),
        }
    }
    groups
        .into_iter()
        .filter_map(|(_, _, _, words)| {
            if words.is_empty() {
                return None;
            }
            let text = words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let mut min_l = f64::MAX;
            let mut min_t = f64::MAX;
            let mut max_r = f64::MIN;
            let mut max_b = f64::MIN;
            let mut conf_sum = 0.0_f32;
            let mut conf_n = 0_u32;
            for w in &words {
                min_l = min_l.min(w.left);
                min_t = min_t.min(w.top);
                max_r = max_r.max(w.left + w.width);
                max_b = max_b.max(w.top + w.height);
                if w.conf >= 0.0 {
                    conf_sum += w.conf;
                    conf_n += 1;
                }
            }
            let confidence = if conf_n > 0 {
                (conf_sum / conf_n as f32).clamp(0.0, 100.0)
            } else {
                0.0
            };
            Some(TsvLine {
                left: min_l,
                top: min_t,
                width: max_r - min_l,
                height: max_b - min_t,
                confidence,
                text,
            })
        })
        .collect()
}

/// Intersection-over-union of two axis-aligned boxes given as (min_x, min_y,
/// max_x, max_y). Returns 0.0 for disjoint or degenerate boxes.
fn iou(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let (ax0, ay0, ax1, ay1) = a;
    let (bx0, by0, bx1, by1) = b;
    let iw = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let ih = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let intersection = iw * ih;
    if intersection <= 0.0 {
        return 0.0;
    }
    let area_a = (ax1 - ax0).max(0.0) * (ay1 - ay0).max(0.0);
    let area_b = (bx1 - bx0).max(0.0) * (by1 - by0).max(0.0);
    let union = area_a + area_b - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Merge OCR-line candidates from the 4 rotate-4x passes: sort by confidence
/// descending, then greedily keep each candidate unless it overlaps (IoU
/// over `IOU_MERGE_THRESHOLD`) an already-kept, higher-confidence candidate
/// — classic non-max suppression. This is the "keep the highest-confidence
/// reading per region, dedupe overlaps" step the mission scopes: the same
/// real text line is expected to be (re-)detected at 1-4 of the passes (the
/// pass whose rotation makes it upright reads it correctly; the other
/// passes typically either miss it or read low-confidence garbage — see the
/// bake-off report's raw-dump section), and only the best reading should
/// survive per region.
fn merge_rotate4x_candidates(mut candidates: Vec<OcrLine>) -> Vec<OcrLine> {
    const IOU_MERGE_THRESHOLD: f64 = 0.3;
    candidates.sort_by(|a, b| {
        b.confidence
            .unwrap_or(0.0)
            .partial_cmp(&a.confidence.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<OcrLine> = Vec::new();
    for candidate in candidates {
        let overlaps_kept = kept
            .iter()
            .any(|k| iou(k.bbox_pdf, candidate.bbox_pdf) > IOU_MERGE_THRESHOLD);
        if !overlaps_kept {
            kept.push(candidate);
        }
    }
    kept
}

/// Reorder `lines` into human reading order: top-to-bottom, then
/// left-to-right within a row. Called once, right after
/// `merge_rotate4x_candidates`, so every downstream consumer (the invisible
/// text-layer writer, and therefore any later text extraction — `lopdf`'s
/// `extract_text` for the Tantivy folder index, PDFium's own text search)
/// sees a page's recognized lines in the same order a person reading the
/// drawing would encounter them.
///
/// Lines are grouped into rows by snapping each line's top edge (`bbox_pdf`'s
/// `max_y`, since PDF user space is y-up) to the nearest `Y_BAND_PTS`
/// multiple — two lines within the same band are treated as visually "the
/// same row" (tolerant of the few points of jitter real OCR boxes have even
/// for text that is genuinely on one line) and ordered by ascending `min_x`
/// within it; rows themselves are ordered top-of-page first (descending y).
/// Sorting on a precomputed `(band, x)` key (rather than a tolerance-based
/// comparator) keeps this a proper total order — a banded *comparator*
/// (`|a, b| (a.y - b.y).abs() < TOL`) is not transitive and can leave
/// `sort_by` with an inconsistent result.
///
/// `Y_BAND_PTS = 6.0` is a deliberately small band: real title-block/label
/// rows in the benchmark corpus (`docs/ocr.md`'s "Measured numbers") are
/// several points apart at minimum, and a too-generous band would risk
/// merging two genuinely distinct rows (e.g. stacked dimension labels) into
/// one, defeating the ordering fix for exactly the layouts it matters most
/// for. Not tuned against a corpus this session — a reasonable follow-up if
/// a real drawing's row spacing turns out tighter than this.
fn sort_by_reading_order(lines: &mut [OcrLine]) {
    const Y_BAND_PTS: f64 = 6.0;
    // Sort on a precomputed integer key: (band index descending, x
    // ascending). `f64` isn't `Ord`, so both components are quantized to
    // `i64` — the band via rounding to `Y_BAND_PTS` multiples, x at
    // sub-point (1/1000 pt) precision, far finer than any real layout
    // distinguishes, purely so `sort_by_key` has a total order to work with.
    lines.sort_by_key(|l| {
        let top_y = l.bbox_pdf.3;
        let band = (top_y / Y_BAND_PTS).round() as i64;
        let x_key = (l.bbox_pdf.0 * 1000.0).round() as i64;
        (-band, x_key)
    });
}

/// Whether `text` plausibly contains real recognized content, as opposed to
/// the short symbol-heavy noise fragments Tesseract routinely produces from
/// the 3 "wrong orientation" rotate-4x passes reading misc. line-work/
/// hatching/borders (`docs/ocr.md`'s "Measured numbers" section: 9-30%
/// precision on lines that already passed the confidence filter, with worked
/// examples like `"fo)"`, `"(=)"`). A line qualifies if it has at least one
/// whitespace-separated token with 2+ characters where a strict majority are
/// alphanumeric — real content (words, initials, drawing numbers, dimension
/// figures) always has at least one such token; a garbage line built from
/// isolated punctuation/symbol characters (`"2 = S"`, `"&z"`) does not,
/// because every one of ITS tokens is either a single stray character or is
/// itself mostly punctuation.
///
/// Deliberately independent of (and applied in addition to) the confidence
/// filter — `docs/ocr.md` already measured that confidence alone doesn't
/// separate noise from real content well (correct lines: 63-82% mean
/// confidence; wrong-orientation noise scores qualitatively low but not
/// reliably below any single fixed threshold). This is a narrow shape check,
/// not a language model: it does not attempt to reject noise that happens to
/// look word-shaped, and does not claim to raise precision to any measured
/// number — see `render_line_ops`'s call site for how it composes with the
/// existing confidence/empty/degenerate-box filters.
pub(crate) fn looks_like_text(text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let total = token.chars().count();
        if total < 2 {
            return false;
        }
        let alnum = token.chars().filter(|c| c.is_alphanumeric()).count();
        alnum * 2 > total
    })
}

/// Map one raster pixel-space point (origin top-left, y down) to PDF
/// user-space (origin bottom-left, y up), per `render::PageRaster::scale`'s
/// doc comment. Unchanged from Phase 1.
fn pixel_to_pdf(x_px: f64, y_px: f64, raster: &PageRaster) -> (f64, f64) {
    let scale = raster.scale as f64;
    let x_pdf = x_px / scale;
    let y_pdf = raster.page_height_pts - (y_px / scale);
    (x_pdf, y_pdf)
}

fn bounding_box(corners: &[(f64, f64); 4]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for &(x, y) in corners {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure logic / coordinate-math tests on synthetic data — no Tesseract
    // binary, no models, no PDFium required, run under plain
    // `cargo test --features ocr` (no `--ignored`).

    fn raster_stub(
        width_px: u32,
        height_px: u32,
        dpi: f32,
        page_w: f64,
        page_h: f64,
    ) -> PageRaster {
        PageRaster {
            doc_id: "test".into(),
            page_index: 0,
            rgb: vec![255u8; (width_px * height_px * 3) as usize],
            width_px,
            height_px,
            dpi,
            scale: dpi / 72.0,
            page_width_pts: page_w,
            page_height_pts: page_h,
        }
    }

    fn line(text: &str, confidence: f32, bbox_pdf: (f64, f64, f64, f64)) -> OcrLine {
        let (x0, y0, x1, y1) = bbox_pdf;
        OcrLine {
            text: text.to_string(),
            confidence: Some(confidence),
            corners_pdf: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
            bbox_pdf,
        }
    }

    #[test]
    fn pixel_to_pdf_maps_top_left_pixel_to_top_left_of_page_in_pdf_space() {
        let raster = raster_stub(2550, 3300, 300.0, 612.0, 792.0);
        let (x, y) = pixel_to_pdf(0.0, 0.0, &raster);
        assert!((x - 0.0).abs() < 1e-9, "x={x}");
        assert!((y - 792.0).abs() < 1e-9, "y={y}");
    }

    #[test]
    fn pixel_to_pdf_maps_bottom_right_pixel_to_bottom_right_of_page_in_pdf_space() {
        let raster = raster_stub(2550, 3300, 300.0, 612.0, 792.0);
        let (x, y) = pixel_to_pdf(2550.0, 3300.0, &raster);
        assert!((x - 612.0).abs() < 1e-3, "x={x}");
        assert!((y - 0.0).abs() < 1e-3, "y={y}");
    }

    #[test]
    fn bounding_box_finds_min_and_max_of_a_rotated_quad() {
        let corners = [(10.0, 20.0), (30.0, 15.0), (35.0, 40.0), (12.0, 45.0)];
        let (min_x, min_y, max_x, max_y) = bounding_box(&corners);
        assert_eq!((min_x, min_y, max_x, max_y), (10.0, 15.0, 35.0, 45.0));
    }

    // --- Rotation::inverse_map -------------------------------------------
    //
    // W=100, H=200 original raster. Each case's expected value is the
    // algebraic inverse of the SAME forward rotation `image::imageops`
    // applies (derivation in `Rotation::inverse_map`'s doc comment); the
    // forward-then-inverse round trip is checked directly rather than only
    // asserting a hand-picked constant, so a sign error in the formula
    // cannot pass by coincidence.

    fn forward_map(rotation: Rotation, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        match rotation {
            Rotation::Deg0 => (x, y),
            Rotation::Deg90 => (h - y, x),
            Rotation::Deg180 => (w - x, h - y),
            Rotation::Deg270 => (y, w - x),
        }
    }

    #[test]
    fn inverse_map_deg0_is_identity() {
        let (x, y) = Rotation::Deg0.inverse_map(37.0, 88.0, 100, 200);
        assert_eq!((x, y), (37.0, 88.0));
    }

    #[test]
    fn inverse_map_round_trips_forward_rotation_for_every_angle() {
        let (w, h) = (100u32, 200u32);
        let sample_points = [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 200.0),
            (0.0, 200.0),
            (42.0, 130.0),
        ];
        for rotation in Rotation::ALL {
            for &(x, y) in &sample_points {
                let (fx, fy) = forward_map(rotation, x, y, w as f64, h as f64);
                let (ix, iy) = rotation.inverse_map(fx, fy, w, h);
                assert!(
                    (ix - x).abs() < 1e-9 && (iy - y).abs() < 1e-9,
                    "{rotation:?}: forward({x},{y})=({fx},{fy}), inverse back=({ix},{iy}), expected ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn inverse_map_deg90_recovers_known_original_point() {
        // Original (30, 150) forward-rotated 90deg CW in a 100x200 raster
        // lands at (H-150, 30) = (50, 30) — see the module doc comment's
        // derivation. inverse_map must undo exactly that.
        let (x, y) = Rotation::Deg90.inverse_map(50.0, 30.0, 100, 200);
        assert!(
            (x - 30.0).abs() < 1e-9 && (y - 150.0).abs() < 1e-9,
            "got ({x},{y})"
        );
    }

    // --- iou ---------------------------------------------------------------

    #[test]
    fn iou_of_identical_boxes_is_one() {
        let b = (0.0, 0.0, 10.0, 10.0);
        assert!((iou(b, b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn iou_of_disjoint_boxes_is_zero() {
        let a = (0.0, 0.0, 10.0, 10.0);
        let b = (20.0, 20.0, 30.0, 30.0);
        assert_eq!(iou(a, b), 0.0);
    }

    #[test]
    fn iou_of_half_overlapping_boxes_matches_expected_fraction() {
        // a: 0..10 x 0..10 (area 100). b: 5..15 x 0..10 (area 100).
        // Intersection: 5..10 x 0..10 = 5*10 = 50. Union = 100+100-50=150.
        let a = (0.0, 0.0, 10.0, 10.0);
        let b = (5.0, 0.0, 15.0, 10.0);
        let expected = 50.0 / 150.0;
        assert!((iou(a, b) - expected).abs() < 1e-9, "iou={}", iou(a, b));
    }

    // --- merge_rotate4x_candidates -----------------------------------------

    #[test]
    fn merge_keeps_higher_confidence_candidate_and_drops_overlapping_duplicate() {
        // Simulates the real bake-off shape: the 0-degree pass reads a
        // vertical dimension string as noise (low confidence), the 90-
        // degree pass reads it correctly (high confidence), both landing on
        // roughly the same PDF-space region.
        let noisy = line("fo) LO Ro)", 0.15, (10.0, 10.0, 30.0, 60.0));
        let correct = line("DIM 3650 MM", 0.92, (11.0, 9.0, 31.0, 61.0));
        let merged = merge_rotate4x_candidates(vec![noisy, correct]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "DIM 3650 MM");
    }

    #[test]
    fn merge_keeps_non_overlapping_regions_separately() {
        let a = line("ROOM 101 OFFICE", 0.9, (0.0, 0.0, 100.0, 20.0));
        let b = line("ROOM 102 MEETING", 0.88, (0.0, 100.0, 100.0, 120.0));
        let mut merged = merge_rotate4x_candidates(vec![a, b]);
        merged.sort_by(|x, y| x.text.cmp(&y.text));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "ROOM 101 OFFICE");
        assert_eq!(merged[1].text, "ROOM 102 MEETING");
    }

    #[test]
    fn merge_of_three_overlapping_readings_keeps_only_the_best() {
        let low = line("garbage1", 0.1, (0.0, 0.0, 10.0, 10.0));
        let mid = line("garbage2", 0.4, (0.5, 0.5, 10.5, 10.5));
        let best = line("DIM 5000 MM", 0.97, (0.2, 0.2, 10.2, 10.2));
        let merged = merge_rotate4x_candidates(vec![low, mid, best]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "DIM 5000 MM");
    }

    // --- TSV parsing ---------------------------------------------------

    /// Real `tesseract … stdout tsv` output (5.5.3), captured this session
    /// against a synthetic two-line test image — see the RETURN payload's
    /// References for the exact command. Exercises: header skip, level
    /// filtering (only level 5 rows survive), a garbled zero-confidence word
    /// ("im"/"gaso" from a font-rendering artifact) still being included
    /// (confidence filtering only affects the AVERAGE, not membership), and
    /// multi-word-per-line grouping.
    const SAMPLE_TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
        1\t1\t0\t0\t0\t0\t0\t0\t400\t150\t-1\t\n\
        2\t1\t1\t0\t0\t0\t21\t22\t70\t8\t-1\t\n\
        3\t1\t1\t1\t0\t0\t21\t22\t70\t8\t-1\t\n\
        4\t1\t1\t1\t1\t0\t21\t22\t70\t8\t-1\t\n\
        5\t1\t1\t1\t1\t1\t21\t22\t70\t8\t90.348671\tHELLOWORLD\n\
        2\t1\t2\t0\t0\t0\t21\t82\t63\t8\t-1\t\n\
        3\t1\t2\t1\t0\t0\t21\t82\t63\t8\t-1\t\n\
        4\t1\t2\t1\t1\t0\t21\t82\t63\t8\t-1\t\n\
        5\t1\t2\t1\t1\t1\t21\t82\t17\t8\t0.000000\tim\n\
        5\t1\t2\t1\t1\t2\t42\t82\t22\t8\t0.000000\tgaso\n\
        5\t1\t2\t1\t1\t3\t70\t82\t14\t8\t52.226261\tMM\n";

    #[test]
    fn parse_tsv_words_keeps_only_word_level_rows() {
        let words = parse_tsv_words(SAMPLE_TSV);
        assert_eq!(
            words.len(),
            4,
            "expected 4 level-5 rows, got {}",
            words.len()
        );
        assert_eq!(words[0].text, "HELLOWORLD");
        assert!((words[0].conf - 90.348_67).abs() < 1e-4);
        assert_eq!(
            (words[0].block_num, words[0].par_num, words[0].line_num),
            (1, 1, 1)
        );
    }

    #[test]
    fn group_words_into_lines_joins_text_and_unions_bbox_for_one_line() {
        let words = parse_tsv_words(SAMPLE_TSV);
        let lines = group_words_into_lines(words);
        assert_eq!(lines.len(), 2, "expected 2 lines, got {}", lines.len());

        assert_eq!(lines[0].text, "HELLOWORLD");
        assert_eq!(
            (lines[0].left, lines[0].top, lines[0].width, lines[0].height),
            (21.0, 22.0, 70.0, 8.0)
        );
        assert!((lines[0].confidence - 90.348_67).abs() < 1e-3);

        // Second line: "im" + "gaso" + "MM", bbox unions left=21 (min) to
        // 70+14=84 (max right), width=63; two zero-confidence words + one
        // real one, so the average pulls it well below the single
        // high-confidence word's score.
        assert_eq!(lines[1].text, "im gaso MM");
        assert_eq!((lines[1].left, lines[1].width), (21.0, 63.0));
        let expected_conf = (0.0 + 0.0 + 52.226_26) / 3.0;
        assert!((lines[1].confidence - expected_conf).abs() < 1e-3);
    }

    #[test]
    fn parse_tsv_words_ignores_structural_rows_with_negative_confidence() {
        let words = parse_tsv_words(SAMPLE_TSV);
        assert!(words.iter().all(|w| w.conf >= 0.0));
    }

    // -----------------------------------------------------------------------
    // sort_by_reading_order / looks_like_text (2026-09-08 search-snippet fix)
    //
    // Owner report: searching "DRAWN" against two OCR'd fixtures returned
    // snippets "DRAWN 2 = S" and "DRAWN &z" — a real matched word glued to
    // an unrelated symbol-heavy noise fragment. Root cause: OcrLines were
    // embedded into the page's invisible text layer (and therefore later
    // extracted, by lopdf for the Tantivy folder index and by PDFium for
    // in-document search) in `merge_rotate4x_candidates`'s internal
    // confidence-descending order, not the page's actual reading order — so
    // a real word and an unrelated noise fragment from a totally different
    // part of the page could land right next to each other in extracted
    // text purely because they scored similarly. These tests cover the two
    // fixes: reordering into reading order, and filtering out symbol-heavy
    // noise shapes before they can ever be embedded.
    // -----------------------------------------------------------------------

    #[test]
    fn sort_by_reading_order_orders_top_to_bottom_left_to_right() {
        // Three lines given in an order that does NOT match their spatial
        // layout — mirrors merge_rotate4x_candidates's confidence-descending
        // output, which reading order must not depend on.
        let mut lines = vec![
            line("BOTTOM RIGHT", 0.6, (100.0, 10.0, 200.0, 20.0)),
            line("TOP LEFT", 0.5, (10.0, 780.0, 60.0, 790.0)),
            line("TOP RIGHT", 0.9, (200.0, 780.0, 260.0, 790.0)),
        ];
        sort_by_reading_order(&mut lines);
        let order: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, vec!["TOP LEFT", "TOP RIGHT", "BOTTOM RIGHT"]);
    }

    #[test]
    fn sort_by_reading_order_treats_lines_within_the_y_band_as_one_row() {
        // Top edges differ by 2pt — real jitter a rotate-4x pass can
        // introduce for text genuinely on one visual row (Y_BAND_PTS is
        // 6.0) — so these must order by x, not by input order.
        let mut lines = vec![
            line("RIGHT", 0.9, (100.0, 100.0, 150.0, 110.0)),
            line("LEFT", 0.5, (10.0, 102.0, 60.0, 112.0)),
        ];
        sort_by_reading_order(&mut lines);
        let order: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, vec!["LEFT", "RIGHT"]);
    }

    #[test]
    fn reading_order_recovers_spatial_order_from_confidence_ordered_merge_output() {
        // Reproduces the owner-reported shape at the merge+sort boundary
        // (no Tesseract/raster needed): a real word near the TOP of the page
        // at LOWER confidence, and a noise-shaped fragment near the BOTTOM
        // at HIGHER confidence — confidence order glues them together with
        // no relation to where they actually sit on the page.
        let candidates = vec![
            line("DRAWN", 0.55, (10.0, 780.0, 60.0, 790.0)), // top of page, real, lower conf
            line("2 = S", 0.95, (10.0, 100.0, 60.0, 110.0)), // bottom of page, noise, higher conf
        ];
        let mut merged = merge_rotate4x_candidates(candidates);
        // Sanity: this IS confidence order — what write_page_text_layer used
        // to embed verbatim before this fix, gluing the two together with no
        // regard for their actual page positions.
        assert_eq!(
            merged[0].text, "2 = S",
            "merge output starts confidence-sorted"
        );

        sort_by_reading_order(&mut merged);
        assert_eq!(
            merged.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["DRAWN", "2 = S"],
            "reading order must put the top-of-page line first, regardless of confidence"
        );
    }

    #[test]
    fn looks_like_text_accepts_real_content() {
        for s in ["DRAWN", "MR", "A-101", "REV A", "2026", "CHECKED BY JR"] {
            assert!(looks_like_text(s), "expected {s:?} to pass");
        }
    }

    #[test]
    fn looks_like_text_rejects_symbol_heavy_noise() {
        // The owner-reported fragments themselves, plus other shapes drawn
        // from the raw extraction dump this fix was diagnosed against
        // (every token in each is either a single stray character or is
        // itself mostly punctuation — see the module doc comment for why
        // `"(=)"` is rejected but a letter-containing fragment like `"fo)"`
        // is deliberately NOT: this is a narrow shape check, not a language
        // model, and doesn't claim to catch noise that happens to look
        // word-shaped).
        for s in ["2 = S", "&z", "? O", "~", ". O =", "> N .", "(=)"] {
            assert!(!looks_like_text(s), "expected {s:?} to be rejected");
        }
    }
}
