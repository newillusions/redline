//! True content removal for redaction.
//!
//! Rewrites a page's content stream so that nothing painted inside a redaction rectangle
//! survives in the file:
//!
//! - **Text**: each glyph whose box touches a region is deleted from its `Tj`/`TJ`/`'`/`"`
//!   operand (later glyphs keep their position through a compensating `TJ` kern). Glyph
//!   widths come from `/Widths` (simple fonts) or `/W` + `/DW` (Identity-H Type0 fonts).
//!   For fonts whose widths cannot be known (no `/Widths`, other CMaps, vertical writing)
//!   the whole show operation is dropped when its conservative extent touches a region.
//! - **Paths**: stroked segments / filled bounding boxes touching a region are dropped
//!   (clip-only paths are kept as pure clips). A fill that is a single rectangle containing
//!   the whole region is kept: a solid background reveals nothing about the region.
//! - **Images**: pixels inside a region are zeroed for 8-bit Gray/RGB raw/Flate/DCT images;
//!   other image kinds (masks, CMYK, indexed, CCITT, ...) that touch a region are dropped.
//! - **Form XObjects**: cloned per use and filtered recursively; the shared original is
//!   never edited.
//! - **Inline images** are converted to image XObjects and handled as above.
//! - `sh` (shading) operators are dropped; marked-content `/ActualText` and `/Alt` entries
//!   that enclose removed content are stripped.
//! - **Annotations** whose `/Rect` touches a region are removed from the page.
//!
//! Anything the parser cannot decode makes the whole call fail (no partial redaction).
//! The caller prunes the orphaned original streams afterwards so the removed bytes are not
//! left behind as unreferenced objects.
//!
//! Also: popups / replies of removed annotations go with them, AcroForm field entries that
//! pointed at removed widgets are pruned, the structure tree is dropped, and `finalize`
//! verifies that no removed original is still reachable (otherwise it refuses).
//!
//! Document Info, XMP and the bookmark tree are deleted; form-field values mentioning a
//! removed word are cleared; removed text that cannot be decoded (CID / Type3 fonts) together
//! with form-field values makes it refuse.

use anyhow::{anyhow, bail, Context, Result};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashMap;

type Mat = [f64; 6];
const IDENT: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
const MAX_FORM_DEPTH: usize = 8;
/// Bounds so hostile files cannot exhaust memory/CPU during redaction.
const MAX_STREAM_BYTES: usize = 256 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 100_000_000;
const MAX_OPS: u64 = 50_000_000;
/// Glyph box in em units: below the baseline / above the baseline.
const DESCENT: f64 = -0.25;
const ASCENT: f64 = 0.9;
/// Wider vertical extent used when glyph metrics are unknown.
const DESCENT_WIDE: f64 = -0.4;
const ASCENT_WIDE: f64 = 1.1;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            x0: x.min(x + w),
            y0: y.min(y + h),
            x1: x.max(x + w),
            y1: y.max(y + h),
        }
    }
    fn padded(&self, p: f64) -> Rect {
        Rect {
            x0: self.x0 - p,
            y0: self.y0 - p,
            x1: self.x1 + p,
            y1: self.y1 + p,
        }
    }
    fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    fn contains_rect(&self, o: &Rect) -> bool {
        self.x0 <= o.x0 && self.y0 <= o.y0 && self.x1 >= o.x1 && self.y1 >= o.y1
    }
}

fn mul(a: &Mat, b: &Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply(m: &Mat, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: &Mat) -> Option<Mat> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

fn scale_of(m: &Mat) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

/// Separating-axis test of a convex polygon (or segment, or point) against a rectangle.
fn poly_hits_rect(poly: &[(f64, f64)], r: &Rect) -> bool {
    let minx = poly.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let maxx = poly.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let miny = poly.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let maxy = poly.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if maxx < r.x0 || minx > r.x1 || maxy < r.y0 || miny > r.y1 {
        return false;
    }
    let rc = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)];
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let (nx, ny) = (-(q.1 - p.1), q.0 - p.0);
        if nx == 0.0 && ny == 0.0 {
            continue;
        }
        let proj = |pt: &(f64, f64)| pt.0 * nx + pt.1 * ny;
        let (pmin, pmax) = poly
            .iter()
            .map(proj)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| {
                (a.0.min(v), a.1.max(v))
            });
        let (rmin, rmax) = rc
            .iter()
            .map(proj)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| {
                (a.0.min(v), a.1.max(v))
            });
        // Tolerance scaled to the axis length so float noise does not hide a touch.
        let eps = 1e-9 * (nx.abs() + ny.abs()).max(1.0);
        if pmax < rmin - eps || pmin > rmax + eps {
            return false;
        }
    }
    true
}

fn any_hit(poly: &[(f64, f64)], rects: &[Rect]) -> bool {
    rects.iter().any(|r| poly_hits_rect(poly, r))
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(num).collect()
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

fn resolve<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

fn resolve_dict(doc: &Document, o: &Object) -> Option<Dictionary> {
    match resolve(doc, o)? {
        Object::Dictionary(d) => Some(d.clone()),
        Object::Stream(s) => Some(s.dict.clone()),
        _ => None,
    }
}

fn resolve_num(doc: &Document, o: &Object) -> Option<f64> {
    resolve(doc, o).and_then(num)
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

struct FontInfo {
    two_byte: bool,
    /// Per-code width in 1/1000 em.
    widths: HashMap<u32, f64>,
    default_w: f64,
    /// `false` when widths / code lengths cannot be trusted.
    known: bool,
    /// Type0 / Type3 / unresolvable: the show-string bytes cannot be read as Latin-1 text.
    composite: bool,
}

/// Text removed from the page, as far as it can be read back (simple fonts: Latin-1).
#[derive(Default, Clone)]
pub struct RemovedText {
    pub text: String,
    /// Some removed text came from a font whose codes cannot be decoded here.
    pub undecoded: bool,
}

fn load_font(doc: &Document, font: &Object) -> FontInfo {
    let unknown = FontInfo {
        two_byte: false,
        widths: HashMap::new(),
        default_w: 0.0,
        known: false,
        composite: false,
    };
    let Some(fd) = resolve_dict(doc, font) else {
        return unknown;
    };
    let subtype = fd
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"");
    if subtype == b"Type3" {
        // Type3 widths are in glyph space (via /FontMatrix), not 1/1000 em.
        return FontInfo {
            composite: true,
            ..unknown
        };
    }
    if subtype == b"Type0" {
        let identity_h = match fd.get(b"Encoding").ok().and_then(|o| resolve(doc, o)) {
            Some(Object::Name(n)) => n.starts_with(b"Identity") && !n.ends_with(b"V"),
            _ => false,
        };
        if !identity_h {
            return FontInfo {
                composite: true,
                ..unknown
            };
        }
        let desc = fd
            .get(b"DescendantFonts")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| resolve_dict(doc, o));
        let Some(desc) = desc else {
            return FontInfo {
                composite: true,
                ..unknown
            };
        };
        let dw = desc
            .get(b"DW")
            .ok()
            .and_then(|o| resolve_num(doc, o))
            .unwrap_or(1000.0);
        let mut widths = HashMap::new();
        if let Some(w) = desc
            .get(b"W")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_array().ok())
        {
            let mut i = 0;
            while i < w.len() {
                let Some(first) = resolve_num(doc, &w[i]) else {
                    break;
                };
                match w.get(i + 1).and_then(|o| resolve(doc, o)) {
                    Some(Object::Array(list)) => {
                        for (k, wv) in list.iter().enumerate() {
                            if let Some(v) = resolve_num(doc, wv) {
                                widths.insert(first as u32 + k as u32, v);
                            }
                        }
                        i += 2;
                    }
                    Some(o) if num(o).is_some() => {
                        let last = num(o).unwrap();
                        let Some(v) = w.get(i + 2).and_then(|o| resolve_num(doc, o)) else {
                            break;
                        };
                        // Guard against absurd ranges in hostile files.
                        if last >= first && last - first <= 70_000.0 {
                            for c in (first as u32)..=(last as u32) {
                                widths.insert(c, v);
                            }
                        }
                        i += 3;
                    }
                    _ => break,
                }
            }
        }
        return FontInfo {
            two_byte: true,
            widths,
            default_w: dw,
            known: true,
            composite: true,
        };
    }
    // Simple font.
    let first = fd
        .get(b"FirstChar")
        .ok()
        .and_then(|o| resolve_num(doc, o))
        .unwrap_or(0.0) as i64;
    let Some(wl) = fd
        .get(b"Widths")
        .ok()
        .and_then(|o| resolve(doc, o))
        .and_then(|o| o.as_array().ok())
    else {
        return unknown;
    };
    if wl.is_empty() {
        return unknown;
    }
    let mut widths = HashMap::new();
    for (k, wv) in wl.iter().enumerate() {
        if let Some(v) = resolve_num(doc, wv) {
            let code = first + k as i64;
            if code >= 0 {
                widths.insert(code as u32, v);
            }
        }
    }
    let missing = fd
        .get(b"FontDescriptor")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .and_then(|d| {
            d.get(b"MissingWidth")
                .ok()
                .and_then(|o| resolve_num(doc, o))
        })
        .unwrap_or(0.0);
    FontInfo {
        two_byte: false,
        widths,
        default_w: missing,
        known: true,
        composite: false,
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Gfx {
    ctm: Mat,
    line_width: f64,
}

struct TextState {
    tm: Mat,
    tlm: Mat,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    ts: f64,
    size: f64,
    font: Option<String>,
    /// Set after a show operation whose advance could not be tracked (unknown font): the
    /// text matrix is then only a lower bound until the next line move.
    uncertain: bool,
}

impl TextState {
    fn new() -> Self {
        TextState {
            tm: IDENT,
            tlm: IDENT,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            ts: 0.0,
            size: 0.0,
            font: None,
            uncertain: false,
        }
    }
    fn move_line(&mut self, tx: f64, ty: f64) {
        self.tlm = mul(&[1.0, 0.0, 0.0, 1.0, tx, ty], &self.tlm);
        self.tm = self.tlm;
        self.uncertain = false;
    }
    fn advance(&mut self, tx: f64) {
        self.tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &self.tm);
    }
}

