//! Normal appearance stream (`/AP /N`) generation for markup annotations (Bluebeam interop).
//!
//! `annotation::to_annotation_dict` writes a semantically-correct annotation dict (subtype,
//! geometry, colour/opacity/weight) but historically never wrote `/AP`. PDFium (redline's own
//! viewer) and Acrobat synthesize an appearance from geometry when `/AP` is absent; Bluebeam is
//! strict and does not render/persist subtypes it cannot self-appearance. This module builds a
//! `/Subtype /Form` XObject content stream, mirroring the geometry the SVG overlay draws
//! (`src/lib/markup-render.ts`), so every markup carries a real appearance.
//!
//! # Coordinate space
//! The Form's `/BBox` is expressed in the SAME PDF user-space coordinates as the markup's own
//! geometry (no `/Matrix`, so it defaults to identity) - drawing operators use the raw geometry
//! coordinates directly. `/BBox` is a dedicated computation ([`ap_bbox`]), independent of the
//! annotation's own `/Rect` (which stays exactly as `to_annotation_dict` already computes it -
//! this module never touches that function or the semantic `/RL*`/`/QuadPoints` keys).
//!
//! # Opacity
//! The annotation-level `/CA` (already written by `to_annotation_dict`) is applied by the
//! viewer to the whole appearance automatically, so most shapes draw solid colour with no
//! extra alpha in the content stream. Two exceptions need their own `/ExtGState`: Highlight
//! (needs `/BM /Multiply` for the highlighter-over-text look) and a Text/Callout box fill that
//! carries its own `fill_opacity` distinct from the overall opacity.
//!
//! # Known simplifications (named, not silently dropped - see PR description)
//! - Cloud draws a plain closed polygon, not the scalloped revision-cloud arcs.
//! - Stamp/StampDynamic with a `StampAsset::PngBase64` asset draws a REAL Image XObject
//!   (see `decode_png_stamp_image` + `StampImageXObject`) - this is the v0.3.1 fix. A
//!   `Svg` or `PdfBase64` asset, or a malformed/undecodable PNG, still falls back to the
//!   v0.3.0 bordered-box + label text (named deferral - vector-SVG-to-PDF-operator
//!   conversion and embedded-PDF content-stream splicing are both out of scope for this
//!   pass; box+label is a valid, already-shipped appearance, not a broken one).

use lopdf::{dictionary, Dictionary, Object, Stream};

use super::{Appearance, LineStyle, Markup, MarkupGeometry, MarkupType};
use crate::geometry::PdfPoint;
use crate::toolchest::StampAsset;

// --- small helpers (deliberately local - annotation.rs's are private to that module) ----------

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

/// Compact PDF numeric literal (mirrors docops::pdf_num - integers with no decimal point,
/// fractional values trimmed of trailing zeros).
fn num(v: f64) -> String {
    if v.fract().abs() < 1e-9 && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        let s = s.trim_end_matches('0');
        s.trim_end_matches('.').to_string()
    }
}

fn hex_to_rgb(hex: &str) -> [f64; 3] {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return [0.0, 0.0, 0.0];
    }
    let c = |a: usize| {
        u8::from_str_radix(&h[a..a + 2], 16)
            .map(|v| v as f64 / 255.0)
            .unwrap_or(0.0)
    };
    [c(0), c(2), c(4)]
}

/// Escape a string for a PDF literal string token `(...)`, dropping to `?` for anything
/// outside the printable Latin-1 range this simple encoder can represent losslessly.
fn escape_pdf_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 256 => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

/// Map a font family to a base-14 resource name + its standard PDF BaseFont name.
/// Mirrors `annotation::base14_da_name`'s Helv/TiRo/Cour mapping (kept in sync manually -
/// small enough that duplicating beats a cross-module visibility change).
fn base14(family: &str) -> (&'static str, &'static str) {
    let lower = family.to_lowercase();
    if lower.starts_with("times") {
        ("TiRo", "Times-Roman")
    } else if lower.starts_with("courier") {
        ("Cour", "Courier")
    } else {
        ("Helv", "Helvetica")
    }
}

const DEFAULT_FONT_SIZE: f64 = 12.0;
/// Static PDF-point radius for count-marker symbols (independent of zoom - there is no
/// zoom in a saved PDF's own coordinate space).
const COUNT_MARKER_RADIUS: f64 = 6.0;
/// Synthesized callout text-box size in PDF points (mirrors markup-render.ts CALLOUT_BOX_PT).
const CALLOUT_BOX: (f64, f64) = (144.0, 18.0);
/// 4-bezier ellipse approximation constant (control-point offset = radius * kappa).
const ELLIPSE_KAPPA: f64 = 0.552_284_75;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// A PNG-backed stamp's Image XObject, not yet added to a `Document` (streams must be
/// indirect objects - PDF spec 7.3.8 - and only the caller holds the `&mut Document`
/// needed to allocate ids; see `finish_ap_stream`).
pub(crate) struct StampImageXObject {
    /// Resource name the Form's content stream references via its `Do` operator (e.g.
    /// `"Im0"`) - decided up front, independent of the eventual indirect object id.
    pub name: String,
    /// The color (or grayscale) image stream itself.
    pub image: Stream,
    /// Optional soft-mask (alpha channel) stream. If present, the caller must add THIS
    /// one to the `Document` first and point `image`'s own `/SMask` at the resulting id
    /// before adding `image` (see `finish_ap_stream`).
    pub smask: Option<Stream>,
}

/// The un-finished result of building `m`'s `/AP /N` appearance: everything that can be
/// computed WITHOUT a `Document` (pure, and what this module's own tests exercise
/// directly), plus any auxiliary Image XObjects the content stream references. Call
/// [`finish_ap_stream`] once the caller has resolved `image_xobjects` into real indirect
/// references.
pub(crate) struct ApBuild {
    bbox: [f64; 4],
    content: String,
    resources: Dictionary,
    pub image_xobjects: Vec<StampImageXObject>,
}

/// Build `m`'s `/AP /N` appearance (pure - no `Document` access, so this module's tests
/// can call it directly). The caller (`document::annots::write_markups`) resolves
/// `image_xobjects` (if any) into real indirect objects and calls [`finish_ap_stream`] to
/// get the final `Stream` it then adds to the `Document`.
pub(crate) fn build_ap_stream(m: &Markup) -> ApBuild {
    let bbox = ap_bbox(m);
    let (content, resources, image_xobjects) = draw(m);
    ApBuild {
        bbox,
        content,
        resources,
        image_xobjects,
    }
}

/// Assemble the final Form XObject `Stream` from an [`ApBuild`] plus the resolved
/// `/XObject` resource dictionary (name -> indirect reference; empty when the markup has
/// no auxiliary images). Kept as a separate step from [`build_ap_stream`] specifically so
/// that step stays `Document`-free and unit-testable on its own.
pub(crate) fn finish_ap_stream(built: ApBuild, xobject_refs: Dictionary) -> Stream {
    let mut resources = built.resources;
    if !xobject_refs.is_empty() {
        resources.set("XObject", Object::Dictionary(xobject_refs));
    }
    let stream_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "FormType" => 1,
        "BBox" => Object::Array(built.bbox.iter().map(|v| real(*v)).collect()),
        "Resources" => Object::Dictionary(resources),
    };
    Stream::new(stream_dict, built.content.into_bytes())
}

// ---------------------------------------------------------------------------
// BBox - independent of the annotation's semantic /Rect (that one stays untouched).
// ---------------------------------------------------------------------------

