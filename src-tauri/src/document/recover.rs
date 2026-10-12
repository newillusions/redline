//! Recovery from a PDF that changed on disk under redline (see `ERR_DISK_CHANGED`).
//!
//! Saving matches annotations by the object ids of the file as loaded, so once another
//! program rewrote the file a normal write is refused. Two ways out keep the user's work:
//! write it to a NEW file from the bytes redline loaded ([`rebuild_inputs`]), or reload the
//! changed file and re-apply the unsaved edits on top of it ([`merge_unsaved`]).

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use crate::markup::{Markup, SourceRef};

/// The user's unsaved work: what differs between the store and the file as loaded or last
/// written.
#[derive(Debug, Default)]
pub struct Unsaved {
    pub added: Vec<Markup>,
    pub modified: Vec<Markup>,
    pub deleted: Vec<Uuid>,
}

/// What a reload-and-keep did, for the dialog.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ReloadReport {
    /// Unsaved edits applied to the reloaded file.
    pub applied: usize,
    /// Unsaved edits that could not be applied (target gone or page missing), one line each.
    pub skipped: Vec<String>,
    /// Applied edits worth a look (the file's own copy of the markup also changed).
    pub notes: Vec<String>,
}

/// A stable identity for a foreign markup that has no `/NM` (its id is random per read, so
/// it never matches across a reload): hash of type, page, geometry and contents. `None` for
/// anything with a name, and for anything not read from a file.
pub fn content_key(m: &Markup) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    if m.source_nm.is_some() || m.source_ref.is_none() {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", m.markup_type).hash(&mut h);
    m.page.hash(&mut h);
    serde_json::to_vec(&m.geometry).ok()?.hash(&mut h);
    m.contents.hash(&mut h);
    Some(h.finish())
}

fn describe(m: &Markup) -> String {
    format!("{:?} on page {}", m.markup_type, m.page + 1)
}

/// The inputs for writing the user's mark-ups into a NEW file built from the original
/// bytes: every markup rebuilt from the model (no original dictionary is written back, so
/// no object reference from the original can dangle or point at another object), and every
/// source annotation of a model markup still stripped from those bytes by its reference, so
/// a foreign markup appears once, not as an original beside its rebuilt copy.
pub fn rebuild_inputs(
    markups: &[Markup],
    removed: &HashSet<SourceRef>,
) -> (Vec<Markup>, HashSet<SourceRef>) {
    let rebuilt = markups
        .iter()
        .cloned()
        .map(|mut m| {
            m.source_dict = None;
            m.source_fp = None;
            m
        })
        .collect();
    (rebuilt, removed.clone())
}

/// Re-apply `unsaved` onto `loaded` (the changed file's markups): a deletion removes the
/// markup (and its source annotation on save), an edit replaces the markup that is still
/// there, an addition is appended. Edits whose target is gone, and anything on a page the
/// file no longer has, are reported and left out. Returns the merged set, the source
/// annotations to strip on the next save, and the report.
pub fn merge_unsaved(
    loaded: Vec<Markup>,
    baseline: &HashMap<Uuid, u64>,
    keys: &HashMap<Uuid, u64>,
    unsaved: Unsaved,
    page_count: u32,
) -> (Vec<Markup>, HashSet<SourceRef>, ReloadReport) {
    let mut merged = loaded;
    let mut removed = HashSet::new();
    let mut report = ReloadReport::default();
    // Markups of the changed file already matched to a user edit (by id or content key).
    let mut claimed: HashSet<Uuid> = HashSet::new();
    // Where the user's markup `id` sits in `merged`: by id, else (a foreign markup with no
    // /NM, whose id is random per read) by the content it had when loaded.
    let find = |merged: &[Markup], claimed: &HashSet<Uuid>, id: Uuid| -> Option<usize> {
        merged
            .iter()
            .position(|m| m.id() == id)
            .or_else(|| {
                let k = keys.get(&id)?;
                merged
                    .iter()
                    .position(|m| !claimed.contains(&m.id()) && content_key(m) == Some(*k))
            })
    };

    for id in unsaved.deleted {
        // Already gone from the file: nothing to delete, nothing lost.
        if let Some(i) = find(&merged, &claimed, id) {
            let gone = merged.remove(i);
            removed.extend(gone.source_ref);
            report.applied += 1;
        }
    }
    for mut m in unsaved.modified {
        let Some(i) = find(&merged, &claimed, m.id()) else {
            report.skipped.push(format!("{}: could not be matched in the changed file", describe(&m)));
            continue;
        };
        if m.page >= page_count {
            report.skipped.push(format!("{}: the page is no longer in the file", describe(&m)));
            continue;
        }
        let file_changed_it = baseline.get(&m.id()).copied() != merged[i].edit_fingerprint()
            && merged[i].id() == m.id();
        m.adopt_reader_owned_from(&merged[i]);
        if file_changed_it {
            report
                .notes
                .push(format!("{}: also changed in the file; your version was kept", describe(&m)));
        }
        claimed.insert(m.id());
        merged[i] = m;
        report.applied += 1;
    }
    for mut m in unsaved.added {
        if m.page >= page_count {
            report.skipped.push(format!("{}: the page is no longer in the file", describe(&m)));
        } else if merged.iter().any(|x| x.id() == m.id()) {
            report.skipped.push(format!("{}: the file already has this mark-up", describe(&m)));
        } else {
            m.clear_reader_owned();
            merged.push(m);
            report.applied += 1;
        }
    }
    (merged, removed, report)
}
