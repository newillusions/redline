//! XFDF export (ISO 19444-1 / Adobe XFDF 3.0): every markup of a document as one `.xfdf`
//! file Acrobat, Revu and other viewers can import as comments.
//!
//! Per markup: the element for its type (`square`, `circle`, `highlight`, `freetext`, ...),
//! `page`, `rect`, `color`, `interior-color`, `width`, `opacity`, `title` (author), `subject`,
//! `date`, `name` (the markup id) and a `<contents>` child. Shape data (`vertices`, `inklist`,
//! `coords` quads) is written where the type has it. A reply is a `<text>` element with
//! `inreplyto` = the id of what it answers and `replyType="R"`; a status is a `<text>` element
//! with `inreplyto`, `state` and `statemodel` (ISO 32000-1 §12.5.6.3), the same shape the PDF
//! itself uses. `rltype` is a private attribute carrying the exact redline type, so
//! [`parse_xfdf`] can give back the type even where several share one XFDF element.
//!
//! Text is mark-up data from PDFs and is untrusted: everything is XML-escaped, and characters
//! XML 1.0 cannot carry (most C0 controls) are replaced with U+FFFD rather than emitted.

use std::fmt::Write as _;

use super::{Markup, MarkupGeometry, MarkupStatus, MarkupType, StatusChange, REVIEW_MODEL};
use crate::geometry::PdfPoint;

const NS: &str = "http://ns.adobe.com/xfdf/";

/// The XFDF element a type is written as.
fn element(t: MarkupType) -> &'static str {
    use MarkupType::*;
    match t {
        Text | Callout => "freetext",
        Note | MeasurementCount => "text",
        Cloud | Polygon | MeasurementArea | MeasurementVolume => "polygon",
        Rectangle | Legend => "square",
        Ellipse => "circle",
        Line | Arrow | MeasurementLength | MeasurementRadius => "line",
        Polyline | MeasurementPerimeter | MeasurementAngle => "polyline",
        Highlight => "highlight",
        Ink => "ink",
        Stamp | StampDynamic => "stamp",
        Underline => "underline",
        StrikeOut => "strikeout",
        Squiggly => "squiggly",
    }
}

/// The `rect` written for a markup: its annotation bounding box, the same `/Rect` the PDF
/// writer gives it.
pub fn xfdf_rect(m: &Markup) -> [f64; 4] {
    super::appearance::ap_bbox(m)
}

/// A markup type from its `{:?}` name (the `rltype` attribute).
fn type_from_name(s: &str) -> Option<MarkupType> {
    use MarkupType::*;
    Some(match s {
        "Text" => Text,
        "Callout" => Callout,
        "Cloud" => Cloud,
        "Rectangle" => Rectangle,
        "Ellipse" => Ellipse,
        "Polygon" => Polygon,
        "Line" => Line,
        "Polyline" => Polyline,
        "Arrow" => Arrow,
        "Highlight" => Highlight,
        "Ink" => Ink,
        "Stamp" => Stamp,
        "StampDynamic" => StampDynamic,
        "Underline" => Underline,
        "StrikeOut" => StrikeOut,
        "Squiggly" => Squiggly,
        "Note" => Note,
        "MeasurementLength" => MeasurementLength,
        "MeasurementPerimeter" => MeasurementPerimeter,
        "MeasurementArea" => MeasurementArea,
        "MeasurementVolume" => MeasurementVolume,
        "MeasurementCount" => MeasurementCount,
        "MeasurementAngle" => MeasurementAngle,
        "MeasurementRadius" => MeasurementRadius,
        "Legend" => Legend,
        _ => return None,
    })
}

fn clean(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// Escape for attribute values and element text alike (newline, tab and CR as character
/// references so attribute-value normalisation cannot flatten them).
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            '\t' => o.push_str("&#9;"),
            c if clean(c) => o.push(c),
            _ => o.push('\u{FFFD}'),
        }
    }
    o
}

fn date(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("D:%Y%m%d%H%M%S+00'00'").to_string()
}

fn pt(p: &PdfPoint) -> String {
    format!("{},{}", p.x, p.y)
}

fn pts(v: &[PdfPoint]) -> String {
    v.iter().map(pt).collect::<Vec<_>>().join(";")
}