#[derive(Clone, Copy)]
enum Seg {
    Line((f64, f64), (f64, f64)),
    /// Axis-aligned bound of a Bezier's control points.
    Curve(Rect),
}

#[derive(Default)]
struct PathState {
    ops: Vec<Operation>,
    segs: Vec<Seg>,
    pts: Vec<(f64, f64)>,
    cur: Option<(f64, f64)>,
    start: Option<(f64, f64)>,
    clip: bool,
    /// `Some(rect)` while the path is exactly one `re`.
    sole_rect: Option<Rect>,
    n_construct: usize,
}

impl PathState {
    fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

struct Engine<'a> {
    rects: &'a [Rect],
    removed: bool,
    next_name: usize,
    /// Original image / form objects that were replaced by a redacted clone or dropped.
    replaced: Vec<ObjectId>,
    ops_seen: u64,
    /// XObject names whose `Do` was dropped in the scope currently being filtered.
    dropped_names: Vec<String>,
    rem: RemovedText,
}

fn hits_user_poly(poly: &[(f64, f64)], rects: &[Rect], pad: f64) -> bool {
    rects.iter().any(|r| poly_hits_rect(poly, &r.padded(pad)))
}

/// Filter a page (or form) content stream. `resources` is an owned, mutable copy; new
/// XObject entries / repointed forms are written into it.
fn filter_ops(
    doc: &mut Document,
    eng: &mut Engine,
    ops: Vec<Operation>,
    resources: &mut Dictionary,
    ctm0: Mat,
    depth: usize,
) -> Result<Vec<Operation>> {
    if depth > MAX_FORM_DEPTH {
        bail!("form XObject nesting too deep to redact safely");
    }
    let rects: Vec<Rect> = eng.rects.to_vec();
    let mut out: Vec<Operation> = Vec::with_capacity(ops.len());
    let mut gstack: Vec<Gfx> = Vec::new();
    let mut g = Gfx {
        ctm: ctm0,
        line_width: 1.0,
    };
    let mut ts = TextState::new();
    let mut path = PathState::default();
    let mut mc_stack: Vec<usize> = Vec::new();
    let mut fonts: HashMap<String, FontInfo> = HashMap::new();
    let outer_dropped = std::mem::take(&mut eng.dropped_names);

    macro_rules! flush_path {
        () => {
            if !path.is_empty() {
                out.append(&mut path.ops);
            }
            path = PathState::default();
        };
    }

    for op in ops {
        eng.ops_seen += 1;
        if eng.ops_seen > MAX_OPS {
            bail!("page content too large to redact safely (operation budget exceeded)");
        }
        let o = op.operator.as_str();
        let is_path_op = matches!(o, "m" | "l" | "c" | "v" | "y" | "h" | "re" | "W" | "W*");
        let is_paint = matches!(
            o,
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n"
        );
        if !is_path_op && !is_paint && !path.is_empty() {
            flush_path!();
        }

        match o {
            // ---- graphics state ------------------------------------------------
            "q" => {
                gstack.push(g.clone());
                out.push(op);
            }
            "Q" => {
                if let Some(p) = gstack.pop() {
                    g = p;
                }
                out.push(op);
            }
            "cm" => {
                let v = nums(&op.operands);
                if v.len() == 6 {
                    g.ctm = mul(&[v[0], v[1], v[2], v[3], v[4], v[5]], &g.ctm);
                }
                out.push(op);
            }
            "w" => {
                if let Some(v) = nums(&op.operands).first() {
                    g.line_width = *v;
                }
                out.push(op);
            }
            // ---- marked content ------------------------------------------------
            "BMC" | "BDC" => {
                mc_stack.push(out.len());
                out.push(op);
            }
            "EMC" => {
                mc_stack.pop();
                out.push(op);
            }
            // ---- path construction ---------------------------------------------
            "m" | "l" | "c" | "v" | "y" | "h" | "re" => {
                let v = nums(&op.operands);
                path.n_construct += 1;
                match o {
                    "m" if v.len() >= 2 => {
                        let p = apply(&g.ctm, v[0], v[1]);
                        path.cur = Some(p);
                        path.start = Some(p);
                        path.pts.push(p);
                    }
                    "l" if v.len() >= 2 => {
                        let p = apply(&g.ctm, v[0], v[1]);
                        if let Some(c) = path.cur {
                            path.segs.push(Seg::Line(c, p));
                        }
                        path.cur = Some(p);
                        path.pts.push(p);
                    }
                    "c" | "v" | "y" if v.len() >= 4 => {
                        let chunks: Vec<(f64, f64)> = v
                            .chunks(2)
                            .filter(|c| c.len() == 2)
                            .map(|c| apply(&g.ctm, c[0], c[1]))
                            .collect();
                        let mut pts = chunks.clone();
                        if let Some(c) = path.cur {
                            pts.push(c);
                        }
                        path.segs.push(Seg::Curve(bbox(&pts)));
                        path.pts.extend_from_slice(&pts);
                        // `c` ends at the third pair, `v`/`y` at the second.
                        let end_idx = if o == "c" { 2 } else { 1 };
                        path.cur = chunks.get(end_idx).copied().or(path.cur);
                    }
                    "h" => {
                        if let (Some(c), Some(s)) = (path.cur, path.start) {
                            path.segs.push(Seg::Line(c, s));
                            path.cur = Some(s);
                        }
                    }
                    "re" if v.len() >= 4 => {
                        let corners = [
                            apply(&g.ctm, v[0], v[1]),
                            apply(&g.ctm, v[0] + v[2], v[1]),
                            apply(&g.ctm, v[0] + v[2], v[1] + v[3]),
                            apply(&g.ctm, v[0], v[1] + v[3]),
                        ];
                        for i in 0..4 {
                            path.segs.push(Seg::Line(corners[i], corners[(i + 1) % 4]));
                        }
                        path.pts.extend_from_slice(&corners);
                        path.cur = Some(corners[0]);
                        path.start = Some(corners[0]);
                        // Axis-aligned under the CTM -> remember as a plain rect.
                        let axis = (g.ctm[1].abs() < 1e-9 && g.ctm[2].abs() < 1e-9)
                            || (g.ctm[0].abs() < 1e-9 && g.ctm[3].abs() < 1e-9);
                        if axis && path.n_construct == 1 {
                            path.sole_rect = Some(bbox_rect(&corners));
                        }
                    }
                    _ => {}
                }
                path.ops.push(op);
            }
            "W" | "W*" => {
                path.clip = true;
                path.ops.push(op);
            }
            // ---- path painting -------------------------------------------------
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                let fill = matches!(o, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                let even_odd = o.ends_with('*');
                let closes = matches!(o, "s" | "b" | "b*");
                if o == "n" || path.is_empty() {
                    out.append(&mut path.ops);
                    path = PathState::default();
                    out.push(op);
                    continue;
                }
                let pad = g.line_width.abs() * scale_of(&g.ctm) / 2.0 + 0.01;
                let stroke_hit = stroke
                    && path.segs.iter().any(|s| match s {
                        Seg::Line(a, b) => hits_user_poly(&[*a, *b], &rects, pad),
                        Seg::Curve(r) => hits_user_poly(
                            &[(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)],
                            &rects,
                            pad,
                        ),
                    });
                let fill_hit = fill && {
                    let b = bbox(&path.pts);
                    let hit_any = rects.iter().any(|r| poly_hits_rect(&b_poly(&b), r));
                    let exempt = path
                        .sole_rect
                        .map(|sr| rects.iter().all(|r| sr.contains_rect(r)))
                        .unwrap_or(false);
                    hit_any && !exempt
                };
                let keep_fill = fill && !fill_hit;
                let keep_stroke = stroke && !stroke_hit;
                if fill_hit || stroke_hit {
                    eng.removed = true;
                    mark_removed(doc, &mut out, &mc_stack, resources);
                }
                if keep_fill || keep_stroke {
                    let new_op = match (keep_fill, keep_stroke, closes, even_odd) {
                        (true, true, false, false) => "B",
                        (true, true, false, true) => "B*",
                        (true, true, true, false) => "b",
                        (true, true, true, true) => "b*",
                        (true, false, _, false) => "f",
                        (true, false, _, true) => "f*",
                        (false, true, false, _) => "S",
                        (false, true, true, _) => "s",
                        _ => unreachable!(),
                    };
                    out.append(&mut path.ops);
                    out.push(Operation::new(new_op, vec![]));
                } else if path.clip {
                    // Painting is gone but the clip must still take effect.
                    out.append(&mut path.ops);
                    out.push(Operation::new("n", vec![]));
                }
                path = PathState::default();
            }
            "sh" => {
                // Shading extent is unknown without parsing the shading dictionary.
                eng.removed = true;
                mark_removed(doc, &mut out, &mc_stack, resources);
            }
            // ---- text state ----------------------------------------------------
            "BT" => {
                ts.tm = IDENT;
                ts.tlm = IDENT;
                ts.uncertain = false;
                out.push(op);
            }
            "Tc" => {
                ts.tc = nums(&op.operands).first().copied().unwrap_or(0.0);
                out.push(op);
            }
            "Tw" => {
                ts.tw = nums(&op.operands).first().copied().unwrap_or(0.0);
                out.push(op);
            }
            "Tz" => {
                ts.th = nums(&op.operands).first().copied().unwrap_or(100.0) / 100.0;
                out.push(op);
            }
            "TL" => {
                ts.tl = nums(&op.operands).first().copied().unwrap_or(0.0);
                out.push(op);
            }
            "Ts" => {
                ts.ts = nums(&op.operands).first().copied().unwrap_or(0.0);
                out.push(op);
            }
            "Tf" => {
                if let Some(Object::Name(n)) = op.operands.first() {
                    ts.font = Some(String::from_utf8_lossy(n).to_string());
                }
                ts.size = op.operands.get(1).and_then(num).unwrap_or(0.0);
                out.push(op);
            }
            "Td" => {
                let v = nums(&op.operands);
                if v.len() >= 2 {
                    ts.move_line(v[0], v[1]);
                }
                out.push(op);
            }
            "TD" => {
                let v = nums(&op.operands);
                if v.len() >= 2 {
                    ts.tl = -v[1];
                    ts.move_line(v[0], v[1]);
                }
                out.push(op);
            }
            "Tm" => {
                let v = nums(&op.operands);
                if v.len() == 6 {
                    ts.tm = [v[0], v[1], v[2], v[3], v[4], v[5]];
                    ts.tlm = ts.tm;
                    ts.uncertain = false;
                }
                out.push(op);
            }
            "T*" => {
                let tl = ts.tl;
                ts.move_line(0.0, -tl);
                out.push(op);
            }
            // ---- text showing --------------------------------------------------
            "Tj" | "TJ" | "'" | "\"" => {
                // Pre-ops carried by ' and ".
                let mut pre: Vec<Operation> = Vec::new();
                if o == "'" || o == "\"" {
                    if o == "\"" {
                        let v = nums(&op.operands);
                        if v.len() >= 2 {
                            ts.tw = v[0];
                            ts.tc = v[1];
                            pre.push(Operation::new("Tw", vec![real(v[0])]));
                            pre.push(Operation::new("Tc", vec![real(v[1])]));
                        }
                    }
                    let tl = ts.tl;
                    ts.move_line(0.0, -tl);
                    pre.push(Operation::new("T*", vec![]));
                }
                let items: Vec<Object> = match (o, op.operands.last()) {
                    ("TJ", Some(Object::Array(a))) => a.clone(),
                    (_, Some(s @ Object::String(..))) => vec![s.clone()],
                    _ => {
                        out.push(op);
                        continue;
                    }
                };
                let font_name = ts.font.clone().unwrap_or_default();
                if !fonts.contains_key(&font_name) {
                    let info = resources
                        .get(b"Font")
                        .ok()
                        .and_then(|f| resolve_dict(doc, f))
                        .and_then(|fd| fd.get(font_name.as_bytes()).ok().cloned())
                        .map(|fo| load_font(doc, &fo))
                        .unwrap_or(FontInfo {
                            two_byte: false,
                            widths: HashMap::new(),
                            default_w: 0.0,
                            known: false,
                            composite: true,
                        });
                    fonts.insert(font_name.clone(), info);
                }
                let font = &fonts[&font_name];
                let (new_items, changed) =
                    process_show(&items, font, &mut ts, &g, &rects, &mut eng.rem);
                if changed {
                    eng.removed = true;
                    mark_removed(doc, &mut out, &mc_stack, resources);
                    out.extend(pre);
                    out.push(Operation::new("TJ", vec![Object::Array(new_items)]));
                } else {
                    out.push(op);
                }
            }
            // ---- XObjects ------------------------------------------------------
            "Do" => {
                let Some(Object::Name(name)) = op.operands.first().cloned() else {
                    out.push(op);
                    continue;
                };
                let name_s = String::from_utf8_lossy(&name).to_string();
                handle_do(
                    doc, eng, &mut out, &mc_stack, resources, &g, &name_s, op, depth,
                )?;
            }
            "BI" => {
                let Some(Object::Stream(s)) = op.operands.first() else {
                    // Unparseable inline image was already skipped by the parser.
                    eng.removed = true;
                    continue;
                };
                let img = inline_to_xobject(s);
                let id = doc.add_object(Object::Stream(img));
                eng.next_name += 1;
                let name = format!("RLInl{}", eng.next_name);
                set_xobject(resources, doc, &name, id);
                handle_do(
                    doc,
                    eng,
                    &mut out,
                    &mc_stack,
                    resources,
                    &g,
                    &name,
                    Operation::new("Do", vec![Object::Name(name.clone().into_bytes())]),
                    depth,
                )?;
            }
            _ => out.push(op),
        }
    }
    out.append(&mut path.ops);
    // A dropped image `Do` must not leave its original reachable through /Resources: remove
    // the entry unless a surviving `Do` still uses that name.
    let dropped = std::mem::replace(&mut eng.dropped_names, outer_dropped);
    for name in dropped {
        let still_used = out.iter().any(|op| {
            op.operator == "Do"
                && matches!(op.operands.first(), Some(Object::Name(n)) if n == name.as_bytes())
        });
        if !still_used {
            if let Ok(Object::Dictionary(xo)) = resources.get_mut(b"XObject") {
                xo.remove(name.as_bytes());
            }
        }
    }
    Ok(out)
}

