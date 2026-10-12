//! Per-page scale records (spec §7). The sidecar is the source of truth;
//! `ScaleStore` is the in-memory mirror keyed by doc_id.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What pages a scale applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ScaleTarget {
    /// One specific page (zero-based index).
    Page { page: u32 },
    /// Fallback for pages without their own scale.
    DocumentDefault,
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
        }
    }
}

/// In-memory scale store keyed by doc_id. Thread-safe via Mutex in AppState.
#[derive(Debug, Default)]
pub struct ScaleStore {
    inner: std::collections::HashMap<String, Vec<ScaleRecord>>,
}

impl ScaleStore {
    /// Add a scale record for the given document. A record replaces any existing one with
    /// the SAME id or the same `applies_to` target (a recalibration of the same page, or of
    /// the document default, supersedes the old scale - as the sidecar and the frontend
    /// store do), so there is never an old scale shadowing the new one.
    pub fn add(&mut self, doc_id: &str, rec: ScaleRecord) {
        let list = self.inner.entry(doc_id.to_string()).or_default();
        list.retain(|r| r.id != rec.id && r.applies_to != rec.applies_to);
        list.push(rec);
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
        let recs = pdf_path
            .and_then(|p| crate::sidecar::load_meta(p).ok())
            .map(|m| m.scales)
            .unwrap_or_default();
        self.seed(doc_id, recs);
    }

    /// Forget the document's scales (on close); the sidecar remains the source of truth.
    pub fn clear(&mut self, doc_id: &str) {
        self.inner.remove(doc_id);
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
        let list = self.inner.get(doc_id)?;
        list.iter()
            .find(|r| matches!(r.applies_to, ScaleTarget::Page { page: p } if p == page))
            .or_else(|| {
                list.iter()
                    .find(|r| matches!(r.applies_to, ScaleTarget::DocumentDefault))
            })
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
}