/// The annotation `/Rect` AND the `/AP` Form `/BBox` for the shapes where the two MUST be
/// identical for a strict foreign viewer (Bluebeam) to render the appearance at the authored
/// size. A strict viewer maps the transformed appearance `/BBox` into the annotation `/Rect`
/// (ISO 32000-1 12.5.5); when they disagree the whole appearance is scaled/distorted. For
/// these types the rect is the exact bounds of what the appearance draws, so `annotation.rs`
/// uses it for `/Rect` and [`ap_bbox`] uses it for `/BBox`, giving an identity map:
///  - `Text`: the geometry `Rect` (box == text bounds).
///  - `Callout`: the leader bounds UNIONED with the synthesized text box, which sits beyond
///    the leader vertices (the plain geometry bbox omits it - the "resized callout tiny in
///    Bluebeam" G9 defect).
///  - `MeasurementCount`: the symbol's bounds around the point. The zero-size Point geometry
///    otherwise yields a zero `/Rect` that Bluebeam drops entirely (the "counts absent" G9
///    defect); the point is recovered from the `/Rect` centre on read.
///
/// Returns `None` for every other type, whose `/Rect` stays the tight geometry bbox and whose
/// `/BBox` keeps its stroke-padded bounds - Bluebeam regenerates those from the geometry keys
/// and ignores the `/AP`, so the two need not match and the stroke pad avoids clipping in
/// `/AP`-honouring viewers (Acrobat/PDFium).
pub(crate) fn interop_rect(m: &Markup) -> Option<[f64; 4]> {
    match (m.markup_type, &m.geometry) {
        (MarkupType::Text, MarkupGeometry::Rect { min, max }) => Some([
            min.x.min(max.x),
            min.y.min(max.y),
            min.x.max(max.x),
            min.y.max(max.y),
        ]),
        (MarkupType::Callout, MarkupGeometry::Polyline(pts)) if !pts.is_empty() => {
            let anchor = *pts.last().unwrap();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in pts {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
            // Union with the synthesized text box (anchor .. anchor + CALLOUT_BOX), which the
            // Callout appearance draws beyond the leader vertices.
            Some([
                x0.min(anchor.x),
                y0.min(anchor.y),
                x1.max(anchor.x + CALLOUT_BOX.0),
                y1.max(anchor.y + CALLOUT_BOX.1),
            ])
        }
        (MarkupType::MeasurementCount, MarkupGeometry::Point(p)) => {
            let pad = COUNT_MARKER_RADIUS + 4.0;
            Some([p.x - pad, p.y - pad, p.x + pad, p.y + pad])
        }
        _ => None,
    }
}

/// The Form's own bounding box: for the interop-critical types ([`interop_rect`]) it equals
/// the annotation `/Rect` exactly (identity map, no viewer rescale); for every other type
/// it is the geometry's tight bounds, padded so strokes, arrowheads, and glyph ascenders/
/// descenders never clip against the edge.
fn ap_bbox(m: &Markup) -> [f64; 4] {
    if let Some(rect) = interop_rect(m) {
        return rect;
    }
    let pts: Vec<PdfPoint> = match &m.geometry {
        MarkupGeometry::Point(p) => vec![*p],
        MarkupGeometry::Rect { min, max } => vec![*min, *max],
        MarkupGeometry::Polyline(v) => v.clone(),
        MarkupGeometry::Ink(strokes) => strokes.iter().flatten().copied().collect(),
        MarkupGeometry::Quads(quads) => quads.iter().flatten().copied().collect(),
    };
    if pts.is_empty() {
        // No geometry at all (e.g. an Ink markup with zero strokes) - pad around the
        // origin so the Form still has a valid, non-degenerate BBox rather than a
        // zero-size one a viewer would clip to nothing.
        let pad = COUNT_MARKER_RADIUS + 4.0;
        return [-pad, -pad, pad, pad];
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }

    let degenerate = (x1 - x0).abs() < 1e-6 && (y1 - y0).abs() < 1e-6;
    if degenerate {
        // Point geometry (count markers): pad by marker radius + a small margin.
        let pad = COUNT_MARKER_RADIUS + 4.0;
        return [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
    }

    let mut pad = (m.appearance.line_weight.max(0.0) * 3.0).max(6.0);
    match m.markup_type {
        MarkupType::Text | MarkupType::Callout => {
            // Callout leader can extend well past the vertex bounds once the synthesized
            // text box is placed at the anchor end; Text just needs room for the font's
            // ascent/descent beyond the tight Rect.
            let font_pt = m
                .appearance
                .font
                .as_ref()
                .map_or(DEFAULT_FONT_SIZE, |f| f.size_pt);
            pad = pad.max(font_pt).max(CALLOUT_BOX.0.max(CALLOUT_BOX.1) + 8.0);
        }
        MarkupType::Arrow => pad = pad.max(m.appearance.line_weight.max(0.0) * 4.0 + 8.0),
        _ => {}
    }
    [x0 - pad, y0 - pad, x1 + pad, y1 + pad]
}

// ---------------------------------------------------------------------------
// Content stream + resources
// ---------------------------------------------------------------------------

/// Resource-dictionary accumulator so each shape only declares what it uses.
struct Resources {
    dict: Dictionary,
    ext_gstate: Dictionary,
    fonts: Dictionary,
}

impl Resources {
    fn new() -> Self {
        Self {
            dict: Dictionary::new(),
            ext_gstate: Dictionary::new(),
            fonts: Dictionary::new(),
        }
    }

    /// Register an ExtGState and return its (unique, auto-numbered) resource name.
    ///
    /// Most shape arms need only one gstate (their whole paint sequence shares one q/gs/Q
    /// scope - the graphics state tracks /CA and /ca independently per PDF 11.6.4.4, so a
    /// single scope suffices even when an operator paints both fill and stroke). Callout is
    /// the exception: the leader/arrowhead (line_gstate - tracks a.opacity for both, since
    /// the leader has no fill concept of its own) and the synthesized text box
    /// (shape_gstate - independent a.opacity/a.fill_opacity) can genuinely differ, so each
    /// needs its OWN resource name - reusing one name would make the second `gs` call's
    /// dict silently win for both (a PDF's /Resources dict is static, not applied in
    /// stream order), corrupting the first scope's alpha.
    fn add_gstate(&mut self, gs: Dictionary) -> String {
        let gs_name = format!("GS{}", self.ext_gstate.len());
        self.ext_gstate
            .set(gs_name.as_str(), Object::Dictionary(gs));
        gs_name
    }

    /// Register the base-14 font used by this markup and return its resource name.
    fn add_font(&mut self, family: &str) -> (&'static str, &'static str) {
        let (res_name, base_font) = base14(family);
        self.fonts.set(
            res_name,
            Object::Dictionary(dictionary! {
                "Type" => "Font",
                "Subtype" => "Type1",
                "BaseFont" => base_font,
                "Encoding" => "WinAnsiEncoding",
            }),
        );
        (res_name, base_font)
    }

    fn finish(mut self) -> Dictionary {
        if !self.ext_gstate.is_empty() {
            self.dict
                .set("ExtGState", Object::Dictionary(self.ext_gstate));
        }
        if !self.fonts.is_empty() {
            self.dict.set("Font", Object::Dictionary(self.fonts));
        }
        self.dict
    }
}

fn dash_array(style: LineStyle, w: f64) -> Option<(f64, f64)> {
    match style {
        LineStyle::Solid => None,
        LineStyle::Dashed => Some((w.max(0.1) * 3.0, w.max(0.1) * 2.0)),
        LineStyle::Dotted => Some((w.max(0.1), w.max(0.1) * 2.0)),
    }
}

/// Build the per-shape ExtGState dict expressing stroke opacity (`/CA`) and fill opacity
/// (`/ca`) as two independent alpha constants - the PDF-native mechanism for per-operation
/// alpha within one content stream (11.6.4.4). Stroking operators (`S`/`s`/`B`/`b`) consult
/// only `/CA`; non-stroking (fill) operators (`f`/`B`/`b`) consult only `/ca`; a combined
/// operator like `B` therefore honours each independently from a single `gs` scope.
///
/// For shapes with a real, independently-controlled `a.fill` (Rectangle, Ellipse, the
/// closed-polygon family, Stamp, and the Text/Callout box): `/ca` always uses
/// `a.fill_opacity` (default fully opaque `1.0`), fully independent of `a.opacity` - this is
/// the fix for "opacity is global": setting one control no longer moves the other. Use
/// [`line_gstate`] instead for shapes with no independent fill control (Line/Arrow/
/// Polyline/Ink/Callout leader/count markers), where any incidental same-colour fill draw
/// (an arrowhead, a marker interior) should track `a.opacity` too, not `a.fill_opacity`
/// (which is meaningless for those types - they never set `a.fill`).
fn shape_gstate(a: &Appearance) -> Dictionary {
    dictionary! {
        "CA" => real(a.opacity),
        "ca" => real(a.fill_opacity.unwrap_or(1.0)),
    }
}

/// ExtGState for line-family shapes with no independent fill control (Line, Arrow,
/// Polyline, Ink, MeasurementCount markers, the Callout leader/arrowhead): both `/CA`
/// (stroke) and `/ca` (any incidental same-colour fill, e.g. an arrowhead) track `a.opacity`
/// alone, so the whole shape dims as one control. See [`shape_gstate`] for shapes with a
/// real, independent fill.
fn line_gstate(a: &Appearance) -> Dictionary {
    dictionary! {
        "CA" => real(a.opacity),
        "ca" => real(a.opacity),
    }
}

/// Emit the `w`/`d`/colour preamble shared by every stroked shape.
fn stroke_preamble(out: &mut String, a: &Appearance) {
    let [r, g, b] = hex_to_rgb(&a.color);
    out.push_str(&format!("{} {} {} RG\n", num(r), num(g), num(b)));
    out.push_str(&format!("{} w\n", num(a.line_weight.max(0.0))));
    if let Some((on, off)) = dash_array(a.line_style, a.line_weight) {
        out.push_str(&format!("[{} {}] 0 d\n", num(on), num(off)));
    }
}

fn fill_color(out: &mut String, hex: &str) {
    let [r, g, b] = hex_to_rgb(hex);
    out.push_str(&format!("{} {} {} rg\n", num(r), num(g), num(b)));
}

fn moveto(out: &mut String, p: PdfPoint) {
    out.push_str(&format!("{} {} m\n", num(p.x), num(p.y)));
}

fn lineto(out: &mut String, p: PdfPoint) {
    out.push_str(&format!("{} {} l\n", num(p.x), num(p.y)));
}

/// Draw one closed or open polyline path (no paint operator - caller appends S/f/B/h).
fn path_from_points(out: &mut String, pts: &[PdfPoint]) {
    if let Some((first, rest)) = pts.split_first() {
        moveto(out, *first);
        for p in rest {
            lineto(out, *p);
        }
    }
}

/// The paint operator for a stroked shape that may also be filled, per PDF content-stream
/// operator semantics (`f`/`S`/`B`, `h` = close path first).
fn paint_op(has_fill: bool, has_stroke: bool, close: bool) -> &'static str {
    match (has_fill, has_stroke, close) {
        (true, true, true) => "b",  // close, fill (nonzero), stroke
        (true, true, false) => "B", // fill, stroke (no close - open path already painted as-is)
        (true, false, true) => "f", // close is implicit for fill
        (true, false, false) => "f",
        (false, true, true) => "s",  // close + stroke
        (false, true, false) => "S", // stroke only
        (false, false, _) => "n",    // nothing to paint (shouldn't normally happen)
    }
}

fn arrow_head(pts: &[PdfPoint], weight: f64) -> Option<(PdfPoint, [PdfPoint; 3])> {
    if pts.len() < 2 {
        return None;
    }
    let tip = pts[pts.len() - 1];
    let prev = pts[pts.len() - 2];
    let dx = tip.x - prev.x;
    let dy = tip.y - prev.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let head_len = (weight.max(0.0) * 4.0).max(8.0);
    let half_w = (weight.max(0.0) * 2.0).max(4.0);
    let base = PdfPoint {
        x: tip.x - head_len * ux,
        y: tip.y - head_len * uy,
    };
    let left = PdfPoint {
        x: base.x + half_w * nx,
        y: base.y + half_w * ny,
    };
    let right = PdfPoint {
        x: base.x - half_w * nx,
        y: base.y - half_w * ny,
    };
    Some((base, [tip, left, right]))
}

fn count_symbol_points(kind: super::CountSymbol, cx: f64, cy: f64, r: f64) -> Vec<PdfPoint> {
    use super::CountSymbol::*;
    let ngon = |n: usize, start: f64, radius: f64| -> Vec<PdfPoint> {
        (0..n)
            .map(|i| {
                let a = start + (i as f64) * std::f64::consts::TAU / (n as f64);
                PdfPoint {
                    x: cx + radius * a.cos(),
                    y: cy + radius * a.sin(),
                }
            })
            .collect()
    };
    match kind {
        Square => vec![
            PdfPoint {
                x: cx - r,
                y: cy - r,
            },
            PdfPoint {
                x: cx + r,
                y: cy - r,
            },
            PdfPoint {
                x: cx + r,
                y: cy + r,
            },
            PdfPoint {
                x: cx - r,
                y: cy + r,
            },
        ],
        Triangle => ngon(3, std::f64::consts::FRAC_PI_2, r),
        Diamond => vec![
            PdfPoint { x: cx, y: cy + r },
            PdfPoint { x: cx + r, y: cy },
            PdfPoint { x: cx, y: cy - r },
            PdfPoint { x: cx - r, y: cy },
        ],
        Hexagon => ngon(6, std::f64::consts::FRAC_PI_2, r),
        Star => {
            let outer = ngon(5, std::f64::consts::FRAC_PI_2, r);
            let inner = ngon(
                5,
                std::f64::consts::FRAC_PI_2 + std::f64::consts::PI / 5.0,
                r * 0.4,
            );
            let mut pts = Vec::with_capacity(10);
            for i in 0..5 {
                pts.push(outer[i]);
                pts.push(inner[i]);
            }
            pts
        }
        Cross | Circle => Vec::new(), // handled separately (stroke lines / bezier circle)
    }
}

/// Emit a 4-bezier ellipse path (no paint op) for the given center/radii.
fn ellipse_path(out: &mut String, cx: f64, cy: f64, rx: f64, ry: f64) {
    let kx = rx * ELLIPSE_KAPPA;
    let ky = ry * ELLIPSE_KAPPA;
    let n = num;
    out.push_str(&format!("{} {} m\n", n(cx + rx), n(cy)));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx + rx),
        n(cy + ky),
        n(cx + kx),
        n(cy + ry),
        n(cx),
        n(cy + ry)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx - kx),
        n(cy + ry),
        n(cx - rx),
        n(cy + ky),
        n(cx - rx),
        n(cy)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx - rx),
        n(cy - ky),
        n(cx - kx),
        n(cy - ry),
        n(cx),
        n(cy - ry)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx + kx),
        n(cy - ry),
        n(cx + rx),
        n(cy - ky),
        n(cx + rx),
        n(cy)
    ));
    out.push_str("h\n");
}