fn bbox(pts: &[(f64, f64)]) -> Rect {
    bbox_rect(pts)
}

fn bbox_rect(pts: &[(f64, f64)]) -> Rect {
    let mut r = Rect {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };
    for p in pts {
        r.x0 = r.x0.min(p.0);
        r.y0 = r.y0.min(p.1);
        r.x1 = r.x1.max(p.0);
        r.y1 = r.y1.max(p.1);
    }
    r
}

fn b_poly(r: &Rect) -> [(f64, f64); 4] {
    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
}

/// Strip `/ActualText` and `/Alt` from every enclosing marked-content property list.
fn mark_removed(doc: &Document, out: &mut [Operation], mc_stack: &[usize], resources: &Dictionary) {
    for &idx in mc_stack {
        let Some(op) = out.get_mut(idx) else { continue };
        if op.operator != "BDC" || op.operands.len() < 2 {
            continue;
        }
        let dict = match &op.operands[1] {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Name(n) => resources
                .get(b"Properties")
                .ok()
                .and_then(|p| resolve_dict(doc, p))
                .and_then(|p| p.get(n).ok().and_then(|o| resolve_dict(doc, o))),
            _ => None,
        };
        if let Some(mut d) = dict {
            let had = d.has(b"ActualText") || d.has(b"Alt") || d.has(b"E");
            d.remove(b"ActualText");
            d.remove(b"Alt");
            d.remove(b"E");
            if had || matches!(op.operands[1], Object::Dictionary(_)) {
                op.operands[1] = Object::Dictionary(d);
            }
        }
    }
}

