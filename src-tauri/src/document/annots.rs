//! lopdf-level read/write of redline markups in a PDF's page /Annots arrays.
//!
//! Managed-annotation policy: an annotation is *managed* (owned/replaced by redline on
//! save) iff it carries an /RLType key OR its /NM matches a markup id in the store.
//! Foreign annotations (links, popups, widgets, third-party markups) are preserved
//! untouched. Import filter: only markup-like subtypes become `Markup`s on read.

use anyhow::{bail, Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::geometry::PdfPoint;
use crate::markup::{appearance, Markup, MarkupGeometry};

// ---------------------------------------------------------------------------
// Rotation- and MediaBox-origin-aware coordinate conversion (interop fix, 2026-08-06
// owner report: "the markups from redline show up in different locations in bluebeam
// now").
//
// redline's frontend captures markup geometry in PDFium's "display" page space -
// `get_page_size` returns PDFium's `PdfPage::page_size()`, which (per the pdfium public
// header: "Changing the rotation of |page| affects the return value") already has the
// page's own `/Rotate` baked in - width/height are SWAPPED for a 90/270-rotated page, and
// `render_tile`'s custom-matrix path renders into that SAME already-rotated space (PDFium
// applies `/Rotate` internally to every page-space API uniformly, including the raw
// `FPDF_RenderPageBitmapWithMatrix` path - confirmed empirically, not assumed; see the
// `rotation_interop` test module below, which keeps the coverage a set of now-deleted
// throwaway probe tests originally proved).
//
// A SECOND, independent effect discovered the same way: PDFium's page space also treats
// `/MediaBox`'s own lower-left corner as its origin, NOT absolute PDF coordinate (0,0) -
// confirmed empirically (a page with `/MediaBox [36 36 576 756]` and content drawn at
// absolute (100,100) rendered 36pt further toward the origin than a naive "content-stream
// coordinates are used directly" model would predict). PDF content-stream operators (and
// therefore annotation `/Rect`) are always in the ABSOLUTE default user space - MediaBox's
// corners are just particular absolute coordinates within it, not a new local origin - so
// this is a second, independent source of drift whenever `/MediaBox` doesn't start at
// (0,0), completely orthogonal to rotation (a cropped/trimmed sheet with a shifted
// MediaBox needs no `/Rotate` at all to trigger it).
//
// PDF spec annotation `/Rect` (and every geometry key derived from it) MUST be expressed
// in the page's TRUE, ABSOLUTE default user space, which neither `/Rotate` NOR a
// non-origin `/MediaBox` affects (ISO 32000-1 §14.4 / §8.4.1: rotation is a viewing-time
// transform applied uniformly to the whole page - content stream AND annotations together
// - never a change to the coordinate system itself; MediaBox merely bounds the physical
// medium within that same absolute system). Writing PDFium's rotated/origin-relative
// "display space" numbers straight into `/Rect` is therefore wrong whenever either effect
// applies: redline's own viewer stays self-consistent (it captures and re-displays using
// the SAME frame), while a spec-conformant reader like Bluebeam correctly re-derives its
// OWN display from the TRUE `/Rect` plus `/Rotate`/`/MediaBox` - and ends up drawing the
// annotation somewhere completely different, because it was never given true-space
// coordinates.
//
// The fix: keep the in-memory `Markup::geometry` in display space everywhere else in the
// app (measurement/takeoff, snap targets, the SVG overlay, selection/hit-testing all stay
// exactly as before - zero behaviour change for the overwhelmingly common case of
// rotation=0 and a MediaBox already starting at (0,0), which is an identity transform
// end-to-end), and convert ONLY at the serialization boundary: `write_markups` maps
// display -> true before building `/Rect`/`/AP`/geometry keys; `read_markups` maps true ->
// display after parsing them back.
//
// Closed-form transforms below, EMPIRICALLY VERIFIED (not just derived) against PDFium's
// own rendering for all four rotations and a non-origin MediaBox - see the
// `rotation_interop` tests. `w0`/`h0` are the page's TRUE (unrotated) MediaBox width and
// height; `ox`/`oy` are its lower-left corner's absolute coordinates (0,0 for the common
// case). The rotation-only step operates in the MediaBox's own LOCAL frame (as if its
// origin were (0,0)) and the origin shift is applied outside that step - order matters:
// subtract-then-rotate going true->display, rotate-then-add going display->true.

/// Rotation-only step (MediaBox assumed to start at its own local (0,0)) - PDF true
/// default user space -> PDFium "display" (rotated) page space.
fn rotate_local_true_to_display(p: PdfPoint, rotation: i32, w0: f64, h0: f64) -> PdfPoint {
    match rotation.rem_euclid(360) {
        90 => PdfPoint {
            x: p.y,
            y: w0 - p.x,
        },
        180 => PdfPoint {
            x: w0 - p.x,
            y: h0 - p.y,
        },
        270 => PdfPoint {
            x: h0 - p.y,
            y: p.x,
        },
        _ => p, // 0 (or a non-multiple-of-90 value some other tool wrote) - identity.
    }
}

/// Exact inverse of [`rotate_local_true_to_display`] (verified by round-trip in
/// `rotation_interop` tests).
fn rotate_local_display_to_true(p: PdfPoint, rotation: i32, w0: f64, h0: f64) -> PdfPoint {
    match rotation.rem_euclid(360) {
        90 => PdfPoint {
            x: w0 - p.y,
            y: p.x,
        },
        180 => PdfPoint {
            x: w0 - p.x,
            y: h0 - p.y,
        },
        270 => PdfPoint {
            x: p.y,
            y: h0 - p.x,
        },
        _ => p,
    }
}

/// PDF true (absolute) default user space -> PDFium "display" page space: shift into the
/// MediaBox's own local frame, then rotate.
fn true_to_display(p: PdfPoint, rotation: i32, w0: f64, h0: f64, ox: f64, oy: f64) -> PdfPoint {
    let local = PdfPoint {
        x: p.x - ox,
        y: p.y - oy,
    };
    rotate_local_true_to_display(local, rotation, w0, h0)
}

/// PDFium "display" page space -> PDF true (absolute) default user space: un-rotate, then
/// shift out of the MediaBox's local frame. Exact inverse of [`true_to_display`].
fn display_to_true(p: PdfPoint, rotation: i32, w0: f64, h0: f64, ox: f64, oy: f64) -> PdfPoint {
    let local = rotate_local_display_to_true(p, rotation, w0, h0);
    PdfPoint {
        x: local.x + ox,
        y: local.y + oy,
    }
}

/// Apply a point-wise transform to every coordinate in a `MarkupGeometry`. `Rect`'s two
/// corners are re-normalised (component-wise min/max) after mapping rather than kept as
/// (mapped_min, mapped_max) verbatim - a 90/270 rotation can flip which mapped corner has
/// the smaller x or y, and `MarkupGeometry::Rect` is documented (and relied on elsewhere,
/// e.g. `appearance::draw`'s `max.x - min.x` width calc) to keep `min <= max` per axis.
fn map_geometry(g: &MarkupGeometry, f: impl Fn(PdfPoint) -> PdfPoint) -> MarkupGeometry {
    match g {
        MarkupGeometry::Point(p) => MarkupGeometry::Point(f(*p)),
        MarkupGeometry::Rect { min, max } => {
            let (a, b) = (f(*min), f(*max));
            MarkupGeometry::Rect {
                min: PdfPoint {
                    x: a.x.min(b.x),
                    y: a.y.min(b.y),
                },
                max: PdfPoint {
                    x: a.x.max(b.x),
                    y: a.y.max(b.y),
                },
            }
        }
        MarkupGeometry::Polyline(pts) => {
            MarkupGeometry::Polyline(pts.iter().copied().map(f).collect())
        }
        MarkupGeometry::Ink(strokes) => MarkupGeometry::Ink(
            strokes
                .iter()
                .map(|s| s.iter().copied().map(&f).collect())
                .collect(),
        ),
        MarkupGeometry::Quads(quads) => MarkupGeometry::Quads(
            quads
                .iter()
                .map(|q| [f(q[0]), f(q[1]), f(q[2]), f(q[3])])
                .collect(),
        ),
    }
}

/// Resolve a page's TRUE (unrotated) `/MediaBox` as `(width, height, origin_x, origin_y)`,
/// where `origin_x`/`origin_y` is the box's own lower-left corner in ABSOLUTE PDF
/// coordinates (0,0 for the common case). Walks the `/Parent` chain if the page dict
/// doesn't set `/MediaBox` directly (a valid, common PDF - the attribute is inheritable
/// per ISO 32000-1 §7.7.3.4, and several real-world producers only set it once on the
/// Pages root for a uniform-size document). The spec allows either diagonal corner pair
/// in either order, so the origin is `min(x0,x1)`/`min(y0,y1)`, not necessarily entries 0/1.
fn true_media_box(doc: &Document, page_id: ObjectId) -> Result<(f64, f64, f64, f64)> {
    let mut current = Some(page_id);
    let mut hops = 0u8;
    while let Some(id) = current {
        hops += 1;
        if hops > 64 {
            bail!(
                "page {:?}: /Parent chain too deep (possible cycle)",
                page_id
            );
        }
        let dict = doc.get_dictionary(id).context("page/pages dict")?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = mb.as_array().context("/MediaBox is not an array")?;
            if arr.len() != 4 {
                bail!("/MediaBox does not have exactly 4 entries");
            }
            let nums: Vec<f64> = arr
                .iter()
                .map(|o| {
                    o.as_float()
                        .map(|f| f as f64)
                        .context("/MediaBox entry is not a number")
                })
                .collect::<Result<_>>()?;
            let (x0, y0, x1, y1) = (nums[0], nums[1], nums[2], nums[3]);
            let (ox, oy) = (x0.min(x1), y0.min(y1));
            return Ok(((x1 - x0).abs(), (y1 - y0).abs(), ox, oy));
        }
        current = match dict.get(b"Parent") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        };
    }
    bail!(
        "page {:?}: no /MediaBox found on page or any ancestor",
        page_id
    )
}

/// Per-page `(rotation, true_width, true_height, origin_x, origin_y)`, resolved lazily and
/// cached so a document with many markups on the same page only reads its `/Rotate`/
/// `/MediaBox` once. `rotation == 0 && origin == (0,0)` is the overwhelming majority of
/// real pages, in which case [`display_to_true`]/[`true_to_display`] are the identity and
/// the whole conversion is a no-op - existing behaviour for those pages is unchanged.
struct PageSpaceCache<'a> {
    doc: &'a Document,
    cache: std::collections::HashMap<ObjectId, (i32, f64, f64, f64, f64)>,
}

impl<'a> PageSpaceCache<'a> {
    fn new(doc: &'a Document) -> Self {
        Self {
            doc,
            cache: std::collections::HashMap::new(),
        }
    }

    fn get(
        &mut self,
        page_id: ObjectId,
        page_idx_0based: u32,
    ) -> Result<(i32, f64, f64, f64, f64)> {
        if let Some(v) = self.cache.get(&page_id) {
            return Ok(*v);
        }
        let rotation = crate::document::page_ops::page_rotation(self.doc, page_idx_0based)?;
        let (w0, h0, ox, oy) = true_media_box(self.doc, page_id)?;
        let v = (rotation, w0, h0, ox, oy);
        self.cache.insert(page_id, v);
        Ok(v)
    }
}

/// PDF annotation subtypes imported as markups (spec section 6 type set).
///
/// `pub(crate)` so `toolchest::btx`'s `.btx` importer can apply the SAME allowlist before
/// calling `Markup::from_annotation_dict` on a `<Raw>` payload - that function's /Subtype
/// match has a permissive `_ => Text` fallback that is only safe here in `read_markups`
/// because every annotation is filtered against this list FIRST. `import_item` originally
/// had no such guard, so an unsupported subtype (Underline/StrikeOut/Widget/Popup/etc -
/// none of which have a `MarkupType` variant) silently became a bogus "Text" tool instead
/// of being skipped and reported (see `toolchest::btx::tests::
/// unsupported_annotation_subtype_is_skipped_not_silently_reclassified_as_text`).
pub(crate) const MARKUP_SUBTYPES: &[&str] = &[
    "Text",
    "FreeText",
    "Square",
    "Circle",
    "Line",
    "Polygon",
    "PolyLine",
    "Highlight",
    "Ink",
    "Stamp",
];

