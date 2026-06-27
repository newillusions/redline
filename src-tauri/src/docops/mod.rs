//! DocOps module — swappable flatten/optimize/redact trait (spec §4, §8).
//!
//! M5 scope: `DocOps` trait with v1 baseline implementation (`LopdfDocOps`).
//! Pluggable backend: MuPDF (AGPL — quarantined here behind the trait) or Apryse Advanced
//! slots in without caller changes. See spec §8 and §16 on licensing.
//!
//! # Design
//!
//! The `DocOps` trait operates on raw PDF bytes (in → out) so the backend is fully
//! swappable without touching callers. The Tauri command (`commands::docops`) uses
//! `flatten_annotations` directly for performance (avoids a bytes round-trip), while
//! the trait's `flatten` method is the correct interface for future library / plugin use.
//!
//! # Flatten (v1 — lopdf)
//!
//! Bakes each annotation's Normal appearance stream (`/AP /N`) into the page content
//! as a Form XObject reference, then removes the annotation from the page `/Annots`
//! array.  After flattening, annotations are visible but no longer selectable or editable.
//!
//! Known v1 limitations (acceptable for the baseline):
//! - Only annotations with an *indirect* `/AP /N` stream are flattened; inline appearance
//!   streams are preserved as-is.
//! - Transparency / blend-mode interactions between the baked overlay and existing content
//!   are not resolved (appearance streams may already use blend modes internally — those
//!   are preserved; new top-level compositing is not added).
//! - Annotations without an appearance stream (e.g. pure popup notes) are kept in place.
//!
//! # Optimize / Redact (stubs)
//!
//! Both are left as passthrough stubs for M5. Planned implementations:
//! - `optimize`: compress streams, remove unused objects, optionally linearise.
//! - `redact`: rasterize the region at high DPI, overlay as an Image XObject, remove the
//!   annotation. True vector redaction requires a mature engine (MuPDF / Apryse) behind
//!   the trait — spec §8.

