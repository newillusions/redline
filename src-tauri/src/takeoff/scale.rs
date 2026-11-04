//! Per-page and per-viewport scale records (spec §7). The sidecar is the source of truth;
//! `ScaleStore` is the in-memory mirror keyed by doc_id.
//!
//! Scale selection (docs/measurement-viewports.md): a measurement uses the scale of the
//! viewport that contains its FIRST point (ISO 32000-1 §12.9 NOTE 1); where viewports
//! overlap the last-drawn one wins (§12.9: "examined, starting with the last one and
//! iterating in reverse"). Outside every viewport the page scale applies, else the
//! document default. See [`scale_at`].

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::geometry::PdfPoint;

/// A rectangle `[x0, y0, x1, y1]` in redline's page space (the space markup geometry is
/// captured in; converted to the PDF's true default user space only when written to `/VP`).
pub type BBox = [f64; 4];

/// What a scale applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ScaleTarget {
    /// One specific page (zero-based index).
    Page { page: u32 },
    /// Fallback for pages without their own scale.
    DocumentDefault,
    /// A named rectangular region of one page with its own scale: a PDF viewport
    /// (ISO 32000-1 §12.9, Table 260), e.g. a 1:20 detail next to a 1:100 plan.
    Viewport {
        page: u32,
        /// Stable identity of the region: survives rename, move and recalibration.
        viewport_id: String,
        name: String,
        bbox: BBox,
    },
}

impl ScaleTarget {
    /// True when `other` is the same slot a recalibration replaces: the same page, the
    /// document default, or the same viewport (by id, whatever its name or rectangle).
    pub fn same_slot(&self, other: &ScaleTarget) -> bool {
        match (self, other) {
            (ScaleTarget::Page { page: a }, ScaleTarget::Page { page: b }) => a == b,
            (ScaleTarget::DocumentDefault, ScaleTarget::DocumentDefault) => true,
            (
                ScaleTarget::Viewport { viewport_id: a, .. },
                ScaleTarget::Viewport { viewport_id: b, .. },
            ) => a == b,
            _ => false,
        }
    }
}

/// `b` with each axis ordered low to high.
pub fn normalise_bbox(b: BBox) -> BBox {
    [
        b[0].min(b[2]),
        b[1].min(b[3]),
        b[0].max(b[2]),
        b[1].max(b[3]),
    ]
}

/// True when `p` lies inside or on the edge of `b`.
pub fn bbox_contains(b: &BBox, p: PdfPoint) -> bool {
    let n = normalise_bbox(*b);
    p.x >= n[0] && p.x <= n[2] && p.y >= n[1] && p.y <= n[3]
}

/// The viewport scale on `page` whose rectangle contains `p`. Records are in drawing order,
/// so the LAST matching one wins (ISO 32000-1 §12.9).
pub fn viewport_at(scales: &[ScaleRecord], page: u32, p: PdfPoint) -> Option<&ScaleRecord> {
    scales.iter().rev().find(|s| {
        !s.retired
            && matches!(&s.applies_to, ScaleTarget::Viewport { page: vp, bbox, .. } if *vp == page && bbox_contains(bbox, p))
    })
}

/// The scale for `page` outside any viewport: the page's own scale, else the document default.
pub fn page_scale(scales: &[ScaleRecord], page: u32) -> Option<&ScaleRecord> {
    scales
        .iter()
        .find(|s| matches!(s.applies_to, ScaleTarget::Page { page: p } if p == page))
        .or_else(|| {
            scales
                .iter()
                .find(|s| matches!(s.applies_to, ScaleTarget::DocumentDefault))
        })
}

/// The scale a measurement starting at `first_point` on `page` uses: the viewport that
/// contains the point, else [`page_scale`]. No point (a geometry without one) = page scale.
pub fn scale_at(
    scales: &[ScaleRecord],
    page: u32,
    first_point: Option<PdfPoint>,
) -> Option<&ScaleRecord> {
    first_point
        .and_then(|p| viewport_at(scales, page, p))
        .or_else(|| page_scale(scales, page))
}

/// How the scale was established.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScaleMethod {
    /// User clicked two known points and entered the real-world distance.
    TwoPoint,
    /// Preset ratio selected from a list (e.g. 1:100).
    Preset,
}