pub(crate) fn subtype(d: &Dictionary) -> Option<String> {
    d.get(b"Subtype")
        .ok()?
        .as_name()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// The `/IRT` (In Reply To) object reference of an annotation, if present - the
/// standard PDF markup-annotation reply/group link (ISO 32000-1 §12.5.6.2). Combined
/// with `/RT /Group` this is exactly the mechanism real Bluebeam-authored PDFs use to
/// group markups (design doc: `docs/design/2026-08-11-grouped-markups.md` §1) - a
/// follower annotation's `/IRT` points at its group's head annotation.
fn irt_ref(d: &Dictionary) -> Option<ObjectId> {
    match d.get(b"IRT").ok()? {
        Object::Reference(r) => Some(*r),
        _ => None,
    }
}

/// True if this annotation's `/RT` (Reply Type) is `/Group` - i.e. it is a GROUP
/// member (not a comment reply, the other `/RT` value ISO 32000-1 defines). Only
/// `/RT /Group` + `/IRT` together mean "grouped with"; `/IRT` alone (no `/RT`, or
/// `/RT /R`) is an ordinary comment reply and must not be treated as a group edge.
fn rt_is_group(d: &Dictionary) -> bool {
    matches!(d.get(b"RT").ok().and_then(|o| o.as_name().ok()), Some(b"Group"))
}

/// Resolve the page's /Annots into a list of (annot ObjectId | inline dict).
/// Returns owned dictionaries plus the id when the annot is an indirect object.
/// Used by Task 3 (write side) - pub(crate) to suppress dead_code until then.
pub(crate) fn page_annots(
    doc: &Document,
    page_id: ObjectId,
) -> Result<Vec<(Option<ObjectId>, Dictionary)>> {
    let page = doc.get_dictionary(page_id).context("page dict")?;
    let Ok(annots_obj) = page.get(b"Annots") else {
        return Ok(Vec::new());
    };
    // /Annots may be a direct array or a Reference to an array.
    let arr: Vec<Object> = match annots_obj {
        Object::Array(a) => a.clone(),
        Object::Reference(r) => {
            let rid = *r;
            match doc.get_object(rid).and_then(|o| o.as_array().cloned()) {
                Ok(a) => a,
                Err(_) => bail!(
                    "page {:?}: /Annots reference {:?} could not be resolved to an array",
                    page_id,
                    rid
                ),
            }
        }
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for entry in arr {
        match entry {
            Object::Reference(rid) => {
                if let Ok(d) = doc.get_dictionary(rid) {
                    out.push((Some(rid), d.clone()));
                }
            }
            Object::Dictionary(d) => out.push((None, d)),
            _ => {}
        }
    }
    Ok(out)
}

fn nm_of(d: &Dictionary) -> Option<String> {
    d.get(b"NM")
        .ok()?
        .as_str()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// The `/Parent` object reference of an annotation (a `/Popup`'s owning markup), if present.
fn parent_ref(d: &Dictionary) -> Option<ObjectId> {
    match d.get(b"Parent").ok()? {
        Object::Reference(r) => Some(*r),
        _ => None,
    }
}

/// True if redline owns this annotation (replace-on-save).
///
/// INTENTIONAL ownership stance: any /RLType-bearing annotation is treated as
/// redline-owned even when its /NM is NOT in the store id set. The store's view is
/// authoritative on save - callers MUST load existing markups into the store before
/// saving, or pre-existing redline annotations are (intentionally) replaced by the
/// store's view. The command layer enforces load-before-save.
fn is_managed(d: &Dictionary, ids: &std::collections::HashSet<String>) -> bool {
    d.has(b"RLType") || nm_of(d).map(|nm| ids.contains(&nm)).unwrap_or(false)
}

/// Splice a `StampAsset::BluebeamFormXObject` graph into `doc` as real indirect objects,
/// rewriting each node's own `/BBObjPtr_<id>` placeholders to the real references just
/// reserved for its siblings. Returns the root node's new `ObjectId`, or `None` if the
/// root itself fails to resolve (a node other than the root failing to parse is not
/// fatal: any `/BBObjPtr_` reference to it stays an inert name PDF viewers ignore,
/// degrading that one visual detail rather than the whole stamp, same never-fatal
/// posture as import). Shared by `write_markups` (splice at save time) and
/// `build_isolated_form_xobject_pdf` (splice into a throwaway single-stamp document for
/// rasterization - see `commands::toolchest::rasterize_form_xobject_asset`).
pub(crate) fn splice_bb_form_xobject(
    doc: &mut Document,
    root_id: &str,
    objects: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Option<ObjectId> {
    let id_to_objid: std::collections::BTreeMap<String, (u32, u16)> = objects
        .keys()
        .map(|id| (id.clone(), doc.new_object_id()))
        .collect();
    for (id, raw) in objects {
        let resolved = crate::toolchest::btx::resolve_bb_objptr_refs(raw, &id_to_objid);
        if let Ok(obj) = crate::toolchest::btx::parse_pdf_object_bytes(&resolved) {
            if let Some(&oid) = id_to_objid.get(id) {
                doc.set_object(oid, obj);
            }
        }
    }
    id_to_objid.get(root_id).copied()
}

/// Build a minimal, self-contained single-page PDF (as bytes) that places `root_id`'s
/// Form XObject at identity scale/translation, sized so the page's own `/MediaBox`
/// exactly matches the Form's `/BBox` - i.e. rendering page 0 of the result shows JUST
/// this stamp's artwork, isolated from any page content or other annotations. Used to
/// rasterize a `StampAsset::BluebeamFormXObject` for display (see
/// `commands::toolchest::rasterize_stamp_asset`) - the actual PDFium rendering happens
/// on the render thread, this function only builds the bytes (no PDFium dependency, so
/// it's plain-unit-testable without the `PDFIUM_DYNAMIC_LIB_PATH` gate).
///
/// Returns `Err` if the root object fails to splice or isn't a Form XObject stream with
/// a readable `/BBox` - both indicate a malformed/unsupported asset, not a transient
/// failure.
pub(crate) fn build_isolated_form_xobject_pdf(
    root_id: &str,
    objects: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut doc = Document::with_version("1.5");
    let root_oid = splice_bb_form_xobject(&mut doc, root_id, objects)
        .context("Form XObject root failed to splice (missing or malformed)")?;
    let bbox: Vec<f64> = {
        let root_obj = doc
            .get_object(root_oid)
            .context("root object vanished after splice")?;
        let stream = root_obj
            .as_stream()
            .context("root object is not a stream (not a Form XObject)")?;
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok());
        if subtype != Some(b"Form".as_slice()) {
            bail!("root object /Subtype is not /Form (got {subtype:?})");
        }
        let arr = stream
            .dict
            .get(b"BBox")
            .context("Form XObject has no /BBox")?
            .as_array()
            .context("/BBox is not an array")?;
        arr.iter()
            .map(|o| {
                o.as_float()
                    .map(|f| f as f64)
                    .context("/BBox entry is not numeric")
            })
            .collect::<Result<Vec<f64>>>()?
    };
    if bbox.len() != 4 {
        bail!("/BBox must have exactly 4 entries, got {}", bbox.len());
    }
    let (x0, y0, x1, y1) = (
        bbox[0].min(bbox[2]),
        bbox[1].min(bbox[3]),
        bbox[0].max(bbox[2]),
        bbox[1].max(bbox[3]),
    );

    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Object::Stream(lopdf::Stream::new(
        Dictionary::new(),
        b"q 1 0 0 1 0 0 cm /Fx0 Do Q".to_vec(),
    )));
    let resources =
        dictionary! { "XObject" => dictionary! { "Fx0" => Object::Reference(root_oid) } };
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![x0.into(), y0.into(), x1.into(), y1.into()],
        "Resources" => resources,
        "Contents" => Object::Reference(content_id),
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
        ),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    doc.save_to(&mut buf)
        .context("failed to serialize isolated stamp PDF")?;
    Ok(buf)
}

/// Write the full markup set into the document: strip managed annotations from every
/// page, keep foreign ones, then append the current set as fresh indirect objects.
pub(crate) fn write_markups(doc: &mut Document, markups: &[Markup]) -> Result<()> {
    let ids: std::collections::HashSet<String> =
        markups.iter().map(|m| m.id().to_string()).collect();
    let pages = doc.get_pages(); // 1-based page no -> page ObjectId

    // Resolve every target page's (rotation, true_width, true_height, origin_x, origin_y)
    // UP FRONT, while `doc` is only borrowed immutably - `PageSpaceCache` can't be held
    // across Phase 2's `doc.add_object` calls (that needs `&mut Document`), so this is
    // plain owned data, not the cache struct itself. See the rotation-interop comment
    // block above for why this conversion exists at all.
    let mut page_space: std::collections::HashMap<ObjectId, (i32, f64, f64, f64, f64)> =
        std::collections::HashMap::new();
    for (page_no, page_id) in &pages {
        let rotation = crate::document::page_ops::page_rotation(doc, page_no - 1)?;
        let (w0, h0, ox, oy) = true_media_box(doc, *page_id)?;
        page_space.insert(*page_id, (rotation, w0, h0, ox, oy));
    }

    // Phase 1: collect surviving foreign entries per page.
    let mut kept: std::collections::BTreeMap<ObjectId, Vec<Object>> =
        std::collections::BTreeMap::new();
    for page_id in pages.values() {
        let annots = page_annots(doc, *page_id)?;
        // Object ids of the foreign annotations a /Popup may legitimately parent to: kept
        // (not redline-managed) and not themselves popups. A redline-owned markup is
        // rewritten to a FRESH object on every save, so a foreign /Popup still parented to
        // one (e.g. a Bluebeam-added popup on a redline arrow) is orphaned and must be
        // dropped - otherwise it shows as a phantom comment note in Bluebeam (G9 defect 4).
        let valid_popup_parents: std::collections::HashSet<ObjectId> = annots
            .iter()
            .filter(|(oid, d)| {
                oid.is_some() && !is_managed(d, &ids) && subtype(d).as_deref() != Some("Popup")
            })
            .filter_map(|(oid, _)| *oid)
            .collect();
        let mut keep = Vec::new();
        for (oid, dict) in annots {
            if is_managed(&dict, &ids) {
                continue;
            }
            if subtype(&dict).as_deref() == Some("Popup")
                && !parent_ref(&dict).is_some_and(|p| valid_popup_parents.contains(&p))
            {
                continue; // orphaned popup - its owning markup is gone or being rewritten
            }
            keep.push(match oid {
                Some(rid) => Object::Reference(rid),
                None => Object::Dictionary(dict),
            });
        }
        kept.insert(*page_id, keep);
    }

    // Collects each group's members' freshly-assigned object ids, in `markups` slice
    // order, for Phase 2.5 below (design doc §3 "Write"). `BTreeMap` keyed by group_id
    // gives deterministic iteration order across saves; the Vec preserves member order
    // within a group so head selection ("first member in slice order") is stable.
    let mut group_members: std::collections::BTreeMap<uuid::Uuid, Vec<ObjectId>> =
        std::collections::BTreeMap::new();

    // Phase 2: append the current markups to their pages as fresh indirect objects.
    for m in markups {
        let page_no = m.page + 1; // store is 0-based, get_pages is 1-based
        let page_id = *pages.get(&page_no).with_context(|| {
            format!(
                "markup {} targets page {} of a {}-page document",
                m.id(),
                m.page,
                pages.len()
            )
        })?;
        // Convert this markup's geometry from PDFium "display" space (what the frontend
        // captured and what `m.geometry` holds everywhere else in the app) into the PDF's
        // TRUE default user space BEFORE building /Rect/AP - see the rotation-interop
        // comment block above `display_to_true`. A borrowed COPY, not `m` itself: nothing
        // outside serialization should ever see transformed coordinates. Identity (a
        // clone with unchanged geometry) for the common case (rotation=0, MediaBox
        // starting at (0,0)) - the vast majority of real pages are byte-for-byte
        // unaffected by this change.
        let (rotation, w0, h0, ox, oy) = page_space[&page_id];
        let m: std::borrow::Cow<Markup> = if rotation == 0 && ox == 0.0 && oy == 0.0 {
            std::borrow::Cow::Borrowed(m)
        } else {
            let mut transformed = m.clone();
            transformed.geometry = map_geometry(&m.geometry, |p| {
                display_to_true(p, rotation, w0, h0, ox, oy)
            });
            std::borrow::Cow::Owned(transformed)
        };
        let m: &Markup = &m;

        // Build the Normal appearance stream first (indirect object), then point the
        // annotation dict's /AP /N at it - PDF requires a stream to be an indirect
        // object (it cannot be embedded inline in a dictionary), so this order is
        // required: to_annotation_dict() itself has no Document to allocate an id from.
        //
        // A PNG-backed Stamp's appearance additionally references an Image XObject
        // (also stream-typed, also requiring its own indirect object - PDF spec 7.3.8).
        // `appearance::build_ap_stream` stays Document-free/pure and returns any such
        // auxiliary images unresolved; THIS is the one place that holds `&mut Document`,
        // so it resolves them (soft-mask first, since the color image's own dict points
        // at it) before finishing the Form stream via `finish_ap_stream`.
        let mut built = appearance::build_ap_stream(m);
        let mut xobject_refs = Dictionary::new();
        for aux in std::mem::take(&mut built.image_xobjects) {
            let mut color = aux.image;
            if let Some(smask) = aux.smask {
                let smask_id = doc.add_object(Object::Stream(smask));
                color.dict.set("SMask", Object::Reference(smask_id));
            }
            let image_id = doc.add_object(Object::Stream(color));
            xobject_refs.set(aux.name, Object::Reference(image_id));
        }
        // A `.btx`-imported Bluebeam stamp's Form XObject graph (`StampAsset::
        // BluebeamFormXObject`, `toolchest::btx` module doc "Stamp artwork"): splice every
        // node in as a real indirect object (see `splice_bb_form_xobject`), then point
        // this resource name at whichever node is the root.
        if let Some(form) = built.form_xobject.take() {
            if let Some(root_oid) = splice_bb_form_xobject(doc, &form.root_id, &form.objects) {
                xobject_refs.set(form.name, Object::Reference(root_oid));
            }
        }
        let ap_id = doc.add_object(Object::Stream(appearance::finish_ap_stream(
            built,
            xobject_refs,
        )));
        let mut dict = m.to_annotation_dict();
        dict.set(
            "AP",
            Object::Dictionary(dictionary! { "N" => Object::Reference(ap_id) }),
        );
        let aid = doc.add_object(Object::Dictionary(dict));
        if let Some(gid) = m.group_id {
            group_members.entry(gid).or_default().push(aid);
        }
        // Invariant: phase 1 inserted an entry for every page id in `pages`, and
        // `page_id` came from `pages`, so the lookup cannot miss.
        kept.get_mut(&page_id)
            .expect("page in map")
            .push(Object::Reference(aid));
    }

    // Phase 2.5: for every group of >= 2 members, pick the first member (in `markups`
    // slice order) as the synthesized head and patch every OTHER member's
    // already-written dict with the standard PDF group-annotation link (ISO 32000-1
    // §12.5.6.2: `/RT /Group` + `/IRT <head's object ref>`) - design doc §3 "Write". A
    // "group" of exactly 1 (a stale/orphaned `group_id` with no surviving sibling this
    // save) gets no `/IRT`/`/RT` - nothing to link to. `/RLGroup` (already written by
    // `to_annotation_dict` above, per-markup) is left untouched as the redline-only
    // fallback.
    for members in group_members.values() {
        let [head, followers @ ..] = members.as_slice() else { continue };
        if followers.is_empty() {
            continue;
        }
        for &follower_aid in followers {
            let dict = doc.get_dictionary_mut(follower_aid).context("group follower dict")?;
            dict.set("RT", Object::Name(b"Group".to_vec()));
            dict.set("IRT", Object::Reference(*head));
        }
    }

    // Phase 3: set each page's /Annots directly (drop any old Reference indirection).
    for (page_id, entries) in kept {
        let page = doc.get_dictionary_mut(page_id).context("page dict")?;
        if entries.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(entries));
        }
    }

    // Replaced managed annot objects are now unreferenced; drop them so repeated
    // saves do not grow the file (same pattern as render normalize).
    doc.prune_objects();
    Ok(())
}