/// Walk a show operation. Returns the (possibly rewritten) TJ array and whether any glyph
/// was removed; always advances the text matrix as the original operation would.
fn process_show(
    items: &[Object],
    font: &FontInfo,
    ts: &mut TextState,
    g: &Gfx,
    rects: &[Rect],
    rem: &mut RemovedText,
) -> (Vec<Object>, bool) {
    let fs = ts.size;
    let mut changed = false;
    let mut out: Vec<Object> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut pending_adjust = 0.0_f64;

    let flush_buf = |out: &mut Vec<Object>, buf: &mut Vec<u8>, adj: &mut f64| {
        if !buf.is_empty() {
            if adj.abs() > 1e-9 {
                out.push(real(*adj));
                *adj = 0.0;
            }
            out.push(Object::String(
                std::mem::take(buf),
                lopdf::StringFormat::Literal,
            ));
        }
    };

    if !font.known || ts.uncertain {
        // Conservative: the extent runs from far left to far right of the origin along the
        // baseline (widths or the running position are not trustworthy).
        ts.uncertain = true;
        let trm = mul(&ts.tm, &g.ctm);
        let far = 1.0e6;
        let quad = [
            apply(&trm, -far, DESCENT_WIDE * fs + ts.ts),
            apply(&trm, far, DESCENT_WIDE * fs + ts.ts),
            apply(&trm, far, ASCENT_WIDE * fs + ts.ts),
            apply(&trm, -far, ASCENT_WIDE * fs + ts.ts),
        ];
        if any_hit(&quad, rects) {
            if font.composite {
                rem.undecoded = true;
            } else {
                for it in items {
                    if let Object::String(b, _) = it {
                        rem.text.extend(b.iter().map(|c| *c as char));
                        rem.text.push(' ');
                    }
                }
            }
            return (Vec::new(), true);
        }
        return (items.to_vec(), false);
    }

    for item in items {
        match item {
            Object::Integer(_) | Object::Real(_) => {
                let n = num(item).unwrap_or(0.0);
                ts.advance(-n / 1000.0 * fs * ts.th);
                pending_adjust += n;
            }
            Object::String(bytes, _) => {
                let step = if font.two_byte { 2 } else { 1 };
                for chunk in bytes.chunks(step) {
                    let code: u32 = chunk.iter().fold(0, |a, b| (a << 8) | *b as u32);
                    let w1000 = *font.widths.get(&code).unwrap_or(&font.default_w);
                    let w0 = w1000 / 1000.0;
                    let is_space = !font.two_byte && code == 32;
                    let tx = (w0 * fs + ts.tc + if is_space { ts.tw } else { 0.0 }) * ts.th;
                    let trm = mul(&ts.tm, &g.ctm);
                    let wbox = w0 * fs * ts.th;
                    let quad = [
                        apply(&trm, 0.0, DESCENT * fs + ts.ts),
                        apply(&trm, wbox, DESCENT * fs + ts.ts),
                        apply(&trm, wbox, ASCENT * fs + ts.ts),
                        apply(&trm, 0.0, ASCENT * fs + ts.ts),
                    ];
                    if any_hit(&quad, rects) {
                        changed = true;
                        if font.composite {
                            rem.undecoded = true;
                        } else {
                            rem.text.push(chunk[0] as char);
                        }
                        flush_buf(&mut out, &mut buf, &mut pending_adjust);
                        // Replace the glyph by the shift it would have caused.
                        if fs.abs() > 1e-9 && ts.th.abs() > 1e-9 {
                            pending_adjust += -(tx / (fs * ts.th)) * 1000.0;
                        }
                    } else {
                        if pending_adjust.abs() > 1e-9 && buf.is_empty() {
                            out.push(real(pending_adjust));
                            pending_adjust = 0.0;
                        }
                        buf.extend_from_slice(chunk);
                    }
                    ts.advance(tx);
                }
            }
            _ => {}
        }
    }
    if !buf.is_empty() {
        out.push(Object::String(buf, lopdf::StringFormat::Literal));
    }
    if pending_adjust.abs() > 1e-9 {
        out.push(real(pending_adjust));
    }
    (out, changed)
}

// ---------------------------------------------------------------------------
// XObjects
// ---------------------------------------------------------------------------

fn set_xobject(resources: &mut Dictionary, doc: &Document, name: &str, id: ObjectId) {
    let mut xo = resources
        .get(b"XObject")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or_default();
    xo.set(name.as_bytes().to_vec(), Object::Reference(id));
    resources.set("XObject", Object::Dictionary(xo));
}

#[allow(clippy::too_many_arguments)]
fn handle_do(
    doc: &mut Document,
    eng: &mut Engine,
    out: &mut Vec<Operation>,
    mc_stack: &[usize],
    resources: &mut Dictionary,
    g: &Gfx,
    name: &str,
    op: Operation,
    depth: usize,
) -> Result<()> {
    let rects: Vec<Rect> = eng.rects.to_vec();
    let xid = resources
        .get(b"XObject")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .and_then(|d| d.get(name.as_bytes()).ok().cloned());
    let Some(Object::Reference(id)) = xid else {
        out.push(op);
        return Ok(());
    };
    let Ok(Object::Stream(st)) = doc.get_object(id).cloned() else {
        out.push(op);
        return Ok(());
    };
    let subtype = st
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"")
        .to_vec();
    if subtype == b"Image" {
        let quad = [
            apply(&g.ctm, 0.0, 0.0),
            apply(&g.ctm, 1.0, 0.0),
            apply(&g.ctm, 1.0, 1.0),
            apply(&g.ctm, 0.0, 1.0),
        ];
        if !any_hit(&quad, &rects) {
            out.push(op);
            return Ok(());
        }
        eng.removed = true;
        eng.replaced.push(id);
        mark_removed(doc, out, mc_stack, resources);
        if let Some(new_stream) = blacken_image(doc, &st, &g.ctm, &rects) {
            let nid = doc.add_object(Object::Stream(new_stream));
            set_xobject(resources, doc, name, nid);
            out.push(op);
        } else {
            eng.dropped_names.push(name.to_string());
        }
        return Ok(());
    }
    if subtype == b"Form" {
        let fm = st
            .dict
            .get(b"Matrix")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(num).collect::<Vec<_>>())
            .filter(|v| v.len() == 6)
            .map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]])
            .unwrap_or(IDENT);
        let m = mul(&fm, &g.ctm);
        let bb: Vec<f64> = st
            .dict
            .get(b"BBox")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|o| resolve_num(doc, o)).collect())
            .unwrap_or_default();
        let hit = if bb.len() == 4 {
            let quad = [
                apply(&m, bb[0], bb[1]),
                apply(&m, bb[2], bb[1]),
                apply(&m, bb[2], bb[3]),
                apply(&m, bb[0], bb[3]),
            ];
            any_hit(&quad, &rects)
        } else {
            true
        };
        if !hit {
            out.push(op);
            return Ok(());
        }
        let data = st
            .decompressed_content_with_limit(MAX_STREAM_BYTES)
            .map_err(|e| anyhow!("cannot decode form XObject /{name}: {e}"))?;
        let content = Content::decode(&data)
            .map_err(|e| anyhow!("cannot parse form XObject /{name}: {e}"))?;
        let mut fres = st
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|o| resolve_dict(doc, o))
            .unwrap_or_else(|| resources.clone());
        let before = eng.removed;
        eng.removed = false;
        let new_ops = filter_ops(doc, eng, content.operations, &mut fres, m, depth + 1)?;
        let form_changed = eng.removed;
        eng.removed = before || form_changed;
        if !form_changed {
            out.push(op);
            return Ok(());
        }
        mark_removed(doc, out, mc_stack, resources);
        eng.replaced.push(id);
        let mut nd = st.dict.clone();
        for k in [&b"PieceInfo"[..], b"Metadata", b"Thumb", b"OC"] {
            nd.remove(k);
        }
        nd.remove(b"Filter");
        nd.remove(b"DecodeParms");
        nd.remove(b"Length");
        nd.set("Resources", Object::Dictionary(fres));
        let bytes = Content {
            operations: new_ops,
        }
        .encode()
        .map_err(|e| anyhow!("cannot encode filtered form: {e}"))?;
        let mut ns = Stream::new(nd, bytes);
        let _ = ns.compress();
        let nid = doc.add_object(Object::Stream(ns));
        set_xobject(resources, doc, name, nid);
        out.push(op);
        return Ok(());
    }
    out.push(op);
    Ok(())
}

