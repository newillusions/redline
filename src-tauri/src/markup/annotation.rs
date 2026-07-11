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
    MarkupStatus, MarkupType, Measurement, Origin, Reply, UserRef, Workflow,
};
use crate::geometry::{PdfPoint, Quad};

// --- small helpers -------------------------------------------------------------

fn name(v: &str) -> Object {
    Object::Name(v.as_bytes().to_vec())
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

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
        MarkupType::MeasurementCount => "Text",
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

fn bbox(g: &MarkupGeometry) -> [f64; 4] {
    let pts: Vec<PdfPoint> = match g {
        MarkupGeometry::Point(p) => vec![*p],
        MarkupGeometry::Rect { min, max } => vec![*min, *max],
        MarkupGeometry::Polyline(v) => v.clone(),
        MarkupGeometry::Ink(strokes) => strokes.iter().flatten().copied().collect(),
        MarkupGeometry::Quads(quads) => quads.iter().flatten().copied().collect(),
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    if pts.is_empty() {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [x0, y0, x1, y1]
    }
}

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
            let r = get_reals(d, b"Rect").unwrap_or_default();
            MarkupGeometry::Point(PdfPoint {
                x: r.first().copied().unwrap_or(0.0),
                y: r.get(1).copied().unwrap_or(0.0),
            })
        }
        Some("poly") => {
            let r = get_reals(d, b"Vertices")
                .or_else(|| get_reals(d, b"CL"))
                .or_else(|| get_reals(d, b"L"))
                .unwrap_or_default();
            MarkupGeometry::Polyline(points_from_reals(&r))
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

        // Bounding box (always) + per-shape geometry.
        let bb = bbox(&self.geometry);
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
            MarkupGeometry::Point(_) | MarkupGeometry::Rect { .. } => {}
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
        if let Some(c) = &self.contents {
            d.set("Contents", Object::string_literal(c.clone()));
        }
        d.set(
            "CreationDate",
            Object::string_literal(to_pdf_date(&self.audit.created_at)),
        );
        d.set(
            "M",
            Object::string_literal(to_pdf_date(&self.audit.modified_at)),
        );

        // Appearance (colour / opacity / weight / fill / line-style).
        if let Some(rgb) = hex_to_rgb(&self.appearance.color) {
            d.set("C", Object::Array(rgb.iter().map(|v| real(*v)).collect()));
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
        d.set("CA", real(1.0));
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
        }

        // Private /RL* keys for lossless redline round-trip.
        d.set("RLType", name(&type_tag(t)));
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

        // Count set (spec §7): the set assignment + symbol via private /RLCountSet* keys.
        // The set COLOUR is carried by the standard /C key (appearance.color == set.color),
        // so external viewers render the marker in the set colour with no extra mapping.
        if let Some(cs) = &self.count_set {
            d.set("RLCountSetId", Object::string_literal(cs.id.to_string()));
            d.set("RLCountSetName", Object::string_literal(cs.name.clone()));
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
                Some("Polygon") => Some(MarkupType::Polygon),
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

        let color = get_reals(d, b"C")
            .map(|c| rgb_to_hex(&c))
            .unwrap_or_else(|| "#000000".to_string());
        let fill = get_reals(d, b"IC").map(|c| rgb_to_hex(&c));
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

        // Count set: reconstruct from /RLCountSet* keys; the colour comes from /C (== `color`).
        let count_set = get_string(d, b"RLCountSetId")
            .and_then(|s| uuid::Uuid::parse_str(&s).ok())
            .map(|set_id| CountSet {
                id: set_id,
                name: get_string(d, b"RLCountSetName").unwrap_or_default(),
                color: color.clone(),
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
                font: get_real(d, b"RLFontSize").map(|size_pt| FontSpec {
                    family: get_string(d, b"RLFontFamily")
                        .unwrap_or_else(|| "Helvetica".to_string()),
                    size_pt,
                }),
                // Box border colour + fill alpha — absent on pre-outline / foreign
                // annotations, which then deserialise to None (a sane default: border
                // falls back to `color`, fill stays fully opaque).
                outline_color: get_string(d, b"RLOutlineColor"),
                fill_opacity: get_real(d, b"RLFillOpacity"),
            },
            subject: get_string(d, b"Subj"),
            layer: get_string(d, b"RLLayer"),
            contents: get_string(d, b"Contents"),
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

    // --- Text-anchored Highlight: /QuadPoints round-trip -----------------------------

    fn sample_quads() -> Vec<super::Quad>{
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
        assert!(d.has(b"QuadPoints"), "Highlight from a text selection must emit /QuadPoints");
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
        // bbox = [left, bottom, right, top] spanning both quads (min x=72, min y=686,
        // max x=500, max y=712).
        assert_eq!(rect, vec![72.0, 686.0, 500.0, 712.0]);
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
                real(72.0), real(712.0), real(500.0), real(712.0),
                real(72.0), real(700.0), real(500.0), real(700.0),
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
        // The text glyph colour stays on the standard /C key (unaffected by the outline).
        assert!(d.has(b"C"), "glyph colour stays on /C");

        assert_roundtrip(&m); // assert_roundtrip now also checks outline_color + fill_opacity
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
}