/// Read all markup-like annotations. Page index (0-based) comes from the page tree.
pub fn read_markups(doc: &Document) -> Result<Vec<Markup>> {
    // Pass 1: collect every markup-subtype annotation dict across the whole document
    // (with its object id, when it has one) BEFORE converting to `Markup` - group
    // membership (`/IRT` + `/RT /Group`) is a cross-annotation relationship that
    // `Markup::from_annotation_dict` cannot see on its own (design doc §3 "Read").
    struct Collected {
        oid: Option<ObjectId>,
        dict: Dictionary,
        page_no_1based: u32,
    }
    let pages = doc.get_pages(); // 1-based page no -> page ObjectId; queried once, reused below
    let mut collected: Vec<Collected> = Vec::new();
    for (&page_no_1based, &page_id) in &pages {
        for (oid, dict) in page_annots(doc, page_id)? {
            let Some(st) = subtype(&dict) else { continue };
            if !MARKUP_SUBTYPES.contains(&st.as_str()) {
                continue;
            }
            collected.push(Collected { oid, dict, page_no_1based });
        }
    }

    // Pass 2: resolve group membership via a union-find over object ids. A follower's
    // `/IRT` points at its group's head; union both into the same component. Multiple
    // followers sharing one head (the star topology every real corpus group uses,
    // including >2-member "compound tool" groups) all land in one component for free.
    let oid_to_index: std::collections::HashMap<ObjectId, usize> = collected
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.oid.map(|oid| (oid, i)))
        .collect();
    let mut parent: Vec<usize> = (0..collected.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    for (i, c) in collected.iter().enumerate() {
        if !rt_is_group(&c.dict) {
            continue;
        }
        let Some(target_oid) = irt_ref(&c.dict) else { continue };
        let Some(&j) = oid_to_index.get(&target_oid) else { continue };
        union(&mut parent, i, j);
    }
    // Assign one fresh synthetic group id per component of size >= 2. Components of
    // size 1 (no group edges, or an unresolvable `/IRT`) are not a group.
    let mut component_size: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..collected.len() {
        *component_size.entry(find(&mut parent, i)).or_insert(0) += 1;
    }
    // Prefer an existing `/RLGroup` value already carried by a component's member, over
    // minting a fresh id, whenever one is present - keeps a redline-authored group's
    // identity STABLE across repeated write/read cycles (write_markups now always
    // stamps real /IRT+/RT alongside /RLGroup, so without this preference every reopen
    // would otherwise synthesize a brand-new, unnecessarily churning id even though
    // membership never changed). `/IRT`+`/RT` still solely decides WHO is grouped
    // (membership); `/RLGroup` only supplies the group's stable VALUE when available -
    // a genuinely foreign (BB-only, no prior redline save) group has no `/RLGroup` on
    // any member and gets a fresh synthesized id, same as before.
    fn existing_rl_group(d: &Dictionary) -> Option<uuid::Uuid> {
        let s = d.get(b"RLGroup").ok()?.as_str().ok()?;
        uuid::Uuid::parse_str(&String::from_utf8_lossy(s)).ok()
    }
    let mut group_ids: std::collections::HashMap<usize, uuid::Uuid> = std::collections::HashMap::new();
    for c in &collected {
        let Some(existing) = existing_rl_group(&c.dict) else { continue };
        // `oid` is None for an inline (non-indirect) annotation dict, which cannot
        // participate in the union-find (nothing can point /IRT at it) - safe to skip.
        let Some(oid) = c.oid else { continue };
        let Some(&i) = oid_to_index.get(&oid) else { continue };
        let root = find(&mut parent, i);
        if component_size.get(&root).copied().unwrap_or(1) < 2 {
            continue; // no surviving group to attach a stable id to
        }
        group_ids.entry(root).or_insert(existing);
    }

    let mut out = Vec::new();
    let mut page_space = PageSpaceCache::new(doc);
    for (i, c) in collected.into_iter().enumerate() {
        let dict = c.dict;
        let page_no_1based = c.page_no_1based;
        let page_id = *pages.get(&page_no_1based).context("page id for collected annotation")?;
        let (rotation, w0, h0, ox, oy) = page_space.get(page_id, page_no_1based - 1)?;
        {
            let mut m = Markup::from_annotation_dict(&dict);
            m.page = page_no_1based - 1;
            // `/IRT`+`/RT /Group` (standards-based, externally visible) takes priority
            // over a bare `/RLGroup` fallback (a prior redline-only save with no group
            // link a foreign viewer can see) whenever both are present - see design doc
            // §3 "Read", point 5.
            let root = find(&mut parent, i);
            if component_size.get(&root).copied().unwrap_or(1) >= 2 {
                let gid = *group_ids.entry(root).or_insert_with(uuid::Uuid::new_v4);
                m.group_id = Some(gid);
            }
            // `from_annotation_dict` recovers geometry in the PDF's TRUE default user
            // space (from /RLRect, /L, /Vertices, /QuadPoints, /InkList, or a legacy/
            // foreign /Rect - all of those are true-space per spec). Convert to PDFium
            // "display" space to match what the rest of the app expects `Markup::geometry`
            // to be (see the rotation-interop comment block above `display_to_true`) -
            // the exact inverse of what `write_markups` does, so redline's own round-trip
            // stays lossless on a rotated page or an offset-origin MediaBox.
            //
            // EXCEPT for a "legacy redline annotation": one with /RLType (redline-owned)
            // but NO /RLCoordV2 marker - written by a pre-2026-08-06 redline that put
            // `self.geometry` straight into /Rect with no display<->true conversion at
            // all. For those, /Rect IS ALREADY effectively display-space (mislabeled as
            // true-space by the old code, not actually true-space per spec). Applying
            // this transform to it would silently re-position every existing markup on a
            // rotated/offset-origin page the instant an upgraded redline reopens the
            // file - moving shapes to a different part of the screen even though the
            // file was never touched. Skipping the transform for legacy annotations
            // preserves their on-screen position exactly as before; `write_markups`
            // always stamps /RLCoordV2 and writes a spec-conformant /Rect on the very
            // next save, so the file self-heals for Bluebeam with zero visual disruption
            // in redline. A genuinely foreign annotation (no /RLType at all, e.g.
            // imported from Bluebeam/Acrobat) is NOT "legacy redline" - its /Rect is
            // real true-space per spec and must always be converted.
            let is_legacy_redline_annotation = dict.has(b"RLType") && !dict.has(b"RLCoordV2");
            if !is_legacy_redline_annotation && (rotation != 0 || ox != 0.0 || oy != 0.0) {
                m.geometry = map_geometry(&m.geometry, |p| {
                    true_to_display(p, rotation, w0, h0, ox, oy)
                });
            }
            if matches!(
                m.markup_type,
                crate::markup::MarkupType::Stamp | crate::markup::MarkupType::StampDynamic
            ) && m.stamp_asset.is_none()
            {
                m.stamp_asset = recover_stamp_asset(doc, &dict);
            }
            out.push(m);
        }
    }
    Ok(out)
}