use anyhow::{Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Public trait
// ---------------------------------------------------------------------------

/// The swappable document-surgery backend (spec §8).
pub trait DocOps: Send + Sync {
    /// Flatten annotation appearance streams into page content.
    fn flatten(&self, pdf_bytes: &[u8]) -> Result<Vec<u8>>;

    /// Strip unused objects + recompress streams.
    /// Note: deep image downsampling is out of scope for the v1 free baseline.
    fn optimize(&self, pdf_bytes: &[u8], level: u8) -> Result<Vec<u8>>;

    /// Rasterize-the-region redaction (safe v1 floor — not a drawn black box).
    /// True vector redaction only via a mature engine behind this trait.
    fn redact(&self, pdf_bytes: &[u8], regions: &[RedactRegion]) -> Result<Vec<u8>>;
}

/// A page region to redact (PDF user space).
#[derive(Debug, Clone)]
pub struct RedactRegion {
    pub page_index: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// ---------------------------------------------------------------------------
// v1 lopdf-backed implementation
// ---------------------------------------------------------------------------

/// Baseline v1 `DocOps` implementation backed by `lopdf`.
///
/// Flatten bakes annotation appearances into page content.
/// Optimize and redact are passthrough stubs (see module doc).
pub struct LopdfDocOps;

impl DocOps for LopdfDocOps {
    fn flatten(&self, pdf_bytes: &[u8]) -> Result<Vec<u8>> {
        use std::io::Cursor;
        let mut doc =
            Document::load_from(Cursor::new(pdf_bytes)).context("load PDF from bytes")?;
        flatten_annotations(&mut doc)?;
        let mut out: Vec<u8> = Vec::new();
        doc.save_to(&mut out).context("save PDF to bytes")?;
        Ok(out)
    }

    fn optimize(&self, pdf_bytes: &[u8], _level: u8) -> Result<Vec<u8>> {
        // M5 stub: passthrough.
        // Real implementation: linearise, recompress streams, remove unused objects.
        Ok(pdf_bytes.to_vec())
    }

    fn redact(&self, pdf_bytes: &[u8], _regions: &[RedactRegion]) -> Result<Vec<u8>> {
        // M5 stub: passthrough.
        // Real implementation: rasterize each region, overlay as Image XObject, clear /Annots.
        Ok(pdf_bytes.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Core flatten logic (pub so commands::docops can call it directly)
// ---------------------------------------------------------------------------

/// Flatten all annotation appearance streams in `doc` into page content in place.
///
/// Called by both:
/// - `LopdfDocOps::flatten` (bytes interface, for the trait / library use)
/// - `commands::docops::flatten_document` (in-place, via `apply_page_edit`)
pub fn flatten_annotations(doc: &mut Document) -> Result<()> {
    let page_ids: Vec<ObjectId> = doc.get_pages().values().cloned().collect();
    for page_id in page_ids {
        flatten_page(doc, page_id)?;
    }
    Ok(())
}

/// Flatten all annotation appearance streams on a single page.
fn flatten_page(doc: &mut Document, page_id: ObjectId) -> Result<()> {
    // -----------------------------------------------------------------------
    // Read phase — collect everything we need as owned data; no mutable borrows.
    // -----------------------------------------------------------------------

    // 1. Fetch /Annots array (resolving indirect reference if needed).
    let annots_array: Vec<Object> = {
        let page = doc.get_dictionary(page_id).context("page dict")?;
        match page.get(b"Annots") {
            Ok(Object::Array(a)) => a.clone(),
            Ok(Object::Reference(r)) => {
                let rid = *r;
                match doc.get_object(rid).and_then(|o| o.as_array()) {
                    Ok(a) => a.clone(),
                    Err(_) => return Ok(()), // malformed — leave page unchanged
                }
            }
            _ => return Ok(()), // no /Annots on this page
        }
    };

    if annots_array.is_empty() {
        return Ok(());
    }

    // 2. Inspect each annotation and build the list of what can be flattened.
    struct Target {
        annot_id: ObjectId, // indirect annotation object to remove from /Annots
        ap_n_id: ObjectId,  // /AP /N appearance stream to bake as Form XObject
        rect: [f64; 4],     // [x0 y0 x1 y1] annotation bounding box (page user space)
        bbox: [f64; 4],     // [bx0 by0 bx1 by1] appearance stream BBox
    }

    let mut targets: Vec<Target> = Vec::new();

    for obj in &annots_array {
        // Only process indirect annotation references (direct inline dicts are uncommon
        // and harder to remove; skip them for v1).
        let (annot_id, annot_dict) = match obj {
            Object::Reference(r) => {
                let id = *r;
                match doc.get_object(id) {
                    Ok(Object::Dictionary(d)) => (id, d.clone()),
                    _ => continue,
                }
            }
            _ => continue,
        };

        // Resolve /AP dict.
        let ap_dict: Dictionary = match annot_dict.get(b"AP") {
            Ok(Object::Dictionary(d)) => d.clone(),
            Ok(Object::Reference(r)) => {
                let rid = *r;
                match doc.get_object(rid) {
                    Ok(Object::Dictionary(d)) => d.clone(),
                    _ => continue,
                }
            }
            _ => continue, // no appearance — skip
        };

        // Get the Normal (/N) appearance as an indirect reference.
        // Inline /N streams are skipped for v1 (see module doc).
        let ap_n_id: ObjectId = match ap_dict.get(b"N") {
            Ok(Object::Reference(r)) => *r,
            _ => continue,
        };

        // Read /Rect from the annotation.
        let rect: [f64; 4] = match annot_dict.get(b"Rect") {
            Ok(Object::Array(arr)) if arr.len() >= 4 => {
                let mut r = [0f64; 4];
                for (i, o) in arr.iter().take(4).enumerate() {
                    r[i] = o.as_float().map(|f| f as f64).unwrap_or(0.0);
                }
                r
            }
            _ => continue,
        };

        // Read /BBox from the appearance stream dict.
        let bbox: [f64; 4] = match doc.get_object(ap_n_id) {
            Ok(Object::Stream(s)) => match s.dict.get(b"BBox") {
                Ok(Object::Array(arr)) if arr.len() >= 4 => {
                    let mut b = [0f64; 4];
                    for (i, o) in arr.iter().take(4).enumerate() {
                        b[i] = o.as_float().map(|f| f as f64).unwrap_or(0.0);
                    }
                    b
                }
                _ => [0.0, 0.0, 1.0, 1.0], // fallback unit bbox
            },
            _ => continue, // appearance stream object not found
        };

        // Skip degenerate appearances.
        let bw = bbox[2] - bbox[0];
        let bh = bbox[3] - bbox[1];
        if bw.abs() < 1e-6 || bh.abs() < 1e-6 {
            continue;
        }

        targets.push(Target {
            annot_id,
            ap_n_id,
            rect,
            bbox,
        });
    }

    if targets.is_empty() {
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Build content overlay: one `q … cm /Name Do Q` per annotation.
    // -----------------------------------------------------------------------

    let mut overlay: Vec<u8> = Vec::new();
    let mut xobj_entries: Vec<(String, ObjectId)> = Vec::new(); // (xobj_name, stream_id)

    for (i, t) in targets.iter().enumerate() {
        // Compute the CTM that maps appearance BBox → annotation Rect.
        let bw = t.bbox[2] - t.bbox[0];
        let bh = t.bbox[3] - t.bbox[1];
        let rw = t.rect[2] - t.rect[0];
        let rh = t.rect[3] - t.rect[1];
        let sx = rw / bw; // x scale
        let sy = rh / bh; // y scale
        let tx = t.rect[0] - t.bbox[0] * sx; // x translation
        let ty = t.rect[1] - t.bbox[1] * sy; // y translation

        let xname = format!("RLF{i}");
        // PDF content operator: q sx 0 0 sy tx ty cm /Name Do Q
        overlay.extend_from_slice(
            format!(
                "q {sx} 0 0 {sy} {tx} {ty} cm /{xname} Do Q\n",
                sx = pdf_num(sx),
                sy = pdf_num(sy),
                tx = pdf_num(tx),
                ty = pdf_num(ty),
            )
            .as_bytes(),
        );
        xobj_entries.push((xname, t.ap_n_id));
    }

    // -----------------------------------------------------------------------
    // Mutation phase
    // -----------------------------------------------------------------------

    // 3. Add the overlay as a new content stream object.
    let overlay_id = doc.add_object(Stream::new(dictionary! {}, overlay));

    // 4. Append overlay to page /Contents (converting single ref → array if needed).
    append_to_page_contents(doc, page_id, overlay_id)?;

    // 5. Register each appearance stream as a Form XObject in page /Resources.
    add_xobjects_to_page_resources(doc, page_id, &xobj_entries)?;

    // 6. Remove the flattened annotations from page /Annots.
    let flattened_ids: HashSet<ObjectId> = targets.iter().map(|t| t.annot_id).collect();
    remove_page_annots(doc, page_id, &flattened_ids)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Append `new_id` to the page's `/Contents` array.
/// If `/Contents` was a single indirect reference, it is promoted to an array.
fn append_to_page_contents(
    doc: &mut Document,
    page_id: ObjectId,
    new_id: ObjectId,
) -> Result<()> {
    // Read existing /Contents (owned).
    let existing: Option<Object> = {
        let page = doc.get_dictionary(page_id).context("page dict for contents")?;
        page.get(b"Contents").ok().cloned()
    };

    let new_array: Vec<Object> = match existing {
        Some(Object::Reference(r)) => vec![Object::Reference(r), Object::Reference(new_id)],
        Some(Object::Array(mut arr)) => {
            arr.push(Object::Reference(new_id));
            arr
        }
        _ => vec![Object::Reference(new_id)],
    };

    let page = doc
        .get_dictionary_mut(page_id)
        .context("page dict mut for contents")?;
    page.set("Contents", Object::Array(new_array));
    Ok(())
}

/// Add XObject entries to the page's `/Resources /XObject` dict.
///
/// Resolves indirect `/Resources` and `/XObject` references via clone-then-set,
/// writing the result back as a direct dict on the page.  This may convert an
/// indirect `/Resources` reference into a direct dict — semantically equivalent
/// and correct per PDF spec.
fn add_xobjects_to_page_resources(
    doc: &mut Document,
    page_id: ObjectId,
    xobj_entries: &[(String, ObjectId)],
) -> Result<()> {
    // --- Step A: resolve /Resources to an owned Dictionary ---
    //
    // Three cases: direct dict on page, indirect ref, or absent.
    let (res_is_indirect, res_ref_id): (bool, ObjectId) = {
        let page = doc.get_dictionary(page_id)?;
        match page.get(b"Resources") {
            Ok(Object::Reference(r)) => (true, *r),
            _ => (false, (0, 0)),
        }
    };

    let mut res_dict: Dictionary = if res_is_indirect {
        doc.get_dictionary(res_ref_id)?.clone()
    } else {
        let page = doc.get_dictionary(page_id)?;
        match page.get(b"Resources") {
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        }
    };

    // --- Step B: resolve /XObject sub-dict to an owned Dictionary ---
    //
    // /XObject inside /Resources can itself be an indirect ref or direct dict.
    let (xobj_is_indirect, xobj_ref_id): (bool, ObjectId) = match res_dict.get(b"XObject") {
        Ok(Object::Reference(r)) => (true, *r),
        _ => (false, (0, 0)),
    };

    let mut xobj_dict: Dictionary = if xobj_is_indirect {
        doc.get_dictionary(xobj_ref_id)?.clone()
    } else {
        match res_dict.get(b"XObject") {
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        }
    };

    // --- Step C: add new entries ---
    for (name, obj_id) in xobj_entries {
        xobj_dict.set(name.as_bytes().to_vec(), Object::Reference(*obj_id));
    }

    // --- Step D: write back ---
    //
    // If /XObject was indirect, update it in place; otherwise embed as direct dict.
    if xobj_is_indirect {
        let xd = doc.get_dictionary_mut(xobj_ref_id)?;
        for (name, obj_id) in xobj_entries {
            xd.set(name.as_bytes().to_vec(), Object::Reference(*obj_id));
        }
        // res_dict and the page already reference the same indirect /XObject — done.
    } else {
        res_dict.set("XObject", Object::Dictionary(xobj_dict));
    }

    // Write /Resources back to page (as a direct dict).  If it was indirect, this
    // creates an additional direct copy on the page; the indirect object becomes
    // unreferenced and will be cleaned up on the next compress/linearise pass.
    let page = doc
        .get_dictionary_mut(page_id)
        .context("page dict mut for resources")?;
    page.set("Resources", Object::Dictionary(res_dict));

    Ok(())
}

/// Remove the given annotation indirect object IDs from the page's `/Annots` array.
/// If all annotations are removed, the `/Annots` key is deleted entirely.
fn remove_page_annots(
    doc: &mut Document,
    page_id: ObjectId,
    to_remove: &HashSet<ObjectId>,
) -> Result<()> {
    // Read existing /Annots (owned), resolving indirect ref if needed.
    let (annots_is_indirect, annots_ref_id): (bool, ObjectId) = {
        let page = doc.get_dictionary(page_id)?;
        match page.get(b"Annots") {
            Ok(Object::Reference(r)) => (true, *r),
            _ => (false, (0, 0)),
        }
    };

    let annots: Vec<Object> = if annots_is_indirect {
        match doc.get_object(annots_ref_id)?.as_array() {
            Ok(a) => a.clone(),
            Err(_) => return Ok(()),
        }
    } else {
        let page = doc.get_dictionary(page_id)?;
        match page.get(b"Annots") {
            Ok(Object::Array(a)) => a.clone(),
            _ => return Ok(()),
        }
    };

    // Filter out the flattened annotations.
    let filtered: Vec<Object> = annots
        .into_iter()
        .filter(|obj| match obj {
            Object::Reference(r) => !to_remove.contains(r),
            _ => true, // keep inline dicts and other object types
        })
        .collect();

    let page = doc
        .get_dictionary_mut(page_id)
        .context("page dict mut for annots")?;
    if filtered.is_empty() {
        page.remove(b"Annots");
    } else {
        page.set("Annots", Object::Array(filtered));
    }

    Ok(())
}

/// Format an `f64` as a concise PDF number.
///
/// PDF content streams accept integer and decimal literals.  Prefer integers when
/// the value has no fractional part; otherwise use up to 4 decimal places, stripping
/// trailing zeros.
fn pdf_num(v: f64) -> String {
    if v.fract().abs() < 1e-9 && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        let s = format!("{:.4}", v);
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a minimal single-page document with no annotations.
    fn bare_page_doc() -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
        let page_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0),
                               Object::Integer(612), Object::Integer(792)],
            "Contents" => content_id,
            "Resources" => Object::Dictionary(dictionary! {}),
        }));
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        }));
        doc.trailer.set("Root", catalog_id);
        (doc, page_id)
    }

    /// Build a minimal single-page document with one annotation that has an
    /// indirect `/AP /N` Form XObject appearance stream.
    fn doc_with_ap_annotation() -> (Document, ObjectId, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();

        // Appearance stream — a grey filled square.
        let ap_stream_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![
                    Object::Integer(0), Object::Integer(0),
                    Object::Integer(100), Object::Integer(100),
                ],
                "Resources" => Object::Dictionary(dictionary! {}),
            },
            b"0.5 g 0 0 100 100 re f".to_vec(),
        ));

        // Annotation referencing the appearance stream.
        let annot_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Square",
            "Rect" => vec![
                Object::Real(10.0_f32), Object::Real(10.0_f32),
                Object::Real(110.0_f32), Object::Real(110.0_f32),
            ],
            "AP" => Object::Dictionary(dictionary! {
                "N" => ap_stream_id,
            }),
        }));

        let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
        let page_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0), Object::Integer(0),
                Object::Integer(612), Object::Integer(792),
            ],
            "Contents" => content_id,
            "Resources" => Object::Dictionary(dictionary! {}),
            "Annots" => vec![Object::Reference(annot_id)],
        }));

        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1_i64,
            }),
        );
        let catalog_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        }));
        doc.trailer.set("Root", catalog_id);

        (doc, page_id, annot_id)
    }

    // -----------------------------------------------------------------------
    // pdf_num formatting
    // -----------------------------------------------------------------------

    #[test]
    fn pdf_num_integer_values() {
        assert_eq!(pdf_num(0.0), "0");
        assert_eq!(pdf_num(1.0), "1");
        assert_eq!(pdf_num(-3.0), "-3");
        assert_eq!(pdf_num(612.0), "612");
    }

    #[test]
    fn pdf_num_fractional_values() {
        assert_eq!(pdf_num(0.5), "0.5");
        assert_eq!(pdf_num(1.25), "1.25");
        assert_eq!(pdf_num(0.1234), "0.1234");
    }

    #[test]
    fn pdf_num_strips_trailing_zeros() {
        assert_eq!(pdf_num(1.5000), "1.5");
        assert_eq!(pdf_num(2.1000), "2.1");
    }

    // -----------------------------------------------------------------------
    // flatten_annotations — no-ops
    // -----------------------------------------------------------------------

    #[test]
    fn flatten_noop_on_no_annots() {
        let (mut doc, page_id) = bare_page_doc();
        flatten_annotations(&mut doc).unwrap();

        // /Annots should still be absent.
        let page = doc.get_dictionary(page_id).unwrap();
        assert!(page.get(b"Annots").is_err(), "/Annots should be absent");
    }

    #[test]
    fn flatten_noop_on_annot_without_ap() {
        // An annotation without /AP is kept in place (nothing to bake).
        let (mut doc, page_id) = bare_page_doc();
        let annot_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![
                Object::Integer(0), Object::Integer(0),
                Object::Integer(50), Object::Integer(50),
            ],
        }));
        {
            let page = doc.get_dictionary_mut(page_id).unwrap();
            page.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        }

        flatten_annotations(&mut doc).unwrap();

        // Annotation without /AP must survive (not removed from /Annots).
        let page = doc.get_dictionary(page_id).unwrap();
        let annots = page.get(b"Annots").unwrap().as_array().unwrap();
        assert_eq!(annots.len(), 1, "annotation without /AP must be preserved");
    }

    // -----------------------------------------------------------------------
    // flatten_annotations — with appearance stream
    // -----------------------------------------------------------------------

    #[test]
    fn flatten_removes_annotation_from_annots() {
        let (mut doc, page_id, _annot_id) = doc_with_ap_annotation();
        flatten_annotations(&mut doc).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        // All annotations had appearance streams, so /Annots must be gone.
        assert!(
            page.get(b"Annots").is_err(),
            "/Annots must be absent after all annotations are flattened"
        );
    }

    #[test]
    fn flatten_adds_overlay_content_stream() {
        let (mut doc, page_id, _) = doc_with_ap_annotation();
        flatten_annotations(&mut doc).unwrap();

        // /Contents must now be an array (promoted from single ref + overlay added).
        let page = doc.get_dictionary(page_id).unwrap();
        let contents = page.get(b"Contents").unwrap();
        match contents {
            Object::Array(arr) => {
                assert!(arr.len() >= 2, "/Contents must have at least 2 streams after flatten");
            }
            _ => panic!("expected /Contents to be an array after flatten"),
        }
    }

    #[test]
    fn flatten_adds_xobject_to_resources() {
        let (mut doc, page_id, _) = doc_with_ap_annotation();
        flatten_annotations(&mut doc).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        let res = page.get(b"Resources").unwrap().as_dict().unwrap();
        let xobj = res.get(b"XObject");
        assert!(
            xobj.is_ok(),
            "/Resources must contain /XObject after flatten"
        );
        let xobj_dict = xobj.unwrap().as_dict().unwrap();
        // At least one XObject entry for the baked annotation.
        assert!(
            !xobj_dict.is_empty(),
            "/XObject dict must be non-empty after flatten"
        );
    }

    #[test]
    fn flatten_overlay_stream_contains_do_operator() {
        let (mut doc, page_id, _) = doc_with_ap_annotation();
        flatten_annotations(&mut doc).unwrap();

        // The last content stream (the overlay) must contain a `Do` operator.
        let page = doc.get_dictionary(page_id).unwrap();
        let contents_arr = page.get(b"Contents").unwrap().as_array().unwrap();
        let last_ref = contents_arr.last().unwrap().as_reference().unwrap();
        let last_stream = doc.get_object(last_ref).unwrap().as_stream().unwrap();
        let content_str = std::str::from_utf8(&last_stream.content).unwrap_or("");
        assert!(
            content_str.contains("Do"),
            "overlay stream must contain 'Do' operator; got: {content_str:?}"
        );
        assert!(
            content_str.contains("cm"),
            "overlay stream must contain 'cm' operator; got: {content_str:?}"
        );
    }

    #[test]
    fn flatten_multiple_annotations() {
        // Two annotations on the same page — both must be flattened.
        let (mut doc, page_id, _) = doc_with_ap_annotation();

        // Add a second annotation with /AP /N.
        let ap2_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![Object::Integer(0), Object::Integer(0),
                               Object::Integer(50), Object::Integer(50)],
                "Resources" => Object::Dictionary(dictionary! {}),
            },
            b"1 0 0 RG 0 0 50 50 re S".to_vec(),
        ));
        let annot2_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Square",
            "Rect" => vec![
                Object::Integer(200), Object::Integer(200),
                Object::Integer(250), Object::Integer(250),
            ],
            "AP" => Object::Dictionary(dictionary! { "N" => ap2_id }),
        }));

        // Append second annotation to /Annots.
        {
            let page = doc.get_dictionary_mut(page_id).unwrap();
            match page.get_mut(b"Annots") {
                Ok(Object::Array(arr)) => arr.push(Object::Reference(annot2_id)),
                _ => panic!("expected /Annots array"),
            }
        }

        flatten_annotations(&mut doc).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        assert!(
            page.get(b"Annots").is_err(),
            "both annotations must be flattened"
        );

        // XObject dict must have two entries (RLF0, RLF1).
        let res = page.get(b"Resources").unwrap().as_dict().unwrap();
        let xobj = res.get(b"XObject").unwrap().as_dict().unwrap();
        assert_eq!(xobj.len(), 2, "two XObject entries expected");
    }

    #[test]
    fn flatten_preserves_non_ap_annotations_alongside_ap_ones() {
        // Mix: one annotation with /AP, one without. Only the AP one is flattened.
        let (mut doc, page_id, _) = doc_with_ap_annotation();

        // Add a Text annotation without /AP.
        let text_annot_id = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![
                Object::Integer(300), Object::Integer(300),
                Object::Integer(320), Object::Integer(320),
            ],
        }));

        {
            let page = doc.get_dictionary_mut(page_id).unwrap();
            match page.get_mut(b"Annots") {
                Ok(Object::Array(arr)) => arr.push(Object::Reference(text_annot_id)),
                _ => panic!("expected /Annots array"),
            }
        }

        flatten_annotations(&mut doc).unwrap();

        let page = doc.get_dictionary(page_id).unwrap();
        let annots = page.get(b"Annots").unwrap().as_array().unwrap();
        assert_eq!(annots.len(), 1, "only the non-AP annotation must remain");
        match &annots[0] {
            Object::Reference(r) => assert_eq!(*r, text_annot_id),
            _ => panic!("expected reference"),
        }
    }

    // -----------------------------------------------------------------------
    // LopdfDocOps trait — bytes round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn trait_flatten_bytes_roundtrip() {
        let (mut doc, _, _) = doc_with_ap_annotation();
        // Serialize to bytes.
        let mut bytes: Vec<u8> = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        // Flatten via the trait interface.
        let ops = LopdfDocOps;
        let out_bytes = ops.flatten(&bytes).unwrap();

        // Load the output and verify.
        let out_doc =
            Document::load_from(std::io::Cursor::new(&out_bytes)).unwrap();
        let page_ids: Vec<ObjectId> = out_doc.get_pages().values().cloned().collect();
        assert_eq!(page_ids.len(), 1, "page count unchanged after flatten");

        let page = out_doc.get_dictionary(page_ids[0]).unwrap();
        assert!(
            page.get(b"Annots").is_err(),
            "/Annots must be absent after trait flatten"
        );
    }

    #[test]
    fn trait_optimize_passthrough() {
        let (mut doc, _, _) = doc_with_ap_annotation();
        let mut bytes: Vec<u8> = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let ops = LopdfDocOps;
        let out = ops.optimize(&bytes, 1).unwrap();
        // Stub: output is identical to input.
        assert_eq!(out, bytes);
    }

    #[test]
    fn trait_redact_passthrough() {
        let (mut doc, _, _) = doc_with_ap_annotation();
        let mut bytes: Vec<u8> = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let ops = LopdfDocOps;
        let out = ops.redact(&bytes, &[]).unwrap();
        assert_eq!(out, bytes);
    }
}
