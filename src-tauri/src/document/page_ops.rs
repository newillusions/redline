//! Page-level operations via lopdf: rotate, delete, reorder, insert (spec §4, M4 S1).
//!
//! All operations work on an in-memory lopdf::Document. Callers are responsible for
//! loading from disk, applying these ops, and writing back via the atomic save pipeline.
//!
//! Precision guardrail: never call `as_f32()` on PDF Number objects - always use
//! `as_float()` (f64) to avoid precision loss on integer-valued reals that lopdf
//! serialises without a decimal point.

use anyhow::{bail, Context, Result};
use lopdf::{dictionary, Document, Object, Stream};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the 1-based page number -> ObjectId map, asserting the page tree is non-empty.
fn pages_map(doc: &Document) -> Result<std::collections::BTreeMap<u32, lopdf::ObjectId>> {
    let pages = doc.get_pages();
    if pages.is_empty() {
        bail!("document has no pages");
    }
    Ok(pages)
}

/// Resolve the pages-node (Pages dict) ObjectId from the document catalog.
fn pages_node_id(doc: &Document) -> Result<lopdf::ObjectId> {
    let catalog_id = doc
        .trailer
        .get(b"Root")
        .context("no /Root in trailer")?
        .as_reference()
        .context("/Root is not a reference")?;
    let catalog = doc.get_dictionary(catalog_id).context("catalog dict")?;
    catalog
        .get(b"Pages")
        .context("no /Pages in catalog")?
        .as_reference()
        .context("/Pages is not a reference")
}

// ---------------------------------------------------------------------------
// Public page operations
// ---------------------------------------------------------------------------

/// Apply an incremental rotation to a page, accumulating with any existing /Rotate.
///
/// `degrees` must be a multiple of 90. The new /Rotate is: `(current + degrees) mod 360`.
/// A result of 0 removes the /Rotate key (0 is the PDF spec default).
///
/// `page_idx` is 0-based.
pub fn rotate_page(doc: &mut Document, page_idx: u32, degrees: i32) -> Result<()> {
    if degrees % 90 != 0 {
        bail!("rotation degrees must be a multiple of 90, got {degrees}");
    }
    let pages = pages_map(doc)?;
    let page_no = page_idx + 1;
    let page_id = *pages
        .get(&page_no)
        .with_context(|| format!("page_idx {page_idx} out of range ({} pages)", pages.len()))?;

    // Read existing rotation (0 if absent), add incremental degrees, normalise.
    let existing = {
        let page = doc.get_dictionary(page_id).context("page dict for read")?;
        match page.get(b"Rotate") {
            Ok(obj) => obj.as_i64().unwrap_or(0) as i32,
            Err(_) => 0,
        }
    };
    let rotation = (((existing + degrees) % 360) + 360) % 360;
    let page = doc.get_dictionary_mut(page_id).context("page dict")?;
    if rotation == 0 {
        page.remove(b"Rotate");
    } else {
        page.set("Rotate", Object::Integer(rotation as i64));
    }
    Ok(())
}

/// Read the /Rotate value for a page (0 if absent). 0-based page index.
pub fn page_rotation(doc: &Document, page_idx: u32) -> Result<i32> {
    let pages = pages_map(doc)?;
    let page_no = page_idx + 1;
    let page_id = *pages
        .get(&page_no)
        .with_context(|| format!("page_idx {page_idx} out of range ({} pages)", pages.len()))?;
    let page = doc.get_dictionary(page_id).context("page dict")?;
    match page.get(b"Rotate") {
        Ok(obj) => {
            let v = obj.as_i64().context("/Rotate is not an integer")?;
            Ok(v as i32)
        }
        Err(_) => Ok(0),
    }
}

/// Delete a page from the document (0-based index).
///
/// lopdf uses 1-based page numbers internally; this wrapper converts.
pub fn delete_page(doc: &mut Document, page_idx: u32) -> Result<()> {
    let pages = pages_map(doc)?;
    let page_no = page_idx + 1;
    if !pages.contains_key(&page_no) {
        bail!("page_idx {page_idx} out of range ({} pages)", pages.len());
    }
    if pages.len() == 1 {
        bail!("cannot delete the only page in a document");
    }
    doc.delete_pages(&[page_no]);
    Ok(())
}

