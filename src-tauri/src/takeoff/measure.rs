//! PDF /VP viewport + /Measure dictionaries (spec §12.7, ISO 32000-1 §12.9): write,
//! read, import and save-time sync of measurement viewports; and the preset-scale picker
//! helpers (M4 S1, M3-deferred takeoff items).
//!
//! The /Measure dict embeds the calibration scale into the PDF in a standard,
//! interoperable form so Acrobat and Bluebeam can read the scale and display
//! measurements. Operates on the M3 `ScaleRecord` (ratio = real-world units per
//! PDF point).
//!
//! Precision guardrail: never call `as_f32()` on PDF Number objects when reading
//! back - always `as_float()` (f64). lopdf serialises integer-valued reals without
//! a decimal point, so they reload as Object::Integer and as_f32() would drop them.

use anyhow::{bail, Context, Result};

use super::scale::{ScaleRecord, ScaleTarget};

// ---------------------------------------------------------------------------
// Preset calibration picker (M3 deferred)
// ---------------------------------------------------------------------------

/// Return the scales applicable to a given page for the preset picker: a
/// page-specific scale plus the document default, so the user can pick a saved
/// scale without re-drawing a calibration line.
///
/// Page-specific scales for OTHER pages are excluded. Order: page-specific first,
/// then document-default.
pub fn applicable_scales(scales: &[ScaleRecord], page_idx: u32) -> Vec<&ScaleRecord> {
    let mut out: Vec<&ScaleRecord> = scales
        .iter()
        .filter(|s| matches!(s.applies_to, ScaleTarget::Page { page } if page == page_idx))
        .collect();
    out.extend(
        scales
            .iter()
            .filter(|s| matches!(s.applies_to, ScaleTarget::DocumentDefault)),
    );
    out
}

/// Find a saved scale by id (for "apply this preset" without recalibrating).
pub fn find_scale<'a>(scales: &'a [ScaleRecord], scale_id: &str) -> Option<&'a ScaleRecord> {
    scales.iter().find(|s| s.id == scale_id)
}

// ---------------------------------------------------------------------------
// PDF viewport (/VP) + measure (/Measure) dictionaries, ISO 32000-1 §12.9
// ---------------------------------------------------------------------------
//
// Keys, checked against the text of ISO 32000-1:2008 (Adobe's PDF32000_2008.pdf):
//
//   Page /VP: an array of viewport dictionaries in drawing order (§12.9, Table 30).
//   Viewport (Table 260): /Type /Viewport, /BBox (required, default user space),
//     /Name (text string), /Measure.
//   Measure (Table 261): /Type /Measure, /Subtype /RL (the default).
//   Rectilinear measure (Table 262): /R (required, text: the scale ratio, e.g. "1:100"),
//     /X (required, number formats: user-space units -> X unit), /D (required: X unit ->
//     distance unit), /A (required: X unit squared -> area unit). /O (origin) is omitted:
//     its default is the BBox's lower-left corner.
//   Number format (Table 263): /Type /NumberFormat, /U (required, unit label), /C
//     (required, conversion factor), /F /D (decimal) with /D the precision as a power of
//     ten (100 = two decimals, the default).
//
//   /VP [<< /Type /Viewport /BBox [x0 y0 x1 y1] /Name (Detail A) /RLViewportId (<uuid>)
//          /Measure << /Type /Measure /Subtype /RL /R (1:20)
//                      /X [<< /Type /NumberFormat /U (m) /C <ratio> /F /D /D 100 >>]
//                      /D [<< /Type /NumberFormat /U (m) /C 1 /F /D /D 100 >>]
//                      /A [<< /Type /NumberFormat /U (sq m) /C 1 /F /D /D 100 >>] >> >>]
//
// /RLViewportId is redline's own key (PDF readers ignore unknown keys): it marks the
// viewports redline wrote, so a save replaces exactly those and never another
// application's.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::geometry::{display_to_true, true_to_display, PdfPoint};
use crate::markup::replies::{decode_text, encode_text};
use crate::takeoff::scale::{normalise_bbox, BBox};

/// redline's key on the viewport dictionaries it writes (its viewport id).
pub const RL_VIEWPORT_ID: &[u8] = b"RLViewportId";

/// Most decimal places written to or read from a number format's `/D` (both ways, so a
/// precision round-trips).
const MAX_PRECISION: u8 = 6;

/// Points of slack when matching a `/BBox` (rounding through f32 PDF reals).
const BBOX_TOLERANCE: f64 = 0.5;

/// A `/VP` entry another application wrote that has been imported as a scale: its page and
/// `/BBox` in the PDF's true default user space, and (for a region viewport) the scale record
/// as it was imported. While the scale still equals that snapshot the user has not edited
/// it, and a save keeps the original `/VP` dictionary byte for byte (its feet-and-inches
/// formats, `/O`, `/Y`, private keys) instead of rewriting it at redline's fidelity.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewportKey {
    pub page: u32,
    pub bbox: BBox,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<ScaleRecord>,
}

impl ViewportKey {
    /// True while the scale imported from this entry exists unchanged (and not deleted).
    fn unedited_in(&self, scales: &[ScaleRecord]) -> bool {
        self.imported
            .as_ref()
            .is_some_and(|snap| scales.iter().any(|s| s == snap && !s.retired))
    }
}

impl ViewportKey {
    fn matches(&self, page: u32, bbox: &BBox) -> bool {
        self.page == page && bbox_eq(&self.bbox, bbox)
    }
}

fn bbox_eq(a: &BBox, b: &BBox) -> bool {
    let (a, b) = (normalise_bbox(*a), normalise_bbox(*b));
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (x - y).abs() <= BBOX_TOLERANCE)
}

/// One rectilinear viewport read from a page's `/VP` array.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfViewport {
    /// Zero-based page index.
    pub page: u32,
    /// `/BBox` in the PDF's true default user space.
    pub true_bbox: BBox,
    /// `/BBox` mapped into redline's page space (where markups and scales live).
    pub bbox: BBox,
    /// The viewport covers the whole page (`/MediaBox`): a page scale, not a region.
    pub full_page: bool,
    pub name: Option<String>,
    /// redline's viewport id when redline wrote it.
    pub redline_id: Option<String>,
    /// Real-world distance units per PDF point (`/X` C times `/D` C).
    pub ratio: f64,
    pub unit: String,
    /// `/R`, the scale ratio text.
    pub label: String,
    pub precision: u8,
}

/// Page geometry needed to map between redline's page space and the PDF's true space.
#[derive(Clone, Copy)]
struct PageSpace {
    rotation: i32,
    w0: f64,
    h0: f64,
    ox: f64,
    oy: f64,
}