/// The XFDF `flags` attribute for a PDF `/F` bit field (ISO 32000-1 table 165; XFDF 3.0 names,
/// comma separated, in bit order). Empty for no bits set: [`open`] then leaves the attribute
/// out rather than writing an empty `flags=""`.
pub(crate) fn xfdf_flags(f: i32) -> String {
    const NAMES: [(i32, &str); 10] = [
        (0x1, "invisible"),
        (0x2, "hidden"),
        (0x4, "print"),
        (0x8, "nozoom"),
        (0x10, "norotate"),
        (0x20, "noview"),
        (0x40, "readonly"),
        (0x80, "locked"),
        (0x100, "togglenoview"),
        (0x200, "lockedcontents"),
    ];
    NAMES
        .iter()
        .filter(|(bit, _)| f & bit != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(",")
}

/// Common attributes of one annotation element.
struct Head<'a> {
    flags: String,
    tag: &'a str,
    rltype: Option<&'a str>,
    page: u32,
    rect: [f64; 4],
    name: String,
    title: &'a str,
    date: String,
    extra: String,
}

fn open(out: &mut String, h: &Head) {
    let _ = write!(
        out,
        "    <{} page=\"{}\" rect=\"{},{},{},{}\" name=\"{}\" title=\"{}\" date=\"{}\"",
        h.tag,
        h.page,
        h.rect[0],
        h.rect[1],
        h.rect[2],
        h.rect[3],
        esc(&h.name),
        esc(h.title),
        h.date
    );
    if !h.flags.is_empty() {
        let _ = write!(out, " flags=\"{}\"", h.flags);
    }
    if let Some(t) = h.rltype {
        let _ = write!(out, " rltype=\"{t}\"");
    }
    out.push_str(&h.extra);
}

fn contents(out: &mut String, text: Option<&str>, tag: &str) {
    match text {
        Some(t) if !t.is_empty() => {
            let _ = write!(
                out,
                ">\n      <contents>{}</contents>\n    </{tag}>\n",
                esc(t)
            );
        }
        _ => out.push_str("/>\n"),
    }
}

/// Serialise `markups` (every one, in the order given) to an XFDF document. `source` is the
/// PDF's file name for the `<f href>` element.
pub fn export_xfdf(markups: &[Markup], source: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(out, "<xfdf xmlns=\"{NS}\" xml:space=\"preserve\">");
    let _ = writeln!(out, "  <f href=\"{}\"/>", esc(source));
    out.push_str("  <annots>\n");
    for m in markups {
        write_markup(&mut out, m);
    }
    out.push_str("  </annots>\n</xfdf>\n");
    out
}

fn write_markup(out: &mut String, m: &Markup) {
    let tag = element(m.markup_type);
    let mut extra = String::new();
    let a = &m.appearance;
    let _ = write!(
        extra,
        " color=\"{}\" width=\"{}\" opacity=\"{}\"",
        esc(&a.color),
        a.line_weight,
        a.opacity
    );
    if let Some(f) = &a.fill {
        let _ = write!(extra, " interior-color=\"{}\"", esc(f));
    }
    if let Some(s) = &m.subject {
        let _ = write!(extra, " subject=\"{}\"", esc(s));
    }
    let _ = write!(extra, " creationdate=\"{}\"", date(&m.audit.created_at));
    match &m.geometry {
        MarkupGeometry::Quads(qs) => {
            let c = qs
                .iter()
                .flat_map(|q| q.iter().map(|p| format!("{},{}", p.x, p.y)))
                .collect::<Vec<_>>()
                .join(",");
            let _ = write!(extra, " coords=\"{c}\"");
        }
        MarkupGeometry::Polyline(v) if tag == "line" && v.len() >= 2 => {
            let _ = write!(
                extra,
                " start=\"{}\" end=\"{}\"",
                pt(&v[0]),
                pt(&v[v.len() - 1])
            );
        }
        _ => {}
    }
    // The padded `rect` and the standard attributes cannot give back a rectangle, a point or a
    // free polyline exactly, so those carry their geometry in a private attribute.
    let exact_elsewhere = match &m.geometry {
        MarkupGeometry::Polyline(v) => match tag {
            "polygon" | "polyline" => true,
            "line" => v.len() == 2,
            _ => false,
        },
        MarkupGeometry::Ink(_) | MarkupGeometry::Quads(_) => true,
        _ => false,
    };
    if !exact_elsewhere {
        if let Ok(j) = serde_json::to_string(&m.geometry) {
            let _ = write!(extra, " rlgeom=\"{}\"", esc(&j));
        }
    }
    let rltype = format!("{:?}", m.markup_type);
    let h = Head {
        flags: xfdf_flags(m.annot_flags),
        tag,
        rltype: Some(&rltype),
        page: m.page,
        rect: xfdf_rect(m),
        name: m.id().to_string(),
        title: &m.audit.created_by.display_name,
        date: date(&m.audit.modified_at),
        extra,
    };
    open(out, &h);
    // Child elements: contents plus any vertex / ink data.
    let mut kids = String::new();
    if let Some(t) = m.contents.as_deref().filter(|t| !t.is_empty()) {
        let _ = writeln!(kids, "      <contents>{}</contents>", esc(t));
    }
    match &m.geometry {
        MarkupGeometry::Polyline(v) if matches!(tag, "polygon" | "polyline") => {
            let _ = writeln!(kids, "      <vertices>{}</vertices>", pts(v));
        }
        MarkupGeometry::Ink(strokes) => {
            kids.push_str("      <inklist>\n");
            for s in strokes {
                let _ = writeln!(kids, "        <gesture>{}</gesture>", pts(s));
            }
            kids.push_str("      </inklist>\n");
        }
        _ => {}
    }
    if kids.is_empty() {
        out.push_str("/>\n");
    } else {
        let _ = write!(out, ">\n{kids}    </{tag}>\n");
    }
    write_workflow(out, m);
}

