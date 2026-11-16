//! XFDF import: turn the annotations of an `.xfdf` file ([`super::xfdf::parse_xfdf`]) into
//! markups for the open document, the counterpart of [`super::xfdf::export_xfdf`].
//!
//! A markup written by redline comes back as it was (type from `rltype`, exact geometry from
//! the private `rlgeom` or the standard `vertices` / `inklist` / `coords` / `start` / `end`).
//! A file from Revu or Acrobat is mapped by element (`square`, `circle`, `ink`, `highlight`,
//! ...); `rect` is then the shape's box. Types that need data an XFDF file does not carry
//! (measurements, legends, dynamic stamps) come in as their plain shape. Elements with no
//! markup equivalent, annotations on pages the document does not have, and anything non-finite
//! are counted as `ignored`, never guessed at.
//!
//! A markup whose `name` is already in the document (as a markup id, or a PDF-origin `/NM`)
//! is skipped, so importing the same file twice adds nothing. The `name` becomes the markup id
//! when it is a UUID (redline's own export), else a stable id derived from it, which is what
//! makes the second import of a foreign file skip too.
//!
//! The file is untrusted: sizes, counts and every number are checked here.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::xfdf::XfdfAnnot;
use super::{
    derive_uuid, Appearance, Markup, MarkupGeometry, MarkupStatus, MarkupType, Reply, StatusChange,
    UserRef, REVIEW_MODEL,
};
use crate::geometry::PdfPoint;

/// Most annotations one import will read (the file is hostile until proven otherwise).
pub const MAX_ANNOTS: usize = 100_000;

/// What an import produced. `markups` are new and not yet in the store: the caller adds them
/// as one undo step.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XfdfImport {
    pub markups: Vec<Markup>,
    /// Markups skipped because their `name` is already in the document (or repeated in the file).
    pub skipped_existing: usize,
    /// Annotations not imported: unsupported element, page out of range, bad numbers, or a
    /// reply / status whose markup was not imported.
    pub ignored: usize,
    /// Markups imported as their plain shape because the file cannot carry their payload.
    pub simplified: usize,
    /// Annotations the parser skipped as malformed (at most [`MAX_WARNINGS`] kept), set by the
    /// caller from `parse_xfdf_lenient`.
    pub warnings: Vec<String>,
}

/// Most parse warnings reported back.
pub const MAX_WARNINGS: usize = 20;

/// `existing` holds the id (as text) and the PDF-origin `/NM` of every markup in the document.
/// `page_count` is the document's page count; XFDF pages are 0-based.
pub fn import_annots(
    annots: &[XfdfAnnot],
    existing: &HashSet<String>,
    page_count: u32,
) -> XfdfImport {
    let mut out = XfdfImport::default();
    // Past the read cap nothing is looked at: say how many were left out.
    out.ignored += annots.len().saturating_sub(MAX_ANNOTS);
    let mut by_name: HashMap<String, usize> = HashMap::new(); // markup name -> index in out
    let mut replies: HashMap<String, usize> = HashMap::new(); // reply name -> markup index
    let mut skipped_names: HashSet<String> = HashSet::new();
    let mut used_ids: HashSet<Uuid> = HashSet::new();
    let mut occurrences: HashMap<String, usize> = HashMap::new();

    // Pass 1: the markups themselves.
    for a in annots.iter().take(MAX_ANNOTS) {
        if is_workflow(a) {
            continue;
        }
        let Some(declared) = type_for(a) else {
            out.ignored += 1;
            continue;
        };
        let t = plain_type(declared);
        if a.page >= page_count {
            out.ignored += 1;
            continue;
        }
        let id = annot_id(a, &mut occurrences);
        let key = id.to_string();
        let named = !a.name.is_empty();
        if existing.contains(&key)
            || (named && existing.contains(&a.name))
            || (named && (by_name.contains_key(&a.name) || skipped_names.contains(&a.name)))
            || used_ids.contains(&id)
        {
            out.skipped_existing += 1;
            if named {
                skipped_names.insert(a.name.clone());
            }
            continue;
        }
        let Some(geometry) = geometry_for(a, t) else {
            out.ignored += 1;
            continue;
        };
        used_ids.insert(id);
        let m = build(a, t, id, geometry);
        if t != declared {
            out.simplified += 1;
        }
        if !a.name.is_empty() {
            by_name.insert(a.name.clone(), out.markups.len());
        }
        out.markups.push(m);
    }

    // Pass 2: replies, then statuses (a status may sit on a reply).
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    for a in annots.iter().take(MAX_ANNOTS) {
        if !is_workflow(a) || a.state.is_some() {
            continue;
        }
        let Some(parent) = a.irt.as_deref() else {
            continue;
        };
        let (mi, to_reply) = match (by_name.get(parent), replies.get(parent)) {
            (Some(&mi), _) => (mi, None),
            (None, Some(&mi)) => (mi, Some(id_for(parent))),
            _ => {
                out.ignored += 1;
                continue;
            }
        };
        let id = annot_id(a, &mut occurrences);
        let nm = (id.to_string() != a.name && !a.name.is_empty()).then(|| a.name.clone());
        out.markups[mi].workflow.thread.push(Reply {
            id,
            author: user(&a.author),
            at: date(Some(&a.date)).unwrap_or_else(Utc::now),
            contents: a.contents.clone(),
            in_reply_to: to_reply,
            nm,
            extras: None,
        });
        if !a.name.is_empty() {
            replies.insert(a.name.clone(), mi);
        }
    }
    for a in annots.iter().take(MAX_ANNOTS) {
        if !is_workflow(a) || a.state.is_none() {
            continue;
        }
        let Some(parent) = a.irt.as_deref() else {
            continue;
        };
        let (mi, on_reply) = match (by_name.get(parent), replies.get(parent)) {
            (Some(&mi), _) => (mi, None),
            (None, Some(&mi)) => (mi, Some(id_for(parent))),
            _ => {
                out.ignored += 1;
                continue;
            }
        };
        let id = annot_id(a, &mut occurrences);
        let nm = (id.to_string() != a.name && !a.name.is_empty()).then(|| a.name.clone());
        out.markups[mi].workflow.status_history.push(StatusChange {
            id,
            author: user(&a.author),
            at: date(Some(&a.date)).unwrap_or_else(Utc::now),
            model: a
                .state_model
                .clone()
                .unwrap_or_else(|| REVIEW_MODEL.to_string()),
            state: a.state.clone().unwrap_or_default(),
            on_reply,
            nm,
            extras: None,
        });
    }
    for m in &mut out.markups {
        m.workflow.thread.sort_by_key(|r| r.at);
        m.workflow.status_history.sort_by_key(|c| c.at);
        if let Some(s) = m
            .workflow
            .latest_review_change()
            .and_then(|c| MarkupStatus::from_review_state(&c.state))
        {
            m.workflow.status = s;
        }
    }
    out
}