/// A single calibration record (spec §7). `ratio` is real-world units per PDF point
/// (e.g. 0.001 means 1 PDF pt = 0.001 m at 1:1000 scale).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaleRecord {
    pub id: String,
    pub applies_to: ScaleTarget,
    pub method: ScaleMethod,
    /// Real-world units per PDF point (f64). Multiply raw_measure (pts or pts²) by ratio.
    pub ratio: f64,
    pub unit: String,
    pub label: String,
    /// Decimal places when displaying computed_quantity.
    pub precision: u8,
    /// A deleted viewport kept so the measurements taken under it keep their values (they
    /// are never silently recalculated, and undoing the delete restores it). A retired
    /// scale is never picked for a new measurement, listed, or written to the PDF.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
}

impl ScaleRecord {
    /// Create a new scale with a fresh UUID id.
    pub fn new(
        applies_to: ScaleTarget,
        method: ScaleMethod,
        ratio: f64,
        unit: String,
        label: String,
        precision: u8,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            applies_to,
            method,
            ratio,
            unit,
            label,
            precision,
            retired: false,
        }
    }
}

/// In-memory scale store keyed by doc_id. Thread-safe via Mutex in AppState.
#[derive(Debug, Default)]
pub struct ScaleStore {
    inner: std::collections::HashMap<String, Vec<ScaleRecord>>,
    seen: std::collections::HashMap<String, Vec<crate::takeoff::ViewportKey>>,
    notices: std::collections::HashMap<String, Vec<String>>,
}

/// Replace the record with `rec`'s id or target slot in place (dropping any further
/// duplicates), else append it.
pub fn replace_or_push(list: &mut Vec<ScaleRecord>, rec: ScaleRecord) {
    let hit = |r: &ScaleRecord| r.id == rec.id || r.applies_to.same_slot(&rec.applies_to);
    match list.iter().position(hit) {
        Some(i) => {
            let mut j = 0;
            list.retain(|r| {
                let keep = j <= i || !hit(r);
                j += 1;
                keep
            });
            list[i] = rec;
        }
        None => list.push(rec),
    }
}

impl ScaleStore {
    /// Add a scale record for the given document. A record replaces any existing one with
    /// the SAME id or the same target slot ([`ScaleTarget::same_slot`]: a recalibration of
    /// the same page, viewport or document default supersedes the old scale - as the
    /// sidecar and the frontend store do), so there is never an old scale shadowing the new
    /// one. A replacement keeps the replaced record's position: list order is the
    /// viewports' drawing order.
    pub fn add(&mut self, doc_id: &str, rec: ScaleRecord) {
        let list = self.inner.entry(doc_id.to_string()).or_default();
        replace_or_push(list, rec);
    }

