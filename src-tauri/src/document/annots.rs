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
        let mut keep = Vec::new();
        for (oid, dict) in page_annots(doc, *page_id)? {
            if !is_managed(&dict, &ids) {
                keep.push(match oid {
                    Some(rid) => Object::Reference(rid),
                    None => Object::Dictionary(dict),
                });
            }
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
        let ap_id = doc.add_object(Object::Stream(appearance::build_ap_stream(m)));
        let mut dict = m.to_annotation_dict();
        dict.set("AP", Object::Dictionary(dictionary! { "N" => Object::Reference(ap_id) }));
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
        let ap = dict.get(b"AP").expect("/AP must be present").as_dict().expect("/AP must be a dict");
        let n_ref = match ap.get(b"N").expect("/AP /N must be present") {
            Object::Reference(r) => *r,
            other => panic!("/AP /N must be an INDIRECT reference (PDF streams cannot be inline), got {other:?}"),
        };
        match doc.get_object(n_ref).expect("/AP /N reference must resolve") {
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
        assert_eq!(stream.dict.get(b"Type").unwrap().as_name().unwrap(), b"XObject");
        let bbox = stream.dict.get(b"BBox").unwrap().as_array().unwrap();
        assert_eq!(bbox.len(), 4);
        assert!(!stream.content.is_empty(), "appearance content stream must not be empty");
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
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "Alice".into() },
        );
        let expected = m.to_annotation_dict();

        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        let annots = page_annots(&doc, page_id).unwrap();
        let (_, got) = &annots[0];

        for (key, expected_val) in expected.iter() {
            let got_val = got.get(key).unwrap_or_else(|_| panic!("missing semantic key {:?}", String::from_utf8_lossy(key)));
            assert_eq!(
                format!("{got_val:?}"),
                format!("{expected_val:?}"),
                "semantic key {:?} must be unchanged by AP wiring",
                String::from_utf8_lossy(key)
            );
        }
        // /AP is the ONLY new key relative to to_annotation_dict()'s own output.
        assert!(got.has(b"AP"), "/AP must be added");
        assert!(!expected.has(b"AP"), "to_annotation_dict() itself still never sets /AP directly");
    }

    /// Coverage sweep: one representative markup per shape family gets a non-empty `/AP`.
    /// Mirrors the subtype table in `annotation::pdf_subtype`.
    #[test]
    fn write_markups_gives_every_shape_family_a_non_empty_appearance() {
        fn user() -> UserRef {
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "Alice".into() }
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
                .find(|(_, d)| get_string_for_test(d, b"NM").as_deref() == Some(id.to_string().as_str()))
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
        d.get(key).ok()?.as_str().ok().map(|b| String::from_utf8_lossy(b).into_owned())
            .or_else(|| d.get(key).ok()?.as_name().ok().map(|b| String::from_utf8_lossy(b).into_owned()))
    }
}
