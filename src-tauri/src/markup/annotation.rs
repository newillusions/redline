//! Map the markup envelope ↔ standard PDF annotation dictionaries (spec §6).
//!
//! Markups serialise to standard PDF annotations so they open correctly in
//! Bluebeam/Acrobat. Each markup becomes one annotation dictionary:
//!   - **Standard keys** (interop): `/Subtype /NM /Rect /Contents /Subj /T
//!     /CreationDate /M /C /CA /IC`, plus per-shape geometry (`/L /Vertices /InkList`).
//!   - **`/RL*` private keys** (app round-trip): the exact redline `MarkupType`, the
//!     stable `user_id`s, review status, revision, origin, layer, line style, and a
//!     geometry-variant tag. PDF readers ignore unknown keys, so foreign tools still
//!     render the annotation while redline reloads it losslessly.
//!
//! Scope of this slice: the §6 envelope + geometry + the universal appearance bits
//! (colour / opacity / weight / fill / line-style) + font (for FreeText annotations:
//! written to `/DA` for interop and `/RLFontFamily`+`/RLFontSize` for lossless round-trip),
//! plus the measurement payload (`/RLMeasure`, opaque JSON) and the reserved workflow
//! assignee/thread (`/RLWorkflowExtra`, opaque JSON). Both are private keys, ignored by
//! foreign viewers, round-tripped losslessly for redline's own reopen. PDF reals are f32
//! (lopdf), so geometry in the annotation is the interop copy - the canonical f64
//! geometry stays in the app model / sidecar (spec §5/§6).

use chrono::{DateTime, NaiveDateTime, Utc};
use lopdf::{Dictionary, Object};

use super::{
    Appearance, Audit, CountSet, CountSymbol, FontSpec, LineStyle, Markup, MarkupGeometry,
    MarkupStatus, MarkupType, Measurement, OptionalContent, Origin, Reply, UserRef, Workflow,
};
use crate::geometry::{PdfPoint, Quad};

// --- small helpers -------------------------------------------------------------

fn name(v: &str) -> Object {
    Object::Name(v.as_bytes().to_vec())
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

/// Highlighter wash factor: the fraction of `appearance.opacity` a Highlight is actually
/// painted at. Mirrors `HIGHLIGHT_FILL_ALPHA` in `src/lib/markup-render.ts` (redline's own
/// viewer renders a highlight at `opacity * 0.35`), so the exported `/CA` matches what the
/// user sees on screen instead of a fully opaque fill (G9 defect 3).
const HIGHLIGHT_WASH_ALPHA: f64 = 0.35;

fn get_string(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key)
        .ok()?
        .as_str()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

fn get_name(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key)
        .ok()?
        .as_name()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

fn get_reals(d: &Dictionary, key: &[u8]) -> Option<Vec<f64>> {
    let arr = d.get(key).ok()?.as_array().ok()?;
    arr.iter()
        .map(|o| o.as_float().ok().map(|f| f as f64))
        .collect()
}

/// Whether a foreign `/Subtype /Polygon` annotation is actually a Bluebeam-style
/// revision cloud rather than a plain polygon. Real Bluebeam clouds carry `/BE << /S /C
/// ... >>` (Cloudy border style) and/or `/IT /PolygonCloud`; a plain Polygon carries
/// neither. Read-side half of the BB-interop fix wave 2026-08-11
/// (obs:je08u4y8rukjzbpm2y5f): without this, every foreign cloud-style Polygon (no
/// `/RLType` of its own) imported as a flat `MarkupType::Polygon`, and the existing
/// write path only emits `/BE`+`/IT` for `MarkupType::Cloud` - so the cloud markers
/// never survived a round-trip, matching the real corpus finding (4/7 golden Polygon
/// items carry `/BE` that the pre-fix round-trip dropped).
fn is_cloud_polygon(d: &Dictionary) -> bool {
    if get_name(d, b"IT").as_deref() == Some("PolygonCloud") {
        return true;
    }
    d.get(b"BE")
        .ok()
        .and_then(|o| o.as_dict().ok())
        .map(|be| get_name(be, b"S").as_deref() == Some("C"))
        .unwrap_or(false)
}

/// Recover a font from a foreign `/DA` default-appearance string (ISO 32000-1
/// §12.7.3.3, e.g. `"1 0.5019608 0.2509804 rg /Calibri 10 Tf"` - the exact shape real
/// Bluebeam FreeText/Callout annotations carry in the BB corpus). Read-side half of the
/// BB-interop fix wave 2026-08-11 (obs:je08u4y8rukjzbpm2y5f): only used when redline's
/// own lossless `/RLFontFamily`+`/RLFontSize` keys are absent (a foreign annotation, or
/// one Bluebeam itself re-wrote). Without this, a golden FreeText item's font vanished
/// on `from_annotation_dict` (no `/RL*` keys, so `appearance.font` read as `None`),
/// which then suppressed the ALREADY-correct `/DA` write in `to_annotation_dict` (its
/// emission is gated on `Some(font)`) - the actual mechanism behind the harness's
/// "FreeText drops /DA on 22/22 golden items" finding; the write side needed nothing
/// changed, only this read-side recovery. Bluebeam's `/DA` carries the real family name
/// directly (e.g. "Calibri"), richer than redline's own base-14-alias write path, so
/// this recovers genuine fidelity rather than merely satisfying the round-trip.
fn font_from_da(da: &str) -> Option<FontSpec> {
    let tf_idx = da.find(" Tf")?;
    let mut tokens = da[..tf_idx].rsplit(' ');
    let size_pt: f64 = tokens.next()?.parse().ok()?;
    let family = tokens.next()?.strip_prefix('/')?.to_string();
    if family.is_empty() {
        return None;
    }
    Some(FontSpec { family, size_pt })
}

/// Best-effort glyph colour recovery from a foreign `/DA` string's `rg` operator (e.g.
/// `"/Helv 12 Tf 0.5 0 1 rg"`, or the colour-only shape `"0.5 0 1 rg"` noted in
/// `raw_da`'s doc comment - a real corpus shape, not hypothetical). Used only for a
/// Text/Callout annotation with no `/RLType` (foreign, not redline-authored) and
/// therefore no `/RLTextColor`, where `/C` itself is the annotation's background (ISO
/// 32000-1 §12.5.6.6) and must NOT be read as the glyph colour. `g`/`G`/`k`/`K`
/// operators are not handled - RGB `rg` covers the real-world DA shapes seen so far;
/// falls through to the caller's own default otherwise.
fn color_from_da(da: &str) -> Option<String> {
    let rg_idx = da.find(" rg")?;
    let mut tokens = da[..rg_idx].rsplit(' ');
    let b: f64 = tokens.next()?.parse().ok()?;
    let g: f64 = tokens.next()?.parse().ok()?;
    let r: f64 = tokens.next()?.parse().ok()?;
    Some(rgb_to_hex(&[r, g, b]))
}

// --- enum <-> tag --------------------------------------------------------------

/// Exact `MarkupType` round-trips via `/RLType`, serialised through serde (the enum is
/// a unit enum, so this is just its variant name). Multiple types share one PDF
/// `/Subtype`, so the standard subtype alone cannot recover the exact type.
fn type_tag(t: MarkupType) -> String {
    serde_json::to_string(&t)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

fn type_from_tag(tag: &str) -> Option<MarkupType> {
    serde_json::from_str(&format!("\"{tag}\"")).ok()
}

/// PDF standard `/Subtype` for interop rendering.
fn pdf_subtype(t: MarkupType) -> &'static str {
    match t {
        // A count marker renders via its /AP; "Stamp" is the interop subtype a foreign viewer
        // draws from the appearance stream (a zero-size FreeText was dropped by Bluebeam - G9
        // defect 5). redline recovers the MeasurementCount type from /RLType on read.
        MarkupType::MeasurementCount => "Stamp",
        MarkupType::Text | MarkupType::Callout => "FreeText",
        MarkupType::Cloud
        | MarkupType::Polygon
        | MarkupType::MeasurementPerimeter
        | MarkupType::MeasurementArea
        | MarkupType::MeasurementVolume => "Polygon",
        MarkupType::Rectangle => "Square",
        MarkupType::Ellipse => "Circle",
        MarkupType::Line
        | MarkupType::Arrow
        | MarkupType::MeasurementLength
        | MarkupType::MeasurementRadius => "Line",
        MarkupType::Polyline | MarkupType::MeasurementAngle => "PolyLine",
        MarkupType::Highlight => "Highlight",
        MarkupType::Ink => "Ink",
        MarkupType::Stamp | MarkupType::StampDynamic => "Stamp",
    }
}

fn status_tag(s: MarkupStatus) -> &'static str {
    match s {
        MarkupStatus::None => "None",
        MarkupStatus::Accepted => "Accepted",
        MarkupStatus::Rejected => "Rejected",
        MarkupStatus::Completed => "Completed",
    }
}

fn status_from_tag(s: &str) -> MarkupStatus {
    match s {
        "Accepted" => MarkupStatus::Accepted,
        "Rejected" => MarkupStatus::Rejected,
        "Completed" => MarkupStatus::Completed,
        _ => MarkupStatus::None,
    }
}

fn origin_tag(o: Origin) -> &'static str {
    match o {
        Origin::Desktop => "Desktop",
        Origin::FieldApp => "FieldApp",
    }
}

fn line_style_tag(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "Solid",
        LineStyle::Dashed => "Dashed",
        LineStyle::Dotted => "Dotted",
    }
}

fn count_symbol_tag(s: CountSymbol) -> &'static str {
    match s {
        CountSymbol::Circle => "Circle",
        CountSymbol::Square => "Square",
        CountSymbol::Triangle => "Triangle",
        CountSymbol::Diamond => "Diamond",
        CountSymbol::Cross => "Cross",
        CountSymbol::Star => "Star",
        CountSymbol::Hexagon => "Hexagon",
    }
}

fn count_symbol_from_tag(s: &str) -> CountSymbol {
    match s {
        "Square" => CountSymbol::Square,
        "Triangle" => CountSymbol::Triangle,
        "Diamond" => CountSymbol::Diamond,
        "Cross" => CountSymbol::Cross,
        "Star" => CountSymbol::Star,
        "Hexagon" => CountSymbol::Hexagon,
        _ => CountSymbol::Circle,
    }
}

fn line_style_from_tag(s: &str) -> LineStyle {
    match s {
        "Dashed" => LineStyle::Dashed,
        "Dotted" => LineStyle::Dotted,
        _ => LineStyle::Solid,
    }
}

// --- colour <-> PDF /C ---------------------------------------------------------

fn hex_to_rgb(hex: &str) -> Option<[f64; 3]> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let c = |a: usize| {
        u8::from_str_radix(&h[a..a + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some([c(0)?, c(2)?, c(4)?])
}

fn rgb_to_hex(c: &[f64]) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c {
        [r, g, b3] => format!("#{:02x}{:02x}{:02x}", b(*r), b(*g), b(*b3)),
        _ => "#000000".to_string(),
    }
}

// --- dates (PDF "D:YYYYMMDDHHmmSSZ", second resolution) ------------------------

fn to_pdf_date(dt: &DateTime<Utc>) -> String {
    dt.format("D:%Y%m%d%H%M%SZ").to_string()
}

fn from_pdf_date(s: &str) -> Option<DateTime<Utc>> {
    let core = s.trim_start_matches("D:").get(0..14)?;
    NaiveDateTime::parse_from_str(core, "%Y%m%d%H%M%S")
        .ok()
        .map(|n| n.and_utc())
}

// --- geometry <-> dict ---------------------------------------------------------

fn geom_tag(g: &MarkupGeometry) -> &'static str {
    match g {
        MarkupGeometry::Point(_) => "point",
        MarkupGeometry::Rect { .. } => "rect",
        MarkupGeometry::Polyline(_) => "poly",
        MarkupGeometry::Ink(_) => "ink",
        MarkupGeometry::Quads(_) => "quads",
    }
}

fn flatten(pts: &[PdfPoint]) -> Object {
    Object::Array(pts.iter().flat_map(|p| [real(p.x), real(p.y)]).collect())
}

fn flatten_quads(quads: &[Quad]) -> Object {
    Object::Array(
        quads
            .iter()
            .flat_map(|q| q.iter().flat_map(|p| [real(p.x), real(p.y)]))
            .collect(),
    )
}

fn points_from_reals(r: &[f64]) -> Vec<PdfPoint> {
    r.chunks_exact(2)
        .map(|c| PdfPoint { x: c[0], y: c[1] })
        .collect()
}

/// Reconstruct `Quad`s from a flat `/QuadPoints` real array (8 values per quad,
/// x1 y1 x2 y2 x3 y3 x4 y4 - the TL/TR/BL/BR order documented on [`Quad`]).
/// A trailing partial group (malformed annotation) is dropped via `chunks_exact`.
fn quads_from_reals(r: &[f64]) -> Vec<Quad> {
    r.chunks_exact(8)
        .map(|c| {
            [
                PdfPoint { x: c[0], y: c[1] },
                PdfPoint { x: c[2], y: c[3] },
                PdfPoint { x: c[4], y: c[5] },
                PdfPoint { x: c[6], y: c[7] },
            ]
        })
        .collect()
}

