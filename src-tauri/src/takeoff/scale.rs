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
    /// Add (or replace by id) a scale record for the given document.
    pub fn add(&mut self, doc_id: &str, rec: ScaleRecord) {
        let list = self.inner.entry(doc_id.to_string()).or_default();
        if let Some(pos) = list.iter().position(|r| r.id == rec.id) {
            list[pos] = rec;
        } else {
            list.push(rec);
        }
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
            .or_else(|| list.iter().find(|r| matches!(r.applies_to, ScaleTarget::DocumentDefault)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scale(applies_to: ScaleTarget, ratio: f64) -> ScaleRecord {
        ScaleRecord::new(applies_to, ScaleMethod::Preset, ratio, "m".into(), "1:100".into(), 2)
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
}