impl PageSpace {
    fn of(doc: &Document, page_id: ObjectId, page_idx: u32) -> Result<Self> {
        let rotation = crate::document::page_ops::page_rotation(doc, page_idx)?;
        let (w0, h0, ox, oy) = crate::document::annots::true_media_box(doc, page_id)?;
        Ok(Self {
            rotation,
            w0,
            h0,
            ox,
            oy,
        })
    }

    fn map(&self, b: &BBox, f: fn(PdfPoint, i32, f64, f64, f64, f64) -> PdfPoint) -> BBox {
        let a = f(
            PdfPoint { x: b[0], y: b[1] },
            self.rotation,
            self.w0,
            self.h0,
            self.ox,
            self.oy,
        );
        let c = f(
            PdfPoint { x: b[2], y: b[3] },
            self.rotation,
            self.w0,
            self.h0,
            self.ox,
            self.oy,
        );
        normalise_bbox([a.x, a.y, c.x, c.y])
    }

    fn to_true(self, b: &BBox) -> BBox {
        self.map(b, display_to_true)
    }

    fn to_display(self, b: &BBox) -> BBox {
        self.map(b, true_to_display)
    }

    /// The page's `/MediaBox` in true space.
    fn media(&self) -> BBox {
        [self.ox, self.oy, self.ox + self.w0, self.oy + self.h0]
    }
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

/// A number format dictionary (Table 263): `/U` unit, `/C` factor, decimal display to
/// `precision` places (`/F /D`, `/D` = 10^precision).
fn number_format(unit: &str, c: f64, precision: u8) -> Dictionary {
    dictionary! {
        "Type" => "NumberFormat",
        "U" => encode_text(unit),
        "C" => real(c),
        "F" => "D",
        "D" => Object::Integer(10_i64.pow(u32::from(precision.min(MAX_PRECISION)))),
    }
}

/// The rectilinear measure dictionary (Tables 261/262) for `scale`.
pub fn measure_dict(scale: &ScaleRecord) -> Dictionary {
    let p = scale.precision;
    dictionary! {
        "Type" => "Measure",
        "Subtype" => "RL",
        "R" => encode_text(&scale.label),
        "X" => vec![Object::Dictionary(number_format(&scale.unit, scale.ratio, p))],
        "D" => vec![Object::Dictionary(number_format(&scale.unit, 1.0, p))],
        "A" => vec![Object::Dictionary(number_format(&format!("sq {}", scale.unit), 1.0, p))],
    }
}

/// A viewport dictionary (Table 260) over `true_bbox` carrying `scale`.
fn viewport_dict(
    true_bbox: BBox,
    name: Option<&str>,
    redline_id: Option<&str>,
    scale: &ScaleRecord,
) -> Dictionary {
    let b = normalise_bbox(true_bbox);
    let mut d = dictionary! {
        "Type" => "Viewport",
        "BBox" => vec![real(b[0]), real(b[1]), real(b[2]), real(b[3])],
    };
    if let Some(n) = name {
        d.set("Name", encode_text(n));
    }
    d.set("Measure", Object::Dictionary(measure_dict(scale)));
    if let Some(id) = redline_id {
        d.set(RL_VIEWPORT_ID, encode_text(id));
    }
    d
}

/// Write a PDF /Measure viewport dictionary covering the whole page, embedding the
/// calibration scale so Acrobat/Bluebeam can read it. The entry goes FIRST in the page's
/// /VP array and replaces any earlier whole-page rectilinear entry: readers examine /VP
/// from the last entry back and take the first that contains the point (ISO 32000-1
/// §12.9), so a whole-page entry after a region viewport would hide that region's scale,
/// and repeated writes would pile up duplicates.
///
/// `page_idx` is 0-based. `scale.ratio` is real-world units per PDF point.
pub fn write_measure_dict(
    doc: &mut lopdf::Document,
    page_idx: u32,
    scale: &ScaleRecord,
) -> Result<()> {
    if scale.ratio == 0.0 {
        bail!("scale {}: ratio is 0", scale.id);
    }

    let pages = doc.get_pages();
    let page_no = page_idx + 1;
    let page_id = *pages
        .get(&page_no)
        .with_context(|| format!("page_idx {page_idx} out of range"))?;
    let space = PageSpace::of(doc, page_id, page_idx)?;
    let media = space.media();

    let existing: Vec<Object> = {
        let page_dict = doc.get_dictionary(page_id).context("page dict")?;
        match page_dict.get(b"VP").ok().and_then(|o| resolve(doc, o)) {
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        }
    };
    let whole_page = |item: &Object| {
        dict_of(doc, item)
            .and_then(|d| read_viewport(doc, d, page_idx, &space))
            .is_some_and(|v| v.full_page && v.redline_id.is_none())
    };
    let mut vps = vec![Object::Dictionary(viewport_dict(media, None, None, scale))];
    vps.extend(existing.iter().filter(|o| !whole_page(o)).cloned());
    doc.get_dictionary_mut(page_id)
        .context("page dict")?
        .set("VP", Object::Array(vps));
    Ok(())
}

/// Follow a reference to the object it names (one hop is all PDF allows here).
fn resolve<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(r) => doc.get_object(*r).ok(),
        other => Some(other),
    }
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    resolve(doc, o)?.as_dict().ok()
}

fn text(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<String> {
    match resolve(doc, d.get(key).ok()?)? {
        Object::String(b, _) => Some(decode_text(b)),
        _ => None,
    }
}

/// The first and last number format dictionaries of the array under `key`.
fn formats<'a>(
    doc: &'a Document,
    measure: &'a Dictionary,
    key: &[u8],
) -> Option<(&'a Dictionary, &'a Dictionary)> {
    let arr = resolve(doc, measure.get(key).ok()?)?.as_array().ok()?;
    let first = dict_of(doc, arr.first()?)?;
    let last = dict_of(doc, arr.last()?)?;
    Some((first, last))
}