/// Zero the pixels of `st` that fall inside `rects`. `None` = unsupported image kind.
fn blacken_image(doc: &Document, st: &Stream, ctm: &Mat, rects: &[Rect]) -> Option<Stream> {
    let d = &st.dict;
    if d.get(b"ImageMask")
        .ok()
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false)
    {
        return None;
    }
    let w = resolve_num(doc, d.get(b"Width").ok()?)? as usize;
    let h = resolve_num(doc, d.get(b"Height").ok()?)? as usize;
    let bpc = d
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| resolve_num(doc, o))
        .unwrap_or(8.0) as u32;
    if bpc != 8 || w == 0 || h == 0 || (w as u64).saturating_mul(h as u64) > MAX_IMAGE_PIXELS {
        return None;
    }
    if d.has(b"Decode") {
        return None;
    }
    let comps = match resolve(doc, d.get(b"ColorSpace").ok()?)? {
        Object::Name(n) if n == b"DeviceGray" => 1,
        Object::Name(n) if n == b"DeviceRGB" => 3,
        _ => return None,
    };
    let filters: Vec<Vec<u8>> = match d.get(b"Filter").ok().and_then(|o| resolve(doc, o)) {
        None => vec![],
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(|o| {
                resolve(doc, o)
                    .and_then(|o| o.as_name().ok())
                    .map(|n| n.to_vec())
            })
            .collect(),
        _ => return None,
    };
    if let Some(Object::Dictionary(dp)) = d.get(b"DecodeParms").ok().and_then(|o| resolve(doc, o)) {
        if dp.get(b"Predictor").ok().and_then(num).unwrap_or(1.0) > 1.0 {
            return None;
        }
    } else if d.has(b"DecodeParms") {
        return None;
    }
    let mut raw: Vec<u8>;
    let mut out_comps = comps;
    match filters.as_slice() {
        [] => raw = st.content.clone(),
        [f] if f == b"FlateDecode" => {
            raw = st.decompressed_content_with_limit(MAX_STREAM_BYTES).ok()?
        }
        [f] if f == b"DCTDecode" => {
            let mut reader = image::ImageReader::with_format(
                std::io::Cursor::new(&st.content),
                image::ImageFormat::Jpeg,
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(w as u32);
            limits.max_image_height = Some(h as u32);
            limits.max_alloc = Some(MAX_STREAM_BYTES as u64);
            reader.limits(limits);
            let img = reader.decode().ok()?;
            if comps == 1 {
                raw = img.to_luma8().into_raw();
            } else {
                raw = img.to_rgb8().into_raw();
            }
            out_comps = comps;
        }
        _ => return None,
    }
    if raw.len() < w * h * out_comps {
        return None;
    }
    let inv = invert(ctm)?;
    // Pixel window covering all regions.
    let (mut pu0, mut pv0, mut pu1, mut pv1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for r in rects {
        for (x, y) in [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)] {
            let (u, v) = apply(&inv, x, y);
            pu0 = pu0.min(u);
            pu1 = pu1.max(u);
            pv0 = pv0.min(v);
            pv1 = pv1.max(v);
        }
    }
    let col0 = ((pu0 * w as f64).floor() as i64 - 1).clamp(0, w as i64 - 1) as usize;
    let col1 = ((pu1 * w as f64).ceil() as i64 + 1).clamp(0, w as i64 - 1) as usize;
    // Row 0 is the top (v = 1).
    let row0 = (((1.0 - pv1) * h as f64).floor() as i64 - 1).clamp(0, h as i64 - 1) as usize;
    let row1 = (((1.0 - pv0) * h as f64).ceil() as i64 + 1).clamp(0, h as i64 - 1) as usize;
    for row in row0..=row1 {
        for col in col0..=col1 {
            // A pixel is removed when its whole footprint could touch a region: test the
            // four corners and the centre.
            let mut touched = false;
            for (du, dv) in [(0.5, 0.5), (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let u = (col as f64 + du) / w as f64;
                let v = 1.0 - (row as f64 + dv) / h as f64;
                let (x, y) = apply(ctm, u, v);
                if rects.iter().any(|r| r.contains_point(x, y)) {
                    touched = true;
                    break;
                }
            }
            if touched {
                let p = (row * w + col) * out_comps;
                for b in &mut raw[p..p + out_comps] {
                    *b = 0;
                }
            }
        }
    }
    let mut nd = d.clone();
    for k in [
        &b"Alternates"[..],
        b"OPI",
        b"Thumb",
        b"Metadata",
        b"PieceInfo",
    ] {
        nd.remove(k);
    }
    nd.remove(b"Filter");
    nd.remove(b"DecodeParms");
    nd.remove(b"Length");
    nd.set(
        "ColorSpace",
        if out_comps == 1 {
            "DeviceGray"
        } else {
            "DeviceRGB"
        },
    );
    nd.set("BitsPerComponent", 8_i64);
    let mut ns = Stream::new(nd, raw);
    let _ = ns.compress();
    Some(ns)
}

/// Expand an inline image's abbreviated keys / names into a full image XObject.
fn inline_to_xobject(s: &Stream) -> Stream {
    let key = |k: &[u8]| -> Vec<u8> {
        match k {
            b"BPC" => b"BitsPerComponent".to_vec(),
            b"CS" => b"ColorSpace".to_vec(),
            b"D" => b"Decode".to_vec(),
            b"DP" => b"DecodeParms".to_vec(),
            b"F" => b"Filter".to_vec(),
            b"H" => b"Height".to_vec(),
            b"IM" => b"ImageMask".to_vec(),
            b"I" => b"Interpolate".to_vec(),
            b"W" => b"Width".to_vec(),
            other => other.to_vec(),
        }
    };
    fn val(o: &Object) -> Object {
        match o {
            Object::Name(n) => Object::Name(
                match n.as_slice() {
                    b"G" => &b"DeviceGray"[..],
                    b"RGB" => b"DeviceRGB",
                    b"CMYK" => b"DeviceCMYK",
                    b"I" => b"Indexed",
                    b"AHx" => b"ASCIIHexDecode",
                    b"A85" => b"ASCII85Decode",
                    b"LZW" => b"LZWDecode",
                    b"Fl" => b"FlateDecode",
                    b"RL" => b"RunLengthDecode",
                    b"CCF" => b"CCITTFaxDecode",
                    b"DCT" => b"DCTDecode",
                    other => other,
                }
                .to_vec(),
            ),
            Object::Array(a) => Object::Array(a.iter().map(val).collect()),
            other => other.clone(),
        }
    }
    let mut d = Dictionary::new();
    for (k, v) in s.dict.iter() {
        d.set(key(k), val(v));
    }
    d.set("Type", "XObject");
    d.set("Subtype", "Image");
    d.remove(b"Length");
    Stream::new(d, s.content.clone())
}

// ---------------------------------------------------------------------------
// Page entry point
// ---------------------------------------------------------------------------

fn effective_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    let mut cur = Some(page_id);
    let mut hops = 0;
    while let Some(id) = cur {
        hops += 1;
        if hops > 64 {
            break;
        }
        let Ok(d) = doc.get_dictionary(id) else { break };
        if let Ok(r) = d.get(b"Resources") {
            if let Some(rd) = resolve_dict(doc, r) {
                return rd;
            }
        }
        cur = d.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    Dictionary::new()
}

/// What one page's removal did, for [`finalize`].
#[derive(Default)]
pub struct StripReport {
    pub page: Option<ObjectId>,
    /// Content (text, paths, images, forms) or annotations were actually removed.
    pub changed: bool,
    /// Original image / form objects that were replaced or dropped.
    pub replaced: Vec<ObjectId>,
    /// Annotation objects removed from the page.
    pub removed_annots: Vec<ObjectId>,
    /// The page's original content streams.
    pub old_contents: Vec<ObjectId>,
    /// Text read back from what was removed.
    pub removed: RemovedText,
}

fn rect_of(doc: &Document, d: &Dictionary) -> Option<Rect> {
    let r = d
        .get(b"Rect")
        .ok()
        .and_then(|o| resolve(doc, o))
        .and_then(|o| o.as_array().ok())?;
    let v: Vec<f64> = r.iter().filter_map(|o| resolve_num(doc, o)).collect();
    if v.len() < 4 {
        return None;
    }
    Some(Rect::new(v[0], v[1], v[2] - v[0], v[3] - v[1]))
}

/// Remove everything painted inside `rects` (page user space) from the page. Fails without
/// modifying the page if any of its content cannot be decoded.
pub fn strip_page_regions(
    doc: &mut Document,
    page_id: ObjectId,
    rects: &[Rect],
) -> Result<StripReport> {
    let mut report = StripReport {
        page: Some(page_id),
        ..Default::default()
    };
    if rects.is_empty() {
        return Ok(report);
    }
    // ---- annotations: those under a region, plus their popups and replies -----------
    let annots: Vec<Object> = {
        let page = doc.get_dictionary(page_id).context("page dict")?;
        match page.get(b"Annots").ok().and_then(|o| resolve(doc, o)) {
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        }
    };
    let mut gone: std::collections::HashSet<ObjectId> = std::collections::HashSet::new();
    for a in &annots {
        if let Object::Reference(id) = a {
            if let Some(d) = resolve_dict(doc, a) {
                if rect_of(doc, &d).is_some_and(|r| any_hit(&b_poly(&r), rects)) {
                    gone.insert(*id);
                }
            }
        }
    }
    // Fixpoint: popups of removed annotations, and anything whose /Parent or /IRT is removed.
    loop {
        let before = gone.len();
        for a in &annots {
            let Object::Reference(id) = a else { continue };
            let Some(d) = resolve_dict(doc, a) else {
                continue;
            };
            let links_to_gone = [&b"Parent"[..], b"IRT"]
                .iter()
                .any(|k| matches!(d.get(k), Ok(Object::Reference(r)) if gone.contains(r)));
            if links_to_gone {
                gone.insert(*id);
            }
            if gone.contains(id) {
                if let Ok(Object::Reference(p)) = d.get(b"Popup") {
                    gone.insert(*p);
                }
            }
        }
        if gone.len() == before {
            break;
        }
    }
    let kept: Vec<Object> = annots
        .iter()
        .filter(|a| !matches!(a, Object::Reference(id) if gone.contains(id)))
        .cloned()
        .collect();
    report.removed_annots = gone.iter().copied().collect();

    // ---- content ---------------------------------------------------------------
    let old_ids = doc.get_page_contents(page_id);
    let raw = {
        let mut buf = Vec::new();
        for cid in &old_ids {
            let s = doc
                .get_object(*cid)
                .and_then(Object::as_stream)
                .map_err(|e| anyhow!("page content stream unreadable: {e}"))?;
            let remaining = MAX_STREAM_BYTES.saturating_sub(buf.len());
            let data = s
                .decompressed_content_with_limit(remaining)
                .map_err(|e| anyhow!("page content stream cannot be decoded for redaction: {e}"))?;
            buf.extend_from_slice(&data);
            buf.push(b'\n');
        }
        buf
    };
    let mut resources = effective_resources(doc, page_id);
    let mut eng = Engine {
        rects,
        removed: false,
        next_name: 0,
        replaced: Vec::new(),
        ops_seen: 0,
        dropped_names: Vec::new(),
        rem: RemovedText::default(),
    };
    let new_ops = if raw.iter().all(|b| b.is_ascii_whitespace()) {
        None
    } else {
        let content = Content::decode(&raw)
            .map_err(|e| anyhow!("page content cannot be parsed for redaction: {e}"))?;
        Some(filter_ops(
            doc,
            &mut eng,
            content.operations,
            &mut resources,
            IDENT,
            0,
        )?)
    };

    // ---- commit ----------------------------------------------------------------
    if let Some(ops) = new_ops {
        let bytes = Content { operations: ops }
            .encode()
            .map_err(|e| anyhow!("cannot encode redacted content: {e}"))?;
        let mut s = Stream::new(Dictionary::new(), bytes);
        let _ = s.compress();
        let nid = doc.add_object(Object::Stream(s));
        let page = doc.get_dictionary_mut(page_id).context("page dict mut")?;
        page.set("Contents", Object::Reference(nid));
        page.set("Resources", Object::Dictionary(resources));
        for k in [&b"Thumb"[..], b"PieceInfo", b"Metadata"] {
            page.remove(k);
        }
        for oid in &old_ids {
            doc.objects.remove(oid);
        }
        report.old_contents = old_ids;
    }
    let page = doc.get_dictionary_mut(page_id).context("page dict mut")?;
    if kept.len() != annots.len() {
        if kept.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(kept));
        }
    }
    report.changed = eng.removed || !gone.is_empty();
    report.removed = eng.rem.clone();
    // Text inside removed annotations (their /Contents, /Subj, /T) counts as removed text.
    for a in &annots {
        if let Object::Reference(id) = a {
            if gone.contains(id) {
                if let Some(d) = resolve_dict(doc, a) {
                    for k in [&b"Contents"[..], b"Subj", b"T", b"RC"] {
                        if let Ok(o) = d.get(k) {
                            if let Some(t) = string_of(doc, o) {
                                report.removed.text.push(' ');
                                report.removed.text.push_str(&t);
                            }
                        }
                    }
                }
            }
        }
    }
    report.replaced = eng.replaced;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Document-level cleanup + verification