/// Reconstruct geometry, preferring the exact `/RL*` shape keys (lossless for redline
/// annotations) and falling back to standard keys for foreign annotations.
fn geometry_from_dict(d: &Dictionary) -> MarkupGeometry {
    let tag = get_name(d, b"RLGeom");
    match tag.as_deref() {
        Some("point") => {
            // A count marker stores its point as the CENTRE of a symbol-sized /Rect (so a
            // foreign viewer has a non-zero rect to render). Recover the centre; a legacy
            // zero-size /Rect ([x y x y]) yields the same (x, y).
            let r = get_reals(d, b"Rect").unwrap_or_default();
            let x0 = r.first().copied().unwrap_or(0.0);
            let y0 = r.get(1).copied().unwrap_or(0.0);
            let x1 = r.get(2).copied().unwrap_or(x0);
            let y1 = r.get(3).copied().unwrap_or(y0);
            MarkupGeometry::Point(PdfPoint {
                x: (x0 + x1) / 2.0,
                y: (y0 + y1) / 2.0,
            })
        }
        Some("rect") => {
            // /Rect is no longer a reliable source for Rect-type geometry (Rectangle,
            // Ellipse, Stamp, StampDynamic): since the 2026-08-06 BBox-to-Rect interop fix,
            // /Rect equals `appearance::ap_bbox`, which is PADDED for every type
            // `interop_rect` doesn't special-case (see `to_annotation_dict`'s doc comment).
            // The exact authored geometry is preserved losslessly in the private /RLRect
            // key instead - same pattern as Polyline's /Vertices alongside /L/CL. Falls
            // back to /Rect for annotations saved before this fix (no /RLRect key yet) -
            // those still have the OLD tight-bbox /Rect, so the fallback is still exact.
            let r = get_reals(d, b"RLRect")
                .or_else(|| get_reals(d, b"Rect"))
                .unwrap_or_else(|| vec![0.0, 0.0, 0.0, 0.0]);
            MarkupGeometry::Rect {
                min: PdfPoint {
                    x: r.first().copied().unwrap_or(0.0),
                    y: r.get(1).copied().unwrap_or(0.0),
                },
                max: PdfPoint {
                    x: r.get(2).copied().unwrap_or(0.0),
                    y: r.get(3).copied().unwrap_or(0.0),
                },
            }
        }
        Some("poly") => {
            let r = get_reals(d, b"Vertices")
                .or_else(|| get_reals(d, b"CL"))
                .or_else(|| get_reals(d, b"L"))
                .unwrap_or_default();
            let points = points_from_reals(&r);
            if !points.is_empty() {
                MarkupGeometry::Polyline(points)
            } else {
                // Bluebeam-nudge interop fallback (2026-08-08 corpus finding): moving a
                // Callout in Bluebeam Revu regenerates its appearance and DROPS /CL
                // entirely (spec-legal - /CL is optional per ISO 32000-1 12.5.6.6) while
                // leaving /RLGeom = "poly" (redline's own private tag) and a still-valid
                // /Rect/RLRect in place. Falling through with an EMPTY Polyline is worse
                // than a degenerate one: `markupToSvg`'s Callout branch reads the empty
                // array's "last point" via `?? {x:0,y:0}` and anchors the whole markup at
                // PDF-space (0,0) - the page's own origin corner - stacking every affected
                // Callout on top of each other there instead of near its real position.
                // A degenerate (zero-length) 2-point line anchored at the annotation's own
                // /RLRect (falling back to /Rect) min corner keeps the anchor inside the
                // markup's real footprint - the leader line itself renders invisibly
                // (zero length) rather than wrongly, and the text box lands in the right
                // neighbourhood instead of the page corner.
                let rect = get_reals(d, b"RLRect").or_else(|| get_reals(d, b"Rect"));
                match rect {
                    Some(r) if r.len() >= 2 => {
                        let anchor = PdfPoint { x: r[0], y: r[1] };
                        MarkupGeometry::Polyline(vec![anchor, anchor])
                    }
                    _ => MarkupGeometry::Polyline(Vec::new()),
                }
            }
        }
        Some("ink") => {
            let strokes = d
                .get(b"InkList")
                .ok()
                .and_then(|o| o.as_array().ok())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_array().ok())
                        .map(|s| {
                            let r: Vec<f64> = s
                                .iter()
                                .filter_map(|o| o.as_float().ok().map(|f| f as f64))
                                .collect();
                            points_from_reals(&r)
                        })
                        .collect()
                })
                .unwrap_or_default();
            MarkupGeometry::Ink(strokes)
        }
        Some("quads") => {
            let r = get_reals(d, b"QuadPoints").unwrap_or_default();
            MarkupGeometry::Quads(quads_from_reals(&r))
        }
        // Default / foreign: prefer explicit shapes, else the bounding /Rect.
        //
        // /QuadPoints is checked before the other foreign fallbacks so a Highlight
        // annotation authored by Acrobat/Bluebeam (no /RLGeom tag) imports losslessly
        // as Quads rather than collapsing to its bounding /Rect.
        _ => {
            if let Some(r) = get_reals(d, b"QuadPoints") {
                MarkupGeometry::Quads(quads_from_reals(&r))
            } else if let Some(r) = get_reals(d, b"InkList") {
                MarkupGeometry::Polyline(points_from_reals(&r))
            } else if let Some(r) = get_reals(d, b"Vertices")
                .or_else(|| get_reals(d, b"CL"))
                .or_else(|| get_reals(d, b"L"))
            {
                MarkupGeometry::Polyline(points_from_reals(&r))
            } else {
                let r = get_reals(d, b"Rect").unwrap_or_else(|| vec![0.0, 0.0, 0.0, 0.0]);
                MarkupGeometry::Rect {
                    min: PdfPoint {
                        x: r.first().copied().unwrap_or(0.0),
                        y: r.get(1).copied().unwrap_or(0.0),
                    },
                    max: PdfPoint {
                        x: r.get(2).copied().unwrap_or(0.0),
                        y: r.get(3).copied().unwrap_or(0.0),
                    },
                }
            }
        }
    }
}