fn number(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<f64> {
    resolve(doc, d.get(key).ok()?)?
        .as_float()
        .ok()
        .map(f64::from)
}

/// True for a measure dictionary with the rectilinear subtype (absent = RL, Table 261).
fn is_rectilinear(doc: &Document, measure: &Dictionary) -> bool {
    match measure.get(b"Subtype").ok().and_then(|o| resolve(doc, o)) {
        None => true,
        Some(o) => o.as_name().map(|n| n == b"RL").unwrap_or(false),
    }
}

/// Read one viewport dictionary as a rectilinear scale, or `None` when it carries no usable
/// RL measure (a geospatial viewport, a missing or zero conversion factor, ...).
fn read_viewport(
    doc: &Document,
    vp: &Dictionary,
    page: u32,
    space: &PageSpace,
) -> Option<PdfViewport> {
    let measure = dict_of(doc, vp.get(b"Measure").ok()?)?;
    if !is_rectilinear(doc, measure) {
        return None;
    }
    let bbox_arr = resolve(doc, vp.get(b"BBox").ok()?)?.as_array().ok()?;
    let nums: Vec<f64> = bbox_arr
        .iter()
        .filter_map(|o| resolve(doc, o)?.as_float().ok().map(f64::from))
        .collect();
    if nums.len() != 4 {
        return None;
    }
    let true_bbox = normalise_bbox([nums[0], nums[1], nums[2], nums[3]]);
    // X converts user space units to the X unit; D converts the X unit to the distance
    // unit (Table 262). Real-world distance per point = X.C * D.C, in D's unit.
    let (x_first, _) = formats(doc, measure, b"X")?;
    let x_c = number(doc, x_first, b"C")?;
    let (d_c, unit, last) = match formats(doc, measure, b"D") {
        Some((d_first, d_last)) => (
            number(doc, d_first, b"C").unwrap_or(1.0),
            text(doc, d_first, b"U"),
            d_last,
        ),
        None => (1.0, None, x_first),
    };
    let ratio = x_c * d_c;
    if !ratio.is_finite() || ratio <= 0.0 {
        return None;
    }
    let unit = unit
        .or_else(|| text(doc, x_first, b"U"))
        .unwrap_or_else(|| "pt".into())
        .trim()
        .to_string();
    // Decimal display only (/F /D, the default): /D is the precision as a power of ten.
    let decimal = match last.get(b"F").ok().and_then(|o| resolve(doc, o)) {
        None => true,
        Some(o) => o.as_name().map(|n| n == b"D").unwrap_or(false),
    };
    let precision = match (decimal, number(doc, last, b"D")) {
        (true, Some(d)) if d >= 1.0 => d.log10().round().clamp(0.0, f64::from(MAX_PRECISION)) as u8,
        _ => 2,
    };
    let label = text(doc, measure, b"R")
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| format!("1 pt = {ratio} {unit}"));
    let media = space.media();
    Some(PdfViewport {
        page,
        true_bbox,
        bbox: space.to_display(&true_bbox),
        full_page: bbox_eq(&true_bbox, &media),
        name: text(doc, vp, b"Name").filter(|n| !n.trim().is_empty()),
        redline_id: text(doc, vp, RL_VIEWPORT_ID),
        ratio,
        unit,
        label,
        precision,
    })
}

/// Every rectilinear viewport on every page, in each page's drawing order.
pub fn read_viewports(doc: &Document) -> Vec<PdfViewport> {
    let mut out = Vec::new();
    for (page_no, page_id) in doc.get_pages() {
        let page = page_no - 1;
        let Ok(page_dict) = doc.get_dictionary(page_id) else {
            continue;
        };
        let Some(arr) = page_dict
            .get(b"VP")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_array().ok())
        else {
            continue;
        };
        let Ok(space) = PageSpace::of(doc, page_id, page) else {
            continue;
        };
        for item in arr {
            if let Some(v) = dict_of(doc, item).and_then(|d| read_viewport(doc, d, page, &space)) {
                out.push(v);
            }
        }
    }
    out
}

/// Which of `found` (read from the PDF) become new scale records, given the document's
/// current `scales` and the `/VP` entries it has already imported (`seen`). Returns the new
/// records (in order) and the new `seen` keys.
///
/// - A viewport redline wrote (`/RLViewportId`) whose id is not among the scales comes
///   back as that viewport (the sidecar was lost, or a deletion was never saved).
/// - Another application's region viewport not seen before becomes a viewport scale.
/// - Another application's whole-page viewport becomes the page's scale when the page has
///   none (and there is no document default); a page scale already set wins.
pub fn plan_viewport_import(
    scales: &[ScaleRecord],
    seen: &[ViewportKey],
    found: &[PdfViewport],
) -> (Vec<ScaleRecord>, Vec<ViewportKey>) {
    let mut records: Vec<ScaleRecord> = Vec::new();
    let mut keys: Vec<ViewportKey> = Vec::new();
    let mut named = scales
        .iter()
        .filter(|s| matches!(s.applies_to, ScaleTarget::Viewport { .. }))
        .count();
    for f in found {
        let has_viewport = |id: &str| {
            scales.iter().chain(records.iter()).any(|s| {
                matches!(&s.applies_to, ScaleTarget::Viewport { viewport_id, .. } if viewport_id == id)
            })
        };
        let target = match &f.redline_id {
            Some(id) if has_viewport(id) => continue,
            Some(id) => viewport_target(f, id.clone(), &mut named),
            None => {
                if seen
                    .iter()
                    .chain(keys.iter())
                    .any(|k| k.matches(f.page, &f.true_bbox))
                {
                    continue;
                }
                keys.push(ViewportKey {
                    page: f.page,
                    bbox: f.true_bbox,
                    imported: None,
                });
                if f.full_page {
                    let page_has_scale = scales.iter().chain(records.iter()).any(|s| {
                        matches!(s.applies_to, ScaleTarget::Page { page } if page == f.page)
                            || matches!(s.applies_to, ScaleTarget::DocumentDefault)
                    });
                    if page_has_scale {
                        continue;
                    }
                    ScaleTarget::Page { page: f.page }
                } else {
                    viewport_target(f, uuid::Uuid::new_v4().to_string(), &mut named)
                }
            }
        };
        let rec = ScaleRecord::new(
            target,
            super::scale::ScaleMethod::Preset,
            f.ratio,
            f.unit.clone(),
            f.label.clone(),
            f.precision,
        );
        // A region viewport imported from another application: remember the record as
        // imported, so a save keeps the original entry while it is unedited.
        if f.redline_id.is_none() && !f.full_page {
            if let Some(k) = keys.last_mut() {
                k.imported = Some(rec.clone());
            }
        }
        records.push(rec);
    }
    (records, keys)
}

fn viewport_target(f: &PdfViewport, viewport_id: String, named: &mut usize) -> ScaleTarget {
    *named += 1;
    ScaleTarget::Viewport {
        page: f.page,
        viewport_id,
        name: f
            .name
            .clone()
            .unwrap_or_else(|| format!("Viewport {}", *named)),
        bbox: f.bbox,
    }
}