fn draw_text_block(
    out: &mut String,
    res: &mut Resources,
    text: &str,
    color: &str,
    origin: PdfPoint,
    font: Option<&super::FontSpec>,
) {
    let size = font.map_or(DEFAULT_FONT_SIZE, |f| f.size_pt);
    let family = font.map_or("Helvetica", |f| f.family.as_str());
    let (res_name, _) = res.add_font(family);
    let [r, g, b] = hex_to_rgb(color);
    out.push_str("BT\n");
    out.push_str(&format!("{} {} {} rg\n", num(r), num(g), num(b)));
    out.push_str(&format!("/{res_name} {} Tf\n", num(size)));
    out.push_str(&format!("{} {} Td\n", num(origin.x), num(origin.y)));
    out.push_str(&format!("({}) Tj\n", escape_pdf_string(text)));
    out.push_str("ET\n");
}

/// Box border + optional fill for Text/Callout. Stroke (border) opacity and fill opacity
/// are fully independent (`shape_gstate`): the box border always honours `a.opacity`, the
/// fill (when present) always honours `a.fill_opacity` (default fully opaque) - setting
/// one never moves the other. Both draws share ONE gs scope (the graphics state tracks
/// `/CA`/`/ca` independently, so `f` and `S` each pick up their own alpha automatically).
/// The caller draws the actual glyph text SEPARATELY, outside this function's `q`/`Q`
/// scope, so text is never dimmed by either control (spec: text carries its own alpha).
fn draw_text_box(out: &mut String, res: &mut Resources, a: &Appearance, rect: [f64; 4]) {
    let outline = a.outline_color.as_deref().unwrap_or(&a.color);
    let gs_name = res.add_gstate(shape_gstate(a));
    out.push_str(&format!("q\n/{gs_name} gs\n"));
    if let Some(fill) = &a.fill {
        fill_color(out, fill);
        out.push_str(&format!(
            "{} {} {} {} re\nf\n",
            num(rect[0]),
            num(rect[1]),
            num(rect[2] - rect[0]),
            num(rect[3] - rect[1])
        ));
    }
    let [r, g, b] = hex_to_rgb(outline);
    out.push_str(&format!(
        "{} {} {} RG\n{} w\n{} {} {} {} re\nS\n",
        num(r),
        num(g),
        num(b),
        num(a.line_weight.max(0.0)),
        num(rect[0]),
        num(rect[1]),
        num(rect[2] - rect[0]),
        num(rect[3] - rect[1])
    ));
    out.push_str("Q\n");
}

/// Stamp/StampDynamic fallback: a bordered box + label text (the v0.3.0 behaviour),
/// unchanged - used whenever there is no decodable `StampAsset::PngBase64` (no asset at
/// all, an `Svg`/`PdfBase64` asset, or a malformed one - see the module doc comment).
fn draw_stamp_box_and_label(
    out: &mut String,
    res: &mut Resources,
    m: &Markup,
    a: &Appearance,
    min: PdfPoint,
    max: PdfPoint,
) {
    let gs_name = res.add_gstate(shape_gstate(a));
    out.push_str(&format!("q\n/{gs_name} gs\n"));
    stroke_preamble(out, a);
    if let Some(fill) = &a.fill {
        fill_color(out, fill);
    }
    out.push_str(&format!(
        "{} {} {} {} re\n",
        num(min.x),
        num(min.y),
        num(max.x - min.x),
        num(max.y - min.y)
    ));
    out.push_str(paint_op(a.fill.is_some(), true, false));
    out.push_str("\nQ\n");
    // Label text is drawn OUTSIDE the gs scope (unaffected by either opacity control -
    // text always renders fully opaque, matching the Text/Callout convention).
    if let Some(text) = &m.contents {
        let origin = PdfPoint {
            x: min.x + 2.0,
            y: (min.y + max.y) / 2.0 - 5.0,
        };
        draw_text_block(out, res, text, &a.color, origin, a.font.as_ref());
    }
}