/// Reorder the pages of a document.
///
/// `new_order` is a permutation of `0..page_count` (0-based indices) describing
/// the desired page order. `new_order[0]` is the current page index that should
/// become page 1 after the operation.
pub fn reorder_pages(doc: &mut Document, new_order: Vec<u32>) -> Result<()> {
    let pages = pages_map(doc)?;
    let page_count = pages.len() as u32;

    if new_order.len() as u32 != page_count {
        bail!(
            "new_order length {} does not match page count {page_count}",
            new_order.len()
        );
    }

    // Validate: must be a permutation of 0..page_count.
    let mut seen = vec![false; page_count as usize];
    for &idx in &new_order {
        if idx >= page_count {
            bail!("new_order contains index {idx} which is out of range (0..{page_count})");
        }
        if seen[idx as usize] {
            bail!("new_order contains duplicate index {idx}");
        }
        seen[idx as usize] = true;
    }

    // Collect page ObjectIds in the desired order (new_order is 0-based).
    let ordered_ids: Vec<lopdf::ObjectId> = new_order
        .iter()
        .map(|&idx| *pages.get(&(idx + 1)).expect("validated above"))
        .collect();

    // Rewrite the Kids array in the pages node.
    let pages_id = pages_node_id(doc)?;
    let pages_node = doc.get_dictionary_mut(pages_id).context("pages node")?;
    pages_node.set(
        "Kids",
        Object::Array(
            ordered_ids
                .iter()
                .map(|id| Object::Reference(*id))
                .collect(),
        ),
    );
    Ok(())
}