/// A reply or a status entry: a `<text>` element that answers another annotation.
fn is_workflow(a: &XfdfAnnot) -> bool {
    a.element == "text" && a.irt.is_some()
}

/// The markup id for an XFDF `name`: the UUID itself (redline's export), else a stable id
/// derived from the name, so the same foreign file always maps to the same ids.
fn id_for(name: &str) -> Uuid {
    Uuid::parse_str(name).unwrap_or_else(|_| derive_uuid(&format!("xfdf:{name}")))
}

/// The id for one annotation. A named one uses [`id_for`]. A nameless one (a file from a tool
/// that writes no `name`) gets an id derived from its content (element, page, rect, date,
/// author, text, parent) plus how many identical annotations came before it in the file, so
/// two nameless annotations never share an id and a re-import of the same file derives the
/// same ids (and skips them).
fn annot_id(a: &XfdfAnnot, seen: &mut HashMap<String, usize>) -> Uuid {
    if !a.name.is_empty() {
        return id_for(&a.name);
    }
    let key = format!(
        "{}|{}|{:?}|{}|{}|{}|{}|{}|{}",
        a.element,
        a.page,
        a.rect.map(f64::to_bits),
        a.date,
        a.author,
        a.contents,
        a.irt.as_deref().unwrap_or(""),
        a.state.as_deref().unwrap_or(""),
        a.subject.as_deref().unwrap_or("")
    );
    let n = seen.entry(key.clone()).or_insert(0);
    let id = derive_uuid(&format!("xfdf-nameless:{key}#{n}"));
    *n += 1;
    id
}

fn user(name: &str) -> UserRef {
    let display_name = if name.trim().is_empty() {
        "Unknown".to_string()
    } else {
        name.to_string()
    };
    UserRef {
        user_id: derive_uuid(&format!("xfdf-author:{display_name}")),
        display_name,
    }
}

fn type_for(a: &XfdfAnnot) -> Option<MarkupType> {
    if let Some(t) = a.markup_type {
        return Some(t);
    }
    use MarkupType::*;
    Some(match a.element.as_str() {
        "square" => Rectangle,
        "circle" => Ellipse,
        "line" => Line,
        "polyline" => Polyline,
        "polygon" => Polygon,
        "highlight" => Highlight,
        "underline" => Underline,
        "strikeout" => StrikeOut,
        "squiggly" => Squiggly,
        "ink" => Ink,
        "freetext" => Text,
        "text" => Note,
        "stamp" => Stamp,
        _ => return None,
    })
}