/// Draw a decoded stamp image filling `[min, max]`: scale the unit-square Image XObject
/// coordinate space (always `[0,1]x[0,1]` per PDF spec 8.9.5.1) up to the Rect's width/
/// height via the `cm` operator, then paint it with the `Do` operator. Scoped in its own
/// `gs` (shape_gstate: `/ca` = `a.fill_opacity` - an image paints as a non-stroking
/// operation, so only `/ca` is meaningful; `/CA` is included too for scope consistency
/// with every other shape's ExtGState, but has no effect on `Do`).
fn draw_stamp_image(
    out: &mut String,
    res: &mut Resources,
    a: &Appearance,
    min: PdfPoint,
    max: PdfPoint,
    name: &str,
) {
    let gs_name = res.add_gstate(shape_gstate(a));
    let (sx, sy) = (max.x - min.x, max.y - min.y);
    out.push_str(&format!(
        "q\n/{gs_name} gs\n{} 0 0 {} {} {} cm\n/{name} Do\nQ\n",
        num(sx),
        num(sy),
        num(min.x),
        num(min.y)
    ));
}

/// Decode a base64 PNG stamp asset into a color Image XObject stream + optional soft-mask
/// (alpha channel) stream. Returns `None` on ANY failure (malformed base64, undecodable
/// image bytes, zero-size image) rather than erroring - callers fall back to the
/// box+label appearance, so a corrupt asset degrades gracefully instead of breaking the
/// whole markup's appearance.
fn decode_png_stamp_image(b64: &str) -> Option<(Stream, Option<Stream>)> {
    let bytes = base64_decode(b64)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }

    let color_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
    };
    let mut color_stream = Stream::new(color_dict, img.to_rgb8().into_raw());
    let _ = color_stream.compress();

    let smask = if img.color().has_alpha() {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
        let smask_dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => w as i64,
            "Height" => h as i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        };
        let mut smask_stream = Stream::new(smask_dict, alpha);
        let _ = smask_stream.compress();
        Some(smask_stream)
    } else {
        None
    };

    Some((color_stream, smask))
}

/// Minimal base64 DECODING (standard alphabet, matches `render::base64_encode`) - avoids
/// pulling in the `base64` crate for the one decode call site this module needs (stamp
/// PNG assets arrive as base64 over IPC, mirroring how render tiles are base64-ENCODED
/// for the trip the other way). Returns `None` on any malformed input (invalid character,
/// truncated final group) rather than panicking.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let trimmed = s.trim().trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4 + 3);
    let mut chunk = [0u8; 4];
    let mut n = 0;
    for b in trimmed.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        chunk[n] = val(b)?;
        n += 1;
        if n == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
            out.push((chunk[2] << 6) | chunk[3]);
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => out.push((chunk[0] << 2) | (chunk[1] >> 4)),
        3 => {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        _ => return None, // a single leftover base64 char is malformed
    }
    Some(out)
}