/// Insert a blank page of the given size at position `at` (0-based).
///
/// `at == 0` inserts before the first page. `at == page_count` appends at the end.
/// `width` and `height` are in PDF user space units (points).
pub fn insert_blank_page(doc: &mut Document, at: u32, width: f32, height: f32) -> Result<()> {
    let pages = pages_map(doc)?;
    let page_count = pages.len() as u32;
    if at > page_count {
        bail!("at={at} out of range (0..={page_count})");
    }

    let pages_id = pages_node_id(doc)?;

    // Create a minimal blank page dict.
    let content_id = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
    let new_page_id = doc.add_object(Object::Dictionary(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            Object::Real(0.0_f32),
            Object::Real(0.0_f32),
            Object::Real(width),
            Object::Real(height),
        ],
        "Contents" => content_id,
    }));

    // Read existing Kids, splice the new page in at the correct position.
    let kids: Vec<lopdf::ObjectId> = pages.values().cloned().collect::<Vec<_>>();
    // pages BTreeMap is 1-based ordered, so kids are already in page order.
    let mut new_kids: Vec<Object> = kids.iter().map(|id| Object::Reference(*id)).collect();
    new_kids.insert(at as usize, Object::Reference(new_page_id));

    // Update the pages node: new Kids array + bumped Count.
    let pages_node = doc.get_dictionary_mut(pages_id).context("pages node")?;
    pages_node.set("Kids", Object::Array(new_kids));
    pages_node.set("Count", Object::Integer((page_count + 1) as i64));

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::annots::tests::{one_page_doc, redline_markup};
    use crate::document::annots::{read_markups, write_markups};
    use crate::document::save::{load_markups_from, save_with_markups};
    use lopdf::{dictionary, Document, Object, Stream};

    // ------------------------------------------------------------------
    // Test helpers
    // ------------------------------------------------------------------

    /// Build a minimal N-page lopdf Document.
    pub(crate) fn n_page_doc(n: u32) -> (Document, Vec<lopdf::ObjectId>) {
        assert!(n > 0, "need at least 1 page");
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut page_ids = Vec::new();
        for _ in 0..n {
            let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
            let pid = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Contents" => content_id,
            });
            page_ids.push(pid);
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().map(|id| (*id).into()).collect::<Vec<Object>>(),
                "Count" => n as i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        (doc, page_ids)
    }

    // ------------------------------------------------------------------
    // rotate_page tests
    // ------------------------------------------------------------------

    #[test]
    fn rotate_page_sets_rotate_entry() {
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, 90).unwrap();
        assert_eq!(page_rotation(&doc, 0).unwrap(), 90);
    }

    #[test]
    fn rotate_page_180() {
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, 180).unwrap();
        assert_eq!(page_rotation(&doc, 0).unwrap(), 180);
    }

    #[test]
    fn rotate_page_270() {
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, 270).unwrap();
        assert_eq!(page_rotation(&doc, 0).unwrap(), 270);
    }

    #[test]
    fn rotate_page_four_times_is_identity() {
        let (mut doc, _) = one_page_doc();
        for _ in 0..4 {
            rotate_page(&mut doc, 0, 90).unwrap();
        }
        // 4 x 90 = 360 = 0 mod 360 -> /Rotate removed
        assert_eq!(page_rotation(&doc, 0).unwrap(), 0);
    }

    #[test]
    fn rotate_page_inverse_removes_key() {
        // Rotating by -90 after +90 yields net 0, which removes /Rotate.
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, 90).unwrap();
        rotate_page(&mut doc, 0, -90).unwrap();
        let pages = doc.get_pages();
        let page_id = *pages.get(&1).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        assert!(
            !page.has(b"Rotate"),
            "/Rotate should be absent when net rotation is 0"
        );
    }

    #[test]
    fn rotate_page_zero_noop() {
        // Rotating by 0 leaves existing rotation unchanged.
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, 90).unwrap();
        rotate_page(&mut doc, 0, 0).unwrap();
        assert_eq!(
            page_rotation(&doc, 0).unwrap(),
            90,
            "0-degree increment is a noop"
        );
    }

    #[test]
    fn rotate_page_non_multiple_of_90_errors() {
        let (mut doc, _) = one_page_doc();
        assert!(rotate_page(&mut doc, 0, 45).is_err());
    }

    #[test]
    fn rotate_page_out_of_range_errors() {
        let (mut doc, _) = one_page_doc();
        assert!(rotate_page(&mut doc, 1, 90).is_err());
    }

    #[test]
    fn rotate_page_negative_degrees_normalised() {
        let (mut doc, _) = one_page_doc();
        rotate_page(&mut doc, 0, -90).unwrap();
        assert_eq!(page_rotation(&doc, 0).unwrap(), 270);
    }

    #[test]
    fn rotate_page_only_affects_target_in_multi_page_doc() {
        let (mut doc, _) = n_page_doc(3);
        rotate_page(&mut doc, 1, 90).unwrap();
        assert_eq!(page_rotation(&doc, 0).unwrap(), 0, "page 0 unchanged");
        assert_eq!(page_rotation(&doc, 1).unwrap(), 90, "page 1 rotated");
        assert_eq!(page_rotation(&doc, 2).unwrap(), 0, "page 2 unchanged");
    }

    // ------------------------------------------------------------------
    // delete_page tests
    // ------------------------------------------------------------------

    #[test]
    fn delete_page_decrements_count() {
        let (mut doc, _) = n_page_doc(3);
        delete_page(&mut doc, 1).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }

    #[test]
    fn delete_page_removes_correct_page() {
        // The remaining pages should have the same ObjectIds as the non-deleted ones.
        let (mut doc, page_ids) = n_page_doc(3);
        delete_page(&mut doc, 1).unwrap(); // delete middle page
        let remaining: Vec<_> = doc.get_pages().values().cloned().collect();
        assert!(!remaining.contains(&page_ids[1]), "deleted page gone");
        assert!(remaining.contains(&page_ids[0]), "page 0 present");
        assert!(remaining.contains(&page_ids[2]), "page 2 present");
    }

    #[test]
    fn delete_page_out_of_range_errors() {
        let (mut doc, _) = n_page_doc(2);
        assert!(delete_page(&mut doc, 2).is_err());
        assert!(delete_page(&mut doc, 10).is_err());
    }

    #[test]
    fn delete_last_page_errors() {
        let (mut doc, _) = one_page_doc();
        assert!(delete_page(&mut doc, 0).is_err());
    }

    // ------------------------------------------------------------------
    // reorder_pages tests
    // ------------------------------------------------------------------

    #[test]
    fn reorder_pages_reverses_order() {
        let (mut doc, page_ids) = n_page_doc(3);
        reorder_pages(&mut doc, vec![2, 1, 0]).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages[&1], page_ids[2]);
        assert_eq!(pages[&2], page_ids[1]);
        assert_eq!(pages[&3], page_ids[0]);
    }

    #[test]
    fn reorder_pages_identity_noop() {
        let (mut doc, page_ids) = n_page_doc(3);
        reorder_pages(&mut doc, vec![0, 1, 2]).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages[&1], page_ids[0]);
        assert_eq!(pages[&2], page_ids[1]);
        assert_eq!(pages[&3], page_ids[2]);
    }

    #[test]
    fn reorder_pages_wrong_length_errors() {
        let (mut doc, _) = n_page_doc(3);
        assert!(reorder_pages(&mut doc, vec![0, 1]).is_err());
        assert!(reorder_pages(&mut doc, vec![0, 1, 2, 3]).is_err());
    }

    #[test]
    fn reorder_pages_duplicate_index_errors() {
        let (mut doc, _) = n_page_doc(3);
        assert!(reorder_pages(&mut doc, vec![0, 0, 2]).is_err());
    }

    #[test]
    fn reorder_pages_out_of_range_errors() {
        let (mut doc, _) = n_page_doc(3);
        assert!(reorder_pages(&mut doc, vec![0, 1, 5]).is_err());
    }

    // ------------------------------------------------------------------
    // insert_blank_page tests
    // ------------------------------------------------------------------

    #[test]
    fn insert_blank_page_increments_count() {
        let (mut doc, _) = one_page_doc();
        insert_blank_page(&mut doc, 0, 612.0, 792.0).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
    }

    #[test]
    fn insert_blank_page_at_start() {
        let (mut doc, orig_id) = one_page_doc();
        insert_blank_page(&mut doc, 0, 612.0, 792.0).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 2);
        // Original page is now page 2.
        assert_eq!(pages[&2], orig_id);
    }

    #[test]
    fn insert_blank_page_at_end() {
        let (mut doc, orig_id) = one_page_doc();
        insert_blank_page(&mut doc, 1, 612.0, 792.0).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 2);
        // Original page is still page 1.
        assert_eq!(pages[&1], orig_id);
    }

    #[test]
    fn insert_blank_page_at_middle() {
        let (mut doc, page_ids) = n_page_doc(2);
        insert_blank_page(&mut doc, 1, 595.0, 842.0).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[&1], page_ids[0]);
        assert_ne!(pages[&2], page_ids[0]);
        assert_ne!(pages[&2], page_ids[1]);
        assert_eq!(pages[&3], page_ids[1]);
    }

    #[test]
    fn insert_blank_page_out_of_range_errors() {
        let (mut doc, _) = one_page_doc();
        assert!(insert_blank_page(&mut doc, 2, 612.0, 792.0).is_err());
    }

    // ------------------------------------------------------------------
    // Round-trip tests (critical guardrail)
    // ------------------------------------------------------------------

    /// After rotate: save to disk, reload, assert markup geometry is exactly preserved.
    #[test]
    fn rotate_page_roundtrip_preserves_markups() {
        let (mut doc, _) = n_page_doc(2);
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        rotate_page(&mut doc, 0, 90).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rotated.pdf");
        doc.save(&path).unwrap();

        let reloaded = load_markups_from(&path).unwrap();
        assert_eq!(reloaded.len(), 1, "markup survives rotate round-trip");
        assert_eq!(reloaded[0].id(), m.id());
        // Geometry coordinates must be exactly preserved (no f32 precision loss).
        match (&reloaded[0].geometry, &m.geometry) {
            (
                crate::markup::MarkupGeometry::Polyline(got),
                crate::markup::MarkupGeometry::Polyline(exp),
            ) => {
                for (g, e) in got.iter().zip(exp.iter()) {
                    assert_eq!(g.x, e.x, "x coordinate exact");
                    assert_eq!(g.y, e.y, "y coordinate exact");
                }
            }
            _ => panic!("geometry type changed"),
        }

        // Rotation is present in the reloaded document.
        let reload_doc = Document::load(&path).unwrap();
        assert_eq!(page_rotation(&reload_doc, 0).unwrap(), 90);
    }

    /// After delete: page count correct, surviving markup on remaining pages intact.
    #[test]
    fn delete_page_roundtrip_preserves_markup_on_other_pages() {
        let (mut doc, _) = n_page_doc(3);
        // Put a markup on page 2 (0-based index 1).
        let m = redline_markup(1);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        // Delete page 0.
        delete_page(&mut doc, 0).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deleted.pdf");
        doc.save(&path).unwrap();

        let reload_doc = Document::load(&path).unwrap();
        assert_eq!(reload_doc.get_pages().len(), 2, "page count decremented");

        // The markup is now on what was page index 1 (now page index 0 after delete),
        // but read_markups assigns page from the page tree position.
        let reloaded_markups = read_markups(&reload_doc).unwrap();
        assert_eq!(
            reloaded_markups.len(),
            1,
            "markup survives delete round-trip"
        );
    }

    /// After reorder: markup geometry is exactly preserved.
    #[test]
    fn reorder_pages_roundtrip_preserves_markups() {
        let (mut doc, _) = n_page_doc(3);
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        reorder_pages(&mut doc, vec![2, 0, 1]).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reordered.pdf");
        doc.save(&path).unwrap();

        let reloaded = load_markups_from(&path).unwrap();
        assert_eq!(reloaded.len(), 1, "markup survives reorder round-trip");
        assert_eq!(reloaded[0].id(), m.id());
        match (&reloaded[0].geometry, &m.geometry) {
            (
                crate::markup::MarkupGeometry::Polyline(got),
                crate::markup::MarkupGeometry::Polyline(exp),
            ) => {
                for (g, e) in got.iter().zip(exp.iter()) {
                    assert_eq!(g.x, e.x);
                    assert_eq!(g.y, e.y);
                }
            }
            _ => panic!("geometry type changed"),
        }
    }

    /// After insert_blank_page: page count increases and existing markup geometry
    /// (on the original page) is exactly preserved.
    #[test]
    fn insert_blank_page_roundtrip_preserves_markups() {
        let (mut doc, _) = one_page_doc();
        let m = redline_markup(0);
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        insert_blank_page(&mut doc, 0, 612.0, 792.0).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inserted.pdf");
        doc.save(&path).unwrap();

        let reload_doc = Document::load(&path).unwrap();
        assert_eq!(reload_doc.get_pages().len(), 2, "page count incremented");

        let reloaded = load_markups_from(&path).unwrap();
        assert_eq!(reloaded.len(), 1, "markup survives insert round-trip");
        assert_eq!(reloaded[0].id(), m.id());
        match (&reloaded[0].geometry, &m.geometry) {
            (
                crate::markup::MarkupGeometry::Polyline(got),
                crate::markup::MarkupGeometry::Polyline(exp),
            ) => {
                for (g, e) in got.iter().zip(exp.iter()) {
                    assert_eq!(g.x, e.x);
                    assert_eq!(g.y, e.y);
                }
            }
            _ => panic!("geometry type changed"),
        }
    }

    /// Full save pipeline round-trip via save_with_markups: rotate + save + reload.
    #[test]
    fn rotate_via_save_pipeline_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        let (mut doc, _) = one_page_doc();
        doc.save(&src).unwrap();

        // Use the real atomic save pipeline.
        let m = redline_markup(0);
        save_with_markups(&src, &src, std::slice::from_ref(&m)).unwrap();

        // Reload and rotate via lopdf.
        let mut doc2 = Document::load(&src).unwrap();
        rotate_page(&mut doc2, 0, 180).unwrap();
        doc2.save(&src).unwrap();

        // Verify rotation and markup survive.
        let doc3 = Document::load(&src).unwrap();
        assert_eq!(page_rotation(&doc3, 0).unwrap(), 180);
        let markups = load_markups_from(&src).unwrap();
        assert_eq!(markups.len(), 1);
        assert_eq!(markups[0].id(), m.id());
    }

    // ------------------------------------------------------------------
    // Regression: no f32 precision loss (as_float, not as_f32)
    // ------------------------------------------------------------------

    /// Verify that page coordinates survive a save -> reload cycle via as_float().
    /// lopdf's Object::Real stores f32; as_float() also handles Integer objects.
    /// The annotation.rs convention: always use as_float().ok().map(|f| f as f64) for
    /// geometry reads so Integer-valued numbers (serialised without decimal) are handled.
    #[test]
    fn page_mediabox_coordinates_survive_roundtrip_via_as_float() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mb.pdf");
        let (mut doc, _) = n_page_doc(1);
        doc.save(&path).unwrap();

        let reload = Document::load(&path).unwrap();
        let pages = reload.get_pages();
        let page_id = *pages.get(&1).unwrap();
        let page = reload.get_dictionary(page_id).unwrap();
        let mb = page.get(b"MediaBox").unwrap().as_array().unwrap();
        // as_float() handles both Integer and Real variants (critical for interop).
        let width = mb[2].as_float().unwrap() as f64;
        assert_eq!(width, 612.0_f64, "MediaBox width readable via as_float()");
    }
}