// ---------------------------------------------------------------------------

fn xobject_ids_in(
    doc: &Document,
    resources: &Dictionary,
    names: &std::collections::HashSet<Vec<u8>>,
    out: &mut std::collections::HashSet<ObjectId>,
) {
    let Some(xo) = resources
        .get(b"XObject")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
    else {
        return;
    };
    for n in names {
        if let Ok(Object::Reference(id)) = xo.get(n) {
            out.insert(*id);
        }
    }
}

/// Collect XObject ids used (via `Do`) by `ops`, recursing into forms.
fn collect_used(
    doc: &Document,
    ops: &[Operation],
    resources: &Dictionary,
    used: &mut std::collections::HashSet<ObjectId>,
    depth: usize,
    budget: &mut u64,
) {
    if depth > MAX_FORM_DEPTH {
        return;
    }
    let mut names: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    for op in ops {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if op.operator == "Do" {
            if let Some(Object::Name(n)) = op.operands.first() {
                names.insert(n.clone());
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    xobject_ids_in(doc, resources, &names, &mut ids);
    for id in ids {
        if !used.insert(id) {
            continue;
        }
        if let Ok(Object::Stream(s)) = doc.get_object(id) {
            let is_form = matches!(s.dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Form");
            if is_form {
                if let Ok(data) = s.decompressed_content_with_limit(MAX_STREAM_BYTES) {
                    if let Ok(c) = Content::decode(&data) {
                        let fres = s
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|o| resolve_dict(doc, o))
                            .unwrap_or_else(|| resources.clone());
                        collect_used(doc, &c.operations, &fres, used, depth + 1, budget);
                    }
                }
            }
        }
    }
}

/// XObjects still painted by pages other than `skip`, or by any annotation appearance.
fn used_elsewhere(
    doc: &Document,
    skip: &std::collections::HashSet<ObjectId>,
) -> std::collections::HashSet<ObjectId> {
    let mut used = std::collections::HashSet::new();
    let mut budget = MAX_OPS;
    for (_, page_id) in doc.get_pages() {
        if skip.contains(&page_id) {
            continue;
        }
        let res = effective_resources(doc, page_id);
        if let Ok(raw) = doc.get_page_content_with_limit(page_id, MAX_STREAM_BYTES) {
            if let Ok(c) = Content::decode(&raw) {
                collect_used(doc, &c.operations, &res, &mut used, 0, &mut budget);
            }
        }
    }
    // Annotation appearance streams (and anything they paint) count as uses too.
    let ap_ids: Vec<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            Object::Dictionary(d) if matches!(d.get(b"Subtype"), Ok(Object::Name(_))) => {
                d.get(b"AP").ok().and_then(|o| resolve_dict(doc, o))
            }
            _ => None,
        })
        .flat_map(|ap| {
            let mut v = Vec::new();
            if let Ok(n) = ap.get(b"N") {
                match n {
                    Object::Reference(r) => v.push(*r),
                    Object::Dictionary(d) => {
                        v.extend(d.iter().filter_map(|(_, o)| o.as_reference().ok()))
                    }
                    _ => {}
                }
            }
            v
        })
        .collect();
    for id in ap_ids {
        used.insert(id);
        if let Ok(Object::Stream(s)) = doc.get_object(id) {
            if let Ok(data) = s.decompressed_content_with_limit(MAX_STREAM_BYTES) {
                if let Ok(c) = Content::decode(&data) {
                    let fres = s
                        .dict
                        .get(b"Resources")
                        .ok()
                        .and_then(|o| resolve_dict(doc, o))
                        .unwrap_or_default();
                    collect_used(doc, &c.operations, &fres, &mut used, 1, &mut budget);
                }
            }
        }
    }
    used
}

/// Remove references to `gone` from an AcroForm field tree. A field left with no kids
/// (its widgets all removed) is dropped with its value.
fn prune_fields(
    doc: &mut Document,
    items: Vec<Object>,
    gone: &std::collections::HashSet<ObjectId>,
    depth: usize,
) -> Vec<Object> {
    let mut out = Vec::new();
    for it in items {
        let Object::Reference(id) = &it else {
            out.push(it);
            continue;
        };
        if gone.contains(id) {
            continue;
        }
        let kids = doc
            .get_dictionary(*id)
            .ok()
            .and_then(|d| d.get(b"Kids").ok())
            .and_then(|k| k.as_array().ok())
            .cloned();
        if let (Some(kids), true) = (kids, depth < 32) {
            let before = kids.len();
            let new_kids = prune_fields(doc, kids, gone, depth + 1);
            if new_kids.is_empty() && before > 0 {
                continue;
            }
            if new_kids.len() != before {
                if let Ok(d) = doc.get_dictionary_mut(*id) {
                    d.set("Kids", Object::Array(new_kids));
                }
            }
        }
        out.push(it);
    }
    out
}

/// A PDF text string / name / array of them as plain text (UTF-16BE with BOM or Latin-1).
fn string_of(doc: &Document, o: &Object) -> Option<String> {
    match resolve(doc, o)? {
        Object::String(b, _) => Some(decode_text_string(b)),
        Object::Name(n) => Some(String::from_utf8_lossy(n).to_string()),
        Object::Array(a) => {
            let parts: Vec<String> = a.iter().filter_map(|x| string_of(doc, x)).collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        _ => None,
    }
}

fn decode_text_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        b.iter().map(|c| *c as char).collect()
    }
}