/// The shape a type comes in as when its payload (scale, table, stamp art) is not in the file.
fn plain_type(t: MarkupType) -> MarkupType {
    use MarkupType::*;
    match t {
        MeasurementLength | MeasurementRadius => Line,
        MeasurementPerimeter | MeasurementAngle => Polyline,
        MeasurementArea | MeasurementVolume => Polygon,
        MeasurementCount => Note,
        Legend => Rectangle,
        StampDynamic => Stamp,
        other => other,
    }
}

fn finite(p: &PdfPoint) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

fn geometry_finite(g: &MarkupGeometry) -> bool {
    match g {
        MarkupGeometry::Point(p) => finite(p),
        MarkupGeometry::Rect { min, max } => finite(min) && finite(max),
        MarkupGeometry::Polyline(v) => v.iter().all(finite),
        MarkupGeometry::Ink(s) => s.iter().flatten().all(finite),
        MarkupGeometry::Quads(q) => q.iter().flatten().all(finite),
    }
}

fn rect_geometry(r: &[f64; 4]) -> MarkupGeometry {
    MarkupGeometry::Rect {
        min: PdfPoint {
            x: r[0].min(r[2]),
            y: r[1].min(r[3]),
        },
        max: PdfPoint {
            x: r[0].max(r[2]),
            y: r[1].max(r[3]),
        },
    }
}

fn geometry_for(a: &XfdfAnnot, t: MarkupType) -> Option<MarkupGeometry> {
    if !a.rect.iter().all(|v| v.is_finite()) {
        return None;
    }
    if let Some(g) = a
        .rlgeom
        .as_deref()
        .and_then(|j| serde_json::from_str::<MarkupGeometry>(j).ok())
        .filter(geometry_finite)
    {
        return Some(g);
    }
    use MarkupType::*;
    let g = match t {
        Highlight | Underline | StrikeOut | Squiggly => {
            if a.coords.len() >= 8 {
                let quads = a
                    .coords
                    .chunks_exact(8)
                    .take(super::appearance::MAX_TEXT_MARKUP_QUADS)
                    .map(|c| {
                        [
                            PdfPoint { x: c[0], y: c[1] },
                            PdfPoint { x: c[2], y: c[3] },
                            PdfPoint { x: c[4], y: c[5] },
                            PdfPoint { x: c[6], y: c[7] },
                        ]
                    })
                    .collect();
                MarkupGeometry::Quads(quads)
            } else {
                rect_geometry(&a.rect)
            }
        }
        Ink => {
            if a.gestures.iter().all(|g| g.is_empty()) {
                return None;
            }
            MarkupGeometry::Ink(a.gestures.clone())
        }
        Line | Arrow => match (a.start, a.end) {
            (Some(s), Some(e)) => MarkupGeometry::Polyline(vec![s, e]),
            _ => MarkupGeometry::Polyline(vec![
                PdfPoint {
                    x: a.rect[0],
                    y: a.rect[1],
                },
                PdfPoint {
                    x: a.rect[2],
                    y: a.rect[3],
                },
            ]),
        },
        Polyline | Polygon | Cloud | Callout => {
            if a.vertices.len() < 2 {
                return None;
            }
            MarkupGeometry::Polyline(a.vertices.clone())
        }
        Note => MarkupGeometry::Point(PdfPoint {
            x: (a.rect[0] + a.rect[2]) / 2.0,
            y: (a.rect[1] + a.rect[3]) / 2.0,
        }),
        _ => rect_geometry(&a.rect),
    };
    geometry_finite(&g).then_some(g)
}

/// `#rrggbb` only; anything else (named colours, short forms, junk) falls back to `default`.
fn colour(s: Option<&str>, default: &str) -> String {
    match s {
        Some(c)
            if c.len() == 7
                && c.starts_with('#')
                && c[1..].chars().all(|h| h.is_ascii_hexdigit()) =>
        {
            c.to_string()
        }
        _ => default.to_string(),
    }
}

/// The `/F` bit field from an XFDF `flags` attribute; `print` when the attribute is absent.
fn annot_flags(flags: Option<&str>) -> i32 {
    let Some(f) = flags else { return 4 };
    f.split(',')
        .map(|n| match n.trim().to_ascii_lowercase().as_str() {
            "invisible" => 0x1,
            "hidden" => 0x2,
            "print" => 0x4,
            "nozoom" => 0x8,
            "norotate" => 0x10,
            "noview" => 0x20,
            "readonly" => 0x40,
            "locked" => 0x80,
            "togglenoview" => 0x100,
            "lockedcontents" => 0x200,
            _ => 0,
        })
        .fold(0, |acc, b| acc | b)
}