    /// The PDF `/VP` entries this document has already imported (so they are not imported
    /// twice, and a save may replace them). See `takeoff::measure::plan_viewport_import`.
    pub fn seen(&self, doc_id: &str) -> &[crate::takeoff::ViewportKey] {
        self.seen.get(doc_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Replace the document's imported `/VP` keys (after a page operation moved them).
    pub fn set_seen(&mut self, doc_id: &str, keys: Vec<crate::takeoff::ViewportKey>) {
        self.seen.insert(doc_id.to_string(), keys);
    }

    /// Queue a message for the user about this document's scales (e.g. the PDF's viewports
    /// could not be saved to the sidecar); the frontend takes and shows them.
    pub fn push_notice(&mut self, doc_id: &str, message: String) {
        self.notices
            .entry(doc_id.to_string())
            .or_default()
            .push(message);
    }

    /// Take (and clear) the document's queued notices.
    pub fn take_notices(&mut self, doc_id: &str) -> Vec<String> {
        self.notices.remove(doc_id).unwrap_or_default()
    }

    /// Record imported `/VP` entries (see [`Self::seen`]).
    pub fn add_seen(&mut self, doc_id: &str, keys: &[crate::takeoff::ViewportKey]) {
        self.seen
            .entry(doc_id.to_string())
            .or_default()
            .extend_from_slice(keys);
    }

    /// True once this document's scales have been loaded (even if it has none).
    pub fn is_loaded(&self, doc_id: &str) -> bool {
        self.inner.contains_key(doc_id)
    }

    /// Replace the document's scales wholesale (used to load the sidecar on open). Later
    /// duplicates of the same target win, matching `add`.
    pub fn seed(&mut self, doc_id: &str, recs: Vec<ScaleRecord>) {
        self.inner.insert(doc_id.to_string(), Vec::new());
        for r in recs {
            self.add(doc_id, r);
        }
    }

    /// Load the document's scales from its sidecar if not loaded yet. A missing or
    /// unreadable sidecar loads as "no scales" (it is retried after `clear`).
    pub fn ensure_loaded(&mut self, doc_id: &str, pdf_path: Option<&std::path::Path>) {
        if self.is_loaded(doc_id) {
            return;
        }
        let meta = pdf_path.and_then(|p| crate::sidecar::load_meta(p).ok());
        let (recs, seen) = meta
            .map(|m| (m.scales, m.pdf_viewports_seen))
            .unwrap_or_default();
        self.seed(doc_id, recs);
        self.seen.insert(doc_id.to_string(), seen);
    }

    /// Forget the document's scales (on close); the sidecar remains the source of truth.
    pub fn clear(&mut self, doc_id: &str) {
        self.inner.remove(doc_id);
        self.seen.remove(doc_id);
    }

    /// Return all scale records for the given document.
    pub fn list(&self, doc_id: &str) -> &[ScaleRecord] {
        self.inner.get(doc_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Delete a scale by id. Returns true if found and removed.
    pub fn delete(&mut self, doc_id: &str, scale_id: &str) -> bool {
        if let Some(list) = self.inner.get_mut(doc_id) {
            let before = list.len();
            list.retain(|r| r.id != scale_id);
            return list.len() < before;
        }
        false
    }

    /// Resolve the effective scale for a given page: page-specific first, then document default.
    pub fn resolve(&self, doc_id: &str, page: u32) -> Option<&ScaleRecord> {
        page_scale(self.inner.get(doc_id)?, page)
    }

    /// Resolve the scale for a measurement whose first point is `p` on `page` ([`scale_at`]).
    pub fn resolve_at(&self, doc_id: &str, page: u32, p: PdfPoint) -> Option<&ScaleRecord> {
        scale_at(self.inner.get(doc_id)?, page, Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scale(applies_to: ScaleTarget, ratio: f64) -> ScaleRecord {
        ScaleRecord::new(
            applies_to,
            ScaleMethod::Preset,
            ratio,
            "m".into(),
            "1:100".into(),
            2,
        )
    }

    #[test]
    fn add_and_list() {
        let mut store = ScaleStore::default();
        store.add("doc1", make_scale(ScaleTarget::DocumentDefault, 0.001));
        assert_eq!(store.list("doc1").len(), 1);
    }

    #[test]
    fn resolve_page_specific_before_default() {
        let mut store = ScaleStore::default();
        store.add("doc1", make_scale(ScaleTarget::DocumentDefault, 0.001));
        store.add("doc1", make_scale(ScaleTarget::Page { page: 3 }, 0.002));
        let r = store.resolve("doc1", 3).unwrap();
        assert!((r.ratio - 0.002).abs() < 1e-9);
    }

    #[test]
    fn resolve_falls_back_to_default() {
        let mut store = ScaleStore::default();
        store.add("doc1", make_scale(ScaleTarget::DocumentDefault, 0.001));
        let r = store.resolve("doc1", 7).unwrap();
        assert!((r.ratio - 0.001).abs() < 1e-9);
    }

    #[test]
    fn resolve_none_when_no_scale() {
        let store = ScaleStore::default();
        assert!(store.resolve("doc1", 0).is_none());
    }

    #[test]
    fn delete_removes_record() {
        let mut store = ScaleStore::default();
        let rec = make_scale(ScaleTarget::DocumentDefault, 0.001);
        let id = rec.id.clone();
        store.add("doc1", rec);
        assert!(store.delete("doc1", &id));
        assert_eq!(store.list("doc1").len(), 0);
    }

    #[test]
    fn add_replaces_a_record_for_the_same_target() {
        let mut st = ScaleStore::default();
        st.add("d", make_scale(ScaleTarget::DocumentDefault, 0.001));
        st.add("d", make_scale(ScaleTarget::Page { page: 2 }, 0.5));
        let b = make_scale(ScaleTarget::DocumentDefault, 0.002);
        let b_id = b.id.clone();
        st.add("d", b);
        assert_eq!(st.list("d").len(), 2, "one default + one page scale");
        assert_eq!(st.resolve("d", 0).unwrap().id, b_id, "the new default wins");
        assert_eq!(st.resolve("d", 2).unwrap().ratio, 0.5);
    }

    #[test]
    fn seed_loads_once_and_clear_forgets() {
        let mut st = ScaleStore::default();
        assert!(!st.is_loaded("d"));
        st.seed("d", vec![]);
        assert!(st.is_loaded("d"), "an empty sidecar still counts as loaded");
        st.seed(
            "d",
            vec![
                make_scale(ScaleTarget::DocumentDefault, 0.001),
                make_scale(ScaleTarget::DocumentDefault, 0.003),
            ],
        );
        assert_eq!(st.list("d").len(), 1);
        assert_eq!(st.resolve("d", 0).unwrap().ratio, 0.003);
        st.clear("d");
        assert!(!st.is_loaded("d"));
    }

    #[test]
    fn a_reopened_document_gets_its_sidecar_scales_back() {
        use crate::sidecar::{load_meta, save_meta};
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("plan.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        // Session 1: calibrate (add_scale persists to the sidecar and the in-memory store).
        let mut meta = load_meta(&pdf).unwrap();
        let rec = make_scale(ScaleTarget::DocumentDefault, 0.004);
        meta.scales.push(rec.clone());
        save_meta(&pdf, &meta).unwrap();
        let mut s1 = ScaleStore::default();
        s1.add("d1", rec.clone());
        s1.clear("d1"); // close
                        // Session 2: fresh store, same file, new doc id.
        let mut s2 = ScaleStore::default();
        s2.ensure_loaded("d2", Some(&pdf));
        assert_eq!(s2.list("d2"), std::slice::from_ref(&rec));
        assert_eq!(s2.resolve("d2", 7).unwrap().ratio, 0.004);
        // Loading is once-per-open: later add() is not clobbered by another ensure_loaded.
        s2.add("d2", make_scale(ScaleTarget::Page { page: 1 }, 0.5));
        s2.ensure_loaded("d2", Some(&pdf));
        assert_eq!(s2.list("d2").len(), 2);
    }

    #[test]
    fn ensure_loaded_without_a_sidecar_or_path_is_empty_not_an_error() {
        let mut st = ScaleStore::default();
        st.ensure_loaded("a", None);
        st.ensure_loaded("b", Some(std::path::Path::new("/nonexistent/x.pdf")));
        assert!(st.list("a").is_empty() && st.list("b").is_empty());
        assert!(st.is_loaded("a") && st.is_loaded("b"));
    }

    fn vp(page: u32, id: &str, bbox: BBox, ratio: f64) -> ScaleRecord {
        make_scale(
            ScaleTarget::Viewport {
                page,
                viewport_id: id.into(),
                name: id.into(),
                bbox,
            },
            ratio,
        )
    }

    fn p(x: f64, y: f64) -> PdfPoint {
        PdfPoint { x, y }
    }

    #[test]
    fn scale_at_picks_the_viewport_under_the_point_else_the_page_else_the_default() {
        let scales = vec![
            make_scale(ScaleTarget::DocumentDefault, 0.001),
            make_scale(ScaleTarget::Page { page: 1 }, 0.002),
            vp(1, "detail", [100.0, 100.0, 200.0, 200.0], 0.02),
            vp(2, "other-page", [0.0, 0.0, 1000.0, 1000.0], 0.5),
        ];
        let at = |page, pt| scale_at(&scales, page, Some(pt)).unwrap().ratio;
        assert_eq!(at(1, p(150.0, 150.0)), 0.02, "inside the viewport");
        assert_eq!(at(1, p(100.0, 200.0)), 0.02, "its edge counts as inside");
        assert_eq!(at(1, p(50.0, 50.0)), 0.002, "outside: the page scale");
        assert_eq!(at(0, p(150.0, 150.0)), 0.001, "no page scale: the default");
        assert_eq!(
            scale_at(&scales, 1, None).unwrap().ratio,
            0.002,
            "no point: page scale"
        );
        assert_eq!(
            at(2, p(5.0, 5.0)),
            0.5,
            "a viewport belongs to its own page only"
        );
    }

    #[test]
    fn overlapping_viewports_resolve_to_the_last_drawn_one() {
        // ISO 32000-1 §12.9: examined from the last one in reverse, first containing wins.
        let scales = vec![
            vp(0, "plan", [0.0, 0.0, 500.0, 500.0], 0.01),
            vp(0, "detail", [100.0, 100.0, 200.0, 200.0], 0.002),
        ];
        assert_eq!(
            viewport_at(&scales, 0, p(150.0, 150.0)).unwrap().ratio,
            0.002
        );
        assert_eq!(
            viewport_at(&scales, 0, p(300.0, 300.0)).unwrap().ratio,
            0.01
        );
        assert!(viewport_at(&scales, 0, p(600.0, 600.0)).is_none());
    }

    #[test]
    fn a_recalibrated_viewport_replaces_its_record_in_place_keeping_drawing_order() {
        let mut st = ScaleStore::default();
        st.add("d", vp(0, "a", [0.0, 0.0, 10.0, 10.0], 0.1));
        st.add("d", vp(0, "b", [0.0, 0.0, 10.0, 10.0], 0.2));
        st.add("d", make_scale(ScaleTarget::DocumentDefault, 0.001));
        // Same viewport id, new name/rectangle/ratio: replaces "a" where it was.
        let mut moved = vp(0, "a", [5.0, 5.0, 20.0, 20.0], 0.3);
        if let ScaleTarget::Viewport { name, .. } = &mut moved.applies_to {
            *name = "renamed".into();
        }
        st.add("d", moved);
        let ids: Vec<String> = st
            .list("d")
            .iter()
            .map(|r| match &r.applies_to {
                ScaleTarget::Viewport { viewport_id, .. } => viewport_id.clone(),
                _ => "default".into(),
            })
            .collect();
        assert_eq!(ids, vec!["a", "b", "default"]);
        assert_eq!(st.list("d")[0].ratio, 0.3);
        // "b" still draws above "a", so it wins where they overlap.
        assert_eq!(st.resolve_at("d", 0, p(6.0, 6.0)).unwrap().ratio, 0.2);
    }

    #[test]
    fn same_slot_matches_by_page_default_or_viewport_id() {
        let a = vp(0, "a", [0.0, 0.0, 1.0, 1.0], 1.0).applies_to;
        let a2 = vp(3, "a", [5.0, 5.0, 9.0, 9.0], 1.0).applies_to;
        let b = vp(0, "b", [0.0, 0.0, 1.0, 1.0], 1.0).applies_to;
        assert!(a.same_slot(&a2) && !a.same_slot(&b));
        assert!(ScaleTarget::Page { page: 2 }.same_slot(&ScaleTarget::Page { page: 2 }));
        assert!(!ScaleTarget::Page { page: 0 }.same_slot(&a));
        assert!(ScaleTarget::DocumentDefault.same_slot(&ScaleTarget::DocumentDefault));
    }

    #[test]
    fn a_viewport_record_round_trips_through_the_sidecar_and_old_sidecars_still_load() {
        use crate::sidecar::{load_meta, save_meta};
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("plan.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        let mut meta = load_meta(&pdf).unwrap();
        meta.scales.push(vp(1, "a", [1.0, 2.0, 3.0, 4.0], 0.02));
        save_meta(&pdf, &meta).unwrap();
        let mut st = ScaleStore::default();
        st.ensure_loaded("d", Some(&pdf));
        assert_eq!(st.list("d"), meta.scales.as_slice());
        // A sidecar written before viewports existed has no new fields and still loads.
        let old = r#"{"schema_version":1,"scales":[{"id":"x","applies_to":{"kind":"Page","page":0},"method":"Preset","ratio":0.01,"unit":"m","label":"1:100","precision":2}]}"#;
        std::fs::write(crate::sidecar::sidecar_dir(&pdf).join("meta.json"), old).unwrap();
        let m = load_meta(&pdf).unwrap();
        assert_eq!(m.scales.len(), 1);
        assert!(m.pdf_viewports_seen.is_empty());
    }
}