/// Replies and status entries as `<text inreplyto=...>` elements after their markup.
fn write_workflow(out: &mut String, m: &Markup) {
    let id = m.id().to_string();
    for r in &m.workflow.thread {
        let parent = r
            .in_reply_to
            .map(|p| p.to_string())
            .unwrap_or_else(|| id.clone());
        let h = Head {
            flags: "print".to_string(),
            tag: "text",
            rltype: Some("Reply"),
            page: m.page,
            rect: xfdf_rect(m),
            name: r.id.to_string(),
            title: &r.author.display_name,
            date: date(&r.at),
            extra: format!(" inreplyto=\"{}\" replyType=\"R\"", esc(&parent)),
        };
        open(out, &h);
        contents(out, Some(&r.contents), "text");
    }
    let mut history: Vec<StatusChange> = m.workflow.status_history.clone();
    let has_review = history
        .iter()
        .any(|c| c.on_reply.is_none() && c.model == REVIEW_MODEL);
    if m.workflow.status != MarkupStatus::None && !has_review {
        history.push(StatusChange {
            id: super::derive_uuid(&format!("{id}:status")),
            author: m.audit.modified_by.clone(),
            at: m.audit.modified_at,
            model: REVIEW_MODEL.into(),
            state: m.workflow.status.review_state().into(),
            on_reply: None,
            nm: None,
            extras: None,
        });
    }
    for c in &history {
        let parent = c
            .on_reply
            .map(|p| p.to_string())
            .unwrap_or_else(|| id.clone());
        let h = Head {
            flags: "print".to_string(),
            tag: "text",
            rltype: Some("Status"),
            page: m.page,
            rect: xfdf_rect(m),
            name: c.id.to_string(),
            title: &c.author.display_name,
            date: date(&c.at),
            extra: format!(
                " inreplyto=\"{}\" replyType=\"R\" statemodel=\"{}\" state=\"{}\"",
                esc(&parent),
                esc(&c.model),
                esc(&c.state)
            ),
        };
        open(out, &h);
        out.push_str("/>\n");
    }
}

/// One annotation read back from an XFDF file (the fields the export writes).
#[derive(Debug, Clone, PartialEq)]
pub struct XfdfAnnot {
    /// XFDF element name (`square`, `text`, ...).
    pub element: String,
    /// Exact redline type for a markup; `None` for a reply / status entry or a foreign file.
    pub markup_type: Option<MarkupType>,
    pub page: u32,
    pub rect: [f64; 4],
    pub color: Option<String>,
    pub interior_color: Option<String>,
    pub contents: String,
    pub subject: Option<String>,
    /// Author (`title`).
    pub author: String,
    pub date: String,
    pub name: String,
    /// `name` of the annotation this answers (`inreplyto`).
    pub irt: Option<String>,
    pub state_model: Option<String>,
    pub state: Option<String>,
    pub vertices: Vec<PdfPoint>,
    pub gestures: Vec<Vec<PdfPoint>>,
    /// Flat quad coordinates (`coords`).
    pub coords: Vec<f64>,
    /// The raw `flags` attribute (comma separated names: `print,locked,...`).
    pub flags: Option<String>,
    /// `creationdate`, a PDF date string.
    pub created: Option<String>,
    /// Stroke `width`.
    pub width: Option<f64>,
    /// Stroke `opacity`.
    pub opacity: Option<f64>,
    /// First point of a `line` (`start`).
    pub start: Option<PdfPoint>,
    /// Last point of a `line` (`end`).
    pub end: Option<PdfPoint>,
    /// Private `rlgeom`: the exact [`MarkupGeometry`] as JSON, written where `rect` (padded)
    /// and the standard attributes cannot give it back (rectangles, points, free polylines).
    pub rlgeom: Option<String>,
}

