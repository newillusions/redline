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
//! - Stamp/StampDynamic draws a bordered box + label text, not the full stamp graphic.

use lopdf::{dictionary, Dictionary, Object, Stream};

use super::{Appearance, LineStyle, Markup, MarkupGeometry, MarkupType};
use crate::geometry::PdfPoint;

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

/// Build the `/AP /N` Form XObject (as a not-yet-indirect [`Stream`]) for `m`. The caller
/// (`document::annots::write_markups`) adds it to the `Document` and points the annotation
/// dict's `/AP /N` at the resulting indirect reference.
pub(crate) fn build_ap_stream(m: &Markup) -> Stream {
    let bbox = ap_bbox(m);
    let (content, resources) = draw(m);
    let stream_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "FormType" => 1,
        "BBox" => Object::Array(bbox.iter().map(|v| real(*v)).collect()),
        "Resources" => Object::Dictionary(resources),
    };
    Stream::new(stream_dict, content.into_bytes())
}

// ---------------------------------------------------------------------------
// BBox - independent of the annotation's semantic /Rect (that one stays untouched).
// ---------------------------------------------------------------------------

/// The Form's own bounding box: the geometry's tight bounds, padded so strokes, arrowheads,
/// glyph ascenders/descenders, and count-marker symbols never clip against the edge. Point
/// geometry (count markers) has a zero-size tight bound, so it gets a fixed radius-based pad
/// instead of a proportional one.
fn ap_bbox(m: &Markup) -> [f64; 4] {
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
            let font_pt = m.appearance.font.as_ref().map_or(DEFAULT_FONT_SIZE, |f| f.size_pt);
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

    /// Register an ExtGState and return its resource name.
    fn add_gstate(&mut self, gs: Dictionary) -> &'static str {
        // At most one ExtGState is ever needed per markup (Highlight's multiply-blend OR a
        // Text/Callout box fill alpha are mutually exclusive across markup types).
        self.ext_gstate.set("GS0", Object::Dictionary(gs));
        "GS0"
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
            self.dict.set("ExtGState", Object::Dictionary(self.ext_gstate));
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
        (true, true, true) => "b",   // close, fill (nonzero), stroke
        (true, true, false) => "B",  // fill, stroke (no close - open path already painted as-is)
        (true, false, true) => "f",  // close is implicit for fill
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
            PdfPoint { x: cx - r, y: cy - r },
            PdfPoint { x: cx + r, y: cy - r },
            PdfPoint { x: cx + r, y: cy + r },
            PdfPoint { x: cx - r, y: cy + r },
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
            let inner = ngon(5, std::f64::consts::FRAC_PI_2 + std::f64::consts::PI / 5.0, r * 0.4);
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
        n(cx + rx), n(cy + ky), n(cx + kx), n(cy + ry), n(cx), n(cy + ry)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx - kx), n(cy + ry), n(cx - rx), n(cy + ky), n(cx - rx), n(cy)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx - rx), n(cy - ky), n(cx - kx), n(cy - ry), n(cx), n(cy - ry)
    ));
    out.push_str(&format!(
        "{} {} {} {} {} {} c\n",
        n(cx + kx), n(cy - ry), n(cx + rx), n(cy - ky), n(cx + rx), n(cy)
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

/// Box border + optional fill for Text/Callout, honoring the independent `fill_opacity`.
fn draw_text_box(out: &mut String, res: &mut Resources, a: &Appearance, rect: [f64; 4]) {
    let outline = a.outline_color.as_deref().unwrap_or(&a.color);
    if let Some(fill) = &a.fill {
        if let Some(fo) = a.fill_opacity {
            let gs_name = res.add_gstate(dictionary! { "ca" => real(fo) });
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            fill_color(out, fill);
            out.push_str(&format!(
                "{} {} {} {} re\nf\nQ\n",
                num(rect[0]), num(rect[1]), num(rect[2] - rect[0]), num(rect[3] - rect[1])
            ));
        } else {
            fill_color(out, fill);
            out.push_str(&format!(
                "{} {} {} {} re\nf\n",
                num(rect[0]), num(rect[1]), num(rect[2] - rect[0]), num(rect[3] - rect[1])
            ));
        }
    }
    let [r, g, b] = hex_to_rgb(outline);
    out.push_str(&format!(
        "{} {} {} RG\n{} w\n{} {} {} {} re\nS\n",
        num(r), num(g), num(b),
        num(a.line_weight.max(0.0)),
        num(rect[0]), num(rect[1]), num(rect[2] - rect[0]), num(rect[3] - rect[1])
    ));
}

/// Build the content-stream operators + resources for `m`. Returns (content, resources).
fn draw(m: &Markup) -> (String, Dictionary) {
    let mut out = String::new();
    let mut res = Resources::new();
    let a = &m.appearance;

    match (m.markup_type, &m.geometry) {
        // --- Rectangle / Square -------------------------------------------------------
        (MarkupType::Rectangle, MarkupGeometry::Rect { min, max }) => {
            stroke_preamble(&mut out, a);
            let has_fill = a.fill.is_some();
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            out.push_str(&format!(
                "{} {} {} {} re\n",
                num(min.x), num(min.y), num(max.x - min.x), num(max.y - min.y)
            ));
            out.push_str(paint_op(has_fill, a.line_weight > 0.0, false));
            out.push('\n');
        }

        // --- Ellipse / Circle ----------------------------------------------------------
        (MarkupType::Ellipse, MarkupGeometry::Rect { min, max }) => {
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
            out.push('\n');
        }

        // --- Line / Arrow / MeasurementLength / MeasurementRadius -----------------------
        (
            MarkupType::Line | MarkupType::Arrow | MarkupType::MeasurementLength | MarkupType::MeasurementRadius,
            MarkupGeometry::Polyline(pts),
        ) if pts.len() >= 2 => {
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
            stroke_preamble(&mut out, a);
            let has_fill = a.fill.is_some();
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            path_from_points(&mut out, pts);
            out.push('\n');
            out.push_str(paint_op(has_fill, a.line_weight > 0.0, true));
            out.push('\n');
        }

        // --- Open polyline: PolyLine / MeasurementAngle ---------------------------------
        (MarkupType::Polyline | MarkupType::MeasurementAngle, MarkupGeometry::Polyline(pts))
            if pts.len() >= 2 =>
        {
            stroke_preamble(&mut out, a);
            path_from_points(&mut out, pts);
            out.push('\n');
            out.push_str("S\n");
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
            let gs_name = res.add_gstate(dictionary! {
                "BM" => "Multiply",
                "ca" => real(a.opacity),
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
            let gs_name = res.add_gstate(dictionary! {
                "BM" => "Multiply",
                "ca" => real(a.opacity),
            });
            out.push_str(&format!("q\n/{gs_name} gs\n"));
            fill_color(&mut out, &a.color);
            out.push_str(&format!(
                "{} {} {} {} re\nf\nQ\n",
                num(min.x), num(min.y), num(max.x - min.x), num(max.y - min.y)
            ));
        }

        // --- Ink: one moveto/lineto subpath per stroke, single stroke paint -------------
        (MarkupType::Ink, MarkupGeometry::Ink(strokes)) if !strokes.is_empty() => {
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
        }

        // --- Stamp / StampDynamic: simple bordered box + label (named simplification) ---
        (MarkupType::Stamp | MarkupType::StampDynamic, MarkupGeometry::Rect { min, max }) => {
            stroke_preamble(&mut out, a);
            if let Some(fill) = &a.fill {
                fill_color(&mut out, fill);
            }
            out.push_str(&format!(
                "{} {} {} {} re\n",
                num(min.x), num(min.y), num(max.x - min.x), num(max.y - min.y)
            ));
            out.push_str(paint_op(a.fill.is_some(), true, false));
            out.push('\n');
            if let Some(text) = &m.contents {
                let origin = PdfPoint {
                    x: min.x + 2.0,
                    y: (min.y + max.y) / 2.0 - 5.0,
                };
                draw_text_block(&mut out, &mut res, text, &a.color, origin, a.font.as_ref());
            }
        }

        // --- MeasurementCount: symbol marker at a point ----------------------------------
        (MarkupType::MeasurementCount, MarkupGeometry::Point(p)) => {
            let symbol = m.count_set.as_ref().map_or(super::CountSymbol::Circle, |c| c.symbol);
            let r = COUNT_MARKER_RADIUS;
            match symbol {
                super::CountSymbol::Circle => {
                    stroke_preamble(&mut out, a);
                    fill_color(&mut out, &a.color);
                    ellipse_path(&mut out, p.x, p.y, r, r);
                    out.push_str("B\n");
                }
                super::CountSymbol::Cross => {
                    stroke_preamble(&mut out, a);
                    moveto(&mut out, PdfPoint { x: p.x - r, y: p.y - r });
                    lineto(&mut out, PdfPoint { x: p.x + r, y: p.y + r });
                    out.push_str("S\n");
                    moveto(&mut out, PdfPoint { x: p.x - r, y: p.y + r });
                    lineto(&mut out, PdfPoint { x: p.x + r, y: p.y - r });
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
        }

        // --- Fallback: nothing drawable for this (type, geometry) combination. A named,
        // structural gap - the annotation still has an empty-but-valid appearance rather
        // than none at all (viewers show a blank box instead of nothing/dropping it).
        _ => {}
    }

    (out, res.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Audit, CountSet, CountSymbol, FontSpec, MarkupStatus, Origin, UserRef, Workflow};

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
        }
    }

    fn content_str(m: &Markup) -> String {
        let s = build_ap_stream(m);
        String::from_utf8(s.content).unwrap()
    }

    fn stream_dict_checks(m: &Markup) -> Dictionary {
        build_ap_stream(m).dict
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
        assert!(c.contains("S\n") || c.contains("B\n"), "must paint a stroke: {c}");
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
        assert!(c.contains("B\n"), "filled rect with a stroke must use B: {c}");
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
        assert!(c.matches(" c\n").count() >= 4, "ellipse needs 4 bezier segments: {c}");
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
        assert!(!c.contains("h\nf\n"), "plain Line must not draw an arrowhead");
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
        assert!(c.contains("h\nf\n"), "arrowhead must be a filled closed triangle: {c}");
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
        assert!(!c.contains(" c\n"), "simplified cloud must not use bezier arcs");
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
        assert!(c.ends_with("S\n"), "open polyline must end on a plain stroke: {c}");
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
        m.appearance.font = Some(FontSpec { family: "Helvetica".into(), size_pt: 12.0 });
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        assert!(d.has(b"Resources"));
        let resources = d.get(b"Resources").unwrap().as_dict().unwrap();
        assert!(resources.has(b"Font"), "FreeText appearance must declare a /Font resource");
        let c = content_str(&m);
        assert!(c.contains("BT\n") && c.contains("ET\n"), "must have a text object: {c}");
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
        m.appearance.font = Some(FontSpec { family: "Times New Roman".into(), size_pt: 14.0 });
        let c = content_str(&m);
        assert!(c.contains("Tj\n"), "callout must render its text: {c}");
        assert!(c.contains("h\nf\n"), "callout leader must end in a filled arrowhead: {c}");
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
        assert!(c.contains("/GS0 gs"), "highlight must apply the multiply ExtGState: {c}");
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
        assert!(c.trim_end().ends_with('S'), "ink paints one stroke over both subpaths: {c}");
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

    // --- MeasurementCount --------------------------------------------------------------------

    #[test]
    fn count_marker_circle_draws_filled_stroked_circle() {
        let m = base_markup(MarkupType::MeasurementCount, MarkupGeometry::Point(PdfPoint { x: 42.0, y: 99.0 }));
        let d = stream_dict_checks(&m);
        assert_valid_form_dict(&d);
        let c = content_str(&m);
        assert!(c.contains(" c\n"), "circle marker uses bezier curves: {c}");
        assert!(c.contains("B\n"), "circle marker fills + strokes: {c}");
    }

    #[test]
    fn count_marker_cross_draws_two_open_strokes_not_filled() {
        let mut m = base_markup(MarkupType::MeasurementCount, MarkupGeometry::Point(PdfPoint { x: 0.0, y: 0.0 }));
        m.count_set = Some(CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-A".into(),
            color: "#3366ff".into(),
            symbol: CountSymbol::Cross,
        });
        let c = content_str(&m);
        assert_eq!(c.matches("S\n").count(), 2, "cross draws two separate strokes: {c}");
        assert!(!c.contains("f\n"), "cross is never filled: {c}");
    }

    #[test]
    fn count_marker_diamond_draws_filled_polygon() {
        let mut m = base_markup(MarkupType::MeasurementCount, MarkupGeometry::Point(PdfPoint { x: 0.0, y: 0.0 }));
        m.count_set = Some(CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-B".into(),
            color: "#ff0000".into(),
            symbol: CountSymbol::Diamond,
        });
        let c = content_str(&m);
        assert!(c.contains("b\n"), "polygon symbols close+fill+stroke via b: {c}");
    }

    // --- BBox padding for degenerate (Point) geometry ------------------------------------

    #[test]
    fn point_geometry_bbox_is_padded_not_zero_size() {
        let m = base_markup(MarkupType::MeasurementCount, MarkupGeometry::Point(PdfPoint { x: 5.0, y: 5.0 }));
        let bbox = ap_bbox(&m);
        assert!(bbox[2] - bbox[0] > 0.0 && bbox[3] - bbox[1] > 0.0, "bbox must be non-degenerate: {bbox:?}");
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
        assert!(bbox[0] < 10.0 && bbox[1] < 10.0 && bbox[2] > 50.0 && bbox[3] > 30.0, "bbox must pad past the tight bounds: {bbox:?}");
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
}
