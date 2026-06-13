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
//! (colour / opacity / weight / fill / line-style). NOT yet mapped here (they live in
//! the sidecar via serde, or land in a later slice): font, the measurement payload,
//! comment thread/replies, and assignee. PDF reals are f32 (lopdf), so geometry in the
//! annotation is the interop copy — the canonical f64 geometry stays in the app model /
//! sidecar (spec §5/§6).

use chrono::{DateTime, NaiveDateTime, Utc};
use lopdf::{Dictionary, Object};

use super::{
    Appearance, Audit, LineStyle, Markup, MarkupGeometry, MarkupStatus, MarkupType, Origin,
    UserRef, Workflow,
};
use crate::geometry::PdfPoint;

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
        .map(|o| o.as_f32().ok().map(|f| f as f64))
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
        MarkupType::Text | MarkupType::MeasurementCount => "Text",
        MarkupType::Callout => "FreeText",
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
    }
}

fn flatten(pts: &[PdfPoint]) -> Object {
    Object::Array(pts.iter().flat_map(|p| [real(p.x), real(p.y)]).collect())
}

fn points_from_reals(r: &[f64]) -> Vec<PdfPoint> {
    r.chunks_exact(2)
        .map(|c| PdfPoint { x: c[0], y: c[1] })
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
                                .filter_map(|o| o.as_f32().ok().map(|f| f as f64))
                                .collect();
                            points_from_reals(&r)
                        })
                        .collect()
                })
                .unwrap_or_default();
            MarkupGeometry::Ink(strokes)
        }
        // Default / foreign: prefer explicit shapes, else the bounding /Rect.
        _ => {
            if let Some(r) = get_reals(d, b"InkList") {
                MarkupGeometry::Polyline(points_from_reals(&r))
            } else if let Some(r) = get_reals(d, b"Vertices").or_else(|| get_reals(d, b"L")) {
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
        d.set("CA", real(self.appearance.opacity));
        if let Some(fill) = &self.appearance.fill {
            if let Some(rgb) = hex_to_rgb(fill) {
                d.set("IC", Object::Array(rgb.iter().map(|v| real(*v)).collect()));
            }
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
        d
    }

    /// Parse a markup from a PDF annotation dictionary. Prefers the `/RL*` private keys
    /// (lossless for redline-authored annotations); for foreign annotations it does a
    /// best-effort import from the standard keys (new id, type inferred from `/Subtype`).
    /// Note: font, measurement payload, thread, and assignee are not carried in the
    /// annotation — they come from the sidecar.
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
                Some("FreeText") => Some(MarkupType::Callout),
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
            .and_then(|w| w.as_f32().ok())
            .map(|f| f as f64)
            .unwrap_or(1.0);
        let line_style = get_name(d, b"RLLineStyle")
            .map(|s| line_style_from_tag(&s))
            .unwrap_or(LineStyle::Solid);
        let opacity = d
            .get(b"CA")
            .ok()
            .and_then(|o| o.as_f32().ok())
            .map(|f| f as f64)
            .unwrap_or(1.0);

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
                font: None,
            },
            subject: get_string(d, b"Subj"),
            layer: get_string(d, b"RLLayer"),
            contents: get_string(d, b"Contents"),
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
            workflow: Workflow {
                status: status_from_tag(&get_name(d, b"RLStatus").unwrap_or_default()),
                assignee: None,
                thread: Vec::new(),
            },
            measurement: None,
        }
    }
}

fn get_int(d: &Dictionary, key: &[u8]) -> Option<i64> {
    d.get(key).ok()?.as_i64().ok()
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
        assert_eq!(back.appearance.line_style, m.appearance.line_style);
        assert!((back.appearance.opacity - m.appearance.opacity).abs() < 0.01);
        assert!((back.appearance.line_weight - m.appearance.line_weight).abs() < 0.01);
        assert_eq!(back.workflow.status, m.workflow.status);
        assert_eq!(back.audit.revision, m.audit.revision);
        assert_eq!(back.audit.created_by, m.audit.created_by);
        assert_eq!(back.audit.modified_by, m.audit.modified_by);
        assert_eq!(back.audit.created_at, m.audit.created_at);
        assert_eq!(back.audit.modified_at, m.audit.modified_at);
        assert_eq!(back.audit.origin, m.audit.origin);
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
        assert_roundtrip(&fixture(g, MarkupType::Text));
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
}