fn floats(s: &str) -> Result<Vec<f64>, String> {
    s.split([',', ';', ' '])
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<f64>()
                .map_err(|e| format!("bad number {t:?}: {e}"))
        })
        .collect()
}

fn points(s: &str) -> Result<Vec<PdfPoint>, String> {
    let v = floats(s)?;
    if v.len() % 2 != 0 {
        return Err("odd coordinate count".into());
    }
    Ok(v.chunks(2).map(|c| PdfPoint { x: c[0], y: c[1] }).collect())
}

/// One `<annots>` child as an [`XfdfAnnot`]; `Err` names what is wrong with just this one.
fn parse_annot(n: roxmltree::Node) -> Result<XfdfAnnot, String> {
    let at = |k: &str| n.attribute(k);
    let rect = floats(at("rect").ok_or("annotation without rect")?)?;
    let rect: [f64; 4] = rect.try_into().map_err(|_| "rect is not 4 numbers")?;
    let child = |tag: &str| {
        n.children()
            .find(|c| c.is_element() && c.tag_name().name() == tag)
    };
    let mut gestures = Vec::new();
    if let Some(il) = child("inklist") {
        for g in il.children().filter(|c| c.is_element()) {
            gestures.push(points(g.text().unwrap_or(""))?);
        }
    }
    Ok(XfdfAnnot {
        element: n.tag_name().name().to_string(),
        markup_type: at("rltype").and_then(type_from_name),
        page: at("page")
            .and_then(|p| p.parse().ok())
            .ok_or("annotation without page")?,
        rect,
        color: at("color").map(str::to_string),
        interior_color: at("interior-color").map(str::to_string),
        contents: child("contents")
            .and_then(|c| c.text())
            .unwrap_or("")
            .to_string(),
        subject: at("subject").map(str::to_string),
        author: at("title").unwrap_or("").to_string(),
        date: at("date").unwrap_or("").to_string(),
        name: at("name").unwrap_or("").to_string(),
        irt: at("inreplyto").map(str::to_string),
        state_model: at("statemodel").map(str::to_string),
        state: at("state").map(str::to_string),
        vertices: match child("vertices") {
            Some(v) => points(v.text().unwrap_or(""))?,
            None => Vec::new(),
        },
        gestures,
        coords: floats(at("coords").unwrap_or(""))?,
        flags: at("flags").map(str::to_string),
        created: at("creationdate").map(str::to_string),
        width: at("width").and_then(|v| v.parse().ok()),
        opacity: at("opacity").and_then(|v| v.parse().ok()),
        start: at("start")
            .map(points)
            .transpose()?
            .and_then(|v| v.first().copied()),
        end: at("end")
            .map(points)
            .transpose()?
            .and_then(|v| v.first().copied()),
        rlgeom: at("rlgeom").map(str::to_string),
    })
}

fn annots_of<'a, 'input>(
    doc: &'a roxmltree::Document<'input>,
) -> Result<roxmltree::Node<'a, 'input>, String> {
    let root = doc.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err("root element is not <xfdf>".into());
    }
    root.children()
        .find(|n| n.is_element() && n.tag_name().name() == "annots")
        .ok_or_else(|| "no <annots> element".to_string())
}

/// Parse an XFDF document back into its annotations, in file order. Strict: the first
/// malformed annotation fails the whole file.
pub fn parse_xfdf(xml: &str) -> Result<Vec<XfdfAnnot>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not valid XFDF: {e}"))?;
    annots_of(&doc)?
        .children()
        .filter(|n| n.is_element())
        .map(parse_annot)
        .collect()
}

/// [`parse_xfdf`] for import: a malformed annotation (no page, no rect, bad numbers) is skipped
/// and reported as a warning ("annotation N: ...", 1-based among the elements) instead of
/// failing the file. Only a file that is not XFDF at all is an error.
pub fn parse_xfdf_lenient(xml: &str) -> Result<(Vec<XfdfAnnot>, Vec<String>), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not valid XFDF: {e}"))?;
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    for (i, n) in annots_of(&doc)?
        .children()
        .filter(|n| n.is_element())
        .enumerate()
    {
        match parse_annot(n) {
            Ok(a) => out.push(a),
            Err(e) => warnings.push(format!("annotation {}: {e}", i + 1)),
        }
    }
    Ok((out, warnings))
}