impl Markup {
    /// Serialise to a standard PDF annotation dictionary (spec §6 persistence map).
    pub fn to_annotation_dict(&self) -> Dictionary {
        let mut d = Dictionary::new();
        let t = self.markup_type;

        d.set("Type", name("Annot"));
        d.set("Subtype", name(pdf_subtype(t)));

        // Bounding box + per-shape geometry. /Rect MUST equal the /AP /BBox for every
        // markup type (`appearance::ap_bbox` is the single shared source for both) so a
        // strict foreign viewer's BBox-to-Rect fit (ISO 32000-1 12.5.5) is always the
        // identity map instead of rescaling the appearance toward its own centre. Before
        // this fix only Text/Callout/MeasurementCount got the identity treatment (via
        // `appearance::interop_rect`); every other type kept the tight geometry bbox here
        // while its `/AP /BBox` was independently padded larger for stroke/arrowhead room
        // (`appearance::ap_bbox`), so a strict reader honouring `/AP` (proven with PDFium's
        // own annotation renderer in `render::tests::
        // strict_reader_annotation_appearance_paints_at_the_authored_rect_not_shrunk_to_fit`,
        // standing in for Bluebeam) visibly shrank every one of those shapes - the root
        // cause of the "markups show up in different locations in Bluebeam" report
        // (2026-08-06). `bbox()` (this module's own tight-geometry helper) is retired:
        // `ap_bbox()` already falls back to the tight bbox, PADDED, for every type
        // `interop_rect` doesn't special-case.
        let bb = super::appearance::ap_bbox(self);
        d.set("Rect", Object::Array(bb.iter().map(|v| real(*v)).collect()));
        match &self.geometry {
            MarkupGeometry::Polyline(pts) => {
                if matches!(pdf_subtype(t), "Line") && pts.len() >= 2 {
                    d.set("L", flatten(&pts[..2]));
                    // A standard PDF Line annotation is spec-defined as exactly 2 points
                    // (/L takes only 4 numbers), so any vertices beyond the first two have
                    // nowhere to go on the interop key. Without this, a >2-point Polyline
                    // on a Line-subtype markup (Line/Arrow/MeasurementLength/
                    // MeasurementRadius) silently lost every point past the first two on
                    // save - write(read(write(x))) != write(x). Also emit /Vertices with
                    // the FULL point list so our own reread (which checks /Vertices before
                    // /L - see geometry_from_dict) recovers everything losslessly; foreign
                    // viewers still get a valid 2-point /Line from the anchor+tip.
                    if pts.len() > 2 {
                        d.set("Vertices", flatten(pts));
                    }
                } else if matches!(t, MarkupType::Callout) {
                    d.set("CL", flatten(pts)); // callout leader line (spec §19.2)
                } else {
                    d.set("Vertices", flatten(pts));
                }
            }
            MarkupGeometry::Ink(strokes) => {
                d.set(
                    "InkList",
                    Object::Array(strokes.iter().map(|s| flatten(s)).collect()),
                );
            }
            MarkupGeometry::Quads(quads) => {
                // The standard PDF key for text-markup quadrilaterals (ISO 32000-1
                // section 12.5.6.10). This is what makes a Highlight annotation a REAL
                // text-anchored markup that round-trips through Acrobat/Bluebeam,
                // instead of the plain bounding-box `/Rect` a foreign viewer would
                // otherwise treat as the only geometry.
                d.set("QuadPoints", flatten_quads(quads));
            }
            MarkupGeometry::Rect { min, max } => {
                // Lossless authored geometry, independent of /Rect - see geometry_from_dict's
                // "rect" arm doc comment for why /Rect alone is no longer sufficient (it is
                // now `ap_bbox`, PADDED for every type `interop_rect` doesn't special-case).
                d.set("RLRect", flatten(&[*min, *max]));
            }
            MarkupGeometry::Point(_) => {}
        }

        // Identity + text (spec §6 embed map).
        d.set("NM", Object::string_literal(self.id().to_string()));
        d.set(
            "T",
            Object::string_literal(self.audit.created_by.display_name.clone()),
        );
        if let Some(s) = &self.subject {
            d.set("Subj", Object::string_literal(s.clone()));
        }
        // Only emit /Contents for a real, non-empty user note. An empty or whitespace-only
        // note must not leak: Bluebeam renders any /Contents as an attached comment note, so
        // a blank one shows up as a stray note on every line/arrow (G9 defect 4).
        if let Some(c) = &self.contents {
            if !c.trim().is_empty() {
                d.set("Contents", Object::string_literal(c.clone()));
            }
        }
        d.set(
            "CreationDate",
            Object::string_literal(to_pdf_date(&self.audit.created_at)),
        );
        d.set(
            "M",
            Object::string_literal(to_pdf_date(&self.audit.modified_at)),
        );

        // Standard `/F` annotation flags (ISO 32000-1 §12.5.3) - always emitted, not
        // conditional, matching every real annotation in the BB corpus (see the
        // `annot_flags` field doc comment on why the default is 4/Print rather than the
        // spec's own bare 0).
        d.set("F", Object::Integer(self.annot_flags as i64));

        // Standard `/RC` rich-text string and `/OC` optional-content value: both
        // preserved verbatim on round-trip when present, never invented (see the field
        // doc comments in mod.rs). Neither has redline UI/model semantics of its own.
        if let Some(rc) = &self.rich_text {
            d.set("RC", Object::string_literal(rc.clone()));
        }
        match &self.optional_content {
            Some(OptionalContent::Text(s)) => {
                d.set("OC", Object::string_literal(s.clone()));
            }
            Some(OptionalContent::Reference(num, gen)) => {
                d.set("OC", Object::Reference((*num, *gen)));
            }
            None => {}
        }

        // Appearance (colour / opacity / weight / fill / line-style).
        //
        // /C semantics are NOT uniform across subtypes (ISO 32000-1 §12.5.6.6, Free Text
        // Annotations): for every OTHER subtype we emit (Square/Circle/Polygon/Line/
        // PolyLine/Ink/Highlight/Stamp), /C is the standard border/stroke colour and
        // `appearance.color` is exactly that. But for FreeText (our Text/Callout) /C is
        // the annotation's BACKGROUND colour - the same role /IC plays for Square/Circle.
        // Writing the glyph colour to /C here unconditionally (pre-fix behaviour) meant
        // any strict viewer that regenerates the appearance from the dictionary keys
        // (Acrobat does this on every move/edit, since redline's own /AP is then stale)
        // painted a fully opaque box in the STROKE colour over the text - the box "filled
        // with blue and became unreadable" defect reported live against mr-desktop
        // Acrobat. redline's own renderer never had this bug (markup-render.ts reads /C-
        // sourced `appearance.color` only for the glyph, and `appearance.fill` - /IC on
        // the wire, see below - for the box background), so it was invisible locally.
        //
        // Fix: for Text/Callout, /C carries the real background (`appearance.fill`) when
        // set, and is OMITTED when unset - redline itself renders an unset fill as a
        // transparent box (`fill ?? "none"` in `styleOf`), and a real background is
        // spec-legal to leave absent. The glyph colour instead round-trips losslessly via
        // the private `/RLTextColor` key below, independent of /C.
        match t {
            MarkupType::Text | MarkupType::Callout => {
                if let Some(rgb) = self.appearance.fill.as_deref().and_then(hex_to_rgb) {
                    d.set("C", Object::Array(rgb.iter().map(|v| real(*v)).collect()));
                }
            }
            _ => {
                if let Some(rgb) = hex_to_rgb(&self.appearance.color) {
                    d.set("C", Object::Array(rgb.iter().map(|v| real(*v)).collect()));
                }
            }
        }
        // /CA (the standard annotation-level constant-opacity key) is fixed at 1.0, NOT
        // `self.appearance.opacity`. A viewer that honours /AP (PDFium, Acrobat, Bluebeam -
        // the whole point of appearance.rs) composites the annotation's rendered form using
        // /CA as a SINGLE blanket group alpha over the ENTIRE painted result, applied on top
        // of whatever alpha the content stream itself already used. If /CA carried the
        // stroke opacity here, every AP-consuming viewer would double-dim the stroke (once
        // via /CA, once via appearance.rs's own ExtGState) and, worse, ALSO dim the fill and
        // text by the stroke opacity - exactly the "opacity is global" bug this model fixes.
        // Per-component alpha (stroke via /CA, fill via /ca, both scoped to just the
        // relevant paint operators, text left unscoped) is applied entirely inside the /AP
        // content stream (appearance.rs); the real stroke-opacity value is preserved
        // losslessly for our own round-trip via the private /RLOpacity key below.
        // Highlight is the one exception to the /CA == 1.0 rule: a strict viewer (Bluebeam)
        // REGENERATES the highlight wash from /C + /CA + /QuadPoints and ignores our /AP, so
        // without the real wash alpha on /CA it renders a fully opaque, unreadable fill (G9
        // defect 3). The wash alpha is `opacity * HIGHLIGHT_WASH_ALPHA` - the exact fraction
        // redline's own viewer paints (markup-render.ts), NOT the raw opacity. The Highlight
        // /AP paints fully opaque under a Multiply blend and lets this /CA supply the wash as
        // a group alpha, so /AP-honouring viewers (Acrobat) match the regenerated result with
        // no double-dim.
        d.set(
            "CA",
            real(if matches!(t, MarkupType::Highlight) {
                self.appearance.opacity * HIGHLIGHT_WASH_ALPHA
            } else {
                1.0
            }),
        );
        d.set("RLOpacity", real(self.appearance.opacity));
        if let Some(fill) = &self.appearance.fill {
            if let Some(rgb) = hex_to_rgb(fill) {
                d.set("IC", Object::Array(rgb.iter().map(|v| real(*v)).collect()));
            }
        }
        // Text/Callout box border colour + fill alpha - redline-private, so foreign viewers
        // are unaffected (they keep /C as the annotation colour). Stored as the literal hex
        // string + a real, mirroring the /RL* private-key pattern (spec §6).
        if let Some(outline) = &self.appearance.outline_color {
            d.set("RLOutlineColor", Object::string_literal(outline.clone()));
        }
        if let Some(fa) = self.appearance.fill_opacity {
            d.set("RLFillOpacity", real(fa));
        }
        let mut bs = Dictionary::new();
        bs.set("W", real(self.appearance.line_weight));
        bs.set(
            "S",
            name(if matches!(self.appearance.line_style, LineStyle::Solid) {
                "S"
            } else {
                "D"
            }),
        );
        d.set("BS", Object::Dictionary(bs));

        // Revision cloud: the standard /BE border effect (Cloudy, /S /C) + /IT PolygonCloud
        // intent. Bluebeam/Acrobat regenerate the scalloped arcs from the polygon /Vertices
        // plus this /BE; without it a foreign viewer draws the raw straight-edged polygon
        // (the "coarse zigzag" G9 defect 2). Only Cloud gets it - a plain Polygon stays sharp.
        if matches!(t, MarkupType::Cloud) {
            let mut be = Dictionary::new();
            be.set("S", name("C")); // Cloudy border style
            be.set("I", real(2.0)); // intensity (0..2); 2 = standard revision-cloud amplitude
            d.set("BE", Object::Dictionary(be));
            d.set("IT", name("PolygonCloud"));
        }

        // Font: FreeText /DA (interop) + lossless /RLFont* round-trip (spec §6).
        //
        // /DA uses the standard base-14 resource name (ISO 32000-1 §12.7.3.3) so external
        // viewers (Acrobat, Bluebeam) render the intended typeface family. The exact family
        // string is preserved losslessly in /RLFontFamily for redline-to-redline round-trips.
        //
        // Base-14 /DA resource name mapping (title-cased, per PDF spec convention):
        //   Helv  = Helvetica / Arial
        //   TiRo  = Times-Roman / Times New Roman
        //   Cour  = Courier / Courier New
        // Viewers recognise these without an explicit /DR entry for FreeText annotations
        // (they are not AcroForm fields). If external-viewer rendering is still wrong after
        // this change, add a /DR resource dict - track as G9 external-viewer-verification.
        if let Some(font) = &self.appearance.font {
            let rgb = hex_to_rgb(&self.appearance.color).unwrap_or([0.0, 0.0, 0.0]);
            d.set(
                "DA",
                Object::string_literal(format!(
                    "/{} {:.0} Tf {:.3} {:.3} {:.3} rg",
                    base14_da_name(&font.family),
                    font.size_pt,
                    rgb[0],
                    rgb[1],
                    rgb[2]
                )),
            );
            d.set("RLFontFamily", Object::string_literal(font.family.clone()));
            d.set("RLFontSize", real(font.size_pt));
        } else if let Some(raw) = &self.raw_da {
            // No font to derive a /DA from (redline never sets one without a font), but
            // a foreign /DA was read that carried no parseable Tf operator - e.g. a
            // real Bluebeam colour-only DA like "0.5 0 1 rg" with no font/size at all
            // (a real corpus shape, not hypothetical - see `raw_da`'s field doc
            // comment). Re-emit it verbatim rather than silently dropping it.
            d.set("DA", Object::string_literal(raw.clone()));
        }

        // Private /RL* keys for lossless redline round-trip.
        //
        // /RLCoordV2 is a compatibility marker, NOT a lossless-roundtrip key: it tells
        // `document::annots::read_markups` whether this annotation's geometry keys were
        // written by the 2026-08-06 rotation/MediaBox-origin interop fix (present) or by
        // an older redline version that wrote `self.geometry` straight into /Rect with no
        // display<->true-space conversion at all (absent). Without this, upgrading redline
        // would silently RE-POSITION every existing markup on a rotated or offset-origin
        // page the instant its file is reopened - the new read-side transform would be
        // applied to old data it was never written against, moving shapes to a
        // completely different part of the screen even though the file was never
        // touched. Any annotation lacking the marker is read WITHOUT the transform
        // (identical to pre-fix behaviour, so on-screen position never changes for an
        // untouched file); the very next save always writes both the marker and a
        // spec-conformant /Rect, so a file self-heals for Bluebeam on its first re-save
        // with zero visual disruption in redline itself. See the module doc comment
        // above `display_to_true` in `document::annots` for the transform this gates.
        d.set("RLCoordV2", Object::Boolean(true));
        d.set("RLType", name(&type_tag(t)));
        // Lossless glyph colour for Text/Callout, independent of /C now that /C carries
        // the background there (see the /C block above). Always written (not gated on
        // /DA/font being present) so the exact glyph colour round-trips even for a plain
        // FreeText with no font size set. Stored as a literal hex string, mirroring the
        // existing /RLOutlineColor convention, to avoid any real-number round-trip
        // precision loss. A pre-fix file (no /RLTextColor) is read as /C == glyph colour
        // on the legacy-compat branch below, then self-heals to this key on next save.
        if matches!(t, MarkupType::Text | MarkupType::Callout) {
            d.set(
                "RLTextColor",
                Object::string_literal(self.appearance.color.clone()),
            );
        }
        d.set("RLGeom", name(geom_tag(&self.geometry)));
        d.set("RLPage", Object::Integer(self.page as i64));
        d.set(
            "RLUserId",
            Object::string_literal(self.audit.created_by.user_id.to_string()),
        );
        d.set(
            "RLModBy",
            Object::string_literal(self.audit.modified_by.display_name.clone()),
        );
        d.set(
            "RLModById",
            Object::string_literal(self.audit.modified_by.user_id.to_string()),
        );
        d.set("RLRevision", Object::Integer(self.audit.revision as i64));
        d.set("RLStatus", name(status_tag(self.workflow.status)));
        d.set("RLOrigin", name(origin_tag(self.audit.origin)));
        d.set(
            "RLLineStyle",
            name(line_style_tag(self.appearance.line_style)),
        );
        if let Some(layer) = &self.layer {
            d.set("RLLayer", Object::string_literal(layer.clone()));
        }
        if let Some(gid) = self.group_id {
            d.set("RLGroup", Object::string_literal(gid.to_string()));
        }

        // Count set (spec §7): the set assignment + symbol + colour via private
        // /RLCountSet* keys. The colour is ALSO carried by the standard /C key
        // (appearance.color) so external viewers render the marker in the set colour
        // with no extra mapping - but /RLCountSetColor is the one redline itself reads
        // back (multi-cycle fidelity fix, 2026-08-31: the set colour used to be derived
        // FROM /C on read instead of persisted independently, so editing a single Count
        // marker's own stroke colour - a normal per-marker restyle - silently rewrote
        // `count_set.color` on that marker's next save/reopen even though the set
        // definition itself, shared by every OTHER marker carrying the same
        // `count_set.id`, was never touched. See
        // `document::annots::tests::fidelity_matrix::multicycle_fidelity` for the
        // regression test that caught this on a second edit-then-reload cycle).
        if let Some(cs) = &self.count_set {
            d.set("RLCountSetId", Object::string_literal(cs.id.to_string()));
            d.set("RLCountSetName", Object::string_literal(cs.name.clone()));
            d.set("RLCountSetColor", Object::string_literal(cs.color.clone()));
            d.set("RLCountSymbol", name(count_symbol_tag(cs.symbol)));
        }

        // Measurement payload (spec §7): a single opaque JSON blob, not hand-mapped keys
        // like CountSet - the shape varies by measurement kind and carries an open
        // `custom_columns` map, so JSON is exact and doesn't need a decoder update every
        // time the payload grows. Previously this field was dropped entirely on read
        // (hardcoded `measurement: None` in from_annotation_dict) - every
        // MeasurementLength/Area/Perimeter/Volume/Count/Angle/Radius markup lost its
        // quantity data on save -> reopen.
        if let Some(meas) = &self.measurement {
            if let Ok(json) = serde_json::to_string(meas) {
                d.set("RLMeasure", Object::string_literal(json));
            }
        }

        // Reserved workflow fields not carried by /RLStatus: assignee + comment thread
        // (spec §6 decision f). No v1 UI surfaces these yet, but they are real fields on
        // every Markup and must round-trip rather than silently reset to empty on reopen.
        // Omitted when both are at their empty defaults so a plain markup's dict is
        // unchanged from before this field existed.
        if self.workflow.assignee.is_some() || !self.workflow.thread.is_empty() {
            if let Ok(json) =
                serde_json::to_string(&(&self.workflow.assignee, &self.workflow.thread))
            {
                d.set("RLWorkflowExtra", Object::string_literal(json));
            }
        }
        d
    }

    /// Parse a markup from a PDF annotation dictionary. Prefers the `/RL*` private keys
    /// (lossless for redline-authored annotations); for foreign annotations it does a
    /// best-effort import from the standard keys (new id, type inferred from `/Subtype`).
    /// Note: the measurement payload, comment thread, and assignee are not carried in the
    /// annotation (later slices). Font IS carried, via `/RLFontFamily`+`/RLFontSize`.
    pub fn from_annotation_dict(d: &Dictionary) -> Markup {
        let markup_type = get_name(d, b"RLType")
            .and_then(|t| type_from_tag(&t))
            .or_else(|| match get_name(d, b"Subtype").as_deref() {
                Some("Square") => Some(MarkupType::Rectangle),
                Some("Circle") => Some(MarkupType::Ellipse),
                Some("Line") => Some(MarkupType::Line),
                Some("Polygon") => Some(if is_cloud_polygon(d) {
                    MarkupType::Cloud
                } else {
                    MarkupType::Polygon
                }),
                Some("PolyLine") => Some(MarkupType::Polyline),
                Some("Highlight") => Some(MarkupType::Highlight),
                Some("Ink") => Some(MarkupType::Ink),
                Some("FreeText") => Some(if d.has(b"CL") {
                    MarkupType::Callout
                } else {
                    MarkupType::Text
                }),
                Some("Stamp") => Some(MarkupType::Stamp),
                _ => Some(MarkupType::Text),
            })
            .unwrap_or(MarkupType::Text);

        let id = get_string(d, b"NM")
            .and_then(|s| uuid::Uuid::parse_str(&s).ok())
            .unwrap_or_else(uuid::Uuid::new_v4);

        let created_name = get_string(d, b"T").unwrap_or_default();
        let created_uid = get_string(d, b"RLUserId")
            .and_then(|s| uuid::Uuid::parse_str(&s).ok())
            .unwrap_or_else(uuid::Uuid::nil);
        let created_by = UserRef {
            user_id: created_uid,
            display_name: created_name,
        };
        let modified_by = UserRef {
            user_id: get_string(d, b"RLModById")
                .and_then(|s| uuid::Uuid::parse_str(&s).ok())
                .unwrap_or(created_uid),
            display_name: get_string(d, b"RLModBy")
                .unwrap_or_else(|| created_by.display_name.clone()),
        };

        let created_at = get_string(d, b"CreationDate")
            .and_then(|s| from_pdf_date(&s))
            .unwrap_or_else(Utc::now);
        let modified_at = get_string(d, b"M")
            .and_then(|s| from_pdf_date(&s))
            .unwrap_or(created_at);

        let line_weight = d
            .get(b"BS")
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|bs| bs.get(b"W").ok())
            .and_then(|w| w.as_float().ok())
            .map(|f| f as f64)
            .unwrap_or(1.0);
        let line_style = get_name(d, b"RLLineStyle")
            .map(|s| line_style_from_tag(&s))
            .unwrap_or(LineStyle::Solid);
        // Prefer the private /RLOpacity key (the real stroke-opacity value, written by this
        // version of redline - see the /CA comment in to_annotation_dict for why /CA itself
        // is always 1.0 now). Fall back to /CA for files saved by a pre-/RLOpacity redline
        // build, or a foreign PDF where /CA is the only opacity signal at all (best-effort
        // import: a foreign annotation's single blanket /CA becomes our stroke opacity).
        let opacity = get_real(d, b"RLOpacity")
            .or_else(|| get_real(d, b"CA"))
            .unwrap_or(1.0);

        // Font: prefer redline's own lossless keys; fall back to parsing a foreign /DA
        // (see `font_from_da`'s doc comment - this is what actually closes the
        // "FreeText drops /DA on round-trip" finding, since the write side only ever
        // needed a non-None `font` to already emit /DA correctly). When a `/DA` is
        // present but carries no parseable font/size (real corpus shape: a colour-only
        // DA like "0.5 0 1 rg", no Tf), keep the raw string so the write side can still
        // re-emit it verbatim instead of losing it - see `raw_da`'s field doc comment.
        let da_string = get_string(d, b"DA");
        let font = get_real(d, b"RLFontSize")
            .map(|size_pt| FontSpec {
                family: get_string(d, b"RLFontFamily").unwrap_or_else(|| "Helvetica".to_string()),
                size_pt,
            })
            .or_else(|| da_string.as_deref().and_then(font_from_da));