fn build(a: &XfdfAnnot, t: MarkupType, id: Uuid, geometry: MarkupGeometry) -> Markup {
    let defaults = Appearance::default();
    let appearance = Appearance {
        color: colour(a.color.as_deref(), &defaults.color),
        line_weight: a
            .width
            .filter(|w| w.is_finite() && (0.0..=1000.0).contains(w))
            .unwrap_or(defaults.line_weight),
        opacity: a
            .opacity
            .filter(|o| o.is_finite())
            .map_or(defaults.opacity, |o| o.clamp(0.0, 1.0)),
        fill: a
            .interior_color
            .as_deref()
            .map(|c| colour(Some(c), ""))
            .filter(|c| !c.is_empty()),
        ..defaults
    };
    let author = user(&a.author);
    let mut m = Markup::new(t, a.page, geometry, appearance, author);
    m.id = id;
    m.subject = a.subject.clone().filter(|s| !s.is_empty());
    m.contents = Some(a.contents.clone()).filter(|c| !c.is_empty());
    m.annot_flags = annot_flags(a.flags.as_deref());
    let modified = date(Some(&a.date));
    let created = date(a.created.as_deref()).or(modified);
    if let Some(c) = created {
        m.audit.created_at = c;
    }
    if let Some(d) = modified {
        m.audit.modified_at = d;
    }
    m
}

/// A PDF date (`D:YYYYMMDDHHmmSS` + `Z` or `+HH'mm'`); `None` when it does not parse.
fn date(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?.trim();
    let s = s.strip_prefix("D:").unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |from: usize, to: usize, default: u32| -> Option<u32> {
        if digits.len() >= to {
            digits[from..to].parse().ok()
        } else {
            Some(default)
        }
    };
    let year: i32 = digits[0..4].parse().ok()?;
    let naive = NaiveDate::from_ymd_opt(year, part(4, 6, 1)?, part(6, 8, 1)?)?.and_hms_opt(
        part(8, 10, 0)?,
        part(10, 12, 0)?,
        part(12, 14, 0)?,
    )?;
    let rest = &s[digits.len().min(14)..];
    let offset_secs: i64 = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let z: Vec<u32> = rest[1..]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.parse().ok())
                .collect();
            let (h, m) = match z.as_slice() {
                [hhmm] if *hhmm >= 100 => (hhmm / 100, hhmm % 100),
                [h] => (*h, 0),
                [h, m, ..] => (*h, *m),
                [] => (0, 0),
            };
            let secs = i64::from(h) * 3600 + i64::from(m) * 60;
            if sign == '+' {
                secs
            } else {
                -secs
            }
        }
        _ => 0,
    };
    Utc.from_utc_datetime(&naive)
        .checked_sub_signed(chrono::Duration::seconds(offset_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_dates_parse_with_and_without_a_zone() {
        let d = date(Some("D:20261114103005+04'00'")).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-11-14T06:30:05+00:00");
        let d = date(Some("D:20261114103005Z")).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-11-14T10:30:05+00:00");
        let d = date(Some("D:20261114")).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-11-14T00:00:00+00:00");
        assert!(date(Some("nonsense")).is_none());
        assert!(date(Some("D:20261340")).is_none());
        assert!(date(None).is_none());
    }

    #[test]
    fn annotations_past_the_read_cap_are_counted_as_ignored() {
        let xml = r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><square page="0" rect="0,0,1,1" name="A000000000000001"/></annots></xfdf>"#;
        let one = super::super::xfdf::parse_xfdf(xml).unwrap().remove(0);
        let all = vec![one; MAX_ANNOTS + 3];
        let got = import_annots(&all, &HashSet::new(), 1);
        assert_eq!(got.markups.len(), 1);
        assert_eq!(got.skipped_existing, MAX_ANNOTS - 1, "repeats of one name");
        assert_eq!(got.ignored, 3, "the three past the cap");
    }

    #[test]
    fn flags_map_to_bits_and_default_to_print() {
        assert_eq!(annot_flags(None), 4);
        assert_eq!(
            annot_flags(Some("print,locked,lockedcontents")),
            4 | 0x80 | 0x200
        );
        assert_eq!(annot_flags(Some("hidden, junk")), 2);
    }

    #[test]
    fn colours_must_be_hex() {
        assert_eq!(colour(Some("#FF00aa"), "#000000"), "#FF00aa");
        assert_eq!(colour(Some("red"), "#000000"), "#000000");
        assert_eq!(colour(Some("#12345"), "#000000"), "#000000");
        assert_eq!(colour(None, "#123456"), "#123456");
    }

    #[test]
    fn foreign_names_map_to_stable_ids() {
        assert_eq!(id_for("REVU0000000000A1"), id_for("REVU0000000000A1"));
        assert_ne!(id_for("REVU0000000000A1"), id_for("REVU0000000000A2"));
        let u = Uuid::new_v4();
        assert_eq!(id_for(&u.to_string()), u);
    }
}