/// Lowercase with everything but letters and digits removed ("1,234,567" -> "1234567").
fn alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Match tokens for removed text, normalised by [`alnum`]: every whitespace chunk and every
/// punctuation-separated run, kept when it is a word of 3+ characters or a digit run of 2+.
fn match_tokens(text: &str) -> Vec<String> {
    let mut t: Vec<String> = Vec::new();
    for chunk in text.split_whitespace() {
        t.push(alnum(chunk));
        t.extend(
            chunk
                .split(|c: char| !c.is_alphanumeric())
                .map(|r| r.to_lowercase()),
        );
    }
    t.retain(|x| {
        let n = x.chars().count();
        n >= 3 || (n >= 2 && x.chars().all(|c| c.is_ascii_digit()))
    });
    t.sort();
    t.dedup();
    t
}

/// Does `s`, normalised the same way, contain any token? Scrub and verify both use this.
fn mentions(s: &str, tokens: &[String]) -> bool {
    let n = alnum(s);
    tokens.iter().any(|t| n.contains(t.as_str()))
}

/// Every piece of text a form field / widget node shows or stores: value, default,
/// tooltip, rich value, options, and the text drawn by its appearance streams.
fn field_texts(doc: &Document, id: ObjectId) -> Vec<String> {
    doc.get_dictionary(id)
        .map(|d| field_texts_dict(doc, d))
        .unwrap_or_default()
}

