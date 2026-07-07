//! Dynamic-stamp sequence counters (spec section 12 decision c: "per-document by default,
//! selectable to a global/project-wide counter per stamp").
//!
//! DEFERRAL (named per dispatch scope): the spec requires counter state to persist in the
//! sidecar (section 15). This slice keeps counters in memory only, scoped for the app
//! session - they reset on restart. Full sidecar persistence is a named follow-up; the
//! `CounterScope` split (per-document vs global) is implemented now so the follow-up is a
//! storage swap, not a model change.

use std::collections::HashMap;
use std::sync::Mutex;

use uuid::Uuid;

use super::stamp::CounterScope;

/// In-memory sequence counters for dynamic stamps. Per-document counters are keyed by
/// `(doc_id, tool_id)`; global counters are keyed by `tool_id` alone. Each call to
/// [`SequenceCounters::next`] returns the next value (starting at 1) and advances state.
#[derive(Default)]
pub struct SequenceCounters {
    per_document: Mutex<HashMap<(String, Uuid), u32>>,
    global: Mutex<HashMap<Uuid, u32>>,
}

impl SequenceCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance and return the next sequence value (1-based) for `tool_id`, scoped per
    /// `scope`. `doc_id` is only consulted for [`CounterScope::PerDocument`].
    pub fn next(&self, scope: CounterScope, tool_id: Uuid, doc_id: &str) -> u32 {
        match scope {
            CounterScope::PerDocument => {
                let mut map = self.per_document.lock().unwrap();
                let entry = map.entry((doc_id.to_string(), tool_id)).or_insert(0);
                *entry += 1;
                *entry
            }
            CounterScope::Global => {
                let mut map = self.global.lock().unwrap();
                let entry = map.entry(tool_id).or_insert(0);
                *entry += 1;
                *entry
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_document_counter_starts_at_one_and_increments() {
        let counters = SequenceCounters::new();
        let tool = Uuid::new_v4();
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-a"), 1);
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-a"), 2);
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-a"), 3);
    }

    #[test]
    fn per_document_counter_is_independent_per_document() {
        let counters = SequenceCounters::new();
        let tool = Uuid::new_v4();
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-a"), 1);
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-b"), 1, "a different doc starts fresh");
        assert_eq!(counters.next(CounterScope::PerDocument, tool, "doc-a"), 2);
    }

    #[test]
    fn global_counter_ignores_doc_id() {
        let counters = SequenceCounters::new();
        let tool = Uuid::new_v4();
        assert_eq!(counters.next(CounterScope::Global, tool, "doc-a"), 1);
        assert_eq!(counters.next(CounterScope::Global, tool, "doc-b"), 2, "global counter is shared across docs");
    }

    #[test]
    fn different_tools_have_independent_counters() {
        let counters = SequenceCounters::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(counters.next(CounterScope::Global, a, "doc"), 1);
        assert_eq!(counters.next(CounterScope::Global, b, "doc"), 1);
        assert_eq!(counters.next(CounterScope::Global, a, "doc"), 2);
    }
}