        // Colour + fill (background). /C's meaning is subtype-dependent (see the
        // write-side /C comment in `to_annotation_dict`): the standard border/stroke
        // colour everywhere EXCEPT Text/Callout, where it is the box BACKGROUND (ISO
        // 32000-1 §12.5.6.6) - three cases for Text/Callout, in priority order:
        let (color, fill) = if matches!(markup_type, MarkupType::Text | MarkupType::Callout) {
            let ic_fill = get_reals(d, b"IC").map(|c| rgb_to_hex(&c));
            if let Some(text_color) = get_string(d, b"RLTextColor") {
                // 1. Post-fix redline file: /IC (always written alongside /C by redline
                // itself) normally carries the background and is kept in lockstep with /C
                // on every redline save. But /IC is a private, non-standard key that a
                // foreign editor (Bluebeam/Acrobat) has no reason to know about - if the
                // user edits the FreeText background in one of those viewers, ONLY /C
                // changes; /IC is left stale. The write side (`to_annotation_dict` above)
                // only ever emits /IC when `appearance.fill` is `Some`, so /IC's mere
                // PRESENCE here already proves the file was written with a background at
                // some point - which makes the two foreign-edit shapes unambiguous once
                // /RLTextColor confirms this is a post-fix file:
                //   - /C present, differs from /IC -> a foreign RECOLOUR; /C wins (the new
                //     background), and the stale /IC is what's wrong, not /C.
                //   - /C absent (but /IC present, proving a fill existed) -> a foreign
                //     REMOVAL, since redline itself never omits /C while a fill is set
                //     (see the /C write-side comment above) - /IC is what's now stale, and
                //     the removal must win rather than resurrecting the old background from
                //     the private key the foreign viewer never touched.
                // Either way the very next redline save re-converges both keys to the
                // winning value (write side: /C and /IC together, or neither when fill is
                // None) - self-healing rather than a permanent split.
                let c_fill = get_reals(d, b"C").map(|c| rgb_to_hex(&c));
                let fill = match &c_fill {
                    Some(c) if Some(c) != ic_fill.as_ref() => c_fill,
                    Some(_) => ic_fill,
                    None => None,
                };
                (text_color, fill)
            } else if get_name(d, b"RLType").is_some() {
                // 2. Pre-fix redline file (has other /RL* markers but no /RLTextColor):
                // /C was written as the glyph colour by the old code path. Preserve that
                // reading so the file's on-screen appearance in redline does not change
                // on open; the very next save writes /RLTextColor and switches /C to
                // background semantics, self-healing the file (mirrors the /RLCoordV2
                // legacy-guard precedent above).
                (
                    get_reals(d, b"C")
                        .map(|c| rgb_to_hex(&c))
                        .unwrap_or_else(|| "#000000".to_string()),
                    ic_fill,
                )
            } else {
                // 3. Foreign FreeText (Bluebeam/Acrobat-authored, no /RL* markers at all):
                // /C is a REAL background colour per spec and must round-trip untouched
                // as `fill`, never be misread as the glyph colour - the same misconception
                // this whole fix corrects, just biting on import instead of on save. /IC
                // is not a standard FreeText entry so foreign producers essentially never
                // write one; recover the glyph colour best-effort from a foreign /DA's
                // `rg` operator when present (a real corpus shape, see `color_from_da`'s
                // doc comment), defaulting to black otherwise.
                let glyph = da_string
                    .as_deref()
                    .and_then(color_from_da)
                    .unwrap_or_else(|| "#000000".to_string());
                let fill = ic_fill.or_else(|| get_reals(d, b"C").map(|c| rgb_to_hex(&c)));
                (glyph, fill)
            }
        } else {
            (
                get_reals(d, b"C")
                    .map(|c| rgb_to_hex(&c))
                    .unwrap_or_else(|| "#000000".to_string()),
                get_reals(d, b"IC").map(|c| rgb_to_hex(&c)),
            )
        };

        let raw_da = if font.is_none() { da_string } else { None };

        // Count set: reconstruct from /RLCountSet* keys. Colour prefers the dedicated
        // /RLCountSetColor key (see the write-side comment in `to_annotation_dict` for
        // why deriving it from /C == `color` instead was a cross-markup fidelity bug);
        // falls back to /C for a file saved by a pre-fix redline build, where the two
        // were always kept equal by construction so the fallback is exact.
        let count_set = get_string(d, b"RLCountSetId")
            .and_then(|s| uuid::Uuid::parse_str(&s).ok())
            .map(|set_id| CountSet {
                id: set_id,
                name: get_string(d, b"RLCountSetName").unwrap_or_default(),
                color: get_string(d, b"RLCountSetColor").unwrap_or_else(|| color.clone()),
                symbol: count_symbol_from_tag(&get_name(d, b"RLCountSymbol").unwrap_or_default()),
            });

        Markup {
            id,
            markup_type,
            page: get_int(d, b"RLPage").unwrap_or(0) as u32,
            geometry: geometry_from_dict(d),
            appearance: Appearance {
                color,
                line_weight,
                opacity,
                fill,
                line_style,
                font,
                // Box border colour + fill alpha — absent on pre-outline / foreign
                // annotations, which then deserialise to None (a sane default: border
                // falls back to `color`, fill stays fully opaque).
                outline_color: get_string(d, b"RLOutlineColor"),
                fill_opacity: get_real(d, b"RLFillOpacity"),
            },
            subject: get_string(d, b"Subj"),
            layer: get_string(d, b"RLLayer"),
            // Normalise an empty / whitespace-only /Contents to None so a foreign or legacy
            // blank note does not re-leak on the next redline save (G9 defect 4, read side).
            contents: get_string(d, b"Contents").filter(|s| !s.trim().is_empty()),
            group_id: get_string(d, b"RLGroup").and_then(|s| uuid::Uuid::parse_str(&s).ok()),
            audit: Audit {
                created_by,
                created_at,
                modified_by,
                modified_at,
                revision: get_int(d, b"RLRevision").unwrap_or(0) as u64,
                origin: match get_name(d, b"RLOrigin").as_deref() {
                    Some("FieldApp") => Origin::FieldApp,
                    _ => Origin::Desktop,
                },
            },
            workflow: {
                let (assignee, thread) = get_string(d, b"RLWorkflowExtra")
                    .and_then(|s| serde_json::from_str::<(Option<UserRef>, Vec<Reply>)>(&s).ok())
                    .unwrap_or((None, Vec::new()));
                Workflow {
                    status: status_from_tag(&get_name(d, b"RLStatus").unwrap_or_default()),
                    assignee,
                    thread,
                }
            },
            measurement: get_string(d, b"RLMeasure")
                .and_then(|s| serde_json::from_str::<Measurement>(&s).ok()),
            count_set,
            // Not reconstructed on reopen - see the field doc comment in markup/mod.rs
            // (the appearance is already baked into the saved /AP /N stream by then).
            stamp_asset: None,
            // Default to Print (4) when absent, matching Markup::new()'s own default -
            // see the `annot_flags` field doc comment.
            annot_flags: get_int(d, b"F").map(|f| f as i32).unwrap_or(4),
            rich_text: get_string(d, b"RC"),
            optional_content: d.get(b"OC").ok().and_then(|o| match o {
                Object::String(bytes, _) => Some(OptionalContent::Text(
                    String::from_utf8_lossy(bytes).into_owned(),
                )),
                Object::Reference((num, gen)) => Some(OptionalContent::Reference(*num, *gen)),
                _ => None,
            }),
            raw_da,
        }
    }
}

fn get_int(d: &Dictionary, key: &[u8]) -> Option<i64> {
    d.get(key).ok()?.as_i64().ok()
}

fn get_real(d: &Dictionary, key: &[u8]) -> Option<f64> {
    d.get(key).ok()?.as_float().ok().map(|f| f as f64)
}