fn field_texts_dict(doc: &Document, d: &Dictionary) -> Vec<String> {
    let mut out = Vec::new();
    for k in [&b"V"[..], b"DV", b"TU", b"RV", b"Opt"] {
        if let Some(t) = d.get(k).ok().and_then(|o| string_of(doc, o)) {
            out.push(t);
        }
    }
    if let Some(ap) = d.get(b"AP").ok().and_then(|o| resolve_dict(doc, o)) {
        let mut streams: Vec<ObjectId> = Vec::new();
        for (_, v) in ap.iter() {
            match v {
                Object::Reference(r) => streams.push(*r),
                Object::Dictionary(sub) => {
                    streams.extend(sub.iter().filter_map(|(_, o)| o.as_reference().ok()))
                }
                _ => {}
            }
        }
        for sid in streams {
            let Ok(Object::Stream(st)) = doc.get_object(sid) else {
                continue;
            };
            let Ok(data) = st.decompressed_content_with_limit(MAX_STREAM_BYTES) else {
                // An appearance we cannot read cannot be shown clean: treat as matching text.
                out.push(String::from("\u{0}unreadable"));
                continue;
            };
            let Ok(c) = Content::decode(&data) else {
                out.push(String::from("\u{0}unreadable"));
                continue;
            };
            for op in &c.operations {
                for o in &op.operands {
                    match o {
                        Object::String(b, _) => out.push(decode_text_string(b)),
                        Object::Array(a) => {
                            for e in a {
                                if let Object::String(b, _) = e {
                                    out.push(decode_text_string(b));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

/// Inline (direct-object) field dictionaries under /AcroForm /Fields, recursively.
fn inline_fields_hit(doc: &Document, tokens: &[String]) -> bool {
    fn walk(doc: &Document, items: &[Object], tokens: &[String], depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        items.iter().any(|o| match o {
            Object::Dictionary(d) => {
                field_texts_dict(doc, d)
                    .iter()
                    .any(|t| t == "\u{0}unreadable" || mentions(t, tokens))
                    || d.get(b"Kids")
                        .ok()
                        .and_then(|k| k.as_array().ok())
                        .is_some_and(|k| walk(doc, k, tokens, depth + 1))
            }
            Object::Reference(r) => doc
                .get_dictionary(*r)
                .ok()
                .and_then(|d| d.get(b"Kids").ok())
                .and_then(|k| k.as_array().ok())
                .is_some_and(|k| walk(doc, k, tokens, depth + 1)),
            _ => false,
        })
    }
    let Some(fields) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| resolve_dict(doc, o))
        .and_then(|a| {
            a.get(b"Fields")
                .ok()
                .and_then(|f| resolve(doc, f))
                .and_then(|f| f.as_array().ok())
                .cloned()
        })
    else {
        return false;
    };
    walk(doc, &fields, tokens, 0)
}

fn field_hits(doc: &Document, fields: &[ObjectId], tokens: &[String]) -> Vec<ObjectId> {
    fields
        .iter()
        .copied()
        .filter(|id| {
            field_texts(doc, *id)
                .iter()
                .any(|t| t == "\u{0}unreadable" || mentions(t, tokens))
        })
        .collect()
}

/// AcroForm fields and widgets (all levels), bounded.
fn field_nodes(doc: &Document) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let fields = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| resolve_dict(doc, o))
        .and_then(|a| {
            a.get(b"Fields")
                .ok()
                .and_then(|f| resolve(doc, f))
                .and_then(|f| f.as_array().ok())
                .cloned()
        })
        .unwrap_or_default();
    let mut stack: Vec<ObjectId> = fields
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > 200_000 {
            continue;
        }
        out.push(id);
        if let Ok(d) = doc.get_dictionary(id) {
            if let Some(k) = d
                .get(b"Kids")
                .ok()
                .and_then(|k| resolve(doc, k))
                .and_then(|k| k.as_array().ok())
            {
                stack.extend(k.iter().filter_map(|o| o.as_reference().ok()));
            }
        }
    }
    out
}

/// Document-level places that can repeat redacted text: document info, XMP and the
/// bookmark tree are removed wholesale; form-field values that mention a removed word are
/// cleared. When some removed text could not be decoded and field values exist, it refuses.
fn scrub_document_text(doc: &mut Document, rem: &RemovedText) -> Result<()> {
    // Wholesale: nothing in these can be proven free of the removed text.
    doc.trailer.remove(b"Info");
    if let Ok(c) = doc.catalog_mut() {
        c.remove(b"Metadata");
        c.remove(b"Outlines");
    }
    let tokens = match_tokens(&rem.text);

    let fields = field_nodes(doc);
    let has_values = fields.iter().any(|id| {
        doc.get_dictionary(*id)
            .map(|d| d.has(b"V") || d.has(b"DV"))
            .unwrap_or(false)
    });
    if rem.undecoded && has_values {
        bail!(
            "redaction refused: text in a font that cannot be decoded was removed, and the \
             document has form-field values that could repeat it; nothing was written"
        );
    }
    if tokens.is_empty() {
        return Ok(());
    }
    let dirty_fields = field_hits(doc, &fields, &tokens);
    for id in dirty_fields {
        // Clear the value on the field and the appearance on it and its widgets.
        let kids: Vec<ObjectId> = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Kids").ok().and_then(|k| k.as_array().ok()).cloned())
            .map(|k| k.iter().filter_map(|o| o.as_reference().ok()).collect())
            .unwrap_or_default();
        for target in std::iter::once(id).chain(kids) {
            if let Ok(d) = doc.get_dictionary_mut(target) {
                for k in [&b"V"[..], b"DV", b"TU", b"RV", b"AP", b"Opt"] {
                    d.remove(k);
                }
            }
        }
    }
    // Field dictionaries written inline (not as indirect objects) cannot be edited here:
    // if one repeats redacted text, refuse.
    if inline_fields_hit(doc, &tokens) {
        bail!(
            "redaction refused: an inline form-field dictionary still repeats redacted text; \
             nothing was written"
        );
    }

    // Verify with the same predicate: any field text still matching a removed token means
    // the redaction cannot be guaranteed, so refuse.
    let still = field_hits(doc, &fields, &tokens);
    if !still.is_empty() {
        bail!(
            "redaction refused: form-field text (object {} {}) still repeats redacted text; \
             nothing was written",
            still[0].0,
            still[0].1
        );
    }
    Ok(())
}

/// Run once after all pages are stripped: purge dangling references to what was removed,
/// drop the structure tree (its alt text can repeat page text), delete the originals no
/// other page still uses, prune, and VERIFY none of the removed originals survive.
/// Returns an error (the caller's edit is then abandoned) when a removed original is still
/// reachable: a partial redaction must never report success.
pub fn finalize(doc: &mut Document, reports: &[StripReport]) -> Result<()> {
    if reports.iter().all(|r| !r.changed) {
        return Ok(());
    }
    let pages: std::collections::HashSet<ObjectId> =
        reports.iter().filter_map(|r| r.page).collect();
    let gone_annots: std::collections::HashSet<ObjectId> = reports
        .iter()
        .flat_map(|r| r.removed_annots.iter().copied())
        .collect();
    let replaced: Vec<ObjectId> = reports
        .iter()
        .flat_map(|r| r.replaced.iter().copied())
        .collect();

    // AcroForm field tree must not keep removed widgets (and their /V) alive.
    if !gone_annots.is_empty() {
        let acro = doc
            .catalog()
            .ok()
            .and_then(|c| c.get(b"AcroForm").ok().cloned())
            .and_then(|o| match o {
                Object::Reference(r) => Some((Some(r), resolve_dict(doc, &Object::Reference(r))?)),
                Object::Dictionary(d) => Some((None, d)),
                _ => None,
            });
        if let Some((acro_id, acro_dict)) = acro {
            if let Some(fields) = acro_dict
                .get(b"Fields")
                .ok()
                .and_then(|o| resolve(doc, o))
                .and_then(|o| o.as_array().ok())
                .cloned()
            {
                let new_fields = prune_fields(doc, fields, &gone_annots, 0);
                match acro_id {
                    Some(r) => {
                        if let Ok(d) = doc.get_dictionary_mut(r) {
                            d.set("Fields", Object::Array(new_fields));
                        }
                    }
                    None => {
                        if let Ok(c) = doc.catalog_mut() {
                            if let Ok(Object::Dictionary(d)) = c.get_mut(b"AcroForm") {
                                d.set("Fields", Object::Array(new_fields));
                            }
                        }
                    }
                }
            }
        }
    }

    // Document info, XMP, bookmarks and field values that could repeat what was removed.
    let mut rem = RemovedText::default();
    for r in reports {
        rem.text.push(' ');
        rem.text.push_str(&r.removed.text);
        rem.undecoded |= r.removed.undecoded;
    }
    scrub_document_text(doc, &rem)?;

    // Structure-tree elements can carry /ActualText, /Alt and /E copies of page text.
    if let Ok(c) = doc.catalog_mut() {
        c.remove(b"StructTreeRoot");
        c.remove(b"MarkInfo");
    }

    // Originals no other page uses: take them out of every /XObject resource dictionary.
    let used = used_elsewhere(doc, &pages);
    let dead: std::collections::HashSet<ObjectId> = replaced
        .iter()
        .copied()
        .filter(|i| !used.contains(i))
        .collect();
    if !dead.is_empty() {
        let strip = |xo: &mut Dictionary| {
            let keys: Vec<Vec<u8>> = xo
                .iter()
                .filter(|(_, v)| matches!(v, Object::Reference(r) if dead.contains(r)))
                .map(|(k, _)| k.clone())
                .collect();
            for k in keys {
                xo.remove(&k);
            }
        };
        // Resource dictionaries live inline in pages / forms / the page tree, or as
        // separate objects; XObject sub-dictionaries may be inline or referenced.
        let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
        for id in ids {
            let mut indirect_xo: Vec<ObjectId> = Vec::new();
            if let Some(obj) = doc.objects.get_mut(&id) {
                let dict = match obj {
                    Object::Dictionary(d) => Some(d),
                    Object::Stream(s) => Some(&mut s.dict),
                    _ => None,
                };
                if let Some(d) = dict {
                    // d itself may be a Resources dict, or contain one.
                    if let Ok(Object::Dictionary(xo)) = d.get_mut(b"XObject") {
                        strip(xo);
                    } else if let Ok(Object::Reference(r)) = d.get(b"XObject") {
                        indirect_xo.push(*r);
                    }
                    if let Ok(Object::Dictionary(res)) = d.get_mut(b"Resources") {
                        if let Ok(Object::Dictionary(xo)) = res.get_mut(b"XObject") {
                            strip(xo);
                        } else if let Ok(Object::Reference(r)) = res.get(b"XObject") {
                            indirect_xo.push(*r);
                        }
                    }
                }
            }
            for r in indirect_xo {
                if let Some(Object::Dictionary(xo)) = doc.objects.get_mut(&r) {
                    strip(xo);
                }
            }
        }
        // Resources dictionaries referenced indirectly from pages / forms.
        let res_ids: Vec<ObjectId> = doc
            .objects
            .values()
            .filter_map(|o| match o {
                Object::Dictionary(d) => {
                    d.get(b"Resources").ok().and_then(|r| r.as_reference().ok())
                }
                Object::Stream(s) => s
                    .dict
                    .get(b"Resources")
                    .ok()
                    .and_then(|r| r.as_reference().ok()),
                _ => None,
            })
            .collect();
        for rid in res_ids {
            let mut indirect_xo = None;
            if let Some(Object::Dictionary(res)) = doc.objects.get_mut(&rid) {
                if let Ok(Object::Dictionary(xo)) = res.get_mut(b"XObject") {
                    strip(xo);
                } else if let Ok(Object::Reference(r)) = res.get(b"XObject") {
                    indirect_xo = Some(*r);
                }
            }
            if let Some(r) = indirect_xo {
                if let Some(Object::Dictionary(xo)) = doc.objects.get_mut(&r) {
                    strip(xo);
                }
            }
        }
    }

    doc.prune_objects();

    // ---- verification: refuse rather than report a partial redaction ----------------
    let mut leftovers: Vec<ObjectId> = Vec::new();
    for id in dead.iter().chain(gone_annots.iter()) {
        if doc.objects.contains_key(id) {
            leftovers.push(*id);
        }
    }
    for r in reports {
        for id in &r.old_contents {
            if doc.objects.contains_key(id) {
                leftovers.push(*id);
            }
        }
    }
    if !leftovers.is_empty() {
        leftovers.sort();
        leftovers.dedup();
        bail!(
            "redaction refused: {} original object(s) that held the redacted content are still \
             reachable from the document (e.g. object {} {}); nothing was written",
            leftovers.len(),
            leftovers[0].0,
            leftovers[0].1
        );
    }
    Ok(())
}