/// Recover a `StampAsset` from an already-placed Stamp/StampDynamic annotation's own
/// `/AP /N` appearance stream when reading it back (2026-08-08 corpus finding -
/// `Markup::from_annotation_dict` never populated `stamp_asset` on read at all, so EVERY
/// reopened Stamp - redline's own placed stamps on save/reopen, not just foreign
/// Bluebeam ones - lost its artwork and rendered as an empty box; see
/// `markup::annotation::from_annotation_dict`'s `stamp_asset: None`).
///
/// Scoped to the case `write_markups`/`appearance::draw` itself produces for a
/// `StampAsset::PngBase64` - a single Image XObject painted via `Do` in the AP's own
/// `/Resources /XObject`, optionally with a `/SMask` alpha channel - re-encoded back into
/// a `PngBase64` asset so a save/reopen round-trip is lossless. Returns `None` (never
/// panics) on anything else: no `/AP`, a Form XObject (the `.btx`-import
/// `BluebeamFormXObject` case - real Bluebeam artwork recovery from an opened PDF, not
/// just `.btx` import, is a further gap, named not fixed here), multiple images, an
/// unsupported colour space, or a malformed/undecodable stream - all degrade gracefully
/// to the pre-existing bordered-box+label fallback rather than erroring the whole read.
fn recover_stamp_asset(doc: &Document, dict: &Dictionary) -> Option<crate::toolchest::StampAsset> {
    let ap = dict.get(b"AP").ok()?.as_dict().ok()?;
    let n_stream = match ap.get(b"N").ok()? {
        Object::Reference(r) => match doc.get_object(*r).ok()? {
            Object::Stream(s) => s,
            _ => return None,
        },
        Object::Stream(s) => s,
        _ => return None,
    };
    let resources = n_stream.dict.get(b"Resources").ok()?.as_dict().ok()?;
    let xobjects = resources.get(b"XObject").ok()?.as_dict().ok()?;

    // Exactly one Image XObject expected (the shape `draw_stamp_image` writes); a Form
    // XObject present instead (Bluebeam-native artwork) or more than one Image XObject
    // is out of scope for this recovery path.
    let mut image_stream = None;
    for (_, obj) in xobjects.iter() {
        let resolved = match obj {
            Object::Reference(r) => doc.get_object(*r).ok()?,
            other => other,
        };
        let s = resolved.as_stream().ok()?;
        if s.dict.get(b"Subtype").ok()?.as_name().ok()? == b"Image" {
            if image_stream.is_some() {
                return None; // more than one image - not the simple case we can recover
            }
            image_stream = Some(s);
        } else {
            return None; // a Form XObject (or anything else) present - not our case
        }
    }
    let image_stream = image_stream?;

    let width = image_stream.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let height = image_stream.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let color_space = image_stream
        .dict
        .get(b"ColorSpace")
        .ok()?
        .as_name()
        .ok()?
        .to_vec();
    let raw = image_stream.decompressed_content().ok()?;

    let rgb: Vec<u8> = match color_space.as_slice() {
        b"DeviceRGB" => {
            if raw.len() < (width as usize) * (height as usize) * 3 {
                return None;
            }
            raw
        }
        b"DeviceGray" => raw.iter().flat_map(|&g| [g, g, g]).collect(),
        _ => return None, // CMYK/indexed/ICC - not the shape our own writer produces
    };

    let smask_alpha: Option<Vec<u8>> = match image_stream.dict.get(b"SMask") {
        Ok(Object::Reference(r)) => doc.get_object(*r).ok().and_then(|o| match o {
            Object::Stream(s) => s.decompressed_content().ok(),
            _ => None,
        }),
        _ => None,
    };

    let img: image::DynamicImage = match smask_alpha {
        Some(alpha) if alpha.len() >= (width as usize) * (height as usize) => {
            let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
            for (px, a) in rgb.chunks_exact(3).zip(alpha.iter()) {
                rgba.extend_from_slice(px);
                rgba.push(*a);
            }
            image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8)?
        }
        _ => image::RgbImage::from_raw(width, height, rgb).map(image::DynamicImage::ImageRgb8)?,
    };

    let mut bytes: Vec<u8> = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .ok()?;
    Some(crate::toolchest::StampAsset::PngBase64(
        crate::render::base64_encode(&bytes),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupType, UserRef};
    use lopdf::{dictionary, Document, Object, Stream};

    /// Minimal valid one-page PDF built programmatically (no file I/O).
    pub(crate) fn one_page_doc() -> (Document, lopdf::ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            // A page dict without /Resources (even an empty one) is spec-invalid -
            // qpdf --check flags every doc built from this helper as needing repair
            // ("kid 0 Resources is missing or invalid"), found during the 2026-08-21
            // cross-viewer verification pass. Harmless while the only content is
            // empty `BT ET` and markup appearance streams carry their own
            // self-contained /Resources (form XObjects), but it makes every fixture
            // PDF built from this helper non-strictly-conformant - real source PDFs
            // from any authoring tool always carry it, so this was a test-fixture
            // gap, not a production code path issue.
            "Resources" => dictionary! {},
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        (doc, page_id)
    }

    /// Same shape as `one_page_doc`, but encrypted with the given owner/user passwords
    /// (RC4-128, revision 3 - `EncryptionVersion::V2`, the widest cross-reader-compatible
    /// option lopdf 0.36 supports). Used to build a password-protected fixture in-memory
    /// for tests, without any external tool or committed binary fixture.
    ///
    /// lopdf requires the trailer `/ID` to be set before building the encryption state
    /// (it feeds the file-encryption-key derivation) - a fresh doc from `one_page_doc()`
    /// has none, so one is set here.
    ///
    /// Cross-checked against PDFium (via pdfium-render) at implementation time: this
    /// exact recipe opens correctly with the right password and fails with
    /// `PdfiumInternalError::PasswordError` with no/wrong password, on both PDFium and
    /// lopdf's own `decrypt()`.
    pub(crate) fn encrypted_one_page_doc(user_password: &str, owner_password: &str) -> Document {
        use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};

        let (mut doc, _page_id) = one_page_doc();
        let id = Object::string_literal(b"redline-test-fixture-id".to_vec());
        doc.trailer.set("ID", vec![id.clone(), id]);

        let state = EncryptionState::try_from(EncryptionVersion::V2 {
            document: &doc,
            owner_password,
            user_password,
            key_length: 128,
            permissions: Permissions::all(),
        })
        .expect("build encryption state for test fixture");
        doc.encrypt(&state).expect("encrypt test fixture");
        // lopdf 0.44.0 upstream bug: Document::save()'s default cross-reference
        // TYPE for a version-1.5+ doc is a compressed CrossReferenceStream, and
        // reloading a saved *encrypted* PDF that used one loses every object
        // except the /Encrypt dict itself (the stream's offset table comes back
        // unreadable - a cross-reference stream must never itself be encrypted
        // per PDF spec §7.5.8.2, and lopdf's reader/writer mishandle that
        // exemption for encrypted docs). Forcing the classic plaintext xref
        // TABLE sidesteps the bug entirely; unencrypted fixtures are unaffected
        // and keep the (working) default. See PR body / HANDOVER for detail.
        doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
        doc
    }

    /// Same shape as `one_page_doc` but with three page objects in Kids / Count = 3.
    /// Returns the page ObjectIds in page order (index 0..=2).
    fn three_page_doc() -> (Document, Vec<lopdf::ObjectId>) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut page_ids = Vec::new();
        for _ in 0..3 {
            let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Contents" => content_id,
            });
            page_ids.push(page_id);
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().map(|id| (*id).into()).collect::<Vec<Object>>(),
                "Count" => 3,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        (doc, page_ids)
    }

    pub(crate) fn redline_markup(page: u32) -> Markup {
        let mut m = Markup::new(
            MarkupType::Cloud,
            page,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 10.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 40.0 },
            ]),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        m.contents = Some("check clearance".into());
        m
    }

    pub(crate) fn link_dict() -> lopdf::Dictionary {
        dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![0.into(), 0.into(), 100.into(), 20.into()],
        }
    }

    #[test]
    fn reads_markup_annots_skips_links_and_fixes_page_index() {
        let (mut doc, page_id) = one_page_doc();
        let m = redline_markup(7); // wrong page index on purpose - read must override to 0
        let a1 = doc.add_object(Object::Dictionary(m.to_annotation_dict()));
        let a2 = doc.add_object(Object::Dictionary(link_dict()));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![a1.into(), a2.into()]));

        let got = read_markups(&doc).unwrap();
        assert_eq!(got.len(), 1, "Link must not import");
        assert_eq!(got[0].id(), m.id());
        assert_eq!(got[0].markup_type, MarkupType::Cloud);
        assert_eq!(
            got[0].page, 0,
            "page index comes from the page tree, not /RLPage"
        );
        assert_eq!(got[0].contents.as_deref(), Some("check clearance"));
    }

    #[test]
    fn reads_direct_and_referenced_annots_arrays() {
        // /Annots may be a direct array (above) or a Reference to an array object.
        let (mut doc, page_id) = one_page_doc();
        let a1 = doc.add_object(Object::Dictionary(redline_markup(0).to_annotation_dict()));
        let arr_id = doc.add_object(Object::Array(vec![a1.into()]));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Reference(arr_id));
        assert_eq!(read_markups(&doc).unwrap().len(), 1);
    }

    #[test]
    fn no_annots_key_reads_empty() {
        let (doc, _) = one_page_doc();
        assert!(read_markups(&doc).unwrap().is_empty());
    }

    #[test]
    fn write_then_read_roundtrips_and_preserves_foreign() {
        let (mut doc, page_id) = one_page_doc();
        // Pre-existing foreign Link on the page.
        let link = doc.add_object(Object::Dictionary(link_dict()));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(vec![link.into()]));

        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        // Our markup reads back; the Link is still in the page's /Annots.
        let got = read_markups(&doc).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), m.id());
        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 2, "link + markup");
        assert!(annots
            .iter()
            .any(|(_, d)| subtype(d).as_deref() == Some("Link")));
    }

    /// G9 defect 4: a foreign /Popup parented to a redline markup is orphaned when the markup
    /// is rewritten on save (redline gives it a fresh object id), so it must be dropped -
    /// otherwise it renders as a phantom comment note in Bluebeam (which is where the popup
    /// came from - BB auto-creates one per markup). A /Popup parented to a surviving FOREIGN
    /// annotation is kept (its link stays valid).
    #[test]
    fn orphaned_popup_on_a_managed_markup_is_dropped_foreign_popup_kept() {
        let (mut doc, page_id) = one_page_doc();

        // A foreign Link + its own foreign Popup (both must survive - the link is not rewritten).
        let link_id = doc.add_object(Object::Dictionary(link_dict()));
        let link_popup = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Popup", "Parent" => link_id,
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        }));
        doc.get_dictionary_mut(page_id).unwrap().set(
            "Annots",
            Object::Array(vec![link_id.into(), link_popup.into()]),
        );

        // Write a redline markup, then find the object id it was assigned.
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let managed_id = page_annots(&doc, page_id)
            .unwrap()
            .into_iter()
            .find(|(_, d)| d.has(b"RLType"))
            .and_then(|(oid, _)| oid)
            .expect("managed markup has an object id");

        // Bluebeam-style: a Popup parented to the redline markup (16-char NM, no /RLType).
        let bb_popup = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Popup", "Parent" => managed_id,
            "NM" => Object::string_literal("RMCTWBEQGCYHWCHZ"),
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        }));
        let mut arr = match doc.get_dictionary(page_id).unwrap().get(b"Annots").unwrap() {
            Object::Array(a) => a.clone(),
            other => panic!("unexpected /Annots: {other:?}"),
        };
        arr.push(bb_popup.into());
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Array(arr));

        // Re-save: the managed markup is rewritten to a NEW object -> its popup is orphaned.
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        let popups = annots
            .iter()
            .filter(|(_, d)| subtype(d).as_deref() == Some("Popup"))
            .count();
        assert_eq!(
            popups, 1,
            "only the foreign-parented popup survives, got {annots:?}"
        );
        assert!(
            annots
                .iter()
                .any(|(_, d)| subtype(d).as_deref() == Some("Link")),
            "the foreign Link is kept"
        );
        assert_eq!(
            annots.iter().filter(|(_, d)| d.has(b"RLType")).count(),
            1,
            "the redline markup is still present (once)"
        );
    }

    #[test]
    fn second_write_replaces_not_duplicates() {
        let (mut doc, page_id) = one_page_doc();
        let mut m = redline_markup(0);
        write_markups(&mut doc, &[m.clone()]).unwrap();
        m.contents = Some("edited".into());
        write_markups(&mut doc, &[m.clone()]).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 1, "managed annot replaced, not duplicated");
        let got = read_markups(&doc).unwrap();
        assert_eq!(got[0].contents.as_deref(), Some("edited"));
    }

    #[test]
    fn deleting_from_store_removes_from_pdf() {
        let (mut doc, _) = one_page_doc();
        write_markups(&mut doc, &[redline_markup(0)]).unwrap();
        write_markups(&mut doc, &[]).unwrap(); // markup deleted in the app
        assert!(read_markups(&doc).unwrap().is_empty());
    }

    #[test]
    fn out_of_range_page_errors() {
        let (mut doc, _) = one_page_doc();
        let m = redline_markup(5); // page 5 doesn't exist
        assert!(write_markups(&mut doc, &[m]).is_err());
    }

    #[test]
    fn repeated_writes_do_not_accumulate_objects() {
        let (mut doc, _) = one_page_doc();
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let after_first = doc.objects.len();
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let after_second = doc.objects.len();
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let after_third = doc.objects.len();
        assert_eq!(
            after_second, after_third,
            "object count must reach steady state, not grow per save"
        );
        assert_eq!(
            after_first, after_second,
            "no growth from the first save on"
        );
    }

    #[test]
    fn multi_page_write_targets_correct_page_and_preserves_others() {
        let (mut doc, page_ids) = three_page_doc();
        // Foreign Link on page 1 (index 0).
        let link = doc.add_object(Object::Dictionary(link_dict()));
        doc.get_dictionary_mut(page_ids[0])
            .unwrap()
            .set("Annots", Object::Array(vec![link.into()]));

        // Markup targets page index 1 (the second page).
        let m = redline_markup(1);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let got = read_markups(&doc).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].page, 1, "markup lands on page index 1");

        // Page 1 keeps only the Link; page 2 holds only the markup; page 3 untouched.
        let p1 = page_annots(&doc, page_ids[0]).unwrap();
        assert_eq!(p1.len(), 1, "link survives on page 1");
        assert!(p1
            .iter()
            .any(|(_, d)| subtype(d).as_deref() == Some("Link")));
        let p2 = page_annots(&doc, page_ids[1]).unwrap();
        assert_eq!(p2.len(), 1, "markup on page 2");
        assert!(!doc.get_dictionary(page_ids[2]).unwrap().has(b"Annots"));
    }

    #[test]
    fn corrupt_annots_reference_errors_instead_of_wiping() {
        let (mut doc, page_id) = one_page_doc();
        let missing = (9999u32, 0u16); // ObjectId that does not exist in the document
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Annots", Object::Reference(missing));
        assert!(
            read_markups(&doc).is_err(),
            "corrupt /Annots ref must error, not read as empty"
        );
    }

    // -----------------------------------------------------------------------
    // Bluebeam interop: every managed annotation gets an indirect /AP /N stream.
    // -----------------------------------------------------------------------

    /// Resolve `dict`'s `/AP /N` to the appearance `Stream` it must reference, panicking
    /// with a precise message at every step that can fail (missing key, wrong variant,
    /// dangling reference) so a broken wiring shows exactly where it broke.
    fn resolve_ap_n_stream<'a>(doc: &'a Document, dict: &Dictionary) -> &'a lopdf::Stream {
        let ap = dict
            .get(b"AP")
            .expect("/AP must be present")
            .as_dict()
            .expect("/AP must be a dict");
        let n_ref = match ap.get(b"N").expect("/AP /N must be present") {
            Object::Reference(r) => *r,
            other => panic!("/AP /N must be an INDIRECT reference (PDF streams cannot be inline), got {other:?}"),
        };
        match doc
            .get_object(n_ref)
            .expect("/AP /N reference must resolve")
        {
            Object::Stream(s) => s,
            other => panic!("/AP /N must resolve to a Stream, got {other:?}"),
        }
    }

    #[test]
    fn write_markups_sets_an_indirect_ap_n_form_xobject_with_content() {
        let (mut doc, page_id) = one_page_doc();
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 1);
        let (_, dict) = &annots[0];

        let stream = resolve_ap_n_stream(&doc, dict);
        assert_eq!(
            stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form",
            "/AP /N must be a Form XObject"
        );
        assert_eq!(
            stream.dict.get(b"Type").unwrap().as_name().unwrap(),
            b"XObject"
        );
        let bbox = stream.dict.get(b"BBox").unwrap().as_array().unwrap();
        assert_eq!(bbox.len(), 4);
        assert!(
            !stream.content.is_empty(),
            "appearance content stream must not be empty"
        );
    }

    /// Full pipeline: a PNG-backed Stamp markup's `/AP /N` Resources must reference a REAL
    /// indirect Image XObject in the `Document` (not just an in-memory struct field) - the
    /// annots.rs-level counterpart to the Document-free unit tests in
    /// `markup::appearance::tests` (which only check `build_ap_stream`'s return value).
    #[test]
    fn write_markups_resolves_a_png_stamp_asset_to_a_real_indirect_image_xobject() {
        use crate::toolchest::StampAsset;
        use image::{DynamicImage, ImageBuffer, Rgba};

        // 2x2 RGBA fixture (left column transparent) built the same way the appearance.rs
        // fixtures are, so this test exercises the real `image` crate decode path.
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_fn(2, 2, |x, _y| {
            Rgba([x as u8 * 200, 50, 100, if x == 0 { 0 } else { 255 }])
        }));
        let mut png_bytes: Vec<u8> = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let png_b64 = crate::render::base64_encode(&png_bytes);

        let (mut doc, page_id) = one_page_doc();
        let mut m = Markup::new(
            MarkupType::Stamp,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 10.0, y: 10.0 },
                max: PdfPoint { x: 50.0, y: 30.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        )
        .with_stamp_asset(StampAsset::PngBase64(png_b64));
        m.contents = Some("APPROVED".into());

        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 1);
        let (_, dict) = &annots[0];
        let ap_stream = resolve_ap_n_stream(&doc, dict);

        let content = String::from_utf8(ap_stream.content.clone()).unwrap();
        assert!(
            content.contains("/Im0 Do\n"),
            "AP content must paint the image: {content}"
        );
        assert!(
            !content.contains(" re\n"),
            "a real image stamp must not draw the box fallback: {content}"
        );

        let resources = ap_stream.dict.get(b"Resources").unwrap().as_dict().unwrap();
        let xobjects = resources
            .get(b"XObject")
            .expect("Resources must carry /XObject")
            .as_dict()
            .unwrap();
        let image_ref = match xobjects.get(b"Im0").unwrap() {
            Object::Reference(r) => *r,
            other => panic!("/XObject /Im0 must be an indirect reference, got {other:?}"),
        };
        let image_stream = match doc.get_object(image_ref).unwrap() {
            Object::Stream(s) => s,
            other => panic!("/XObject /Im0 must resolve to a Stream, got {other:?}"),
        };
        assert_eq!(
            image_stream
                .dict
                .get(b"Subtype")
                .unwrap()
                .as_name()
                .unwrap(),
            b"Image"
        );
        assert_eq!(
            image_stream.dict.get(b"Width").unwrap().as_i64().unwrap(),
            2
        );
        assert_eq!(
            image_stream.dict.get(b"Height").unwrap().as_i64().unwrap(),
            2
        );

        // The RGBA source must chain to a real, separately-indirect SMask.
        let smask_ref = match image_stream.dict.get(b"SMask").unwrap() {
            Object::Reference(r) => *r,
            other => panic!("/SMask must be an indirect reference, got {other:?}"),
        };
        let smask_stream = match doc.get_object(smask_ref).unwrap() {
            Object::Stream(s) => s,
            other => panic!("/SMask must resolve to a Stream, got {other:?}"),
        };
        assert_eq!(
            smask_stream
                .dict
                .get(b"ColorSpace")
                .unwrap()
                .as_name()
                .unwrap(),
            b"DeviceGray"
        );
    }

    /// 2026-08-08 corpus finding: `Markup::from_annotation_dict` unconditionally set
    /// `stamp_asset: None` on read - a REAL regression independent of Bluebeam interop,
    /// since it means redline's OWN placed stamps lose their artwork the moment a file
    /// is saved and reopened (not just foreign/Bluebeam-authored ones). Proves the
    /// round-trip: write a Stamp with a real `PngBase64` asset, reload the saved
    /// document, and confirm `read_markups` recovers an equivalent asset from the AP's
    /// own Image XObject rather than rendering an empty box on every reopen.
    #[test]
    fn read_markups_recovers_a_png_stamp_asset_from_its_own_ap_image_xobject() {
        use crate::toolchest::StampAsset;
        use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};

        let img = DynamicImage::ImageRgba8(ImageBuffer::from_fn(3, 2, |x, y| {
            Rgba([
                x as u8 * 80,
                y as u8 * 80,
                200,
                if x == 0 { 0 } else { 255 },
            ])
        }));
        let mut png_bytes: Vec<u8> = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let png_b64 = crate::render::base64_encode(&png_bytes);

        let (mut doc, _page_id) = one_page_doc();
        let m = Markup::new(
            MarkupType::Stamp,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 10.0, y: 10.0 },
                max: PdfPoint { x: 40.0, y: 30.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        )
        .with_stamp_asset(StampAsset::PngBase64(png_b64));
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let read_back = read_markups(&doc).unwrap();
        assert_eq!(read_back.len(), 1);
        let recovered_b64 = match &read_back[0].stamp_asset {
            Some(StampAsset::PngBase64(b64)) => b64.clone(),
            other => panic!("expected a recovered PngBase64 stamp_asset, got {other:?}"),
        };

        // Compare decoded pixels rather than raw bytes/base64 - a re-encode through the
        // `image` crate need not produce byte-identical PNG output, but must be visually
        // lossless (this asset is a raw RGBA round-trip through FlateDecode, no lossy
        // step anywhere in the chain).
        let recovered_bytes = crate::markup::appearance::base64_decode(&recovered_b64).unwrap();
        let recovered_img = image::load_from_memory(&recovered_bytes).unwrap();
        assert_eq!(recovered_img.dimensions(), (3, 2));
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(
                    recovered_img.get_pixel(x, y),
                    img.get_pixel(x, y),
                    "pixel ({x},{y}) must round-trip losslessly"
                );
            }
        }
    }

    #[test]
    fn write_markups_splices_a_bluebeam_form_x_object_graph_into_real_indirect_objects() {
        // A `.btx`-imported Bluebeam stamp (`StampAsset::BluebeamFormXObject`,
        // `toolchest::btx` module doc "Stamp artwork") must, on save, produce a real
        // /AP/N whose /Resources/XObject points at a genuine indirect Form XObject
        // stream - AND have its nested /BBObjPtr_ reference (mirroring the real
        // ExtGState-chain shape found in bench/corpus/btx/my-tools.btx) resolved to a
        // real indirect reference too, not left as an inert placeholder name.
        use crate::toolchest::StampAsset;
        use std::collections::BTreeMap;

        let mut objects = BTreeMap::new();
        // /Length must be present and exact (mirrors every real bench/corpus/btx/ sample -
        // lopdf's stream reader uses it, not "scan for endstream", to know where the body
        // ends).
        objects.insert(
            "ROOTID".to_string(),
            b"<</Type/XObject/Subtype/Form/FormType 1/BBox[0 0 100 100]/Length 4/Resources<</ExtGState/BBObjPtr_GSID>>>>\nstream\nfake\nendstream"
                .to_vec(),
        );
        objects.insert("GSID".to_string(), b"<</Type/ExtGState/OPM 1>>".to_vec());

        let (mut doc, page_id) = one_page_doc();
        let m = Markup::new(
            MarkupType::Stamp,
            0,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 0.0, y: 0.0 },
                max: PdfPoint { x: 100.0, y: 100.0 },
            },
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        )
        .with_stamp_asset(StampAsset::BluebeamFormXObject { root_id: "ROOTID".to_string(), objects });

        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 1);
        let (_, dict) = &annots[0];
        let ap_stream = resolve_ap_n_stream(&doc, dict);

        let content = String::from_utf8(ap_stream.content.clone()).unwrap();
        assert!(content.contains("/Fx0 Do\n"), "AP content must reference the spliced Form: {content}");
        assert!(!content.contains(" re\n"), "a real Form-backed stamp must not draw the box fallback: {content}");

        let resources = ap_stream.dict.get(b"Resources").unwrap().as_dict().unwrap();
        let xobjects = resources.get(b"XObject").expect("Resources must carry /XObject").as_dict().unwrap();
        let fx0_ref = match xobjects.get(b"Fx0").unwrap() {
            Object::Reference(r) => *r,
            other => panic!("/XObject /Fx0 must be an indirect reference, got {other:?}"),
        };
        let fx0 = match doc.get_object(fx0_ref).unwrap() {
            Object::Stream(s) => s,
            other => panic!("/XObject /Fx0 must resolve to a real Form XObject Stream, got {other:?}"),
        };
        assert_eq!(fx0.dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Form");

        // The nested /BBObjPtr_GSID reference (the root's OWN /Resources/ExtGState
        // entry) must also have been resolved to a real indirect reference, not left as
        // a dangling placeholder name - proving multi-level graph splicing, not just the
        // root object.
        let nested_gs = fx0.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"ExtGState").unwrap();
        let nested_gs_ref = match nested_gs {
            Object::Reference(r) => *r,
            other => panic!("nested /BBObjPtr_GSID must resolve to a real reference, not a dangling name: {other:?}"),
        };
        let nested_gs_obj = match doc.get_object(nested_gs_ref).unwrap() {
            Object::Dictionary(d) => d,
            other => panic!("nested ExtGState must resolve to a real Dictionary, got {other:?}"),
        };
        assert_eq!(nested_gs_obj.get(b"Type").unwrap().as_name().unwrap(), b"ExtGState");
    }

    // --- 2026-08-08 Form XObject stamp RENDERING follow-up: build_isolated_form_xobject_pdf ---
    //
    // Pure structural tests only (no PDFium) - the actual rasterization is covered by
    // `render::tests::rasterize_pdf_bytes_*` (PDFIUM_DYNAMIC_LIB_PATH-gated), which feeds
    // THIS function's output into PDFium and asserts on real rendered pixels.

    fn sample_form_xobject_objects() -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut objects = std::collections::BTreeMap::new();
        objects.insert(
            "ROOTID".to_string(),
            b"<</Type/XObject/Subtype/Form/FormType 1/BBox[10 20 110 220]/Length 4/Resources<</ExtGState/BBObjPtr_GSID>>>>\nstream\nfake\nendstream"
                .to_vec(),
        );
        objects.insert("GSID".to_string(), b"<</Type/ExtGState/OPM 1>>".to_vec());
        objects
    }

    #[test]
    fn build_isolated_form_xobject_pdf_sizes_media_box_to_the_form_bbox() {
        let objects = sample_form_xobject_objects();
        let bytes = build_isolated_form_xobject_pdf("ROOTID", &objects).unwrap();

        let doc = Document::load_mem(&bytes).expect("output must be a loadable PDF");
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 1, "must be a single-page document");
        let (_, page_id) = pages.iter().next().unwrap();
        let page = doc.get_dictionary(*page_id).unwrap();
        let media_box: Vec<f64> = page
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap() as f64)
            .collect();
        // BBox was [10 20 110 220] - MediaBox must match exactly (identity placement,
        // no extra scale/translate - the whole point is an isolated 1:1 render).
        assert_eq!(media_box, vec![10.0, 20.0, 110.0, 220.0]);

        let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
        let xobjects = resources.get(b"XObject").unwrap().as_dict().unwrap();
        let fx0_ref = match xobjects.get(b"Fx0").unwrap() {
            Object::Reference(r) => *r,
            other => panic!("/Fx0 must be an indirect reference, got {other:?}"),
        };
        let fx0 = doc.get_object(fx0_ref).unwrap().as_stream().unwrap();
        assert_eq!(fx0.dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Form");
        // Nested /BBObjPtr_GSID must be resolved too (same multi-level splice guarantee
        // as write_markups's own test).
        let nested = fx0.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"ExtGState").unwrap();
        assert!(matches!(nested, Object::Reference(_)), "nested ref must resolve, got {nested:?}");

        let content_id = match page.get(b"Contents").unwrap() {
            Object::Reference(r) => *r,
            other => panic!("/Contents must be an indirect reference, got {other:?}"),
        };
        let content = doc.get_object(content_id).unwrap().as_stream().unwrap();
        let content_str = String::from_utf8(content.content.clone()).unwrap();
        assert!(content_str.contains("/Fx0 Do"), "content stream must paint the form: {content_str}");
    }

    #[test]
    fn build_isolated_form_xobject_pdf_errors_on_missing_root() {
        let objects = sample_form_xobject_objects();
        let err = build_isolated_form_xobject_pdf("NOT_A_REAL_ID", &objects).unwrap_err();
        assert!(
            err.to_string().contains("failed to splice"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn build_isolated_form_xobject_pdf_errors_on_non_form_root() {
        let mut objects = std::collections::BTreeMap::new();
        objects.insert("ROOTID".to_string(), b"<</Type/ExtGState/OPM 1>>".to_vec());
        let err = build_isolated_form_xobject_pdf("ROOTID", &objects).unwrap_err();
        assert!(
            err.to_string().contains("not a stream"),
            "unexpected error message: {err}"
        );
    }

    /// Regression guard: adding `/AP` must not change any of the semantic keys
    /// `to_annotation_dict` already wrote (subtype, geometry incl. `/QuadPoints`, the
    /// private `/RL*` round-trip keys). Compares the dict written into the document
    /// against calling `to_annotation_dict()` directly, key-by-key except `/AP` itself.
    #[test]
    fn write_markups_does_not_alter_any_semantic_key() {
        let (mut doc, page_id) = one_page_doc();
        let quads = vec![[
            PdfPoint { x: 72.0, y: 712.0 },
            PdfPoint { x: 500.0, y: 712.0 },
            PdfPoint { x: 72.0, y: 700.0 },
            PdfPoint { x: 500.0, y: 700.0 },
        ]];
        let m = Markup::new(
            MarkupType::Highlight,
            0,
            MarkupGeometry::Quads(quads),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        let expected = m.to_annotation_dict();

        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let annots = page_annots(&doc, page_id).unwrap();
        let (_, got) = &annots[0];

        for (key, expected_val) in expected.iter() {
            let got_val = got.get(key).unwrap_or_else(|_| {
                panic!("missing semantic key {:?}", String::from_utf8_lossy(key))
            });
            assert_eq!(
                format!("{got_val:?}"),
                format!("{expected_val:?}"),
                "semantic key {:?} must be unchanged by AP wiring",
                String::from_utf8_lossy(key)
            );
        }
        // /AP is the ONLY new key relative to to_annotation_dict()'s own output.
        assert!(got.has(b"AP"), "/AP must be added");
        assert!(
            !expected.has(b"AP"),
            "to_annotation_dict() itself still never sets /AP directly"
        );
    }

    /// Coverage sweep: one representative markup per shape family gets a non-empty `/AP`.
    /// Mirrors the subtype table in `annotation::pdf_subtype`.
    #[test]
    fn write_markups_gives_every_shape_family_a_non_empty_appearance() {
        fn user() -> UserRef {
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            }
        }
        fn markup(t: MarkupType, g: MarkupGeometry) -> Markup {
            Markup::new(t, 0, g, Appearance::default(), user())
        }

        let rect = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 60.0, y: 40.0 },
        };
        let line2 = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 30.0, y: 20.0 },
        ]);
        let poly3 = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 30.0, y: 0.0 },
            PdfPoint { x: 30.0, y: 20.0 },
        ]);
        let ink = MarkupGeometry::Ink(vec![vec![
            PdfPoint { x: 1.0, y: 1.0 },
            PdfPoint { x: 2.0, y: 3.0 },
        ]]);
        let quads = MarkupGeometry::Quads(vec![[
            PdfPoint { x: 0.0, y: 20.0 },
            PdfPoint { x: 40.0, y: 20.0 },
            PdfPoint { x: 0.0, y: 10.0 },
            PdfPoint { x: 40.0, y: 10.0 },
        ]]);
        let point = MarkupGeometry::Point(PdfPoint { x: 5.0, y: 5.0 });

        let markups = vec![
            markup(MarkupType::Rectangle, rect.clone()),
            markup(MarkupType::Ellipse, rect.clone()),
            markup(MarkupType::Line, line2.clone()),
            markup(MarkupType::Arrow, line2.clone()),
            markup(MarkupType::Polygon, poly3.clone()),
            markup(MarkupType::Cloud, poly3.clone()),
            markup(MarkupType::Polyline, poly3.clone()),
            markup(MarkupType::Highlight, quads),
            markup(MarkupType::Ink, ink),
            markup(MarkupType::Text, rect.clone()),
            markup(MarkupType::Callout, line2),
            markup(MarkupType::Stamp, rect),
            markup(MarkupType::MeasurementCount, point),
        ];
        let ids: Vec<_> = markups.iter().map(|m| m.id()).collect();

        let (mut doc, page_id) = one_page_doc();
        write_markups(&mut doc, &markups).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), markups.len());
        for id in ids {
            let (_, dict) = annots
                .iter()
                .find(|(_, d)| {
                    get_string_for_test(d, b"NM").as_deref() == Some(id.to_string().as_str())
                })
                .expect("every markup must be present");
            let stream = resolve_ap_n_stream(&doc, dict);
            assert!(
                !stream.content.is_empty(),
                "markup {id} must get a non-empty appearance (subtype {:?})",
                get_string_for_test(dict, b"Subtype")
            );
        }
    }

    fn get_string_for_test(d: &Dictionary, key: &[u8]) -> Option<String> {
        d.get(key)
            .ok()?
            .as_str()
            .ok()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .or_else(|| {
                d.get(key)
                    .ok()?
                    .as_name()
                    .ok()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
            })
    }

    // -----------------------------------------------------------------------
    // Full-type-matrix round-trip fidelity harness.
    //
    // Builds one Markup per MarkupType with non-default values in every applicable
    // field, writes the whole set into a real lopdf Document via write_markups, reads
    // it back via read_markups, and asserts field-by-field equality. Then writes the
    // REREAD set again and confirms a second reread is a fixed point (idempotence) -
    // no further drift beyond the first write's expected f32/PDF-date rounding.
    //
    // This is the harness that caught two real drift bugs (see PR description):
    // the Measurement payload being dropped entirely on read, and >2-point Polyline
    // geometry on a Line-subtype markup (Line/Arrow/MeasurementLength/
    // MeasurementRadius) being truncated to 2 points by the /L write path.
    // -----------------------------------------------------------------------

    mod fidelity_matrix {
        use super::*;
        use crate::markup::{
            CountSet, CountSymbol, FontSpec, LineStyle, MarkupStatus, Measurement, Origin, Reply,
        };
        use chrono::{DateTime, TimeZone, Utc};

        fn user(name: &str) -> UserRef {
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: name.to_string(),
            }
        }

        fn fixed_ts(secs_offset: i64) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap()
                + chrono::Duration::seconds(secs_offset)
        }

        fn full_appearance() -> Appearance {
            Appearance {
                color: "#3366ff".into(),
                line_weight: 2.5,
                opacity: 0.8,
                fill: Some("#ffeecc".into()),
                line_style: LineStyle::Dashed,
                font: None,
                outline_color: Some("#112233".into()),
                fill_opacity: Some(0.4),
            }
        }

        /// Common non-default envelope: subject/contents/layer/status/origin/audit, all
        /// set to values distinct from every field's default.
        fn matrix_markup(t: MarkupType, geometry: MarkupGeometry, creator: UserRef) -> Markup {
            let mut m = Markup::new(t, 0, geometry, full_appearance(), creator);
            m.subject = Some(format!("{t:?} subject"));
            m.contents = Some(format!("{t:?} contents - non-default note text"));
            m.layer = Some("A-TEST".into());
            m.touch(user("Modifier"));
            m.audit.created_at = fixed_ts(0);
            m.audit.modified_at = fixed_ts(60);
            m.audit.origin = Origin::FieldApp;
            m.workflow.status = MarkupStatus::Accepted;
            m
        }

        fn measurement(depth: Option<f64>, count_value: Option<u32>) -> Measurement {
            let mut cols = std::collections::BTreeMap::new();
            cols.insert("cost_code".to_string(), "03-30-00".to_string());
            cols.insert("trade".to_string(), "electrical".to_string());
            Measurement {
                scale_ref: Some("scale-1/8in=1ft".to_string()),
                raw_measure: 1234.5678,
                unit: "sf".to_string(),
                computed_quantity: 987.654321,
                depth,
                count_value,
                custom_columns: cols,
            }
        }

        /// One Markup per `MarkupType` variant (20 total), each with non-default values
        /// in every field that type applies to. `stamp_asset` is deliberately left unset
        /// on the Stamp/StampDynamic fixtures - it is a documented, already-tested
        /// exception (see the field's doc comment in markup/mod.rs and
        /// `write_markups_resolves_a_png_stamp_asset_to_a_real_indirect_image_xobject`),
        /// not something this harness re-litigates.
        fn full_fixture_set() -> Vec<Markup> {
            let creator = user("Alice");
            let group_a = uuid::Uuid::new_v4();

            let rect = || MarkupGeometry::Rect {
                min: PdfPoint { x: 12.25, y: 34.5 },
                max: PdfPoint {
                    x: 212.75,
                    y: 134.125,
                },
            };
            let line2 = || {
                MarkupGeometry::Polyline(vec![
                    PdfPoint { x: 5.0, y: 5.0 },
                    PdfPoint { x: 305.0, y: 205.0 },
                ])
            };
            // >2 points on a Line-subtype markup - exercises the /L-truncation fix.
            let line3 = || {
                MarkupGeometry::Polyline(vec![
                    PdfPoint { x: 0.0, y: 0.0 },
                    PdfPoint { x: 100.0, y: 40.0 },
                    PdfPoint { x: 220.0, y: 10.0 },
                ])
            };
            let poly3 = || {
                MarkupGeometry::Polyline(vec![
                    PdfPoint { x: 0.0, y: 0.0 },
                    PdfPoint { x: 80.0, y: 0.0 },
                    PdfPoint { x: 40.0, y: 60.0 },
                ])
            };
            let poly4 = || {
                MarkupGeometry::Polyline(vec![
                    PdfPoint { x: 0.0, y: 0.0 },
                    PdfPoint { x: 100.0, y: 0.0 },
                    PdfPoint { x: 100.0, y: 60.0 },
                    PdfPoint { x: 0.0, y: 60.0 },
                ])
            };
            let ink = || {
                MarkupGeometry::Ink(vec![
                    vec![
                        PdfPoint { x: 1.0, y: 1.0 },
                        PdfPoint { x: 6.0, y: 9.0 },
                        PdfPoint { x: 11.0, y: 3.0 },
                    ],
                    vec![PdfPoint { x: 20.0, y: 20.0 }, PdfPoint { x: 25.0, y: 30.0 }],
                ])
            };
            let quads = || {
                MarkupGeometry::Quads(vec![
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
                ])
            };
            let point = || MarkupGeometry::Point(PdfPoint { x: 55.5, y: 66.25 });

            let mut out = Vec::new();

            let mut text = matrix_markup(MarkupType::Text, rect(), creator.clone());
            text.appearance.font = Some(FontSpec {
                family: "Helvetica".into(),
                size_pt: 14.0,
            });
            out.push(text);

            let mut callout = matrix_markup(MarkupType::Callout, poly3(), creator.clone());
            callout.appearance.font = Some(FontSpec {
                family: "Times New Roman".into(),
                size_pt: 11.0,
            });
            out.push(callout);

            out.push(matrix_markup(MarkupType::Cloud, poly4(), creator.clone()));

            let mut r1 = matrix_markup(MarkupType::Rectangle, rect(), creator.clone());
            r1.group_id = Some(group_a);
            out.push(r1);

            let mut r2 = matrix_markup(MarkupType::Ellipse, rect(), creator.clone());
            r2.group_id = Some(group_a); // shares the group with Rectangle above
            out.push(r2);

            out.push(matrix_markup(MarkupType::Polygon, poly3(), creator.clone()));
            out.push(matrix_markup(MarkupType::Line, line3(), creator.clone()));
            out.push(matrix_markup(MarkupType::Arrow, line2(), creator.clone()));
            out.push(matrix_markup(
                MarkupType::Polyline,
                poly3(),
                creator.clone(),
            ));
            out.push(matrix_markup(
                MarkupType::Highlight,
                quads(),
                creator.clone(),
            ));
            out.push(matrix_markup(MarkupType::Ink, ink(), creator.clone()));
            out.push(matrix_markup(MarkupType::Stamp, rect(), creator.clone()));
            out.push(matrix_markup(
                MarkupType::StampDynamic,
                rect(),
                creator.clone(),
            ));

            let mut ml = matrix_markup(MarkupType::MeasurementLength, line2(), creator.clone());
            ml.measurement = Some(measurement(None, None));
            out.push(ml);

            let mut mp = matrix_markup(MarkupType::MeasurementPerimeter, poly4(), creator.clone());
            mp.measurement = Some(measurement(None, None));
            out.push(mp);

            let mut ma = matrix_markup(MarkupType::MeasurementArea, poly4(), creator.clone());
            ma.measurement = Some(measurement(None, None));
            out.push(ma);

            let mut mv = matrix_markup(MarkupType::MeasurementVolume, poly4(), creator.clone());
            mv.measurement = Some(measurement(Some(8.25), None));
            out.push(mv);

            let mut mc = matrix_markup(MarkupType::MeasurementCount, point(), creator.clone());
            mc.measurement = Some(measurement(None, Some(7)));
            mc.count_set = Some(CountSet {
                id: uuid::Uuid::new_v4(),
                name: "Type-A fixture".into(),
                color: mc.appearance.color.clone(),
                symbol: CountSymbol::Star,
            });
            out.push(mc);

            let mut mang = matrix_markup(MarkupType::MeasurementAngle, poly3(), creator.clone());
            mang.measurement = Some(measurement(None, None));
            out.push(mang);

            let mut mr = matrix_markup(MarkupType::MeasurementRadius, line2(), creator.clone());
            mr.measurement = Some(measurement(None, None));
            // Exercise the workflow assignee + comment-thread round-trip (RLWorkflowExtra)
            // on this one markup - no need to repeat it on every fixture.
            mr.workflow.assignee = Some(user("Reviewer"));
            mr.workflow.thread.push(Reply {
                id: uuid::Uuid::new_v4(),
                author: user("Commenter"),
                at: fixed_ts(120),
                contents: "please confirm radius".into(),
            });
            out.push(mr);

            out
        }

        fn assert_pt_close(a: PdfPoint, b: PdfPoint, ctx: &str) {
            assert!(
                (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01,
                "{ctx}: point {a:?} != {b:?}"
            );
        }

        fn assert_geometry_close(a: &MarkupGeometry, b: &MarkupGeometry, ctx: &str) {
            match (a, b) {
                (MarkupGeometry::Point(p), MarkupGeometry::Point(q)) => {
                    assert_pt_close(*p, *q, ctx)
                }
                (MarkupGeometry::Rect { min, max }, MarkupGeometry::Rect { min: m2, max: x2 }) => {
                    assert_pt_close(*min, *m2, ctx);
                    assert_pt_close(*max, *x2, ctx);
                }
                (MarkupGeometry::Polyline(u), MarkupGeometry::Polyline(v)) => {
                    assert_eq!(
                        u.len(),
                        v.len(),
                        "{ctx}: polyline point count {} != {}",
                        u.len(),
                        v.len()
                    );
                    for (p, q) in u.iter().zip(v) {
                        assert_pt_close(*p, *q, ctx);
                    }
                }
                (MarkupGeometry::Ink(u), MarkupGeometry::Ink(v)) => {
                    assert_eq!(u.len(), v.len(), "{ctx}: ink stroke count");
                    for (s, t) in u.iter().zip(v) {
                        assert_eq!(s.len(), t.len(), "{ctx}: ink stroke point count");
                        for (p, q) in s.iter().zip(t) {
                            assert_pt_close(*p, *q, ctx);
                        }
                    }
                }
                (MarkupGeometry::Quads(u), MarkupGeometry::Quads(v)) => {
                    assert_eq!(u.len(), v.len(), "{ctx}: quad count");
                    for (qa, qb) in u.iter().zip(v) {
                        for (p, q) in qa.iter().zip(qb) {
                            assert_pt_close(*p, *q, ctx);
                        }
                    }
                }
                _ => panic!("{ctx}: geometry variant mismatch: {a:?} vs {b:?}"),
            }
        }

        /// Field-by-field fidelity check. Numeric fields that pass through a PDF `/Real`
        /// (lopdf f32) use an epsilon; everything else (strings, enums, ids, the JSON-blob
        /// Measurement/workflow-extra fields) must be exactly equal.
        fn assert_markup_fidelity(orig: &Markup, got: &Markup) {
            let ctx = format!("{:?} (id {})", orig.markup_type, orig.id());

            assert_eq!(got.id(), orig.id(), "{ctx}: id");
            assert_eq!(got.markup_type, orig.markup_type, "{ctx}: markup_type");
            assert_eq!(got.page, orig.page, "{ctx}: page");
            assert_geometry_close(&orig.geometry, &got.geometry, &ctx);
            assert_eq!(got.subject, orig.subject, "{ctx}: subject");
            assert_eq!(got.contents, orig.contents, "{ctx}: contents");
            assert_eq!(got.layer, orig.layer, "{ctx}: layer");
            assert_eq!(got.group_id, orig.group_id, "{ctx}: group_id");

            assert_eq!(
                got.appearance.color, orig.appearance.color,
                "{ctx}: appearance.color"
            );
            assert!(
                (got.appearance.line_weight - orig.appearance.line_weight).abs() < 0.01,
                "{ctx}: line_weight {} != {}",
                got.appearance.line_weight,
                orig.appearance.line_weight
            );
            assert!(
                (got.appearance.opacity - orig.appearance.opacity).abs() < 0.01,
                "{ctx}: opacity {} != {}",
                got.appearance.opacity,
                orig.appearance.opacity
            );
            assert_eq!(got.appearance.fill, orig.appearance.fill, "{ctx}: fill");
            assert_eq!(
                got.appearance.line_style, orig.appearance.line_style,
                "{ctx}: line_style"
            );
            assert_eq!(got.appearance.font, orig.appearance.font, "{ctx}: font");
            assert_eq!(
                got.appearance.outline_color, orig.appearance.outline_color,
                "{ctx}: outline_color"
            );
            match (got.appearance.fill_opacity, orig.appearance.fill_opacity) {
                (Some(g), Some(o)) => {
                    assert!((g - o).abs() < 0.01, "{ctx}: fill_opacity {g} != {o}")
                }
                (g, o) => assert_eq!(g, o, "{ctx}: fill_opacity"),
            }

            assert_eq!(
                got.workflow.status, orig.workflow.status,
                "{ctx}: workflow.status"
            );
            assert_eq!(
                got.workflow.assignee, orig.workflow.assignee,
                "{ctx}: workflow.assignee"
            );
            assert_eq!(
                got.workflow.thread, orig.workflow.thread,
                "{ctx}: workflow.thread"
            );

            assert_eq!(
                got.audit.created_by, orig.audit.created_by,
                "{ctx}: audit.created_by"
            );
            assert_eq!(
                got.audit.modified_by, orig.audit.modified_by,
                "{ctx}: audit.modified_by"
            );
            assert_eq!(
                got.audit.created_at, orig.audit.created_at,
                "{ctx}: audit.created_at"
            );
            assert_eq!(
                got.audit.modified_at, orig.audit.modified_at,
                "{ctx}: audit.modified_at"
            );
            assert_eq!(
                got.audit.revision, orig.audit.revision,
                "{ctx}: audit.revision"
            );
            assert_eq!(got.audit.origin, orig.audit.origin, "{ctx}: audit.origin");

            assert_eq!(got.measurement, orig.measurement, "{ctx}: measurement");
            assert_eq!(got.count_set, orig.count_set, "{ctx}: count_set");
        }

        #[test]
        fn full_type_matrix_round_trips_every_field_and_is_idempotent_on_a_second_write() {
            let originals = full_fixture_set();
            assert_eq!(
                originals.len(),
                20,
                "fixture set must cover all 20 MarkupType variants"
            );

            // First write -> real Document -> read back.
            let (mut doc1, _page_id) = one_page_doc();
            write_markups(&mut doc1, &originals).unwrap();
            let reread1 = read_markups(&doc1).unwrap();
            assert_eq!(
                reread1.len(),
                originals.len(),
                "every fixture must survive the round-trip"
            );
            for orig in &originals {
                let got = reread1
                    .iter()
                    .find(|m| m.id() == orig.id())
                    .unwrap_or_else(|| {
                        panic!(
                            "{:?} (id {}) missing after round-trip",
                            orig.markup_type,
                            orig.id()
                        )
                    });
                assert_markup_fidelity(orig, got);
            }

            // Idempotence: write the REREAD set again into a fresh document and reread.
            // A second reread must be a fixed point of the first - no further drift
            // beyond the (already-applied) f32/PDF-date rounding from the first write.
            let (mut doc2, _page_id2) = one_page_doc();
            write_markups(&mut doc2, &reread1).unwrap();
            let reread2 = read_markups(&doc2).unwrap();
            assert_eq!(reread2.len(), reread1.len());
            for m1 in &reread1 {
                let m2 = reread2
                    .iter()
                    .find(|m| m.id() == m1.id())
                    .unwrap_or_else(|| {
                        panic!(
                            "{:?} (id {}) missing on second write",
                            m1.markup_type,
                            m1.id()
                        )
                    });
                assert_markup_fidelity(m1, m2);
            }
        }

        /// Emits ONE PDF per `MarkupType` (20 files) plus one "all-types" PDF placing
        /// every fixture from `full_fixture_set()` together on a real project-like
        /// backdrop, for the cross-viewer verification pass (pdfium/mutool/poppler/
        /// Chromium/Bluebeam/Acrobat). Every fixture already carries non-default
        /// color/opacity/line-weight/font/measurement values - see `full_fixture_set`
        /// above; nothing here diverges from what the round-trip test already exercises,
        /// this just serializes the same set to disk instead of re-reading it in-process.
        /// Run on demand:
        ///   cargo test crossviewer_emit -- --ignored --nocapture
        /// Writes into `$REDLINE_CROSSVIEWER_OUT` (default `/tmp/redline-crossviewer`).
        #[test]
        #[ignore]
        fn emit_crossviewer_corpus() {
            use crate::document::save::save_with_markups;
            use std::path::PathBuf;

            let out_dir = std::env::var("REDLINE_CROSSVIEWER_OUT")
                .unwrap_or_else(|_| "/tmp/redline-crossviewer".to_string());
            let out_dir = PathBuf::from(out_dir);
            std::fs::create_dir_all(&out_dir).unwrap();

            let fixtures = full_fixture_set();
            assert_eq!(
                fixtures.len(),
                20,
                "fixture set must cover all 20 MarkupType variants"
            );

            // Synthetic blank-page backdrop, reused for every single-type file.
            let blank_backdrop = out_dir.join("_blank_backdrop.pdf");
            {
                let (mut doc, _pid) = one_page_doc();
                doc.save(&blank_backdrop).unwrap();
            }

            // One PDF per type: a single non-default fixture on the blank backdrop, so
            // each viewer comparison isolates exactly one MarkupType at a time.
            for m in &fixtures {
                let dest = out_dir.join(format!("{:?}.pdf", m.markup_type));
                save_with_markups(&blank_backdrop, &dest, std::slice::from_ref(m)).unwrap();
                eprintln!("wrote {}", dest.display());
            }

            // All-types page: every fixture together, on a real project plan PDF when the
            // BB-interop corpus is present locally (gitignored, machine-local, see
            // bench/README.md), else falling back to the synthetic blank backdrop.
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let real_backdrop =
                manifest_dir.join("../bench/corpus/bb-ref/markup-test-original.pdf");
            let backdrop_src = if real_backdrop.exists() {
                real_backdrop
            } else {
                eprintln!(
                    "NOTE: real BB-interop corpus not found at {}, using synthetic blank backdrop for AllTypes.pdf",
                    real_backdrop.display()
                );
                blank_backdrop.clone()
            };
            let all_types_dest = out_dir.join("AllTypes.pdf");
            save_with_markups(&backdrop_src, &all_types_dest, &fixtures).unwrap();
            eprintln!("wrote {}", all_types_dest.display());
            eprintln!("crossviewer corpus written to: {}", out_dir.display());
        }
    }

    // -----------------------------------------------------------------------
    // Rotation- and MediaBox-origin-interop tests (2026-08-06 fix). Covers exactly the
    // geometry classes the interop investigation named: rotated pages (0/90/180/270,
    // matching the dispatch's "prime suspect 1"), an offset-origin MediaBox (a second,
    // independent effect FOUND during this investigation - see the module doc comment
    // above `display_to_true`), and the two combined. The BBox-padding fix (`ap_bbox`
    // widening `/Rect`) is deliberately orthogonal to these - a Rectangle-type markup with
    // default `line_weight` still gets padded, so these tests read `/RLRect` (the exact,
    // unpadded authored geometry) rather than `/Rect` to isolate what THIS fix changes.
    // -----------------------------------------------------------------------

    mod rotation_interop {
        use super::*;

        fn reals(d: &Dictionary, key: &[u8]) -> Vec<f64> {
            d.get(key)
                .unwrap_or_else(|_| panic!("{} missing", String::from_utf8_lossy(key)))
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o.as_float().unwrap() as f64)
                .collect()
        }

        /// Independent hand-computation of the true-space point for a display-space
        /// point, mirroring `display_to_true`'s derivation but written separately here on
        /// purpose - a test that imports the function under test as its own oracle proves
        /// nothing about correctness, only that the code agrees with itself. Derivation +
        /// empirical verification against real PDFium rendering: the module doc comment
        /// above `display_to_true` in the parent module.
        fn expected_true(
            display: PdfPoint,
            rotation: i32,
            w0: f64,
            h0: f64,
            ox: f64,
            oy: f64,
        ) -> PdfPoint {
            let local = match rotation.rem_euclid(360) {
                90 => PdfPoint {
                    x: w0 - display.y,
                    y: display.x,
                },
                180 => PdfPoint {
                    x: w0 - display.x,
                    y: h0 - display.y,
                },
                270 => PdfPoint {
                    x: display.y,
                    y: h0 - display.x,
                },
                _ => display,
            };
            PdfPoint {
                x: local.x + ox,
                y: local.y + oy,
            }
        }

        fn rect_markup(min: PdfPoint, max: PdfPoint) -> Markup {
            Markup::new(
                MarkupType::Rectangle,
                0,
                MarkupGeometry::Rect { min, max },
                Appearance::default(),
                UserRef {
                    user_id: uuid::Uuid::new_v4(),
                    display_name: "Alice".into(),
                },
            )
        }

        /// A markup authored at the SAME display-space geometry on pages with each of the
        /// four rotations must write `/RLRect` (the exact, unpadded authored geometry) as
        /// the hand-computed TRUE-space box, then round-trip back to the EXACT original
        /// display-space geometry on reread. `min=(20,30) max=(80,130)` on a 612x792
        /// MediaBox with rotation=0 is deliberately the identity case (min==RLRect==
        /// original) - the pre-fix behaviour, still correct and unchanged.
        #[test]
        fn rlrect_uses_true_space_and_round_trips_for_every_rotation() {
            let (min, max) = (
                PdfPoint { x: 20.0, y: 30.0 },
                PdfPoint { x: 80.0, y: 130.0 },
            );
            for rotation in [0, 90, 180, 270] {
                let (mut doc, page_id) = one_page_doc(); // 612x792 MediaBox, rotation 0
                if rotation != 0 {
                    crate::document::page_ops::rotate_page(&mut doc, 0, rotation).unwrap();
                }
                let m = rect_markup(min, max);
                write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

                let annots = page_annots(&doc, page_id).unwrap();
                let (_, dict) = annots
                    .iter()
                    .find(|(_, d)| d.has(b"RLType"))
                    .unwrap_or_else(|| panic!("rotation {rotation}: markup dict present"));

                let exp_min = expected_true(min, rotation, 612.0, 792.0, 0.0, 0.0);
                let exp_max = expected_true(max, rotation, 612.0, 792.0, 0.0, 0.0);
                // A 90/270 rotation can flip which authored corner ends up smaller on a
                // given axis (see `map_geometry`'s Rect re-normalisation) - compare the
                // WRITTEN box's own min/max against the component-wise min/max of the two
                // hand-computed transformed corners, not positionally.
                let got = reals(dict, b"RLRect");
                let want = [
                    exp_min.x.min(exp_max.x),
                    exp_min.y.min(exp_max.y),
                    exp_min.x.max(exp_max.x),
                    exp_min.y.max(exp_max.y),
                ];
                for i in 0..4 {
                    assert!(
                        (got[i] - want[i]).abs() < 1e-3,
                        "rotation {rotation}: /RLRect[{i}] = {} != hand-computed {}",
                        got[i],
                        want[i]
                    );
                }

                let reread = read_markups(&doc).unwrap();
                assert_eq!(reread.len(), 1, "rotation {rotation}: markup survives");
                match &reread[0].geometry {
                    MarkupGeometry::Rect {
                        min: g_min,
                        max: g_max,
                    } => {
                        assert!(
                            (g_min.x - min.x).abs() < 1e-3,
                            "rotation {rotation}: round-trip min.x"
                        );
                        assert!(
                            (g_min.y - min.y).abs() < 1e-3,
                            "rotation {rotation}: round-trip min.y"
                        );
                        assert!(
                            (g_max.x - max.x).abs() < 1e-3,
                            "rotation {rotation}: round-trip max.x"
                        );
                        assert!(
                            (g_max.y - max.y).abs() < 1e-3,
                            "rotation {rotation}: round-trip max.y"
                        );
                    }
                    other => panic!("rotation {rotation}: geometry type changed: {other:?}"),
                }
            }
        }

        /// Second, INDEPENDENT effect found during this investigation (not one of the
        /// dispatch's named suspects): a page whose `/MediaBox` doesn't start at absolute
        /// (0,0) drifts the same way rotation does, with NO rotation involved at all.
        /// Confirmed empirically against real PDFium rendering - see the module doc
        /// comment above `display_to_true`.
        #[test]
        fn offset_origin_mediabox_is_corrected_with_no_rotation_involved() {
            use lopdf::{dictionary, Document, Object, Stream};

            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
            // MediaBox origin at (36,36), NOT (0,0) - a trimmed/cropped sheet.
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![36.into(), 36.into(), 576.into(), 756.into()],
                "Contents" => content_id,
            });
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![page_id.into()],
                    "Count" => 1,
                }),
            );
            let catalog_id =
                doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog_id);

            // `min`/`max` here are, BY THE SAME CONVENTION AS EVERYWHERE ELSE IN THE APP,
            // "PDFium display space" values - i.e. LOCAL to this page's own MediaBox
            // origin (36,36), exactly what a real captured screen click would produce via
            // `get_page_size`/`screenToPdfUserSpace`. There is no way for `write_markups`
            // to distinguish "this Markup was built by a test" from "this Markup was
            // authored by a real click" - and there must not be one, or the conversion
            // would silently depend on how a caller happened to construct the value.
            let (min, max) = (
                PdfPoint { x: 100.0, y: 100.0 },
                PdfPoint { x: 150.0, y: 150.0 },
            );
            let m = rect_markup(min, max);
            write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

            let annots = page_annots(&doc, page_id).unwrap();
            let (_, dict) = annots.iter().find(|(_, d)| d.has(b"RLType")).unwrap();
            let got = reals(dict, b"RLRect");
            // Hand-computed true (absolute) space: rotation=0 so the rotation step is the
            // identity, and the offset is added straight through - true = display + origin.
            let exp_min = expected_true(min, 0, 540.0, 720.0, 36.0, 36.0);
            let exp_max = expected_true(max, 0, 540.0, 720.0, 36.0, 36.0);
            assert!(
                (got[0] - exp_min.x).abs() < 1e-3,
                "/RLRect min.x = {} != {}",
                got[0],
                exp_min.x
            );
            assert!(
                (got[1] - exp_min.y).abs() < 1e-3,
                "/RLRect min.y = {} != {}",
                got[1],
                exp_min.y
            );
            assert!(
                (got[2] - exp_max.x).abs() < 1e-3,
                "/RLRect max.x = {} != {}",
                got[2],
                exp_max.x
            );
            assert!(
                (got[3] - exp_max.y).abs() < 1e-3,
                "/RLRect max.y = {} != {}",
                got[3],
                exp_max.y
            );

            let reread = read_markups(&doc).unwrap();
            match &reread[0].geometry {
                MarkupGeometry::Rect {
                    min: g_min,
                    max: g_max,
                } => {
                    assert!((g_min.x - min.x).abs() < 1e-3);
                    assert!((g_min.y - min.y).abs() < 1e-3);
                    assert!((g_max.x - max.x).abs() < 1e-3);
                    assert!((g_max.y - max.y).abs() < 1e-3);
                }
                other => panic!("geometry type changed: {other:?}"),
            }
        }

        /// The compound case: a rotated page whose `/MediaBox` ALSO doesn't start at
        /// (0,0) - the two effects are independent and must compose without a residual
        /// offset. `ox=20,oy=40`, 90-degree rotation, 592x752 true page (W0=592,H0=752
        /// deliberately different from the other tests' 612x792 so a copy-paste of the
        /// wrong constant would be caught).
        #[test]
        fn rotation_and_offset_origin_compose_correctly() {
            use lopdf::{dictionary, Document, Object, Stream};

            let (w0, h0, ox, oy) = (592.0, 752.0, 20.0, 40.0);
            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![ox.into(), oy.into(), (ox + w0).into(), (oy + h0).into()],
                "Rotate" => 90,
                "Contents" => content_id,
            });
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![page_id.into()],
                    "Count" => 1,
                }),
            );
            let catalog_id =
                doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog_id);

            let (min, max) = (
                PdfPoint { x: 30.0, y: 40.0 },
                PdfPoint { x: 90.0, y: 140.0 },
            );
            let m = rect_markup(min, max);
            write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();

            let annots = page_annots(&doc, page_id).unwrap();
            let (_, dict) = annots.iter().find(|(_, d)| d.has(b"RLType")).unwrap();
            let exp_min = expected_true(min, 90, w0, h0, ox, oy);
            let exp_max = expected_true(max, 90, w0, h0, ox, oy);
            let want = [
                exp_min.x.min(exp_max.x),
                exp_min.y.min(exp_max.y),
                exp_min.x.max(exp_max.x),
                exp_min.y.max(exp_max.y),
            ];
            let got = reals(dict, b"RLRect");
            for i in 0..4 {
                assert!(
                    (got[i] - want[i]).abs() < 1e-3,
                    "/RLRect[{i}] = {} != hand-computed {}",
                    got[i],
                    want[i]
                );
            }

            let reread = read_markups(&doc).unwrap();
            match &reread[0].geometry {
                MarkupGeometry::Rect {
                    min: g_min,
                    max: g_max,
                } => {
                    assert!((g_min.x - min.x).abs() < 1e-3, "round-trip min.x");
                    assert!((g_min.y - min.y).abs() < 1e-3, "round-trip min.y");
                    assert!((g_max.x - max.x).abs() < 1e-3, "round-trip max.x");
                    assert!((g_max.y - max.y).abs() < 1e-3, "round-trip max.y");
                }
                other => panic!("geometry type changed: {other:?}"),
            }
        }

        /// THE bug this whole compatibility marker exists to prevent: a file saved by a
        /// pre-2026-08-06 redline on a rotated page must NOT visually jump to a
        /// different part of the screen the instant it's reopened in a fixed redline.
        ///
        /// Hand-builds a "legacy" annotation exactly as the OLD code would have written
        /// it - `self.geometry` put straight into /Rect with no rotation conversion and
        /// no `/RLCoordV2` marker (the marker did not exist yet) - on a page with
        /// `/Rotate 90`. Proves three things in sequence:
        /// 1. `read_markups` on the untouched legacy file recovers geometry EXACTLY
        ///    equal to the raw /Rect numbers (no transform applied) - the file's
        ///    on-screen appearance in redline is completely unaffected by the fix.
        /// 2. Re-saving that markup (`write_markups`, simulating the user opening and
        ///    saving the file with NO edits) stamps `/RLCoordV2` and writes a NEW,
        ///    spec-conformant `/RLRect` - the self-heal - while the geometry handed to
        ///    `write_markups` (unchanged self.geometry from step 1) is preserved
        ///    on-screen: reading the freshly-saved file back recovers the SAME geometry
        ///    as step 1, proving zero visual disruption across the migration.
        /// 3. The newly-written `/RLRect` matches the hand-computed TRUE-space value
        ///    for that same display-space point - i.e. the file is now ALSO correct for
        ///    Bluebeam, not just visually stable in redline.
        #[test]
        fn legacy_pre_fix_annotation_is_read_unchanged_then_self_heals_on_next_save() {
            let (mut doc, page_id) = one_page_doc(); // 612x792 MediaBox, rotation 0
            crate::document::page_ops::rotate_page(&mut doc, 0, 90).unwrap();

            // Exactly what pre-fix `to_annotation_dict` + `write_markups` would have
            // produced: `self.geometry` (a display-space value the old code never
            // transformed) written straight into /Rect, /RLType present, NO /RLCoordV2 -
            // hand-built rather than going through today's `to_annotation_dict` (which
            // always stamps the marker now) precisely because this must simulate a file
            // that predates the marker's existence.
            let legacy_display_rect = [700.0_f64, 10.0, 750.0, 60.0];
            let legacy_dict = lopdf::dictionary! {
                "Type" => "Annot",
                "Subtype" => "Square",
                "Rect" => legacy_display_rect.iter().map(|v| Object::Real(*v as f32)).collect::<Vec<_>>(),
                "NM" => Object::string_literal("legacy-annot-nm"),
                "RLType" => Object::Name(b"Rectangle".to_vec()),
                "RLGeom" => Object::Name(b"rect".to_vec()),
                // Deliberately no RLRect and no RLCoordV2 - both post-date this "file".
            };
            let aid = doc.add_object(Object::Dictionary(legacy_dict));
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("Annots", Object::Array(vec![Object::Reference(aid)]));

            // Step 1: reading the untouched legacy file must NOT apply the rotation
            // transform - geometry must equal the raw /Rect numbers exactly.
            let read1 = read_markups(&doc).unwrap();
            assert_eq!(read1.len(), 1);
            let MarkupGeometry::Rect { min: m1, max: m2 } = &read1[0].geometry else {
                panic!("geometry type changed");
            };
            assert!(
                (m1.x - legacy_display_rect[0]).abs() < 1e-6,
                "legacy read must be untransformed: min.x"
            );
            assert!(
                (m1.y - legacy_display_rect[1]).abs() < 1e-6,
                "legacy read must be untransformed: min.y"
            );
            assert!(
                (m2.x - legacy_display_rect[2]).abs() < 1e-6,
                "legacy read must be untransformed: max.x"
            );
            assert!(
                (m2.y - legacy_display_rect[3]).abs() < 1e-6,
                "legacy read must be untransformed: max.y"
            );
            let original_geometry = read1[0].geometry.clone();

            // Step 2: re-save with NO edits (the file is "touched" only by opening it).
            write_markups(&mut doc, &read1).unwrap();

            let annots = page_annots(&doc, page_id).unwrap();
            let (_, saved_dict) = annots.iter().find(|(_, d)| d.has(b"RLType")).unwrap();
            assert!(
                saved_dict.has(b"RLCoordV2"),
                "re-save must stamp the migration marker"
            );

            // Step 3: the newly-written /RLRect must be the hand-computed TRUE-space
            // value for the same display-space point (R=90 on a 612x792 page:
            // (x,y) -> (612-y, x); see `display_to_true`'s doc comment for the
            // derivation) - the file is now ALSO correct for Bluebeam.
            let expected_true_min = expected_true(*m1, 90, 612.0, 792.0, 0.0, 0.0);
            let expected_true_max = expected_true(*m2, 90, 612.0, 792.0, 0.0, 0.0);
            let want = [
                expected_true_min.x.min(expected_true_max.x),
                expected_true_min.y.min(expected_true_max.y),
                expected_true_min.x.max(expected_true_max.x),
                expected_true_min.y.max(expected_true_max.y),
            ];
            let got = reals(saved_dict, b"RLRect");
            for i in 0..4 {
                assert!(
                    (got[i] - want[i]).abs() < 1e-3,
                    "post-migration /RLRect[{i}] = {} != hand-computed true-space {}",
                    got[i],
                    want[i]
                );
            }

            // Re-reading the now-migrated file recovers the EXACT SAME display-space
            // geometry as step 1 - zero visual disruption across the self-heal.
            let read2 = read_markups(&doc).unwrap();
            assert_eq!(
                read2[0].geometry, original_geometry,
                "geometry must be pixel-stable across the migration save"
            );
        }
    }

    // --- Grouped markups: /IRT + /RT /Group round-trip (design doc 2026-08-11) ---

    #[test]
    fn write_markups_emits_irt_rt_group_for_a_two_member_group() {
        let (mut doc, page_id) = one_page_doc();
        let gid = uuid::Uuid::new_v4();
        let mut a = redline_markup(0);
        a.group_id = Some(gid);
        let mut b = redline_markup(0);
        b.group_id = Some(gid);
        let markups = vec![a, b];

        write_markups(&mut doc, &markups).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 2);
        let (head_oid, head_dict) = &annots[0];
        let (_, follower_dict) = &annots[1];

        assert!(
            !head_dict.has(b"IRT") && !head_dict.has(b"RT"),
            "the first member in slice order (the synthesized head) must carry neither \
             /IRT nor /RT - matches every real corpus head"
        );
        assert_eq!(
            follower_dict.get(b"RT").unwrap().as_name().unwrap(),
            b"Group",
            "follower must be tagged /RT /Group"
        );
        let irt = follower_dict.get(b"IRT").unwrap();
        assert_eq!(
            irt,
            &Object::Reference(head_oid.expect("head must be an indirect object")),
            "follower's /IRT must reference the head's own object id"
        );
        // /RLGroup stays too - belt-and-braces redline-only fallback (design doc §3).
        assert!(head_dict.has(b"RLGroup") && follower_dict.has(b"RLGroup"));
    }

    #[test]
    fn write_markups_supports_an_n_ary_star_group() {
        // Mirrors the real corpus's 21-member "Consultant Stamp" compound tool: one
        // head, many followers, all pointing /IRT at the SAME head object.
        let (mut doc, page_id) = one_page_doc();
        let gid = uuid::Uuid::new_v4();
        let markups: Vec<Markup> = (0..5)
            .map(|_| {
                let mut m = redline_markup(0);
                m.group_id = Some(gid);
                m
            })
            .collect();

        write_markups(&mut doc, &markups).unwrap();

        let annots = page_annots(&doc, page_id).unwrap();
        assert_eq!(annots.len(), 5);
        let head_oid = annots[0].0.expect("head must be indirect");
        for (oid, dict) in &annots[1..] {
            assert_eq!(dict.get(b"RT").unwrap().as_name().unwrap(), b"Group");
            assert_eq!(dict.get(b"IRT").unwrap(), &Object::Reference(head_oid));
            assert_ne!(*oid, Some(head_oid), "followers must be distinct objects from the head");
        }
    }

    #[test]
    fn write_markups_leaves_an_ungrouped_markup_without_irt_rt() {
        let (mut doc, page_id) = one_page_doc();
        let markups = vec![redline_markup(0)];
        write_markups(&mut doc, &markups).unwrap();
        let annots = page_annots(&doc, page_id).unwrap();
        let (_, dict) = &annots[0];
        assert!(!dict.has(b"IRT") && !dict.has(b"RT"));
    }

    #[test]
    fn write_markups_a_group_of_one_survivor_gets_no_irt_rt() {
        // A group_id with no OTHER member surviving this particular save (e.g. its
        // sibling was deleted) must not crash and must not emit a dangling /IRT.
        let (mut doc, page_id) = one_page_doc();
        let mut m = redline_markup(0);
        m.group_id = Some(uuid::Uuid::new_v4());
        write_markups(&mut doc, &[m]).unwrap();
        let annots = page_annots(&doc, page_id).unwrap();
        assert!(!annots[0].1.has(b"IRT") && !annots[0].1.has(b"RT"));
    }

    /// Build a two-annotation page directly (bypassing `write_markups`) with a real
    /// `/IRT` object reference + `/RT /Group`, mimicking a genuine Bluebeam-authored
    /// PDF - proves `read_markups` recovers group membership from foreign input, not
    /// just from its own prior writes.
    fn two_annot_grouped_doc() -> (Document, lopdf::ObjectId, lopdf::ObjectId, lopdf::ObjectId) {
        let (mut doc, page_id) = one_page_doc();
        let head_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Square",
            "Rect" => vec![0.into(), 0.into(), 50.into(), 50.into()],
            "NM" => Object::string_literal("head-nm"),
            "F" => 4,
        }));
        let follower_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "FreeText",
            "Rect" => vec![5.into(), 5.into(), 45.into(), 45.into()],
            "NM" => Object::string_literal("follower-nm"),
            "F" => 4,
            "RT" => Object::Name(b"Group".to_vec()),
            "IRT" => Object::Reference(head_id),
        }));
        let page = doc.get_dictionary_mut(page_id).unwrap();
        page.set("Annots", Object::Array(vec![Object::Reference(head_id), Object::Reference(follower_id)]));
        (doc, page_id, head_id, follower_id)
    }

    #[test]
    fn read_markups_recovers_group_id_from_foreign_irt_rt() {
        let (doc, _page_id, _head_id, _follower_id) = two_annot_grouped_doc();
        let markups = read_markups(&doc).unwrap();
        assert_eq!(markups.len(), 2);
        assert!(markups[0].group_id.is_some(), "head must be assigned a group_id on read");
        assert_eq!(
            markups[0].group_id, markups[1].group_id,
            "head and follower must share the SAME synthesized group_id"
        );
    }

    #[test]
    fn read_markups_a_lone_annotation_with_no_irt_has_no_group_id() {
        let (mut doc, page_id) = one_page_doc();
        let markups = vec![redline_markup(0)];
        write_markups(&mut doc, &markups).unwrap();
        let read = read_markups(&doc).unwrap();
        assert_eq!(read.len(), 1);
        assert!(read[0].group_id.is_none());
        let _ = page_id;
    }

    #[test]
    fn read_markups_ignores_irt_without_rt_group_an_ordinary_comment_reply() {
        // /IRT alone (no /RT, or /RT /R) is an ordinary comment reply, NOT a group -
        // must not be misread as grouping (design doc §1 "RT is Group... not a rare
        // edge case" - the inverse mistake, treating a reply as a group, would be
        // equally wrong).
        let (mut doc, page_id) = one_page_doc();
        let head_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Text",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "NM" => Object::string_literal("head-nm"), "F" => 4,
        }));
        let reply_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Text",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "NM" => Object::string_literal("reply-nm"), "F" => 4,
            "IRT" => Object::Reference(head_id),
            // no /RT at all - PDF default reply type is /R (comment reply)
        }));
        let page = doc.get_dictionary_mut(page_id).unwrap();
        page.set("Annots", Object::Array(vec![Object::Reference(head_id), Object::Reference(reply_id)]));

        let markups = read_markups(&doc).unwrap();
        assert!(
            markups.iter().all(|m| m.group_id.is_none()),
            "an /IRT with no /RT /Group must not be treated as a group"
        );
    }

    #[test]
    fn full_cycle_group_round_trip_survives_write_read_write_read() {
        let (mut doc, _page_id) = one_page_doc();
        let gid = uuid::Uuid::new_v4();
        let mut a = redline_markup(0);
        a.group_id = Some(gid);
        let mut b = redline_markup(0);
        b.group_id = Some(gid);
        write_markups(&mut doc, &[a, b]).unwrap();

        let read1 = read_markups(&doc).unwrap();
        assert_eq!(read1.len(), 2);
        assert!(read1[0].group_id.is_some());
        assert_eq!(read1[0].group_id, read1[1].group_id, "cycle 1: still grouped");

        // Re-save with no edits, then re-read - the group link must not degrade.
        write_markups(&mut doc, &read1).unwrap();
        let read2 = read_markups(&doc).unwrap();
        assert_eq!(read2.len(), 2);
        assert!(read2[0].group_id.is_some());
        assert_eq!(read2[0].group_id, read2[1].group_id, "cycle 2: still grouped after re-save");
    }
}