/// Map a font family name to the standard PDF base-14 /DA resource name (ISO 32000-1 §12.7.3.3).
///
/// Matching is case-insensitive on a normalised prefix so common aliases ("Arial" for
/// Helvetica, "Times New Roman" for Times-Roman, "Courier New" for Courier) resolve
/// correctly. Unknown families fall back to `Helv` (Helvetica), consistent with Acrobat's
/// own default. The exact family string is always preserved in `/RLFontFamily` for lossless
/// redline round-trips - this mapping affects only external-viewer rendering.
fn base14_da_name(family: &str) -> &'static str {
    let lower = family.to_lowercase();
    if lower.starts_with("times") {
        "TiRo"
    } else if lower.starts_with("courier") {
        "Cour"
    } else {
        // Helvetica, Arial, and all unrecognised families -> Helv.
        "Helv"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn user(name: &str) -> UserRef {
        UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: name.to_string(),
        }
    }

    /// A markup using only the fields this slice maps (no font/measurement/thread), with
    /// second-resolution timestamps, so the annotation round-trip is exact (geometry to
    /// f32 precision).
    fn fixture(geom: MarkupGeometry, t: MarkupType) -> Markup {
        let mut m = Markup::new(t, 4, geom, Appearance::default(), user("Alice"));
        m.subject = Some("Door schedule".into());
        m.contents = Some("verify fire rating".into());
        m.layer = Some("A-DOOR".into());
        m.appearance.color = "#3366ff".into();
        m.appearance.opacity = 0.8;
        m.appearance.line_weight = 2.5;
        m.appearance.fill = Some("#ffeecc".into());
        m.appearance.outline_color = Some("#112233".into());
        m.appearance.fill_opacity = Some(0.4);
        m.workflow.status = MarkupStatus::Accepted;
        m.touch(user("Bob")); // revision 1, distinct modifier
        m.audit.created_at = Utc.with_ymd_and_hms(2026, 6, 8, 10, 30, 0).unwrap();
        m.audit.modified_at = Utc.with_ymd_and_hms(2026, 6, 8, 11, 0, 0).unwrap();
        m
    }

    fn assert_pt_eq(a: PdfPoint, b: PdfPoint) {
        assert!(
            (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01,
            "{a:?} != {b:?}"
        );
    }

    fn assert_geom_eq(a: &MarkupGeometry, b: &MarkupGeometry) {
        match (a, b) {
            (MarkupGeometry::Point(p), MarkupGeometry::Point(q)) => assert_pt_eq(*p, *q),
            (MarkupGeometry::Rect { min, max }, MarkupGeometry::Rect { min: m2, max: x2 }) => {
                assert_pt_eq(*min, *m2);
                assert_pt_eq(*max, *x2);
            }
            (MarkupGeometry::Polyline(u), MarkupGeometry::Polyline(v)) => {
                assert_eq!(u.len(), v.len());
                u.iter().zip(v).for_each(|(p, q)| assert_pt_eq(*p, *q));
            }
            (MarkupGeometry::Ink(u), MarkupGeometry::Ink(v)) => {
                assert_eq!(u.len(), v.len());
                for (s, t) in u.iter().zip(v) {
                    s.iter().zip(t).for_each(|(p, q)| assert_pt_eq(*p, *q));
                }
            }
            (MarkupGeometry::Quads(u), MarkupGeometry::Quads(v)) => {
                assert_eq!(u.len(), v.len(), "quad count must match");
                for (qa, qb) in u.iter().zip(v) {
                    for (p, q) in qa.iter().zip(qb) {
                        assert_pt_eq(*p, *q);
                    }
                }
            }
            _ => panic!("geometry variant mismatch: {a:?} vs {b:?}"),
        }
    }

    fn assert_roundtrip(m: &Markup) {
        let back = Markup::from_annotation_dict(&m.to_annotation_dict());
        assert_eq!(back.id(), m.id(), "id");
        assert_eq!(back.markup_type, m.markup_type, "type");
        assert_eq!(back.page, m.page, "page");
        assert_geom_eq(&back.geometry, &m.geometry);
        assert_eq!(back.subject, m.subject);
        assert_eq!(back.contents, m.contents);
        assert_eq!(back.layer, m.layer);
        assert_eq!(back.appearance.color, m.appearance.color);
        assert_eq!(back.appearance.fill, m.appearance.fill);
        assert_eq!(
            back.appearance.outline_color, m.appearance.outline_color,
            "outline_color"
        );
        match (back.appearance.fill_opacity, m.appearance.fill_opacity) {
            (Some(b), Some(a)) => assert!((b - a).abs() < 0.01, "fill_opacity {b} != {a}"),
            (b, a) => assert_eq!(b, a, "fill_opacity"),
        }
        assert_eq!(back.appearance.line_style, m.appearance.line_style);
        assert!((back.appearance.opacity - m.appearance.opacity).abs() < 0.01);
        assert!((back.appearance.line_weight - m.appearance.line_weight).abs() < 0.01);
        assert_eq!(back.appearance.font, m.appearance.font, "font");
        assert_eq!(back.workflow.status, m.workflow.status);
        assert_eq!(back.audit.revision, m.audit.revision);
        assert_eq!(back.audit.created_by, m.audit.created_by);
        assert_eq!(back.audit.modified_by, m.audit.modified_by);
        assert_eq!(back.audit.created_at, m.audit.created_at);
        assert_eq!(back.audit.modified_at, m.audit.modified_at);
        assert_eq!(back.audit.origin, m.audit.origin);
        assert_eq!(back.group_id, m.group_id, "group_id");
        assert_eq!(back.count_set, m.count_set, "count_set");
    }

    #[test]
    fn rect_markup_round_trips() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.5, y: 20.25 },
            max: PdfPoint { x: 110.0, y: 70.0 },
        };
        assert_roundtrip(&fixture(g, MarkupType::Rectangle));
    }

    #[test]
    fn line_markup_emits_l_segment_and_round_trips() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 50.0 },
        ]);
        let m = fixture(g, MarkupType::Line);
        let d = m.to_annotation_dict();
        assert!(d.has(b"L"), "Line annotation must emit /L");
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("Line"));
        assert_roundtrip(&m);
    }

    #[test]
    fn polygon_markup_emits_vertices_and_round_trips() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 50.0 },
        ]);
        let m = fixture(g, MarkupType::Polygon);
        let d = m.to_annotation_dict();
        assert!(d.has(b"Vertices"), "Polygon must emit /Vertices");
        assert_roundtrip(&m);
    }

    #[test]
    fn ink_markup_emits_inklist_and_round_trips() {
        let g = MarkupGeometry::Ink(vec![
            vec![PdfPoint { x: 1.0, y: 1.0 }, PdfPoint { x: 2.0, y: 3.0 }],
            vec![PdfPoint { x: 5.0, y: 5.0 }, PdfPoint { x: 6.0, y: 7.0 }],
        ]);
        let m = fixture(g, MarkupType::Ink);
        let d = m.to_annotation_dict();
        assert!(d.has(b"InkList"));
        assert_roundtrip(&m);
    }

    #[test]
    fn point_markup_round_trips() {
        let g = MarkupGeometry::Point(PdfPoint { x: 42.0, y: 99.0 });
        assert_roundtrip(&fixture(g, MarkupType::MeasurementCount));
    }

    #[test]
    fn emits_standard_interop_keys() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 10.0, y: 10.0 },
        };
        let d = fixture(g, MarkupType::Rectangle).to_annotation_dict();
        assert_eq!(get_name(&d, b"Type").as_deref(), Some("Annot"));
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("Square"));
        for k in [
            &b"NM"[..],
            b"Rect",
            b"Contents",
            b"Subj",
            b"T",
            b"CreationDate",
            b"M",
            b"C",
            b"CA",
        ] {
            assert!(
                d.has(k),
                "missing standard key {:?}",
                String::from_utf8_lossy(k)
            );
        }
    }

    #[test]
    fn exact_type_preserved_when_subtype_is_shared() {
        // Cloud and Polygon both map to PDF /Subtype Polygon — /RLType must disambiguate.
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 1.0, y: 1.0 },
        ]);
        let m = fixture(g, MarkupType::Cloud);
        let back = Markup::from_annotation_dict(&m.to_annotation_dict());
        assert_eq!(back.markup_type, MarkupType::Cloud);
    }

    #[test]
    fn foreign_annotation_imports_best_effort() {
        // A dict with only standard keys (no /RL*) — e.g. from Acrobat.
        let mut d = Dictionary::new();
        d.set("Subtype", name("Square"));
        d.set(
            "Rect",
            Object::Array(vec![real(5.0), real(6.0), real(15.0), real(26.0)]),
        );
        d.set("Contents", Object::string_literal("imported note"));
        let m = Markup::from_annotation_dict(&d);
        assert_eq!(m.markup_type, MarkupType::Rectangle);
        assert_eq!(m.contents.as_deref(), Some("imported note"));
        assert_eq!(m.audit.revision, 0);
        match m.geometry {
            MarkupGeometry::Rect { min, max } => {
                assert_pt_eq(min, PdfPoint { x: 5.0, y: 6.0 });
                assert_pt_eq(max, PdfPoint { x: 15.0, y: 26.0 });
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    #[test]
    fn freetext_with_font_round_trips_and_emits_da() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 12.0,
        });
        let d = m.to_annotation_dict();
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("FreeText"));
        assert!(d.has(b"DA"), "FreeText with a font must emit /DA");
        assert_roundtrip(&m); // assert_roundtrip now also checks font
    }

    #[test]
    fn callout_emits_cl_leader_and_round_trips() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 50.0, y: 60.0 },
        ]);
        let mut m = fixture(g, MarkupType::Callout);
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 14.0,
        });
        let d = m.to_annotation_dict();
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("FreeText"));
        assert!(d.has(b"CL"), "Callout must emit /CL leader");
        assert!(!d.has(b"Vertices"), "Callout uses /CL, not /Vertices");
        assert_roundtrip(&m);
    }

    /// Reproduces the 2026-08-08 Bluebeam-nudge corpus finding: moving a Callout in
    /// Bluebeam Revu regenerates its appearance and DROPS `/CL` entirely (spec-legal -
    /// `/CL` is optional per ISO 32000-1 12.5.6.6) while leaving redline's own
    /// `/RLGeom = "poly"` tag and a still-valid `/Rect`/`/RLRect` in place. Before the
    /// fix, `geometry_from_dict`'s "poly" branch fell through to an EMPTY `Polyline`,
    /// and `markupToSvg`'s Callout branch (frontend) reads that empty array's "last
    /// point" via `?? {x:0,y:0}` - anchoring the whole markup at the PDF page's own
    /// origin corner instead of near its real position. Every affected Callout in the
    /// real corpus (`Comment`/`e-callout`/`Cloud+`) collapsed onto that one spot.
    #[test]
    fn callout_missing_cl_falls_back_to_rect_anchor_not_page_origin() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 100.0, y: 200.0 },
            PdfPoint { x: 150.0, y: 260.0 },
        ]);
        let m = fixture(g, MarkupType::Callout);
        let mut d = m.to_annotation_dict();
        assert!(d.has(b"CL"), "fixture sanity: written Callout has /CL");

        // Simulate Bluebeam's nudge rewrite: /CL is gone, /RLGeom (and /RLRect/Rect)
        // remain - exactly the key set diffed from the real corpus files.
        d.remove(b"CL");
        assert!(get_reals(&d, b"Vertices").is_none());
        assert!(get_reals(&d, b"CL").is_none());
        assert!(get_reals(&d, b"L").is_none());

        let recovered = Markup::from_annotation_dict(&d);
        let MarkupGeometry::Polyline(pts) = recovered.geometry else {
            panic!(
                "Callout must still recover Polyline geometry, got {:?}",
                recovered.geometry
            );
        };
        assert!(
            !pts.is_empty(),
            "must not silently degrade to an empty Polyline (collapses the anchor to (0,0))"
        );
        let anchor = pts.last().expect("non-empty");
        assert!(
            anchor.x != 0.0 || anchor.y != 0.0,
            "anchor must not fall back to the PDF page origin (0,0), got {anchor:?}"
        );
        // The synthetic anchor must land inside/near the annotation's own Rect
        // footprint, not at some unrelated point.
        let rect = get_reals(&d, b"RLRect")
            .or_else(|| get_reals(&d, b"Rect"))
            .expect("Rect must still be present after simulated nudge");
        assert!(
            (anchor.x - rect[0]).abs() < 1e-6 && (anchor.y - rect[1]).abs() < 1e-6,
            "expected anchor at the Rect's min corner {:?}, got {anchor:?}",
            (rect[0], rect[1])
        );
    }

    // --- Text-anchored Highlight: /QuadPoints round-trip -----------------------------

    fn sample_quads() -> Vec<super::Quad> {
        vec![
            [
                PdfPoint { x: 72.0, y: 712.0 },
                PdfPoint { x: 500.0, y: 712.0 },
                PdfPoint { x: 72.0, y: 700.0 },
                PdfPoint { x: 500.0, y: 700.0 },
            ],
            [
                PdfPoint { x: 72.0, y: 698.0 },
                PdfPoint { x: 220.0, y: 698.0 },
                PdfPoint { x: 72.0, y: 686.0 },
                PdfPoint { x: 220.0, y: 686.0 },
            ],
        ]
    }

    #[test]
    fn highlight_quads_markup_emits_quadpoints_not_just_rect_and_round_trips() {
        let quads = sample_quads();
        let m = fixture(MarkupGeometry::Quads(quads.clone()), MarkupType::Highlight);
        let d = m.to_annotation_dict();

        assert_eq!(
            get_name(&d, b"Subtype").as_deref(),
            Some("Highlight"),
            "text-anchored highlight must use the standard /Highlight subtype"
        );
        assert!(
            d.has(b"QuadPoints"),
            "Highlight from a text selection must emit /QuadPoints"
        );
        // /Rect (the bounding box) is still required on every annotation - a viewer with
        // no QuadPoints support at least shows the right area.
        assert!(d.has(b"Rect"), "/Rect bounding box must still be present");

        let qp = get_reals(&d, b"QuadPoints").expect("/QuadPoints must be readable as reals");
        assert_eq!(qp.len(), 16, "2 quads x 8 floats each");
        // First quad, first point (top-left) must be exactly quads[0][0].
        assert_eq!(qp[0], 72.0);
        assert_eq!(qp[1], 712.0);

        assert_eq!(get_name(&d, b"RLGeom").as_deref(), Some("quads"));
        assert_roundtrip(&m);
    }

    #[test]
    fn highlight_quads_bbox_covers_every_quad_point() {
        let quads = sample_quads();
        let m = fixture(MarkupGeometry::Quads(quads), MarkupType::Highlight);
        let d = m.to_annotation_dict();
        let rect = get_reals(&d, b"Rect").expect("/Rect present");
        // Tight bbox spanning both quads (min x=72, min y=686, max x=500, max y=712),
        // PADDED by `appearance::ap_bbox` (2026-08-06 fix: /Rect == /AP /BBox for every
        // type, so a strict reader's BBox-to-Rect fit is always the identity map - see
        // `to_annotation_dict`'s doc comment). Highlight isn't one of `interop_rect`'s
        // identity-mapped types, so it gets the generic pad: `fixture()` sets
        // line_weight=2.5, so pad = (2.5*3.0).max(6.0) = 7.5 on every side.
        assert_eq!(rect, vec![64.5, 678.5, 507.5, 719.5]);
    }

    #[test]
    fn foreign_highlight_with_quadpoints_imports_as_quads_not_bounding_rect() {
        // A Highlight annotation authored by Acrobat/Bluebeam: no /RLGeom tag, but a
        // standard /QuadPoints array. Must import as Quads geometry (lossless line
        // shape), not collapse to the bounding /Rect.
        let mut d = Dictionary::new();
        d.set("Subtype", name("Highlight"));
        d.set(
            "Rect",
            Object::Array(vec![real(72.0), real(686.0), real(500.0), real(712.0)]),
        );
        d.set(
            "QuadPoints",
            Object::Array(vec![
                real(72.0),
                real(712.0),
                real(500.0),
                real(712.0),
                real(72.0),
                real(700.0),
                real(500.0),
                real(700.0),
            ]),
        );
        let m = Markup::from_annotation_dict(&d);
        assert_eq!(m.markup_type, MarkupType::Highlight);
        match m.geometry {
            MarkupGeometry::Quads(q) => {
                assert_eq!(q.len(), 1);
                assert_pt_eq(q[0][0], PdfPoint { x: 72.0, y: 712.0 });
                assert_pt_eq(q[0][3], PdfPoint { x: 500.0, y: 700.0 });
            }
            other => panic!("expected Quads from foreign /QuadPoints, got {other:?}"),
        }
    }

    // --- G7.2: base14_da_name unit tests -------------------------------------------

    #[test]
    fn base14_da_name_helvetica_and_arial_map_to_helv() {
        assert_eq!(base14_da_name("Helvetica"), "Helv");
        assert_eq!(base14_da_name("Arial"), "Helv");
        assert_eq!(base14_da_name("helvetica"), "Helv");
        assert_eq!(base14_da_name("ARIAL"), "Helv");
    }

    #[test]
    fn base14_da_name_times_variants_map_to_tiro() {
        assert_eq!(base14_da_name("Times"), "TiRo");
        assert_eq!(base14_da_name("Times New Roman"), "TiRo");
        assert_eq!(base14_da_name("Times-Roman"), "TiRo");
        assert_eq!(base14_da_name("times"), "TiRo");
        assert_eq!(base14_da_name("TIMES NEW ROMAN"), "TiRo");
    }

    #[test]
    fn base14_da_name_courier_variants_map_to_cour() {
        assert_eq!(base14_da_name("Courier"), "Cour");
        assert_eq!(base14_da_name("Courier New"), "Cour");
        assert_eq!(base14_da_name("courier new"), "Cour");
    }

    #[test]
    fn base14_da_name_unknown_falls_back_to_helv() {
        assert_eq!(base14_da_name("Comic Sans"), "Helv");
        assert_eq!(base14_da_name("Roboto"), "Helv");
        assert_eq!(base14_da_name(""), "Helv");
    }

    // --- G7.2: /DA emits correct base-14 resource name ----------------------------

    #[test]
    fn freetext_times_font_da_contains_tiro() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Times New Roman".into(),
            size_pt: 11.0,
        });
        let d = m.to_annotation_dict();
        let da = get_string(&d, b"DA").expect("/DA must be present");
        assert!(da.contains("/TiRo"), "/DA should contain /TiRo, got: {da}");
        assert!(
            da.contains(" Tf"),
            "/DA should contain Tf operator, got: {da}"
        );
        // Round-trip: family is preserved via /RLFontFamily, not inferred from /DA.
        assert_roundtrip(&m);
    }

    #[test]
    fn freetext_courier_font_da_contains_cour() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Courier New".into(),
            size_pt: 10.0,
        });
        let d = m.to_annotation_dict();
        let da = get_string(&d, b"DA").expect("/DA must be present");
        assert!(da.contains("/Cour"), "/DA should contain /Cour, got: {da}");
        assert!(
            da.contains(" Tf"),
            "/DA should contain Tf operator, got: {da}"
        );
        assert_roundtrip(&m);
    }

    #[test]
    fn freetext_helvetica_da_contains_helv() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 12.0,
        });
        let d = m.to_annotation_dict();
        let da = get_string(&d, b"DA").expect("/DA must be present");
        assert!(da.contains("/Helv"), "/DA should contain /Helv, got: {da}");
        assert!(
            da.contains(" Tf"),
            "/DA should contain Tf operator, got: {da}"
        );
        assert_roundtrip(&m);
    }

    // --- end G7.2 tests ------------------------------------------------------------

    #[test]
    fn foreign_freetext_imports_as_text_without_cl_callout_with_cl() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(5.0), real(6.0), real(100.0), real(26.0)]),
        );
        d.set("Contents", Object::string_literal("foreign text"));
        assert_eq!(
            Markup::from_annotation_dict(&d).markup_type,
            MarkupType::Text
        );
        d.set(
            "CL",
            Object::Array(vec![real(0.0), real(0.0), real(5.0), real(6.0)]),
        );
        assert_eq!(
            Markup::from_annotation_dict(&d).markup_type,
            MarkupType::Callout
        );
    }

    // --- G8: /RLGroup round-trip tests ---

    #[test]
    fn grouped_markup_rl_group_round_trips() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 60.0, y: 70.0 },
        };
        let mut m = fixture(g, MarkupType::Rectangle);
        let gid = uuid::Uuid::new_v4();
        m.group_id = Some(gid);

        let d = m.to_annotation_dict();

        // /RLGroup must be present and equal to the UUID string.
        let rl_group =
            get_string(&d, b"RLGroup").expect("/RLGroup must be present for grouped markup");
        assert_eq!(
            rl_group,
            gid.to_string(),
            "/RLGroup must equal the group UUID"
        );

        // Full annotation round-trip via assert_roundtrip (now checks group_id).
        assert_roundtrip(&m);
    }

    #[test]
    fn ungrouped_markup_omits_rl_group() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 60.0, y: 70.0 },
        };
        let m = fixture(g, MarkupType::Rectangle);
        assert!(m.group_id.is_none(), "fixture must start ungrouped");

        let d = m.to_annotation_dict();
        assert!(
            !d.has(b"RLGroup"),
            "/RLGroup must be absent for ungrouped markup"
        );

        let back = Markup::from_annotation_dict(&d);
        assert!(
            back.group_id.is_none(),
            "round-tripped ungrouped markup must have group_id == None"
        );
    }

    #[test]
    fn foreign_annotation_without_rl_group_is_ungrouped() {
        // A bare foreign dict with only standard keys — no /RLGroup.
        let mut d = Dictionary::new();
        d.set("Subtype", name("Square"));
        d.set(
            "Rect",
            Object::Array(vec![real(5.0), real(6.0), real(15.0), real(26.0)]),
        );
        d.set("Contents", Object::string_literal("imported"));

        let m = Markup::from_annotation_dict(&d);
        assert!(
            m.group_id.is_none(),
            "foreign annotation without /RLGroup must import with group_id == None"
        );
    }

    // --- end G8 tests ---

    // --- Count set round-trip ---

    #[test]
    fn count_markup_with_set_round_trips_via_annotation() {
        use super::super::{CountSet, CountSymbol};
        let g = MarkupGeometry::Point(PdfPoint { x: 42.0, y: 99.0 });
        let mut m = fixture(g, MarkupType::MeasurementCount);
        // The set colour must equal the annotation colour (/C carries the set colour);
        // the fixture sets appearance.color = "#3366ff".
        let cs = CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-A fixture".into(),
            color: "#3366ff".into(),
            symbol: CountSymbol::Diamond,
        };
        m.count_set = Some(cs.clone());

        let d = m.to_annotation_dict();
        assert_eq!(
            get_string(&d, b"RLCountSetId").as_deref(),
            Some(cs.id.to_string().as_str()),
            "/RLCountSetId must carry the set id"
        );
        assert_eq!(get_name(&d, b"RLCountSymbol").as_deref(), Some("Diamond"));
        // Colour is carried by the standard /C key (not a private one).
        assert!(d.has(b"C"), "set colour must be on standard /C");

        assert_roundtrip(&m); // assert_roundtrip now also checks count_set
    }

    #[test]
    fn count_markup_without_set_omits_keys() {
        let g = MarkupGeometry::Point(PdfPoint { x: 1.0, y: 2.0 });
        let m = fixture(g, MarkupType::MeasurementCount);
        assert!(m.count_set.is_none(), "fixture starts with no count set");
        let d = m.to_annotation_dict();
        assert!(!d.has(b"RLCountSetId"), "no /RLCountSetId without a set");
        assert!(Markup::from_annotation_dict(&d).count_set.is_none());
    }

    #[test]
    fn foreign_save_strips_rlcountsetcolor_falls_back_to_c_gracefully() {
        // Resilience test (PR #87 follow-up, 2026-08-31 review cycle): simulate a
        // foreign save that drops the private /RLCountSetColor key while leaving the
        // rest of the /RLCountSet* markers (and /RLType) intact - the documented
        // fallback path (write-side comment above `d.set("RLCountSetColor", ...)`)
        // says the standard /C key ALSO carries the set colour precisely so a viewer
        // that strips unknown private keys still leaves enough for redline to recover
        // a sane colour, rather than corrupting or panicking.
        use super::super::{CountSet, CountSymbol};
        let g = MarkupGeometry::Point(PdfPoint { x: 10.0, y: 10.0 });
        let mut m = fixture(g, MarkupType::MeasurementCount);
        m.appearance.color = "#7b2ff7".into();
        let cs = CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-B fixture".into(),
            color: "#7b2ff7".into(), // == appearance.color, as redline always writes it
            symbol: CountSymbol::Square,
        };
        m.count_set = Some(cs.clone());

        let mut d = m.to_annotation_dict();
        assert!(
            d.has(b"RLCountSetColor"),
            "sanity: key exists before the strip"
        );
        d.remove(b"RLCountSetColor"); // simulated foreign strip of just this key

        let back = Markup::from_annotation_dict(&d);
        let restored = back
            .count_set
            .as_ref()
            .expect("RLCountSetId/Name/Symbol survived - set membership must not be lost");
        assert_eq!(
            restored.id, cs.id,
            "set id unaffected by the stripped colour key"
        );
        assert_eq!(
            restored.color, "#7b2ff7",
            "documented fallback: colour recovers from the standard /C key"
        );
        assert_eq!(restored.symbol, CountSymbol::Square);
    }

    #[test]
    fn foreign_save_strips_all_rl_keys_from_count_marker_degrades_to_plain_stamp() {
        // Harsher resilience case: a foreign viewer that regenerates the annotation
        // dictionary from its OWN model (rather than selectively dropping one unknown
        // key) keeps only standard PDF keys and discards every /RL* extension key,
        // including /RLType itself. Losing /RLType means redline can no longer tell
        // this was ever a MeasurementCount marker - proving that degrades to a plain,
        // well-formed markup (never a panic, never a corrupted/garbage count_set) is
        // the graceful-fallback contract this test locks in.
        use super::super::{CountSet, CountSymbol};
        let g = MarkupGeometry::Point(PdfPoint { x: 10.0, y: 10.0 });
        let mut m = fixture(g, MarkupType::MeasurementCount);
        m.count_set = Some(CountSet {
            id: uuid::Uuid::new_v4(),
            name: "Type-C fixture".into(),
            color: "#3366ff".into(),
            symbol: CountSymbol::Circle,
        });

        let mut d = m.to_annotation_dict();
        let rl_keys: Vec<Vec<u8>> = d
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| k.starts_with(b"RL"))
            .collect();
        assert!(
            !rl_keys.is_empty(),
            "sanity: some /RL* keys exist before the strip"
        );
        for k in rl_keys {
            d.remove(&k);
        }

        // Must not panic, and must fall back to the standard-key-derived reading
        // (Subtype "Stamp" -> MarkupType::Stamp) rather than yielding a MeasurementCount
        // with a corrupted or dangling count_set reference.
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(
            back.markup_type,
            MarkupType::Stamp,
            "with no /RLType left, the standard /Subtype Stamp must be trusted instead"
        );
        assert!(
            back.count_set.is_none(),
            "no /RLCountSetId survives the strip - count_set must be None, not stale/garbage"
        );
    }

    // --- Text-box outline colour + fill alpha round-trip ---

    #[test]
    fn text_box_outline_and_fill_opacity_emit_private_keys_and_round_trip() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        // fixture() already sets outline_color = "#112233" and fill_opacity = 0.4.
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 12.0,
        });

        let d = m.to_annotation_dict();
        assert_eq!(
            get_string(&d, b"RLOutlineColor").as_deref(),
            Some("#112233"),
            "/RLOutlineColor must carry the box border colour"
        );
        assert!(d.has(b"RLFillOpacity"), "/RLFillOpacity must be present");
        // /C on a FreeText is the BACKGROUND (ISO 32000-1 §12.5.6.6, not the glyph
        // colour) - fixture() sets fill = "#ffeecc" (distinct from color = "#3366ff"),
        // so /C must carry the fill, and the glyph colour must be on /RLTextColor
        // instead, never on /C. This is the exact defect this fix corrects (a strict
        // viewer like Acrobat regenerating the appearance from /C painted the glyph
        // colour as a solid opaque background box).
        assert_eq!(
            get_reals(&d, b"C").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#ffeecc"),
            "/C must be the background colour (appearance.fill), not the glyph colour"
        );
        assert_eq!(
            get_string(&d, b"RLTextColor").as_deref(),
            Some("#3366ff"),
            "/RLTextColor must carry the exact glyph colour"
        );

        assert_roundtrip(&m); // assert_roundtrip now also checks outline_color + fill_opacity
    }

    #[test]
    fn freetext_c_is_background_not_glyph_colour_for_callout_too() {
        // Same defect, same fix, for Callout (the other FreeText subtype) - a bare
        // MarkupGeometry::Point is enough since geometry doesn't matter here.
        let mut m = fixture(
            MarkupGeometry::Point(PdfPoint { x: 5.0, y: 5.0 }),
            MarkupType::Callout,
        );
        m.appearance.color = "#ff0000".into(); // glyph
        m.appearance.fill = Some("#00ff00".into()); // background

        let d = m.to_annotation_dict();
        assert_eq!(
            get_reals(&d, b"C").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#00ff00"),
            "/C must be the background for Callout too"
        );
        assert_eq!(get_string(&d, b"RLTextColor").as_deref(), Some("#ff0000"));
        assert_roundtrip(&m);
    }

    #[test]
    fn freetext_with_no_fill_omits_c_entirely() {
        // redline itself renders a Text/Callout with no `fill` as a transparent box
        // (`fill ?? "none"` in markup-render.ts's styleOf) - /C must be omitted rather
        // than written as some colour Acrobat would then paint solid. This is the
        // Acrobat-regeneration defect's exact live-reported shape: a bare FreeText with
        // no background set must never gain one on save.
        let mut m = fixture(
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 50.0, y: 20.0 },
            },
            MarkupType::Text,
        );
        m.appearance.fill = None;

        let d = m.to_annotation_dict();
        assert!(
            !d.has(b"C"),
            "/C must be OMITTED for a FreeText with no background, not defaulted to a colour"
        );
        // The glyph colour must still be present via the dedicated key.
        assert_eq!(get_string(&d, b"RLTextColor").as_deref(), Some("#3366ff"));
        assert_roundtrip(&m);
    }

    #[test]
    fn freetext_acrobat_regeneration_no_longer_paints_glyph_colour_as_background() {
        // Simulates what the live-reported defect actually was: strip the /AP redline
        // wrote (as Acrobat does on any move/edit, since it regenerates from the
        // dictionary keys rather than trusting a stale appearance stream) and confirm
        // the ONLY colour information left for a strict-viewer-regenerated background is
        // /C == the real fill, never the stroke/glyph colour.
        let mut m = fixture(
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 80.0, y: 24.0 },
            },
            MarkupType::Text,
        );
        m.appearance.color = "#0000ff".into(); // the stroke/glyph colour that used to leak
        m.appearance.fill = Some("#ffffff".into());

        let mut d = m.to_annotation_dict();
        d.remove(b"AP"); // annotation.rs never sets /AP itself, but be explicit either way

        let c = get_reals(&d, b"C")
            .map(|c| rgb_to_hex(&c))
            .expect("/C must be present when a background is set");
        assert_ne!(
            c, "#0000ff",
            "/C must never be the stroke/glyph colour - that IS the defect"
        );
        assert_eq!(c, "#ffffff", "/C must be the real background");
    }

    #[test]
    fn legacy_pre_fix_freetext_reads_c_as_glyph_colour_then_self_heals() {
        // A file saved by a pre-fix redline build: has other /RL* markers (proving
        // redline authorship) but no /RLTextColor, and /C was written as the glyph
        // colour by the old code path. Reading it must preserve that on-screen
        // appearance exactly (never silently reinterpret /C as a background the user
        // never set) - then the very next save must self-heal: /RLTextColor gets
        // written and /C moves to background semantics.
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set("RLType", name("Text")); // proves redline authorship, no /RLTextColor
        d.set("RLPage", Object::Integer(0));
        d.set(
            "C",
            Object::Array(vec![real(0.2), real(0.4), real(0.8)]), // old: glyph colour
        );
        d.set(
            "IC",
            Object::Array(vec![real(1.0), real(1.0), real(0.9)]), // background, via /IC
        );

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(
            back.appearance.color, "#3366cc",
            "legacy file: /C must still be read as the glyph colour"
        );
        assert_eq!(
            back.appearance.fill.as_deref(),
            Some("#ffffe5"),
            "legacy file: background still comes from /IC, unaffected"
        );

        // Self-heal: re-serialising the imported markup must now write /RLTextColor
        // and move /C to background semantics (matching what a fresh save would do).
        let healed = back.to_annotation_dict();
        assert_eq!(
            get_string(&healed, b"RLTextColor").as_deref(),
            Some("#3366cc"),
            "self-heal must add /RLTextColor"
        );
        assert_eq!(
            get_reals(&healed, b"C").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#ffffe5"),
            "self-heal must move /C to the background colour"
        );
    }

    #[test]
    fn bluebeam_edits_c_on_reopen_of_post_fix_file_win_over_stale_ic() {
        // A post-fix redline file (has /RLTextColor), then re-opened after a foreign
        // viewer (Bluebeam/Acrobat) edited the FreeText background. The foreign editor
        // only knows about the standard /C key - it has no reason to touch our private
        // /IC - so after such an edit /C and /IC disagree. The read side must trust /C
        // (the foreign edit), not the now-stale /IC, or the edit is silently dropped the
        // moment redline reopens the file (PR #88 follow-up, 2026-08-31 review cycle).
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set("RLType", name("Text"));
        d.set("RLPage", Object::Integer(0));
        d.set("RLTextColor", Object::string_literal("#3366ff")); // glyph, untouched
        d.set(
            "IC",
            Object::Array(vec![real(1.0), real(1.0), real(0.9)]), // stale: old background (#ffffe5)
        );
        d.set(
            "C",
            Object::Array(vec![real(0.0), real(1.0), real(0.0)]), // Bluebeam's edit: new background (#00ff00)
        );

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(
            back.appearance.color, "#3366ff",
            "glyph colour must still come from /RLTextColor, untouched by the /C edit"
        );
        assert_eq!(
            back.appearance.fill.as_deref(),
            Some("#00ff00"),
            "the Bluebeam-edited /C must win as the new background over the stale /IC"
        );

        // Re-converge: the very next redline save must write both /C and /IC back to
        // the winning value, so the split self-heals rather than persisting forever.
        let healed = back.to_annotation_dict();
        assert_eq!(
            get_reals(&healed, b"C").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#00ff00"),
            "self-heal: /C must carry the converged background"
        );
        assert_eq!(
            get_reals(&healed, b"IC").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#00ff00"),
            "self-heal: /IC must converge to match /C"
        );
    }

    #[test]
    fn freetext_c_matching_ic_is_unaffected_by_the_bluebeam_edit_check() {
        // Control case: /C and /IC agree (the normal, un-edited-by-a-foreign-viewer
        // shape every redline save produces). The new "/C wins on mismatch" logic must
        // not disturb this - it only fires on a genuine disagreement.
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 40.0, y: 16.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.color = "#123456".into();
        m.appearance.fill = Some("#abcdef".into());

        assert_roundtrip(&m); // to_annotation_dict always writes /C == /IC for Text/Callout
    }

    #[test]
    fn bluebeam_removes_c_on_reopen_of_post_fix_file_wins_over_stale_ic() {
        // PR #91 review finding: the recolour case (/C present, differs from /IC) was
        // fixed, but a foreign REMOVAL of the background - expressed per ISO 32000-1
        // §12.5.6.6 by omitting /C entirely, not by setting it to some sentinel - was not.
        // Reading fell through to the stale /IC, silently resurrecting a background the
        // user had just cleared in Bluebeam/Acrobat. /IC's mere presence already proves a
        // fill existed at some point (the write side never emits /IC without /C - see the
        // write-side /IC comment), so /C absent + /IC present is unambiguous evidence of a
        // foreign removal, not of a background that was never set.
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set("RLType", name("Text"));
        d.set("RLPage", Object::Integer(0));
        d.set("RLTextColor", Object::string_literal("#3366ff")); // glyph, untouched
        d.set(
            "IC",
            Object::Array(vec![real(1.0), real(1.0), real(0.9)]), // stale: the old background (#ffffe5)
        );
        // No /C at all - Bluebeam's edit: the user cleared the background.

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(
            back.appearance.color, "#3366ff",
            "glyph colour must still come from /RLTextColor, untouched by the removal"
        );
        assert_eq!(
            back.appearance.fill, None,
            "the Bluebeam removal (/C absent) must win over the stale /IC, not resurrect it"
        );

        // Re-converge: the very next redline save must write neither /C nor /IC (fill is
        // None), so the removal self-heals into a consistent no-fill file rather than the
        // stale /IC surviving to cause the same bug again on a third open.
        let healed = back.to_annotation_dict();
        assert!(
            !healed.has(b"C"),
            "self-heal: /C must stay omitted (no fill)"
        );
        assert!(
            !healed.has(b"IC"),
            "self-heal: /IC must be dropped, not carried forward stale"
        );

        // Third open: re-reading the healed dict must reproduce the same no-fill result,
        // proving the split is fully resolved rather than merely deferred.
        let reopened = Markup::from_annotation_dict(&healed);
        assert_eq!(
            reopened.appearance.fill, None,
            "third open must remain stable at None"
        );
    }

    #[test]
    fn freetext_genuinely_never_had_fill_is_unaffected_by_the_removal_check() {
        // Control case for the removal fix: a post-fix file that never had a background at
        // all (no /C, no /IC - the genuine "no fill was ever set" shape, distinct from the
        // "/C removed but /IC still stale" shape above only by /IC's absence). Must read as
        // fill == None, exactly as it always has - the new None-on-/C-absent branch must
        // not be reachable any differently here since /IC is also absent.
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set("RLType", name("Text"));
        d.set("RLPage", Object::Integer(0));
        d.set("RLTextColor", Object::string_literal("#3366ff"));
        // No /C, no /IC - never had a background.

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.appearance.color, "#3366ff");
        assert_eq!(
            back.appearance.fill, None,
            "never-had-fill file must read as no fill"
        );

        // Stays fill == None through a real save/reload cycle. (Not `assert_roundtrip`
        // here: `back`'s audit timestamps come from the `Utc::now()` fallback since `d`
        // carries no /CreationDate or /M, and that fallback's sub-second precision doesn't
        // survive the PDF date string's whole-second serialisation - an unrelated
        // round-trip mismatch this test isn't about. `legacy_pre_fix_freetext_reads_c_as_
        // glyph_colour_then_self_heals` above avoids `assert_roundtrip` for the same
        // reason.)
        let resaved = back.to_annotation_dict();
        assert!(!resaved.has(b"C"));
        assert!(!resaved.has(b"IC"));
        assert_eq!(Markup::from_annotation_dict(&resaved).appearance.fill, None);
    }

    #[test]
    fn foreign_freetext_c_recovered_as_background_glyph_from_da() {
        // A genuinely foreign (Bluebeam/Acrobat-authored) FreeText: no /RL* markers at
        // all, /C is a real background per spec, and /DA carries a colour-only `rg`
        // operator (the real corpus shape noted on `raw_da`'s doc comment) as the only
        // source of the glyph colour. /C must round-trip as `fill`, never as `color`.
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set(
            "C",
            Object::Array(vec![real(1.0), real(0.0), real(0.0)]), // real background: red
        );
        d.set("DA", Object::string_literal("0 0.5 1 rg")); // glyph colour only, no Tf

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(
            back.appearance.fill.as_deref(),
            Some("#ff0000"),
            "foreign file: /C must round-trip as the background, not the glyph colour"
        );
        assert_eq!(
            back.appearance.color, "#0080ff",
            "foreign file: glyph colour recovered from /DA's rg operator"
        );

        // Round-trips through redline's own writer without losing the recovered
        // background (fill survives even though /DA's colour-only string does not
        // survive verbatim - font is None so raw_da preserves it separately; see
        // `foreign_da_without_font_operator_preserves_raw_string_verbatim`).
        let saved = back.to_annotation_dict();
        assert_eq!(
            get_reals(&saved, b"C").map(|c| rgb_to_hex(&c)).as_deref(),
            Some("#ff0000"),
            "re-save must keep the recovered background on /C"
        );
    }

    #[test]
    fn foreign_freetext_c_recovered_as_background_no_da_defaults_glyph_black() {
        // Same foreign-file case with no /DA at all: glyph colour has no recoverable
        // source, so it must default to black rather than fabricating a value or
        // (the actual defect) inheriting the background colour.
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(50.0), real(20.0)]),
        );
        d.set(
            "C",
            Object::Array(vec![real(0.0), real(1.0), real(0.0)]), // real background: green
        );

        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.appearance.fill.as_deref(), Some("#00ff00"));
        assert_eq!(
            back.appearance.color, "#000000",
            "no /DA colour to recover: glyph colour defaults to black, not the background"
        );
    }

    #[test]
    fn markup_without_outline_or_fill_opacity_omits_keys_and_defaults_to_none() {
        // A plain markup with neither field set must not emit the private keys, and a
        // foreign annotation lacking them imports with both as None (sane default).
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 10.0, y: 10.0 },
        };
        let mut m = Markup::new(MarkupType::Text, 0, g, Appearance::default(), user("Alice"));
        m.contents = Some("plain".into());
        assert!(m.appearance.outline_color.is_none());
        assert!(m.appearance.fill_opacity.is_none());

        let d = m.to_annotation_dict();
        assert!(!d.has(b"RLOutlineColor"), "no /RLOutlineColor when unset");
        assert!(!d.has(b"RLFillOpacity"), "no /RLFillOpacity when unset");

        let back = Markup::from_annotation_dict(&d);
        assert!(back.appearance.outline_color.is_none());
        assert!(back.appearance.fill_opacity.is_none());
    }

    // --- Opacity model: /CA is always 1.0, real stroke opacity lives in /RLOpacity ---

    #[test]
    fn ca_is_always_1_0_regardless_of_stroke_opacity() {
        // A viewer that honours /AP composites the WHOLE rendered form using /CA as one
        // blanket group alpha (see the comment in to_annotation_dict). If /CA carried the
        // stroke opacity, every AP-consuming viewer would double-dim strokes and, worse,
        // ALSO dim fill/text by it - the "opacity is global" bug. /CA must stay 1.0 no
        // matter what stroke opacity the user picks; appearance.rs applies opacity itself.
        for stroke_opacity in [0.0, 0.1, 0.5, 0.8, 1.0] {
            let mut m = fixture(
                MarkupGeometry::Rect {
                    min: PdfPoint { x: 0.0, y: 0.0 },
                    max: PdfPoint { x: 10.0, y: 10.0 },
                },
                MarkupType::Rectangle,
            );
            m.appearance.opacity = stroke_opacity;
            let d = m.to_annotation_dict();
            let ca = d.get(b"CA").unwrap().as_float().unwrap();
            assert!(
                (ca - 1.0).abs() < 1e-6,
                "/CA must be 1.0 for stroke_opacity={stroke_opacity}, got {ca}"
            );
        }
    }

    #[test]
    fn rl_opacity_carries_the_real_stroke_opacity_and_round_trips() {
        let mut m = fixture(
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            MarkupType::Rectangle,
        );
        m.appearance.opacity = 0.35;
        let d = m.to_annotation_dict();
        let rl_opacity = d.get(b"RLOpacity").unwrap().as_float().unwrap();
        assert!(
            (rl_opacity - 0.35).abs() < 1e-4,
            "/RLOpacity must carry the real stroke opacity, got {rl_opacity}"
        );
        let back = Markup::from_annotation_dict(&d);
        assert!(
            (back.appearance.opacity - 0.35).abs() < 1e-4,
            "opacity must round-trip via /RLOpacity, got {}",
            back.appearance.opacity
        );
    }

    #[test]
    fn legacy_file_with_only_ca_no_rl_opacity_falls_back_to_ca() {
        // A file saved by a pre-/RLOpacity redline build (or a foreign PDF) only has /CA.
        // Import must still treat that as the stroke opacity (best-effort backward compat).
        let mut d = Dictionary::new();
        d.set("Subtype", name("Square"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(10.0), real(10.0)]),
        );
        d.set("CA", real(0.6));
        let back = Markup::from_annotation_dict(&d);
        assert!(
            (back.appearance.opacity - 0.6).abs() < 1e-4,
            "must fall back to /CA when /RLOpacity is absent, got {}",
            back.appearance.opacity
        );
    }

    #[test]
    fn rl_opacity_takes_priority_over_a_stale_ca() {
        // If both keys are present (our own files always write both), /RLOpacity wins -
        // /CA is always 1.0 on our own output and must never shadow the real value.
        let mut d = Dictionary::new();
        d.set("Subtype", name("Square"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(10.0), real(10.0)]),
        );
        d.set("CA", real(1.0));
        d.set("RLOpacity", real(0.42));
        let back = Markup::from_annotation_dict(&d);
        assert!(
            (back.appearance.opacity - 0.42).abs() < 1e-4,
            "must prefer /RLOpacity over /CA, got {}",
            back.appearance.opacity
        );
    }

    // --- G9 Bluebeam-interop regression suite (2026-07-12) --------------------------------
    //
    // Each test asserts the STANDARD annotation-dict keys a strict foreign viewer (Bluebeam
    // Revu) regenerates its appearance from, inspecting the raw lopdf `Dictionary` produced
    // by `to_annotation_dict` directly (NOT via `from_annotation_dict`, so the assertion is
    // independent of redline's own reader). Screenshots + root causes: G9 dispatch 2026-07-12.

    /// Defect 2: a revision cloud must carry the standard /BE border-effect + /IT intent so
    /// Bluebeam regenerates the scalloped arcs from the polygon vertices. Without them BB
    /// draws the raw straight-edged polygon (the "coarse zigzag" defect).
    #[test]
    fn cloud_emits_border_effect_and_polygon_cloud_intent() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 0.0 },
            PdfPoint { x: 100.0, y: 80.0 },
            PdfPoint { x: 0.0, y: 80.0 },
        ]);
        let d = fixture(g, MarkupType::Cloud).to_annotation_dict();
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("Polygon"));
        assert!(d.has(b"Vertices"), "cloud must carry the polygon /Vertices");
        let be = d
            .get(b"BE")
            .expect("cloud must emit a /BE border effect")
            .as_dict()
            .expect("/BE is a dictionary");
        assert_eq!(
            be.get(b"S").unwrap().as_name().unwrap(),
            b"C",
            "/BE /S must be Cloudy"
        );
        let i = be
            .get(b"I")
            .expect("/BE /I intensity present")
            .as_float()
            .unwrap();
        assert!(
            i > 0.0,
            "/BE /I must be a positive cloud intensity, got {i}"
        );
        assert_eq!(
            get_name(&d, b"IT").as_deref(),
            Some("PolygonCloud"),
            "cloud must declare /IT PolygonCloud so viewers treat the polygon as a cloud"
        );
        // A plain (non-cloud) polygon must NOT get a border effect.
        let plain = fixture(
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 10.0 },
            ]),
            MarkupType::Polygon,
        )
        .to_annotation_dict();
        assert!(
            !plain.has(b"BE"),
            "a plain Polygon must not carry a /BE cloud effect"
        );
    }

    /// Defect 3: a Highlight must publish its real WASH alpha (opacity * HIGHLIGHT_WASH_ALPHA,
    /// the fraction redline's own viewer paints) on the STANDARD /CA key so a viewer that
    /// regenerates the highlight (Bluebeam) renders the same translucent wash - not a fully
    /// opaque fill, and not the raw opacity. Every other markup keeps /CA == 1.0 (opacity
    /// lives inside the /AP; see the /CA comment in to_annotation_dict).
    #[test]
    fn highlight_ca_carries_real_wash_alpha_other_shapes_stay_opaque() {
        let quads = vec![[
            PdfPoint { x: 0.0, y: 10.0 },
            PdfPoint { x: 40.0, y: 10.0 },
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 40.0, y: 0.0 },
        ]];
        let mut hl = fixture(MarkupGeometry::Quads(quads), MarkupType::Highlight);
        hl.appearance.opacity = 0.8;
        let d = hl.to_annotation_dict();
        let ca = d.get(b"CA").unwrap().as_float().unwrap() as f64;
        let expected = 0.8 * HIGHLIGHT_WASH_ALPHA; // 0.28 - matches the on-screen wash
        assert!(
            (ca - expected).abs() < 1e-4,
            "Highlight /CA must carry the wash alpha (opacity*{HIGHLIGHT_WASH_ALPHA}={expected}), got {ca}"
        );
        assert!(
            ca < 0.8,
            "wash /CA must be dimmer than raw opacity, got {ca}"
        );

        let mut rect = fixture(
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 10.0, y: 10.0 },
            },
            MarkupType::Rectangle,
        );
        rect.appearance.opacity = 0.35;
        let cad = rect
            .to_annotation_dict()
            .get(b"CA")
            .unwrap()
            .as_float()
            .unwrap();
        assert!(
            (cad - 1.0).abs() < 1e-6,
            "non-highlight /CA must stay 1.0, got {cad}"
        );
    }

    /// Defect 4: a Line/Arrow with no user-typed note must NOT leak a /Contents or /Popup
    /// (Bluebeam renders any /Contents as an attached comment note). An empty / whitespace
    /// note counts as no note. A real note is still emitted.
    #[test]
    fn line_without_note_emits_no_contents_or_popup() {
        let geom = || {
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 50.0, y: 50.0 },
            ])
        };
        let mut m = Markup::new(
            MarkupType::Line,
            0,
            geom(),
            Appearance::default(),
            user("Alice"),
        );
        m.contents = None;
        let d = m.to_annotation_dict();
        assert!(
            !d.has(b"Contents"),
            "a line with no note must not emit /Contents"
        );
        assert!(!d.has(b"Popup"), "redline must never emit a /Popup object");

        m.contents = Some("   ".into());
        assert!(
            !m.to_annotation_dict().has(b"Contents"),
            "a whitespace-only note must be treated as no note"
        );

        m.contents = Some("check clearance".into());
        assert_eq!(
            get_string(&m.to_annotation_dict(), b"Contents").as_deref(),
            Some("check clearance"),
            "a real user note is still emitted on /Contents"
        );
    }

    /// Defect 4 (read side): a foreign / legacy annotation carrying an empty /Contents ()
    /// must import as no note, so a redline re-save does not re-leak it.
    #[test]
    fn empty_contents_imports_as_none() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("Line"));
        d.set("Contents", Object::string_literal(""));
        assert!(
            Markup::from_annotation_dict(&d).contents.is_none(),
            "an empty /Contents must import as None"
        );
    }

    /// Defect 5: a count marker must serialise with a subtype a foreign viewer renders via
    /// its /AP (Stamp) and a NON-ZERO /Rect around the point - the previous FreeText subtype
    /// with a zero-size /Rect was dropped entirely by Bluebeam. redline still recovers the
    /// MeasurementCount type (via /RLType) and the exact point (from the /Rect centre).
    #[test]
    fn count_marker_uses_stamp_subtype_and_nonzero_centred_rect() {
        let g = MarkupGeometry::Point(PdfPoint { x: 42.0, y: 99.0 });
        let d = fixture(g, MarkupType::MeasurementCount).to_annotation_dict();
        assert_eq!(
            get_name(&d, b"Subtype").as_deref(),
            Some("Stamp"),
            "count markers must use a subtype foreign viewers render from /AP"
        );
        let r = get_reals(&d, b"Rect").expect("/Rect present");
        assert_eq!(r.len(), 4);
        assert!(
            (r[2] - r[0]).abs() > 1.0 && (r[3] - r[1]).abs() > 1.0,
            "count /Rect must be non-degenerate so Bluebeam renders it, got {r:?}"
        );
        assert!(
            ((r[0] + r[2]) / 2.0 - 42.0).abs() < 1e-6 && ((r[1] + r[3]) / 2.0 - 99.0).abs() < 1e-6,
            "the point must sit at the /Rect centre, got {r:?}"
        );
        // Round-trips back to the exact point + MeasurementCount type.
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.markup_type, MarkupType::MeasurementCount);
        match back.geometry {
            MarkupGeometry::Point(p) => {
                assert!(
                    (p.x - 42.0).abs() < 1e-6 && (p.y - 99.0).abs() < 1e-6,
                    "point centre round-trips"
                );
            }
            other => panic!("count marker must reload as a Point, got {other:?}"),
        }
    }

    /// Defect 1 (dict side): the annotation /Rect for a Callout must include the synthesized
    /// text box (which sits beyond the leader vertices), so it matches the /AP /BBox and a
    /// strict viewer does not scale the appearance. The plain leader bbox omits the box.
    #[test]
    fn callout_rect_includes_the_synthesized_text_box() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },   // target
            PdfPoint { x: 50.0, y: 60.0 }, // anchor (box origin)
        ]);
        let r = get_reals(
            &fixture(g, MarkupType::Callout).to_annotation_dict(),
            b"Rect",
        )
        .expect("/Rect present");
        // Anchor is (50,60); the box extends right + up from it, so the rect must reach past it.
        assert!(
            r[2] > 50.0,
            "/Rect right edge must include the callout box width, got {r:?}"
        );
        assert!(
            r[3] > 60.0,
            "/Rect top edge must include the callout box height, got {r:?}"
        );
    }

    // --- BB-interop fix wave 2026-08-11 (obs:je08u4y8rukjzbpm2y5f) ---------------------

    #[test]
    fn annot_flags_default_to_print_and_round_trip() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 10.0, y: 10.0 },
        };
        let m = fixture(g, MarkupType::Rectangle);
        assert_eq!(m.annot_flags, 4, "Markup::new default must be Print (4)");
        let d = m.to_annotation_dict();
        assert_eq!(get_int(&d, b"F"), Some(4), "/F must always be emitted");
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.annot_flags, 4);
    }

    #[test]
    fn annot_flags_preserve_a_non_default_foreign_value() {
        // A real corpus value: Print + NoZoom + NoRotate (4 + 8 + 16 = 28).
        let mut d = Dictionary::new();
        d.set("Subtype", name("Square"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(10.0), real(10.0)]),
        );
        d.set("F", Object::Integer(28));
        let m = Markup::from_annotation_dict(&d);
        assert_eq!(m.annot_flags, 28);
        let back_d = m.to_annotation_dict();
        assert_eq!(
            get_int(&back_d, b"F"),
            Some(28),
            "must round-trip exactly, not reset to the default"
        );
    }

    #[test]
    fn rich_text_rc_round_trips_when_present_and_is_absent_by_default() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 10.0, y: 10.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        assert!(
            m.to_annotation_dict().get(b"RC").is_err(),
            "no /RC by default"
        );

        m.rich_text = Some("<?xml version=\"1.0\"?><body>note</body>".to_string());
        let d = m.to_annotation_dict();
        assert_eq!(
            get_string(&d, b"RC").as_deref(),
            Some(m.rich_text.as_deref().unwrap())
        );
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.rich_text, m.rich_text);
    }

    #[test]
    fn optional_content_text_and_reference_both_round_trip() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 10.0, y: 10.0 },
        };

        // Bluebeam .btx-export shape: a plain PDF string naming the layer.
        let mut m = fixture(g.clone(), MarkupType::Rectangle);
        m.optional_content = Some(OptionalContent::Text("emittiv markups".to_string()));
        let d = m.to_annotation_dict();
        assert_eq!(
            get_string(&d, b"OC").as_deref(),
            Some("emittiv markups"),
            "/OC must be written as the exact string, not reinterpreted"
        );
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.optional_content, m.optional_content);

        // A real opened-PDF shape: an indirect reference to an OCG dictionary.
        let mut m2 = fixture(g, MarkupType::Rectangle);
        m2.optional_content = Some(OptionalContent::Reference(17, 0));
        let d2 = m2.to_annotation_dict();
        assert_eq!(
            d2.get(b"OC").ok().and_then(|o| o.as_reference().ok()),
            Some((17, 0))
        );
        let back2 = Markup::from_annotation_dict(&d2);
        assert_eq!(back2.optional_content, m2.optional_content);
    }

    /// Read-side classification fix: a foreign (no `/RLType`) `/Subtype /Polygon`
    /// annotation carrying `/BE << /S /C >>` (Cloudy border) must import as
    /// `MarkupType::Cloud`, not a plain `Polygon` - otherwise the existing write path
    /// (which only emits `/BE`+`/IT` for `MarkupType::Cloud`) never gets a chance to
    /// re-emit them, and the real BB corpus's cloud markups lose their scalloped-arc
    /// styling on every round-trip (4/7 golden Polygon items, obs:je08u4y8rukjzbpm2y5f).
    #[test]
    fn foreign_polygon_with_cloudy_be_imports_as_cloud_and_be_survives_round_trip() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("Polygon"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(10.0), real(10.0)]),
        );
        d.set(
            "Vertices",
            flatten(&[
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 10.0, y: 0.0 },
                PdfPoint { x: 5.0, y: 10.0 },
            ]),
        );
        let mut be = Dictionary::new();
        be.set("S", name("C"));
        be.set("I", real(2.0));
        d.set("BE", Object::Dictionary(be));
        d.set("IT", name("PolygonCloud"));

        let m = Markup::from_annotation_dict(&d);
        assert_eq!(
            m.markup_type,
            MarkupType::Cloud,
            "a foreign Polygon with a Cloudy /BE must classify as Cloud, not plain Polygon"
        );

        let round_tripped = m.to_annotation_dict();
        assert!(round_tripped.has(b"BE"), "/BE must survive the round-trip");
        assert_eq!(
            get_name(&round_tripped, b"IT").as_deref(),
            Some("PolygonCloud")
        );
    }

    /// A plain foreign Polygon (no /BE, no /IT PolygonCloud) must still import as plain
    /// `Polygon` - the Cloud-detection fix must not misclassify every Polygon.
    #[test]
    fn foreign_polygon_without_be_stays_plain_polygon() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("Polygon"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(10.0), real(10.0)]),
        );
        let m = Markup::from_annotation_dict(&d);
        assert_eq!(m.markup_type, MarkupType::Polygon);
    }

    /// The actual mechanism behind the harness's "FreeText drops /DA on 22/22 golden
    /// items" finding: a real Bluebeam `/DA` (no `/RLFontFamily`/`/RLFontSize`, since
    /// it's foreign data) must be parsed into a font so the ALREADY-correct write path
    /// (gated on `Some(font)`) re-emits it. The write side needed no change - only this
    /// read-side recovery.
    #[test]
    fn foreign_da_with_font_operator_recovers_font_and_da_round_trips() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(100.0), real(20.0)]),
        );
        d.set(
            "DA",
            Object::string_literal("1 0.5019608 0.2509804 rg /Calibri 10 Tf"),
        );
        let m = Markup::from_annotation_dict(&d);
        let font = m
            .appearance
            .font
            .as_ref()
            .expect("font must be recovered from /DA");
        assert_eq!(font.family, "Calibri");
        assert_eq!(font.size_pt, 10.0);

        let back_d = m.to_annotation_dict();
        assert!(
            back_d.has(b"DA"),
            "/DA must round-trip once a font is recovered"
        );
    }

    /// A real corpus shape: a `/DA` with a colour operator but NO `Tf` (font/size) at
    /// all, e.g. `"0.5019608 0 1 rg"`. No font can be recovered, but the raw string
    /// must still be preserved and re-emitted verbatim rather than silently dropped.
    #[test]
    fn foreign_da_without_font_operator_preserves_raw_string_verbatim() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set(
            "Rect",
            Object::Array(vec![real(0.0), real(0.0), real(100.0), real(20.0)]),
        );
        d.set("DA", Object::string_literal("0.5019608 0 1 rg"));
        let m = Markup::from_annotation_dict(&d);
        assert!(
            m.appearance.font.is_none(),
            "no Tf operator means no recoverable font"
        );
        assert_eq!(m.raw_da.as_deref(), Some("0.5019608 0 1 rg"));

        let back_d = m.to_annotation_dict();
        assert_eq!(
            get_string(&back_d, b"DA").as_deref(),
            Some("0.5019608 0 1 rg"),
            "the raw /DA must round-trip verbatim when no font could be derived from it"
        );
    }

    /// A redline-authored markup with a real font always writes /DA from the font model
    /// (RLFontFamily/RLFontSize win over any raw_da fallback, which only applies when
    /// there is no font at all).
    #[test]
    fn own_font_model_wins_over_raw_da_fallback_when_both_present() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 0.0, y: 0.0 },
            max: PdfPoint { x: 100.0, y: 20.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec {
            family: "Helvetica".into(),
            size_pt: 12.0,
        });
        m.raw_da = Some("this should never be written".to_string());
        let d = m.to_annotation_dict();
        let da = get_string(&d, b"DA").expect("/DA present");
        assert!(
            da.contains("Tf"),
            "must derive from the real font model: {da:?}"
        );
        assert_ne!(da, "this should never be written");
    }
}