/// `scales` and `seen` after a page operation, with each viewport moved to where its `/VP`
/// entry now is in `found` (read back after the operation): a reorder changes the page, a
/// rotation the page-space rectangle. A redline-written entry is found by its id; an
/// unedited imported one (kept byte for byte) by its `/BBox`. A viewport whose entry is
/// gone (its page was deleted) is dropped; a retired (deleted) one is kept as it is.
pub fn follow_viewports(
    scales: &[ScaleRecord],
    seen: &[ViewportKey],
    found: &[PdfViewport],
) -> (Vec<ScaleRecord>, Vec<ViewportKey>) {
    let mut seen = seen.to_vec();
    let mut out = Vec::new();
    for s in scales {
        let ScaleTarget::Viewport {
            viewport_id, name, ..
        } = &s.applies_to
        else {
            out.push(s.clone());
            continue;
        };
        if s.retired {
            out.push(s.clone());
            continue;
        }
        let key = seen.iter().position(|k| k.imported.as_ref() == Some(s));
        let f = match key {
            Some(i) => {
                let k = &seen[i];
                let candidates = || {
                    found
                        .iter()
                        .filter(|f| f.redline_id.is_none() && bbox_eq(&f.true_bbox, &k.bbox))
                };
                candidates()
                    .find(|f| f.page == k.page)
                    .or_else(|| candidates().next())
            }
            None => found
                .iter()
                .find(|f| f.redline_id.as_deref() == Some(viewport_id.as_str())),
        };
        let Some(f) = f else {
            continue;
        };
        let mut moved = s.clone();
        moved.applies_to = ScaleTarget::Viewport {
            page: f.page,
            viewport_id: viewport_id.clone(),
            name: name.clone(),
            bbox: f.bbox,
        };
        if let Some(i) = key {
            seen[i].page = f.page;
            seen[i].imported = Some(moved.clone());
        }
        out.push(moved);
    }
    (out, seen)
}

