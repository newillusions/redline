//! lopdf-level read/write of redline markups in a PDF's page /Annots arrays.
//!
//! Managed-annotation policy: an annotation is *managed* (owned/replaced by redline on
//! save) iff it carries an /RLType key OR its /NM matches a markup id in the store.
//! Foreign annotations (links, popups, widgets, third-party markups) are preserved
//! untouched. Import filter: only markup-like subtypes become `Markup`s on read.

use anyhow::{bail, Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::markup::{appearance, Markup};

/// PDF annotation subtypes imported as markups (spec section 6 type set).
const MARKUP_SUBTYPES: &[&str] = &[
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

fn subtype(d: &Dictionary) -> Option<String> {
    d.get(b"Subtype")
        .ok()?
        .as_name()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
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

/// Write the full markup set into the document: strip managed annotations from every
/// page, keep foreign ones, then append the current set as fresh indirect objects.
pub(crate) fn write_markups(doc: &mut Document, markups: &[Markup]) -> Result<()> {
    let ids: std::collections::HashSet<String> =
        markups.iter().map(|m| m.id().to_string()).collect();
    let pages = doc.get_pages(); // 1-based page no -> page ObjectId

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
        // Invariant: phase 1 inserted an entry for every page id in `pages`, and
        // `page_id` came from `pages`, so the lookup cannot miss.
        kept.get_mut(&page_id)
            .expect("page in map")
            .push(Object::Reference(aid));
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
    let mut out = Vec::new();
    for (page_no_1based, page_id) in doc.get_pages() {
        for (_, dict) in page_annots(doc, page_id)? {
            let Some(st) = subtype(&dict) else { continue };
            if !MARKUP_SUBTYPES.contains(&st.as_str()) {
                continue;
            }
            let mut m = Markup::from_annotation_dict(&dict);
            m.page = page_no_1based - 1;
            out.push(m);
        }
    }
    Ok(out)
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
    }
}