/// Build the content-stream operators + resources for `m`. Returns (content, resources,
/// auxiliary image xobjects - empty except for a PNG-backed Stamp/StampDynamic).
fn draw(m: &Markup) -> (String, Dictionary, Vec<StampImageXObject>) {
    let mut out = String::new();
    let mut res = Resources::new();
    let mut image_xobjects: Vec<StampImageXObject> = Vec::new();
    let a = &m.appearance;

    match (m.markup_type, &m.geometry) {
        // --- Rectangle / Square -------------------------------------------------------
        (MarkupType::Rectangle, MarkupGeometry::Rect { min, max }) => {
            let gs_name = res.add_gstate(shape_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            let has_fill = a.fill.is_some();
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            out.push_str(&format!(
                "{} {} {} {} re\n",
                num(min.x),
                num(min.y),
                num(max.x - min.x),
                num(max.y - min.y)
            ));
            out.push_str(paint_op(has_fill, a.line_weight > 0.0, false));
            out.push_str("\nQ\n");
        }

        // --- Ellipse / Circle ----------------------------------------------------------
        (MarkupType::Ellipse, MarkupGeometry::Rect { min, max }) => {
            let gs_name = res.add_gstate(shape_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            let has_fill = a.fill.is_some();
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            let cx = (min.x + max.x) / 2.0;
            let cy = (min.y + max.y) / 2.0;
            let rx = (max.x - min.x).abs() / 2.0;
            let ry = (max.y - min.y).abs() / 2.0;
            ellipse_path(&mut out, cx, cy, rx, ry);
            out.push_str(paint_op(has_fill, a.line_weight > 0.0, true));
            out.push_str("\nQ\n");
        }

        // --- Line / Arrow / MeasurementLength / MeasurementRadius -----------------------
        (
            MarkupType::Line
            | MarkupType::Arrow
            | MarkupType::MeasurementLength
            | MarkupType::MeasurementRadius,
            MarkupGeometry::Polyline(pts),
        ) if pts.len() >= 2 => {
            // No independent fill control for these types - line_gstate() uses a.opacity
            // for the arrowhead's incidental fill too.
            let gs_name = res.add_gstate(line_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            let (p0, p1) = (pts[0], pts[1]);
            if m.markup_type == MarkupType::Arrow {
                if let Some((base, head)) = arrow_head(&[p0, p1], a.line_weight) {
                    moveto(&mut out, p0);
                    lineto(&mut out, base);
                    out.push_str("S\n");
                    fill_color(&mut out, &a.color);
                    moveto(&mut out, head[0]);
                    lineto(&mut out, head[1]);
                    lineto(&mut out, head[2]);
                    out.push_str("h\nf\n");
                } else {
                    moveto(&mut out, p0);
                    lineto(&mut out, p1);
                    out.push_str("S\n");
                }
            } else {
                moveto(&mut out, p0);
                lineto(&mut out, p1);
                out.push_str("S\n");
            }
            out.push_str("Q\n");
        }

        // --- Closed polygon family: Polygon / Cloud / Measurement{Perimeter,Area,Volume} -
        (
            MarkupType::Polygon
            | MarkupType::Cloud
            | MarkupType::MeasurementPerimeter
            | MarkupType::MeasurementArea
            | MarkupType::MeasurementVolume,
            MarkupGeometry::Polyline(pts),
        ) if pts.len() >= 2 => {
            let gs_name = res.add_gstate(shape_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            let has_fill = a.fill.is_some();
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            path_from_points(&mut out, pts);
            out.push('\n');
            out.push_str(paint_op(has_fill, a.line_weight > 0.0, true));
            out.push_str("\nQ\n");
        }

        // --- Open polyline: PolyLine / MeasurementAngle ---------------------------------
        (MarkupType::Polyline | MarkupType::MeasurementAngle, MarkupGeometry::Polyline(pts))
            if pts.len() >= 2 =>
        {
            let gs_name = res.add_gstate(line_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            path_from_points(&mut out, pts);
            out.push('\n');
            out.push_str("S\nQ\n");
        }

        // --- FreeText: Text ---------------------------------------------------------
        (MarkupType::Text, MarkupGeometry::Rect { min, max }) => {
            draw_text_box(&mut out, &mut res, a, [min.x, min.y, max.x, max.y]);
            if let Some(text) = &m.contents {
                let size = a.font.as_ref().map_or(DEFAULT_FONT_SIZE, |f| f.size_pt);
                let origin = PdfPoint {
                    x: min.x + 2.0,
                    y: (max.y - size).max(min.y),
                };
                draw_text_block(&mut out, &mut res, text, &a.color, origin, a.font.as_ref());
            }
        }

        // --- FreeText: Callout (leader line + synthesized box) --------------------------
        (MarkupType::Callout, MarkupGeometry::Polyline(pts)) if !pts.is_empty() => {
            // The leader/arrowhead has its own gs scope (line_gstate: a.opacity drives both
            // the stroke and the arrowhead's incidental fill) - a SEPARATE resource name
            // from the box's shape_gstate below, since the two can legitimately differ (a
            // faint leader with an opaque box fill, or vice versa) and /Resources is a
            // single static dict, not applied in stream order (see Resources::add_gstate).
            let leader_gs = res.add_gstate(line_gstate(a));
            out.push_str(&format!("q\n/{leader_gs} gs\n"));
            stroke_preamble(&mut out, a);
            // Leader points AT the target (index 0); the box anchors at the last point.
            let target = pts[0];
            let anchor = *pts.last().unwrap();
            if pts.len() >= 2 {
                let reversed: Vec<PdfPoint> = pts.iter().rev().copied().collect();
                if let Some((base, head)) = arrow_head(&reversed, a.line_weight) {
                    moveto(&mut out, anchor);
                    for p in pts.iter().rev().skip(1) {
                        lineto(&mut out, *p);
                    }
                    // Shorten the final segment to the arrowhead base.
                    let _ = base; // base already used inline below for the tip segment
                    out.push_str("S\n");
                    fill_color(&mut out, &a.color);
                    moveto(&mut out, head[0]);
                    lineto(&mut out, head[1]);
                    lineto(&mut out, head[2]);
                    out.push_str("h\nf\n");
                } else {
                    moveto(&mut out, anchor);
                    lineto(&mut out, target);
                    out.push_str("S\n");
                }
            }
            out.push_str("Q\n");
            let rect = [
                anchor.x,
                anchor.y,
                anchor.x + CALLOUT_BOX.0,
                anchor.y + CALLOUT_BOX.1,
            ];
            draw_text_box(&mut out, &mut res, a, rect);
            if let Some(text) = &m.contents {
                let size = a.font.as_ref().map_or(DEFAULT_FONT_SIZE, |f| f.size_pt);
                let origin = PdfPoint {
                    x: rect[0] + 2.0,
                    y: (rect[3] - size).max(rect[1]),
                };
                draw_text_block(&mut out, &mut res, text, &a.color, origin, a.font.as_ref());
            }
        }

        // --- Highlight: text-anchored quads ---------------------------------------------
        (MarkupType::Highlight, MarkupGeometry::Quads(quads)) if !quads.is_empty() => {
            // Opacity is published on the annotation-level /CA (see to_annotation_dict): a
            // strict viewer (Bluebeam) regenerates the highlight from /C + /CA and ignores
            // this /AP, while an /AP-honouring viewer (Acrobat) applies /CA as a group alpha
            // over this whole form - so the form itself paints fully opaque (ca = 1.0) under
            // the Multiply blend and lets /CA supply the wash, keeping both views identical
            // (no double-dim).
            let gs_name = res.add_gstate(dictionary! {
                "BM" => "Multiply",
                "ca" => real(1.0),
            });
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            fill_color(&mut out, &a.color);
            for q in quads {
                moveto(&mut out, q[0]); // TL
                lineto(&mut out, q[1]); // TR
                lineto(&mut out, q[3]); // BR
                lineto(&mut out, q[2]); // BL
                out.push_str("h\n");
            }
            out.push_str("f\nQ\n");
        }

        // --- Highlight: freeform rectangle drag (non-text areas) -------------------------
        (MarkupType::Highlight, MarkupGeometry::Rect { min, max }) => {
            // Opacity is published on the annotation-level /CA (see to_annotation_dict): a
            // strict viewer (Bluebeam) regenerates the highlight from /C + /CA and ignores
            // this /AP, while an /AP-honouring viewer (Acrobat) applies /CA as a group alpha
            // over this whole form - so the form itself paints fully opaque (ca = 1.0) under
            // the Multiply blend and lets /CA supply the wash, keeping both views identical
            // (no double-dim).
            let gs_name = res.add_gstate(dictionary! {
                "BM" => "Multiply",
                "ca" => real(1.0),
            });
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            fill_color(&mut out, &a.color);
            out.push_str(&format!(
                "{} {} {} {} re\nf\nQ\n",
                num(min.x),
                num(min.y),
                num(max.x - min.x),
                num(max.y - min.y)
            ));
        }

        // --- Ink: one moveto/lineto subpath per stroke, single stroke paint -------------
        (MarkupType::Ink, MarkupGeometry::Ink(strokes)) if !strokes.is_empty() => {
            let gs_name = res.add_gstate(line_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            stroke_preamble(&mut out, a);
            let mut any = false;
            for stroke in strokes {
                if stroke.is_empty() {
                    continue;
                }
                path_from_points(&mut out, stroke);
                any = true;
            }
            if any {
                out.push_str("S\n");
            }
            out.push_str("Q\n");
        }

        // --- Stamp / StampDynamic: real Image XObject when a decodable PNG asset is
        // present, else the bordered-box + label fallback (named simplification for
        // Svg/PdfBase64 assets and malformed PNGs - see the module doc comment) ---
        (MarkupType::Stamp | MarkupType::StampDynamic, MarkupGeometry::Rect { min, max }) => {
            let png_image = match &m.stamp_asset {
                Some(StampAsset::PngBase64(b64)) => decode_png_stamp_image(b64),
                _ => None,
            };
            if let Some((color, smask)) = png_image {
                let name = "Im0".to_string();
                draw_stamp_image(&mut out, &mut res, a, *min, *max, &name);
                image_xobjects.push(StampImageXObject {
                    name,
                    image: color,
                    smask,
                });
                // Dynamic-stamp overlay text (e.g. composed date/user on a static asset
                // background) draws OUTSIDE any alpha scope, same convention as the
                // box+label fallback and Text/Callout - text always renders fully opaque.
                if let Some(text) = &m.contents {
                    let origin = PdfPoint {
                        x: min.x + 2.0,
                        y: (min.y + max.y) / 2.0 - 5.0,
                    };
                    draw_text_block(&mut out, &mut res, text, &a.color, origin, a.font.as_ref());
                }
            } else {
                draw_stamp_box_and_label(&mut out, &mut res, m, a, *min, *max);
            }
        }

        // --- MeasurementCount: symbol marker at a point ----------------------------------
        (MarkupType::MeasurementCount, MarkupGeometry::Point(p)) => {
            let symbol = m
                .count_set
                .as_ref()
                .map_or(super::CountSymbol::Circle, |c| c.symbol);
            let r = COUNT_MARKER_RADIUS;
            // No independent fill control for count markers - line_gstate() uses a.opacity
            // for the marker's own-colour interior fill too, so the whole marker dims as
            // one control (matches Line/Arrow's convention for incidental fills).
            let gs_name = res.add_gstate(line_gstate(a));
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            match symbol {
                super::CountSymbol::Circle => {
                    stroke_preamble(&mut out, a);
                    fill_color(&mut out, &a.color);
                    ellipse_path(&mut out, p.x, p.y, r, r);
                    out.push_str("B\n");
                }
                super::CountSymbol::Cross => {
                    stroke_preamble(&mut out, a);
                    moveto(
                        &mut out,
                        PdfPoint {
                            x: p.x - r,
                            y: p.y - r,
                        },
                    );
                    lineto(
                        &mut out,
                        PdfPoint {
                            x: p.x + r,
                            y: p.y + r,
                        },
                    );
                    out.push_str("S\n");
                    moveto(
                        &mut out,
                        PdfPoint {
                            x: p.x - r,
                            y: p.y + r,
                        },
                    );
                    lineto(
                        &mut out,
                        PdfPoint {
                            x: p.x + r,
                            y: p.y - r,
                        },
                    );
                    out.push_str("S\n");
                }
                other => {
                    stroke_preamble(&mut out, a);
                    fill_color(&mut out, &a.color);
                    let pts = count_symbol_points(other, p.x, p.y, r);
                    path_from_points(&mut out, &pts);
                    out.push('\n');
                    out.push_str("b\n");
                }
            }
            out.push_str("Q\n");
        }

        // --- Fallback: nothing drawable for this (type, geometry) combination. A named,
        // structural gap - the annotation still has an empty-but-valid appearance rather
        // than none at all (viewers show a blank box instead of nothing/dropping it).
        _ => {}
    }

    (out, res.finish(), image_xobjects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{
        Audit, CountSet, CountSymbol, FontSpec, MarkupStatus, Origin, UserRef, Workflow,
    };

    fn user() -> UserRef {
        UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        }
    }

    fn base_markup(t: MarkupType, geometry: MarkupGeometry) -> Markup {
        let now = chrono::Utc::now();
        Markup {
            id: uuid::Uuid::new_v4(),
            markup_type: t,
            page: 0,
            geometry,
            appearance: Appearance::default(),
            subject: None,
            layer: None,
            contents: None,
            group_id: None,
            audit: Audit {
                created_by: user(),
                created_at: now,
                modified_by: user(),
                modified_at: now,
                revision: 0,
                origin: Origin::Desktop,
            },
            workflow: Workflow {
                status: MarkupStatus::None,
                assignee: None,
                thread: Vec::new(),
            },
            measurement: None,
            count_set: None,
            stamp_asset: None,
        }
    }

    /// Finish an `ApBuild` with no resolved image xobject references - what every test
    /// that doesn't care about a stamp's embedded image wants. Tests that DO care call
    /// `build_ap_stream(m).image_xobjects` directly instead (Document-free).
    fn full_stream(m: &Markup) -> Stream {
        finish_ap_stream(build_ap_stream(m), Dictionary::new())
    }

    fn content_str(m: &Markup) -> String {
        String::from_utf8(full_stream(m).content).unwrap()
    }

    fn stream_dict_checks(m: &Markup) -> Dictionary {
        full_stream(m).dict
    }

    fn assert_valid_form_dict(d: &Dictionary) {
        assert_eq!(
            d.get(b"Type").unwrap().as_name().unwrap(),
            b"XObject",
            "/AP /N must be /Type /XObject"
        );
        assert_eq!(
            d.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form",
            "/AP /N must be /Subtype /Form"
        );
        let bbox = d.get(b"BBox").unwrap().as_array().unwrap();
        assert_eq!(bbox.len(), 4, "/BBox must have 4 entries");
        let vals: Vec<f64> = bbox.iter().map(|o| o.as_float().unwrap() as f64).collect();
        assert!(
            (vals[2] - vals[0]).abs() > 1e-6 && (vals[3] - vals[1]).abs() > 1e-6,
            "/BBox must be non-degenerate, got {vals:?}"
        );
        assert!(d.has(b"Resources"), "/AP /N must carry /Resources");
    }

    // --- Rectangle / Square ---------------------------------------------------------

    #[test]
    fn rectangle_appearance_draws_re_and_strokes() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 110.0, y: 70.0 },
        };
        let m = base_markup(MarkupType::Rectangle, g);
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let c = content_str(&m);
        assert!(c.contains(" re\n"), "rectangle must draw with re: {c}");
        assert!(
            c.contains("S\n") || c.contains("B\n"),
            "must paint a stroke: {c}"
        );
    }

    #[test]
    fn rectangle_with_fill_uses_fill_and_stroke_paint_op() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 50.0 },
        };
        let mut m = base_markup(MarkupType::Rectangle, g);
        m.appearance.fill = Some("#ffcc00".into());
        let c = content_str(&m);
        assert!(
            c.contains("B\n"),
            "filled rect with a stroke must use B: {c}"
        );
    }

    // --- Ellipse / Circle -------------------------------------------------------------

    #[test]
    fn ellipse_appearance_draws_bezier_curves() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 40.0, y: 20.0 },
        };
        let m = base_markup(MarkupType::Ellipse, g);
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let c = content_str(&m);
        assert!(
            c.matches(" c\n").count() >= 4,
            "ellipse needs 4 bezier segments: {c}"
        );
    }

    // --- Line / Arrow -------------------------------------------------------------------

    #[test]
    fn line_appearance_draws_single_stroked_segment() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 50.0 },
        ]);
        let m = base_markup(MarkupType::Line, g);
        let c = content_str(&m);
        assert!(c.contains(" m\n") && c.contains(" l\n") && c.contains("S\n"));
        assert!(
            !c.contains("h\nf\n"),
            "plain Line must not draw an arrowhead"
        );
    }

    #[test]
    fn arrow_appearance_draws_shaft_plus_filled_arrowhead() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 0.0 },
        ]);
        let m = base_markup(MarkupType::Arrow, g);
        let c = content_str(&m);
        assert!(c.contains("S\n"), "shaft must be stroked: {c}");
        assert!(
            c.contains("h\nf\n"),
            "arrowhead must be a filled closed triangle: {c}"
        );
    }

    // --- Closed polygon family ----------------------------------------------------------

    #[test]
    fn polygon_appearance_closes_path_and_strokes() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 50.0 },
        ]);
        let m = base_markup(MarkupType::Polygon, g);
        let c = content_str(&m);
        assert!(c.contains("S\n") || c.contains("s\n") || c.contains("b\n") || c.contains("B\n"));
    }

    #[test]
    fn cloud_appearance_is_a_simple_closed_polygon() {
        // Named simplification: Cloud draws a plain closed polygon here, not the
        // scalloped revision-cloud arcs the SVG overlay renders.
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 40.0, y: 0.0 },
            PdfPoint { x: 40.0, y: 40.0 },
        ]);
        let m = base_markup(MarkupType::Cloud, g);
        let c = content_str(&m);
        assert!(
            !c.contains(" c\n"),
            "simplified cloud must not use bezier arcs"
        );
        assert!(c.contains(" m\n") && c.contains(" l\n"));
    }

    // --- Open polyline --------------------------------------------------------------------

    #[test]
    fn polyline_appearance_is_open_stroke_only() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 10.0, y: 10.0 },
            PdfPoint { x: 20.0, y: 0.0 },
        ]);
        let m = base_markup(MarkupType::Polyline, g);
        let c = content_str(&m);
        // The stroke is scoped in its own gs (opacity independence) - ends on the paint op
        // immediately followed by the scope's closing Q, not a bare trailing S.
        assert!(
            c.contains("S\nQ\n") && c.trim_end().ends_with('Q'),
            "open polyline must end on a plain stroke inside its gs scope: {c}"
        );
        assert!(!c.contains("f\n"), "open polyline must not fill: {c}");
    }

    // --- FreeText: Text / Callout -----------------------------------------------------------

    #[test]
    fn text_appearance_draws_box_and_glyphs() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = base_markup(MarkupType::Text, g);
        m.contents = Some("verify fire rating".into());
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 12.0,
        });
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        assert!(d.has(b"Resources"));
        let resources = d.get(b"Resources").unwrap().as_dict().unwrap();
        assert!(
            resources.has(b"Font"),
            "FreeText appearance must declare a /Font resource"
        );
        let c = content_str(&m);
        assert!(
            c.contains("BT\n") && c.contains("ET\n"),
            "must have a text object: {c}"
        );
        assert!(c.contains("Tj\n"), "must show text via Tj: {c}");
        assert!(c.contains(" re\n"), "must draw the text box border: {c}");
    }

    #[test]
    fn callout_appearance_draws_leader_arrow_and_text_box() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 50.0, y: 60.0 },
        ]);
        let mut m = base_markup(MarkupType::Callout, g);
        m.contents = Some("note".into());
        m.appearance.font = Some(FontSpec {
            family: "Times New Roman".into(),
            size_pt: 14.0,
        });
        let c = content_str(&m);
        assert!(c.contains("Tj\n"), "callout must render its text: {c}");
        assert!(
            c.contains("h\nf\n"),
            "callout leader must end in a filled arrowhead: {c}"
        );
    }

    #[test]
    fn text_without_contents_still_draws_a_valid_box_appearance() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 20.0 },
        };
        let m = base_markup(MarkupType::Text, g);
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let c = content_str(&m);
        assert!(!c.contains("Tj\n"), "no contents means no Tj call");
        assert!(c.contains(" re\n"), "the box border still draws");
    }

    // --- Highlight: quads + freeform rect ----------------------------------------------------

    #[test]
    fn highlight_quads_appearance_uses_multiply_blend_and_fills_every_quad() {
        let quads = vec![[
            PdfPoint { x: 72.0, y: 712.0 },
            PdfPoint { x: 500.0, y: 712.0 },
            PdfPoint { x: 72.0, y: 700.0 },
            PdfPoint { x: 500.0, y: 700.0 },
        ]];
        let m = base_markup(MarkupType::Highlight, MarkupGeometry::Quads(quads));
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let resources = d.get(b"Resources").unwrap().as_dict().unwrap();
        let gs = resources
            .get(b"ExtGState")
            .expect("Highlight must declare an ExtGState resource")
            .as_dict()
            .unwrap();
        let gs0 = gs.get(b"GS0").unwrap().as_dict().unwrap();
        assert_eq!(gs0.get(b"BM").unwrap().as_name().unwrap(), b"Multiply");
        let c = content_str(&m);
        assert!(c.contains("f\n"), "highlight quads must be filled: {c}");
        assert!(
            c.contains("/GS0 gs"),
            "highlight must apply the multiply ExtGState: {c}"
        );
    }

    #[test]
    fn highlight_freeform_rect_appearance_also_uses_multiply_blend() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 100.0, y: 20.0 },
        };
        let m = base_markup(MarkupType::Highlight, g);
        let c = content_str(&m);
        assert!(c.contains("/GS0 gs"));
        assert!(c.contains(" re\n") && c.contains("f\n"));
    }

    // --- Ink -------------------------------------------------------------------------------

    #[test]
    fn ink_appearance_draws_one_subpath_per_stroke() {
        let g = MarkupGeometry::Ink(vec![
            vec![PdfPoint { x: 1.0, y: 1.0 }, PdfPoint { x: 2.0, y: 3.0 }],
            vec![PdfPoint { x: 5.0, y: 5.0 }, PdfPoint { x: 6.0, y: 7.0 }],
        ]);
        let m = base_markup(MarkupType::Ink, g);
        let c = content_str(&m);
        assert_eq!(c.matches(" m\n").count(), 2, "one moveto per stroke: {c}");
        // The stroke is scoped in its own gs (opacity independence) - ends on the paint op
        // immediately followed by the scope's closing Q, not a bare trailing S.
        assert!(
            c.contains("S\nQ") && c.trim_end().ends_with('Q'),
            "ink paints one stroke over both subpaths inside its gs scope: {c}"
        );
    }

    // --- Stamp -----------------------------------------------------------------------------

    #[test]
    fn stamp_appearance_draws_bordered_box_and_names_the_simplification() {
        // Named limitation (per PR description): a bordered box + label text, not the
        // full stamp graphic (icons/logos are out of scope for this appearance pass).
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 80.0, y: 30.0 },
        };
        let mut m = base_markup(MarkupType::Stamp, g);
        m.contents = Some("APPROVED".into());
        let c = content_str(&m);
        assert!(c.contains(" re\n"), "stamp must draw a bordered box: {c}");
        assert!(c.contains("Tj\n"), "stamp must render its label: {c}");
    }

    #[test]
    fn stamp_without_a_stamp_asset_has_no_image_xobjects() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 80.0, y: 30.0 },
        };
        let m = base_markup(MarkupType::Stamp, g);
        assert!(build_ap_stream(&m).image_xobjects.is_empty());
    }

    // --- Stamp: PNG-backed real Image XObject (v0.3.1) ---------------------------------------

    /// Build a tiny in-memory PNG (via the `image` crate, already a project dependency)
    /// and base64-encode it with the project's own encoder (`render::base64_encode`) - so
    /// the fixture round-trips through the exact same code path a real Tool Chest stamp
    /// asset would, rather than a hand-crafted base64 literal.
    fn tiny_png_base64(w: u32, h: u32, with_alpha: bool) -> String {
        use image::{DynamicImage, ImageBuffer, Rgb, Rgba};
        let img = if with_alpha {
            DynamicImage::ImageRgba8(ImageBuffer::from_fn(w, h, |x, y| {
                let a: u8 = if x == 0 { 0 } else { 255 }; // left column transparent
                Rgba([(x * 50) as u8, (y * 50) as u8, 128, a])
            }))
        } else {
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |x, y| {
                Rgb([(x * 50) as u8, (y * 50) as u8, 128])
            }))
        };
        let mut bytes: Vec<u8> = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode fixture png");
        crate::render::base64_encode(&bytes)
    }

    fn png_stamp_markup(w: u32, h: u32, with_alpha: bool) -> Markup {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 90.0, y: 40.0 },
        };
        let mut m = base_markup(MarkupType::Stamp, g);
        m.stamp_asset = Some(StampAsset::PngBase64(tiny_png_base64(w, h, with_alpha)));
        m
    }

    #[test]
    fn png_stamp_asset_emits_one_image_xobject_with_matching_dimensions() {
        let m = png_stamp_markup(4, 3, false);
        let built = build_ap_stream(&m);
        assert_eq!(
            built.image_xobjects.len(),
            1,
            "exactly one aux image for a PNG stamp"
        );
        let img = &built.image_xobjects[0];
        assert_eq!(img.name, "Im0");
        assert_eq!(
            img.image.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Image"
        );
        assert_eq!(img.image.dict.get(b"Width").unwrap().as_i64().unwrap(), 4);
        assert_eq!(img.image.dict.get(b"Height").unwrap().as_i64().unwrap(), 3);
        assert_eq!(
            img.image
                .dict
                .get(b"ColorSpace")
                .unwrap()
                .as_name()
                .unwrap(),
            b"DeviceRGB"
        );
        assert_eq!(
            img.image
                .dict
                .get(b"BitsPerComponent")
                .unwrap()
                .as_i64()
                .unwrap(),
            8
        );
        assert!(
            img.smask.is_none(),
            "opaque RGB source must not get an SMask"
        );
    }

    #[test]
    fn png_stamp_content_stream_draws_a_do_operator_scaled_to_the_rect_no_border() {
        let m = png_stamp_markup(4, 3, false);
        let c = content_str(&m);
        assert!(c.contains("/Im0 Do\n"), "must paint the image xobject: {c}");
        // Scaled by the Rect's width/height (80 x 30) via the cm operator.
        assert!(
            c.contains("80 0 0 30 10 10 cm\n"),
            "cm matrix must map the unit square to the Rect: {c}"
        );
        assert!(
            !c.contains(" re\n"),
            "a real image stamp must not also draw the bordered-box fallback: {c}"
        );
    }

    #[test]
    fn png_stamp_with_alpha_channel_gets_an_smask_image() {
        let m = png_stamp_markup(2, 2, true);
        let built = build_ap_stream(&m);
        let img = &built.image_xobjects[0];
        let smask = img
            .smask
            .as_ref()
            .expect("RGBA source must produce an SMask");
        assert_eq!(
            smask.dict.get(b"ColorSpace").unwrap().as_name().unwrap(),
            b"DeviceGray"
        );
        assert_eq!(smask.dict.get(b"Width").unwrap().as_i64().unwrap(), 2);
        assert_eq!(smask.dict.get(b"Height").unwrap().as_i64().unwrap(), 2);
    }

    #[test]
    fn dynamic_png_stamp_draws_image_plus_overlay_text() {
        let mut m = png_stamp_markup(4, 3, false);
        m.markup_type = MarkupType::StampDynamic;
        m.contents = Some("2026-07-07".into());
        let c = content_str(&m);
        assert!(
            c.contains("/Im0 Do\n"),
            "dynamic stamp must still draw the image: {c}"
        );
        assert!(
            c.contains("Tj\n"),
            "dynamic stamp must draw the composed overlay text: {c}"
        );
        let do_pos = c.find("Do\n").unwrap();
        let bt_pos = c.find("BT\n").unwrap();
        assert!(
            do_pos < bt_pos,
            "overlay text must be drawn after (on top of) the image: {c}"
        );
    }

    #[test]
    fn stamp_with_svg_asset_falls_back_to_box_and_label_named_deferral() {
        // Named deferral (module doc comment): vector-SVG-to-PDF-operator conversion is
        // out of scope for this pass - an Svg asset keeps the pre-existing appearance.
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 80.0, y: 30.0 },
        };
        let mut m = base_markup(MarkupType::Stamp, g);
        m.stamp_asset = Some(StampAsset::Svg("<svg><rect/></svg>".into()));
        let built = build_ap_stream(&m);
        assert!(
            built.image_xobjects.is_empty(),
            "Svg asset must not produce an image xobject"
        );
        assert!(
            String::from_utf8(finish_ap_stream(built, Dictionary::new()).content)
                .unwrap()
                .contains(" re\n")
        );
    }

    #[test]
    fn stamp_with_malformed_png_base64_falls_back_gracefully_no_panic() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 80.0, y: 30.0 },
        };
        let mut m = base_markup(MarkupType::Stamp, g);
        m.stamp_asset = Some(StampAsset::PngBase64("not valid base64 png data!!".into()));
        let built = build_ap_stream(&m);
        assert!(
            built.image_xobjects.is_empty(),
            "malformed asset must not produce an image xobject"
        );
        let c = String::from_utf8(finish_ap_stream(built, Dictionary::new()).content).unwrap();
        assert!(
            c.contains(" re\n"),
            "must fall back to the bordered-box appearance, not an empty stream: {c}"
        );
    }

    #[test]
    fn base64_decode_round_trips_with_base64_encode() {
        let data = b"the quick brown fox jumps over the lazy dog 0123456789!";
        let encoded = crate::render::base64_encode(data);
        assert_eq!(base64_decode(&encoded).unwrap(), data);
    }

    #[test]
    fn base64_decode_rejects_invalid_characters() {
        assert!(base64_decode("not base64 at all!!").is_none());
    }

    // --- MeasurementCount --------------------------------------------------------------------

    #[test]
    fn count_marker_circle_draws_filled_stroked_circle() {
        let m = base_markup(
            MarkupType::MeasurementCount,
            MarkupGeometry::Point(PdfPoint { x: 42.0, y: 99.0 }),
        );
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let c = content_str(&m);
        assert!(c.contains(" c\n"), "circle marker uses bezier curves: {c}");
        assert!(c.contains("B\n"), "circle marker fills + strokes: {c}");
    }

    #[test]
    fn count_marker_cross_draws_two_open_strokes_not_filled() {
        let mut m = base_markup(
            MarkupType::MeasurementCount,
            MarkupGeometry::Point(PdfPoint { x: 0.0, y: 0.0 }),
        );
        m.count_set = Some(CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-A".into(),
            color: "#3366ff".into(),
            symbol: CountSymbol::Cross,
        });
        let c = content_str(&m);
        assert_eq!(
            c.matches("S\n").count(),
            2,
            "cross draws two separate strokes: {c}"
        );
        assert!(!c.contains("f\n"), "cross is never filled: {c}");
    }

    #[test]
    fn count_marker_diamond_draws_filled_polygon() {
        let mut m = base_markup(
            MarkupType::MeasurementCount,
            MarkupGeometry::Point(PdfPoint { x: 0.0, y: 0.0 }),
        );
        m.count_set = Some(CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-B".into(),
            color: "#ff0000".into(),
            symbol: CountSymbol::Diamond,
        });
        let c = content_str(&m);
        assert!(
            c.contains("b\n"),
            "polygon symbols close+fill+stroke via b: {c}"
        );
    }

    // --- BBox padding for degenerate (Point) geometry ------------------------------------

    #[test]
    fn point_geometry_bbox_is_padded_not_zero_size() {
        let m = base_markup(
            MarkupType::MeasurementCount,
            MarkupGeometry::Point(PdfPoint { x: 5.0, y: 5.0 }),
        );
        let bbox = ap_bbox(&m);
        assert!(
            bbox[2] - bbox[0] > 0.0 && bbox[3] - bbox[1] > 0.0,
            "bbox must be non-degenerate: {bbox:?}"
        );
        // Independent of the annotation's own /Rect (which IS zero-size for Point geometry -
        // that key is untouched by this module; see annotation::bbox).
    }

    #[test]
    fn shape_bbox_is_padded_beyond_the_tight_geometry_bounds() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 50.0, y: 30.0 },
        };
        let mut m = base_markup(MarkupType::Rectangle, g);
        m.appearance.line_weight = 4.0;
        let bbox = ap_bbox(&m);
        assert!(
            bbox[0] < 10.0 && bbox[1] < 10.0 && bbox[2] > 50.0 && bbox[3] > 30.0,
            "bbox must pad past the tight bounds: {bbox:?}"
        );
    }

    // --- Unrecognised (type, geometry) pair: never returns an invalid empty stream --------

    #[test]
    fn build_ap_stream_always_returns_a_valid_form_dict_even_for_unmatched_geometry() {
        // A geometry variant that doesn't match any drawing arm for the given type (e.g. an
        // Ink drawn with no strokes) must still be a structurally valid, non-degenerate Form.
        let m = base_markup(MarkupType::Ink, MarkupGeometry::Ink(vec![]));
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
    }

    // --- Opacity independence: stroke opacity, fill opacity, text alpha never couple ------

    fn gs0(m: &Markup) -> Dictionary {
        let d = stream_dict_checks(m);
        let resources = d.get(b"Resources").unwrap().as_dict().unwrap().clone();
        let ext_gstate = resources.get(b"ExtGState").unwrap().as_dict().unwrap();
        ext_gstate.get(b"GS0").unwrap().as_dict().unwrap().clone()
    }

    #[test]
    fn rectangle_stroke_and_fill_opacity_are_independent_in_the_gstate() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 50.0 },
        };
        let mut m = base_markup(MarkupType::Rectangle, g);
        m.appearance.fill = Some("#00ff00".into());
        m.appearance.opacity = 0.2; // faint stroke
        m.appearance.fill_opacity = Some(0.9); // near-opaque fill
        let gs = gs0(&m);
        let ca = gs.get(b"CA").unwrap().as_float().unwrap();
        let ca_fill = gs.get(b"ca").unwrap().as_float().unwrap();
        assert!(
            (ca - 0.2).abs() < 1e-4,
            "/CA (stroke) must equal opacity, got {ca}"
        );
        assert!(
            (ca_fill - 0.9).abs() < 1e-4,
            "/ca (fill) must equal fill_opacity, NOT be coupled to the faint stroke opacity, got {ca_fill}"
        );
    }

    #[test]
    fn changing_fill_opacity_alone_does_not_move_stroke_opacity() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 50.0 },
        };
        let mut a = base_markup(MarkupType::Ellipse, g.clone());
        a.appearance.fill = Some("#0000ff".into());
        a.appearance.opacity = 0.7;
        a.appearance.fill_opacity = Some(0.1);
        let mut b = a.clone();
        b.appearance.fill_opacity = Some(1.0); // only fill_opacity changes
        let ca_a = gs0(&a).get(b"CA").unwrap().as_float().unwrap();
        let ca_b = gs0(&b).get(b"CA").unwrap().as_float().unwrap();
        assert!(
            (ca_a - ca_b).abs() < 1e-6,
            "stroke /CA must stay put when only fill_opacity changes: {ca_a} vs {ca_b}"
        );
    }

    #[test]
    fn changing_stroke_opacity_alone_does_not_move_fill_opacity() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 50.0 },
        };
        let mut a = base_markup(MarkupType::Rectangle, g);
        a.appearance.fill = Some("#ff00ff".into());
        a.appearance.opacity = 0.9;
        a.appearance.fill_opacity = Some(0.5);
        let mut b = a.clone();
        b.appearance.opacity = 0.1; // only stroke opacity changes
        let ca_fill_a = gs0(&a).get(b"ca").unwrap().as_float().unwrap();
        let ca_fill_b = gs0(&b).get(b"ca").unwrap().as_float().unwrap();
        assert!(
            (ca_fill_a - ca_fill_b).abs() < 1e-6,
            "fill /ca must stay put when only stroke opacity changes: {ca_fill_a} vs {ca_fill_b}"
        );
    }

    #[test]
    fn unset_fill_opacity_defaults_to_fully_opaque_regardless_of_stroke_opacity() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 50.0, y: 50.0 },
        };
        let mut m = base_markup(MarkupType::Rectangle, g);
        m.appearance.fill = Some("#123456".into());
        m.appearance.opacity = 0.05; // near-invisible stroke
        m.appearance.fill_opacity = None; // unset -> must default to opaque fill
        let ca_fill = gs0(&m).get(b"ca").unwrap().as_float().unwrap();
        assert!(
            (ca_fill - 1.0).abs() < 1e-4,
            "unset fill_opacity must default to 1.0 even when stroke opacity is tiny, got {ca_fill}"
        );
    }

    #[test]
    fn text_box_glyphs_are_drawn_outside_the_alpha_gs_scope() {
        // Text carries its own alpha, independent of both stroke and fill opacity: the BT/ET
        // glyph block must not be nested inside the box's "q /GS0 gs ... Q" scope.
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 100.0, y: 20.0 },
        };
        let mut m = base_markup(MarkupType::Text, g);
        m.contents = Some("dim stroke, opaque text".into());
        m.appearance.fill = Some("#eeeeee".into());
        m.appearance.opacity = 0.1;
        m.appearance.fill_opacity = Some(0.1);
        let c = content_str(&m);
        // The box's gs scope closes (Q) strictly before the glyph block opens (BT).
        let q_pos = c.find("Q\n").expect("box gs scope must close");
        let bt_pos = c.find("BT\n").expect("glyph block must be present");
        assert!(
            q_pos < bt_pos,
            "text glyphs must be drawn AFTER the box's gs scope closes (outside it): {c}"
        );
        // No gs operator appears between Q and BT - text picks up the page's default alpha
        // (1.0), never a value from the box's stroke/fill gstate.
        assert!(
            !c[q_pos..bt_pos].contains(" gs"),
            "no gs operator may apply between the box close and the glyph block: {c}"
        );
    }

    #[test]
    fn callout_box_glyphs_are_also_drawn_outside_the_alpha_gs_scope() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 40.0, y: 40.0 },
        ]);
        let mut m = base_markup(MarkupType::Callout, g);
        m.contents = Some("callout text".into());
        m.appearance.fill = Some("#ffdddd".into());
        m.appearance.opacity = 0.2;
        m.appearance.fill_opacity = Some(0.3);
        let c = content_str(&m);
        let last_q = c.rfind("Q\n").expect("box gs scope must close");
        let bt_pos = c.find("BT\n").expect("glyph block must be present");
        assert!(
            last_q < bt_pos,
            "callout text glyphs must be drawn after the box's gs scope closes: {c}"
        );
    }
}