/// Bring every page's `/VP` array in line with the document's viewport scales, in drawing
/// order after the entries kept:
/// - viewports redline wrote earlier (`/RLViewportId`) are replaced by the current ones;
/// - a region viewport imported from another application is kept byte for byte while its
///   scale is unedited (see [`ViewportKey`]); once edited it is replaced by redline's copy,
///   and once deleted it is removed;
/// - everything else - whole-page scales, geospatial or other non-rectilinear viewports,
///   another application's viewports never imported - is kept as it is.
///
/// Retired (deleted) viewport scales are not written. A page whose geometry cannot be read
/// is skipped (logged), not a reason to fail the save.
pub fn sync_viewports(
    doc: &mut Document,
    scales: &[ScaleRecord],
    seen: &[ViewportKey],
) -> Result<()> {
    let pages = doc.get_pages();
    for (page_no, page_id) in pages {
        let page = page_no - 1;
        let existing: Vec<Object> = {
            let page_dict = doc.get_dictionary(page_id).context("page dict")?;
            match page_dict.get(b"VP").ok().and_then(|o| resolve(doc, o)) {
                Some(Object::Array(a)) => a.clone(),
                _ => Vec::new(),
            }
        };
        let on_page = |s: &&ScaleRecord| {
            !s.retired
                && matches!(&s.applies_to, ScaleTarget::Viewport { page: p, .. } if *p == page)
        };
        if existing.is_empty() && !scales.iter().any(|s| on_page(&s)) {
            continue;
        }
        let space = match PageSpace::of(doc, page_id, page) {
            Ok(sp) => sp,
            Err(e) => {
                log::warn!("measurement viewports: page {page} skipped: {e:#}");
                continue;
            }
        };
        let media = space.media();
        // Imported scales whose original entry is kept (so not written again).
        let mut preserved: Vec<&str> = Vec::new();
        let mut kept: Vec<Object> = Vec::new();
        for item in &existing {
            let Some(d) = dict_of(doc, item) else {
                kept.push(item.clone());
                continue;
            };
            if d.has(RL_VIEWPORT_ID) {
                continue; // ours: rewritten below
            }
            let imported = read_viewport(doc, d, page, &space)
                .filter(|v| !bbox_eq(&v.true_bbox, &media))
                .and_then(|v| seen.iter().find(|k| k.matches(page, &v.true_bbox)));
            match imported {
                None => kept.push(item.clone()),
                Some(k) if k.unedited_in(scales) => {
                    if let Some(snap) = &k.imported {
                        preserved.push(&snap.id);
                    }
                    kept.push(item.clone());
                }
                Some(_) => {} // edited or deleted: replaced by redline's copy, or gone
            }
        }
        let ours: Vec<&ScaleRecord> = scales
            .iter()
            .filter(on_page)
            .filter(|s| !preserved.contains(&s.id.as_str()))
            .collect();
        if kept.len() == existing.len() && ours.is_empty() {
            continue;
        }
        for s in &ours {
            let ScaleTarget::Viewport {
                viewport_id,
                name,
                bbox,
                ..
            } = &s.applies_to
            else {
                continue;
            };
            kept.push(Object::Dictionary(viewport_dict(
                space.to_true(bbox),
                Some(name),
                Some(viewport_id),
                s,
            )));
        }
        let page_dict = doc.get_dictionary_mut(page_id).context("page dict")?;
        if kept.is_empty() {
            page_dict.remove(b"VP");
        } else {
            page_dict.set("VP", Object::Array(kept));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests::one_page_doc;
    use crate::takeoff::scale::{BBox, ScaleMethod, ScaleRecord, ScaleTarget};

    fn scale(applies_to: ScaleTarget, ratio: f64, unit: &str, precision: u8) -> ScaleRecord {
        ScaleRecord::new(
            applies_to,
            ScaleMethod::Preset,
            ratio,
            unit.into(),
            "1:100".into(),
            precision,
        )
    }

    // --- preset picker ---

    #[test]
    fn applicable_scales_page_specific_plus_default() {
        let s_default = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s_p3 = scale(ScaleTarget::Page { page: 3 }, 0.002, "m", 2);
        let s_p5 = scale(ScaleTarget::Page { page: 5 }, 0.003, "m", 2);
        let scales = vec![s_default, s_p3, s_p5];

        let got = applicable_scales(&scales, 3);
        assert_eq!(got.len(), 2, "page-3 scale + document default");
        // Page-specific listed first.
        assert!(matches!(got[0].applies_to, ScaleTarget::Page { page: 3 }));
        assert!(matches!(got[1].applies_to, ScaleTarget::DocumentDefault));
    }

    #[test]
    fn applicable_scales_only_default_for_uncalibrated_page() {
        let s_default = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s_p3 = scale(ScaleTarget::Page { page: 3 }, 0.002, "m", 2);
        let scales = vec![s_default, s_p3];

        let got = applicable_scales(&scales, 9);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].applies_to, ScaleTarget::DocumentDefault));
    }

    #[test]
    fn find_scale_by_id() {
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let id = s.id.clone();
        let scales = vec![s];
        assert!(find_scale(&scales, &id).is_some());
        assert!(find_scale(&scales, "nonexistent").is_none());
    }

    // --- /Measure dict write ---

    #[test]
    fn write_measure_dict_creates_vp_array() {
        let (mut doc, page_id) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "mm", 4);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        assert!(page.has(b"VP"), "/VP must be added to page");
        let vp_arr = page.get(b"VP").unwrap().as_array().unwrap();
        assert_eq!(vp_arr.len(), 1);
    }

    #[test]
    fn write_measure_dict_vp_has_measure_subdict_rl() {
        let (mut doc, page_id) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.005, "mm", 2);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        let vp = page.get(b"VP").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        assert!(vp.has(b"Measure"));
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        assert_eq!(m.get(b"Subtype").unwrap().as_name().unwrap(), b"RL");
    }

    #[test]
    fn write_measure_dict_replaces_the_whole_page_entry_and_keeps_other_entries() {
        // Review of PR #166 (finding 1): a second whole-page write used to append a duplicate;
        // it now replaces the earlier whole-page entry, and other entries are kept.
        let (mut doc, _) = one_page_doc();
        let s1 = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let s2 = scale(ScaleTarget::Page { page: 0 }, 0.002, "m", 2);
        write_measure_dict(&mut doc, 0, &s1).unwrap();
        let mut entries: Vec<lopdf::Object> = vp_entries(&doc)
            .into_iter()
            .map(lopdf::Object::Dictionary)
            .collect();
        entries.push(foreign_vp([400.0, 400.0, 500.0, 500.0], "Revu detail"));
        set_vp(&mut doc, entries);
        write_measure_dict(&mut doc, 0, &s2).unwrap();

        let vps = read_viewports(&doc);
        assert_eq!(vps.len(), 2, "one whole-page entry + the region: {vps:?}");
        assert!(
            vps[0].full_page && (vps[0].ratio - 0.002).abs() < 1e-9,
            "the new whole-page entry, first"
        );
        assert_eq!(vps[1].name.as_deref(), Some("Revu detail"));
    }

    #[test]
    fn a_whole_page_write_after_a_synced_save_goes_first_and_a_resave_is_idempotent() {
        // Readers take the LAST /VP entry containing the point (§12.9): the whole-page entry
        // must come before the regions or it hides their scales.
        let (mut doc, _) = one_page_doc();
        let detail = vp_scale(0, "d", "Detail", [100.0, 100.0, 200.0, 200.0], 0.002);
        let page = scale(ScaleTarget::Page { page: 0 }, 0.01, "m", 2);
        sync_viewports(&mut doc, std::slice::from_ref(&detail), &[]).unwrap();
        write_measure_dict(&mut doc, 0, &page).unwrap();
        write_measure_dict(&mut doc, 0, &page).unwrap();
        sync_viewports(&mut doc, std::slice::from_ref(&detail), &[]).unwrap();
        let doc = reload(&mut doc);
        let vps = read_viewports(&doc);
        assert_eq!(vps.len(), 2, "{vps:?}");
        assert!(vps[0].full_page, "whole page first");
        assert_eq!(
            vps[1].redline_id.as_deref(),
            Some("d"),
            "the region last, so it wins inside"
        );
        let before = vp_entries(&doc);
        let mut again = doc.clone();
        sync_viewports(&mut again, std::slice::from_ref(&detail), &[]).unwrap();
        // Compared after the same file round trip (lopdf re-types integer-valued reals).
        assert_eq!(
            vp_entries(&reload(&mut again)),
            before,
            "re-saving changes nothing"
        );
    }

    /// A Revu-style viewport: feet and inches with a fractional inch format, an /O origin, a
    /// /Y format and a private key - details redline's own writer does not produce.
    fn revu_feet_inches_vp(bbox: [f32; 4]) -> lopdf::Object {
        use lopdf::{dictionary, Object};
        let nf = |u: &str, c: f32| {
            dictionary! { "Type" => "NumberFormat", "U" => Object::string_literal(u), "C" => Object::Real(c) }
        };
        let mut inch = nf("in", 12.0);
        inch.set("F", "F");
        inch.set("D", Object::Integer(16));
        Object::Dictionary(dictionary! {
            "Type" => "Viewport",
            "BBox" => bbox.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "Name" => Object::string_literal("Enlarged plan"),
            "BSIViewport" => Object::string_literal("revu-private"),
            "Measure" => dictionary! {
                "Type" => "Measure", "Subtype" => "RL",
                "R" => Object::string_literal("1/4 in = 1 ft"),
                "X" => vec![Object::Dictionary(nf("ft", 1.0 / 18.0))],
                "Y" => vec![Object::Dictionary(nf("ft", 1.0 / 18.0))],
                "D" => vec![Object::Dictionary(nf("ft", 1.0)), Object::Dictionary(inch.clone())],
                "A" => vec![Object::Dictionary(nf("sq ft", 1.0))],
                "O" => vec![Object::Real(400.0), Object::Real(400.0)],
            },
        })
    }

    #[test]
    fn an_imported_viewport_is_kept_byte_for_byte_until_the_user_edits_or_deletes_it() {
        let (mut doc, _) = one_page_doc();
        let original = revu_feet_inches_vp([400.0, 400.0, 500.0, 500.0]);
        set_vp(&mut doc, vec![original.clone()]);
        let found = read_viewports(&doc);
        assert_eq!(found[0].unit, "ft");
        assert_eq!(
            found[0].precision, 2,
            "a fractional /D is not a decimal precision"
        );
        let (recs, keys) = plan_viewport_import(&[], &[], &found);
        assert_eq!(
            keys[0].imported.as_ref(),
            Some(&recs[0]),
            "the import snapshot"
        );

        // An unrelated save: the original dictionary survives exactly (compared before the
        // file round trip, where lopdf re-types integer-valued reals by itself).
        sync_viewports(&mut doc, &recs, &keys).unwrap();
        assert_eq!(vp_entries(&doc), vec![original.as_dict().unwrap().clone()]);
        let doc1 = reload(&mut doc);
        let kept = &vp_entries(&doc1)[0];
        let m = kept.get(b"Measure").unwrap().as_dict().unwrap();
        assert!(m.has(b"O") && m.has(b"Y"), "/O and /Y kept");
        assert_eq!(
            m.get(b"D").unwrap().as_array().unwrap().len(),
            2,
            "feet-and-inches kept"
        );
        assert!(
            kept.has(b"BSIViewport") && !kept.has(RL_VIEWPORT_ID),
            "untouched: no redline key"
        );

        // The user edits it (recalibrates): now redline's copy replaces it.
        let mut edited = recs[0].clone();
        edited.ratio = 0.05;
        sync_viewports(&mut doc, std::slice::from_ref(&edited), &keys).unwrap();
        let vps = read_viewports(&reload(&mut doc));
        assert_eq!(vps.len(), 1);
        assert!(vps[0].redline_id.is_some() && (vps[0].ratio - 0.05).abs() < 1e-8);

        // Deleted (retired): gone from the file.
        let (mut doc, _) = one_page_doc();
        set_vp(&mut doc, vec![original]);
        let mut retired = recs[0].clone();
        retired.retired = true;
        sync_viewports(&mut doc, &[retired], &keys).unwrap();
        assert!(read_viewports(&doc).is_empty());
    }

    #[test]
    fn a_page_op_keeps_an_unedited_imported_viewport_and_its_page_follows() {
        let (mut doc, _) = one_page_doc();
        set_vp(
            &mut doc,
            vec![revu_feet_inches_vp([400.0, 400.0, 500.0, 500.0])],
        );
        let found = read_viewports(&doc);
        let (recs, keys) = plan_viewport_import(&[], &[], &found);
        // After a reorder the same entry is read back on page 3.
        let mut moved = found.clone();
        moved[0].page = 3;
        let (after, seen) = follow_viewports(&recs, &keys, &moved);
        assert_eq!(after.len(), 1, "not dropped: matched by its /BBox");
        assert!(matches!(
            after[0].applies_to,
            ScaleTarget::Viewport { page: 3, .. }
        ));
        assert_eq!(seen[0].page, 3);
        assert!(
            seen[0].unedited_in(&after),
            "still unedited, so still kept byte for byte"
        );
    }

    #[test]
    fn retired_viewports_are_not_written_and_survive_a_page_op() {
        let (mut doc, _) = one_page_doc();
        let mut v = vp_scale(0, "v", "V", [10.0, 10.0, 100.0, 100.0], 0.02);
        v.retired = true;
        sync_viewports(&mut doc, std::slice::from_ref(&v), &[]).unwrap();
        assert!(read_viewports(&doc).is_empty());
        let (after, _) = follow_viewports(std::slice::from_ref(&v), &[], &[]);
        assert_eq!(after, vec![v], "kept so its measurements keep their scale");
    }

    #[test]
    fn write_measure_dict_out_of_range_errors() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        assert!(write_measure_dict(&mut doc, 5, &s).is_err());
    }

    #[test]
    fn write_measure_dict_zero_ratio_errors() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.0, "m", 2);
        assert!(write_measure_dict(&mut doc, 0, &s).is_err());
    }

    /// Save doc with /Measure dict, reload from disk, assert /VP + /Measure survive.
    #[test]
    fn measure_dict_survives_file_roundtrip() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "mm", 4);
        write_measure_dict(&mut doc, 0, &s).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("measure.pdf");
        doc.save(&path).unwrap();

        let reloaded = lopdf::Document::load(&path).unwrap();
        let pages = reloaded.get_pages();
        let page = reloaded.get_dictionary(*pages.get(&1).unwrap()).unwrap();
        assert!(page.has(b"VP"), "/VP survives round-trip");
        let vp = page.get(b"VP").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        assert!(vp.has(b"Measure"), "/Measure survives round-trip");
        // Scale C value readable via as_float (Integer-or-Real safe).
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        let x = m.get(b"X").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        let c = x.get(b"C").unwrap().as_float().unwrap();
        assert!((c - 0.001).abs() < 1e-6, "scale C survives via as_float");
    }

    // --- measurement viewports (/VP, ISO 32000-1 §12.9) ---

    fn vp_scale(page: u32, id: &str, name: &str, bbox: BBox, ratio: f64) -> ScaleRecord {
        let mut s = ScaleRecord::new(
            ScaleTarget::Viewport {
                page,
                viewport_id: id.into(),
                name: name.into(),
                bbox,
            },
            ScaleMethod::Preset,
            ratio,
            "m".into(),
            "1:20".into(),
            3,
        );
        s.id = format!("scale-{id}");
        s
    }

    fn reload(doc: &mut lopdf::Document) -> lopdf::Document {
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        lopdf::Document::load_mem(&bytes).unwrap()
    }

    fn vp_entries(doc: &lopdf::Document) -> Vec<lopdf::Dictionary> {
        let page_id = *doc.get_pages().get(&1).unwrap();
        match doc.get_dictionary(page_id).unwrap().get(b"VP") {
            Ok(o) => o
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_dict().unwrap().clone())
                .collect(),
            Err(_) => vec![],
        }
    }

    /// A foreign (Revu-style) viewport dict: feet, a /D conversion of 1, "1/4 in = 1 ft".
    fn foreign_vp(bbox: [f32; 4], name: &str) -> lopdf::Object {
        use lopdf::{dictionary, Object};
        let nf = |u: &str, c: f32| {
            Object::Dictionary(dictionary! {
                "Type" => "NumberFormat", "U" => Object::string_literal(u), "C" => Object::Real(c),
                "F" => "D", "D" => Object::Integer(1000),
            })
        };
        Object::Dictionary(dictionary! {
            "Type" => "Viewport",
            "BBox" => bbox.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
            "Name" => Object::string_literal(name),
            "Measure" => dictionary! {
                "Type" => "Measure", "Subtype" => "RL",
                "R" => Object::string_literal("1/4 in = 1 ft"),
                // 1/4 in (18 pt) = 1 ft  ->  1/18 ft per point.
                "X" => vec![nf("ft", 1.0 / 18.0)],
                "D" => vec![nf("ft", 1.0)],
                "A" => vec![nf("sq ft", 1.0)],
            },
        })
    }

    fn set_vp(doc: &mut lopdf::Document, entries: Vec<lopdf::Object>) {
        let page_id = *doc.get_pages().get(&1).unwrap();
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("VP", lopdf::Object::Array(entries));
    }

    #[test]
    fn viewport_round_trips_through_vp_with_the_spec_keys() {
        let (mut doc, _) = one_page_doc();
        let s = vp_scale(0, "v1", "Detail A", [100.0, 200.0, 300.0, 350.0], 0.007056);
        sync_viewports(&mut doc, std::slice::from_ref(&s), &[]).unwrap();
        let doc = reload(&mut doc);

        // Spec keys (Tables 260-263), not just what our reader happens to use.
        let vp = &vp_entries(&doc)[0];
        assert_eq!(vp.get(b"Type").unwrap().as_name().unwrap(), b"Viewport");
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        assert_eq!(m.get(b"Type").unwrap().as_name().unwrap(), b"Measure");
        assert_eq!(m.get(b"Subtype").unwrap().as_name().unwrap(), b"RL");
        for key in [&b"R"[..], b"X", b"D", b"A"] {
            assert!(
                m.has(key),
                "rectilinear measure needs /{}",
                String::from_utf8_lossy(key)
            );
        }
        let x = m.get(b"X").unwrap().as_array().unwrap()[0]
            .as_dict()
            .unwrap();
        assert_eq!(x.get(b"Type").unwrap().as_name().unwrap(), b"NumberFormat");
        assert_eq!(
            x.get(b"D").unwrap().as_i64().unwrap(),
            1000,
            "3 decimals = /D 1000"
        );
        assert!(!m.has(b"O") || m.get(b"O").unwrap().as_array().unwrap().len() == 2);

        let got = read_viewports(&doc);
        assert_eq!(got.len(), 1);
        let v = &got[0];
        assert_eq!(v.page, 0);
        assert_eq!(v.name.as_deref(), Some("Detail A"));
        assert_eq!(v.redline_id.as_deref(), Some("v1"));
        assert_eq!(v.unit, "m");
        assert_eq!(v.label, "1:20");
        assert_eq!(v.precision, 3);
        assert!(!v.full_page);
        assert!((v.ratio - 0.007056).abs() < 1e-8, "{}", v.ratio);
        for (a, b) in v.bbox.iter().zip([100.0, 200.0, 300.0, 350.0]) {
            assert!((a - b).abs() < 1e-3);
        }
    }

    #[test]
    fn non_ascii_viewport_names_are_utf16_text_strings() {
        let (mut doc, _) = one_page_doc();
        let s = vp_scale(0, "v1", "Détail Ø", [10.0, 10.0, 50.0, 50.0], 0.01);
        sync_viewports(&mut doc, &[s], &[]).unwrap();
        let doc = reload(&mut doc);
        assert_eq!(read_viewports(&doc)[0].name.as_deref(), Some("Détail Ø"));
    }

    #[test]
    fn sync_replaces_its_own_entries_keeps_foreign_and_page_ones_and_drops_deleted() {
        let (mut doc, _) = one_page_doc();
        let page_scale = scale(ScaleTarget::Page { page: 0 }, 0.01, "m", 2);
        write_measure_dict(&mut doc, 0, &page_scale).unwrap(); // a whole-page /VP
        let mut entries = vp_entries(&doc)
            .into_iter()
            .map(lopdf::Object::Dictionary)
            .collect::<Vec<_>>();
        entries.push(foreign_vp([400.0, 400.0, 500.0, 500.0], "Revu detail"));
        set_vp(&mut doc, entries);

        let a = vp_scale(0, "a", "A", [10.0, 10.0, 100.0, 100.0], 0.02);
        let b = vp_scale(0, "b", "B", [200.0, 10.0, 300.0, 100.0], 0.05);
        sync_viewports(&mut doc, &[a.clone(), b], &[]).unwrap();
        sync_viewports(
            &mut doc,
            &[
                a.clone(),
                vp_scale(0, "b", "B2", [200.0, 10.0, 300.0, 100.0], 0.05),
            ],
            &[],
        )
        .unwrap();
        let doc1 = reload(&mut doc);
        let names: Vec<Option<String>> =
            read_viewports(&doc1).into_iter().map(|v| v.name).collect();
        assert_eq!(
            names,
            vec![
                None,
                Some("Revu detail".into()),
                Some("A".into()),
                Some("B2".into())
            ],
            "page /VP and the foreign one kept, ours rewritten once each in drawing order"
        );

        // Deleting viewport b removes only its entry.
        sync_viewports(&mut doc, &[a], &[]).unwrap();
        let doc2 = reload(&mut doc);
        assert_eq!(read_viewports(&doc2).len(), 3);
        // No viewports at all and nothing foreign left: /VP keeps the remaining entries.
        sync_viewports(&mut doc, &[], &[]).unwrap();
        assert_eq!(read_viewports(&reload(&mut doc)).len(), 2);
    }

    #[test]
    fn an_empty_vp_array_is_removed_and_a_page_without_viewports_is_untouched() {
        let (mut doc, page_id) = one_page_doc();
        sync_viewports(&mut doc, &[], &[]).unwrap();
        assert!(!doc.get_dictionary(page_id).unwrap().has(b"VP"));
        let a = vp_scale(0, "a", "A", [10.0, 10.0, 100.0, 100.0], 0.02);
        sync_viewports(&mut doc, &[a], &[]).unwrap();
        sync_viewports(&mut doc, &[], &[]).unwrap();
        assert!(!doc.get_dictionary(page_id).unwrap().has(b"VP"));
    }

    #[test]
    fn foreign_viewports_import_once_and_a_save_replaces_the_imported_entry() {
        let (mut doc, _) = one_page_doc();
        set_vp(
            &mut doc,
            vec![
                foreign_vp([0.0, 0.0, 612.0, 792.0], "Sheet"),
                foreign_vp([400.0, 400.0, 500.0, 500.0], ""),
            ],
        );
        let found = read_viewports(&doc);
        assert_eq!(found.len(), 2);
        assert!(found[0].full_page && !found[1].full_page);
        assert_eq!(found[1].unit, "ft");
        assert!((found[1].ratio - 1.0 / 18.0).abs() < 1e-7);
        assert_eq!(found[1].precision, 3);
        assert_eq!(found[1].label, "1/4 in = 1 ft");

        let (recs, keys) = plan_viewport_import(&[], &[], &found);
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0].applies_to,
            ScaleTarget::Page { page: 0 },
            "whole page = page scale"
        );
        let ScaleTarget::Viewport { name, .. } = &recs[1].applies_to else {
            panic!("region = viewport")
        };
        assert_eq!(
            name, "Viewport 1",
            "an unnamed viewport gets a numbered name"
        );
        assert_eq!(keys.len(), 2);

        // Seen entries are not imported again, even after the user deleted the scales.
        let (again, more) = plan_viewport_import(&[], &keys, &found);
        assert!(again.is_empty() && more.is_empty());

        // A page that already has a scale keeps it: a whole-page /VP does not override it.
        let mine = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let (recs2, _) = plan_viewport_import(std::slice::from_ref(&mine), &[], &found);
        assert_eq!(recs2.len(), 1);
        assert!(matches!(recs2[0].applies_to, ScaleTarget::Viewport { .. }));

        // Saving keeps the unedited imported original as it is (review finding 2: never
        // rewritten at lower fidelity); the whole-page entry is never touched.
        sync_viewports(&mut doc, &recs[1..], &keys).unwrap();
        let after = read_viewports(&reload(&mut doc));
        assert_eq!(after.len(), 2);
        assert!(after[0].full_page && after[0].redline_id.is_none());
        assert!(after[1].redline_id.is_none(), "kept, not rewritten");
        // Once edited, redline's copy (marked) replaces the imported original.
        let mut edited = recs[1].clone();
        edited.label = "renamed scale".into();
        sync_viewports(&mut doc, std::slice::from_ref(&edited), &keys).unwrap();
        let after = read_viewports(&reload(&mut doc));
        assert_eq!(after.len(), 2);
        assert!(after[1].redline_id.is_some() && after[1].label == "renamed scale");
        // ... and once deleted (no scales left) the imported viewport is gone from the file.
        sync_viewports(&mut doc, &[], &keys).unwrap();
        assert_eq!(read_viewports(&reload(&mut doc)).len(), 1);
    }

    #[test]
    fn a_redline_viewport_missing_from_the_scales_is_imported_with_its_id() {
        let (mut doc, _) = one_page_doc();
        let a = vp_scale(0, "keep-me", "A", [10.0, 10.0, 100.0, 100.0], 0.02);
        sync_viewports(&mut doc, std::slice::from_ref(&a), &[]).unwrap();
        let found = read_viewports(&reload(&mut doc));
        let (recs, keys) = plan_viewport_import(&[], &[], &found);
        assert!(
            keys.is_empty(),
            "marked entries are tracked by id, not by key"
        );
        assert!(
            matches!(&recs[0].applies_to, ScaleTarget::Viewport { viewport_id, name, .. } if viewport_id == "keep-me" && name == "A")
        );
        let (none, _) = plan_viewport_import(&[a], &[], &found);
        assert!(none.is_empty(), "already present: not imported twice");
    }

    #[test]
    fn non_rectilinear_and_unusable_viewports_are_ignored_and_kept() {
        use lopdf::{dictionary, Object};
        let (mut doc, _) = one_page_doc();
        let geo = Object::Dictionary(dictionary! {
            "Type" => "Viewport",
            "BBox" => vec![Object::Real(1.0), Object::Real(1.0), Object::Real(50.0), Object::Real(50.0)],
            "Measure" => dictionary! { "Type" => "Measure", "Subtype" => "GEO" },
        });
        let zero = Object::Dictionary(dictionary! {
            "Type" => "Viewport",
            "BBox" => vec![Object::Real(1.0), Object::Real(1.0), Object::Real(50.0), Object::Real(50.0)],
            "Measure" => dictionary! {
                "Type" => "Measure", "R" => Object::string_literal("x"),
                "X" => vec![Object::Dictionary(dictionary! { "U" => Object::string_literal("m"), "C" => Object::Real(0.0) })],
            },
        });
        set_vp(&mut doc, vec![geo, zero]);
        assert!(read_viewports(&doc).is_empty());
        let key = ViewportKey {
            imported: None,
            page: 0,
            bbox: [1.0, 1.0, 50.0, 50.0],
        };
        sync_viewports(&mut doc, &[], &[key]).unwrap();
        assert_eq!(
            vp_entries(&doc).len(),
            2,
            "a non-RL viewport is never ours to drop"
        );
    }

    #[test]
    fn viewport_bbox_maps_between_page_space_and_true_space_on_a_rotated_page() {
        let (mut doc, page_id) = one_page_doc();
        doc.get_dictionary_mut(page_id).unwrap().set("Rotate", 90);
        let a = vp_scale(0, "a", "A", [10.0, 20.0, 110.0, 70.0], 0.02);
        sync_viewports(&mut doc, std::slice::from_ref(&a), &[]).unwrap();
        let doc = reload(&mut doc);
        let v = &read_viewports(&doc)[0];
        assert!(
            v.true_bbox
                .iter()
                .zip(v.bbox.iter())
                .any(|(t, d)| (t - d).abs() > 1.0),
            "rotation maps the rectangle: {:?} vs {:?}",
            v.true_bbox,
            v.bbox
        );
        for (got, want) in v.bbox.iter().zip([10.0, 20.0, 110.0, 70.0]) {
            assert!((got - want).abs() < 1e-3, "{:?}", v.bbox);
        }
    }

    #[test]
    fn follow_viewports_moves_each_to_its_entry_and_drops_ones_whose_page_went() {
        let a = vp_scale(2, "a", "A", [10.0, 10.0, 100.0, 100.0], 0.02);
        let b = vp_scale(3, "b", "B", [10.0, 10.0, 100.0, 100.0], 0.02);
        let d = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        let found = vec![PdfViewport {
            page: 0,
            true_bbox: [1.0, 2.0, 3.0, 4.0],
            bbox: [5.0, 6.0, 7.0, 8.0],
            full_page: false,
            name: Some("A".into()),
            redline_id: Some("a".into()),
            ratio: 0.02,
            unit: "m".into(),
            label: "1:20".into(),
            precision: 3,
        }];
        let (after, _) = follow_viewports(&[d.clone(), a.clone(), b], &[], &found);
        assert_eq!(after.len(), 2, "b's page is gone");
        assert_eq!(after[0], d);
        assert!(
            matches!(&after[1].applies_to, ScaleTarget::Viewport { page: 0, bbox, .. } if *bbox == [5.0, 6.0, 7.0, 8.0])
        );
        assert_eq!(after[1].id, a.id, "same scale, so its measurements keep it");
    }

    #[test]
    fn write_measure_dict_is_a_whole_page_rectilinear_viewport_with_the_spec_keys() {
        let (mut doc, _) = one_page_doc();
        let s = scale(ScaleTarget::DocumentDefault, 0.001, "m", 2);
        write_measure_dict(&mut doc, 0, &s).unwrap();
        let vp = &vp_entries(&doc)[0];
        let m = vp.get(b"Measure").unwrap().as_dict().unwrap();
        assert!(
            m.has(b"R") && m.has(b"A"),
            "R and A are required (Table 262)"
        );
        let v = &read_viewports(&doc)[0];
        assert!(v.full_page);
        assert_eq!(v.precision, 2);
    }
}
