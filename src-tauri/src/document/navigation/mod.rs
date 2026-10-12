//! Document navigation structure: bookmarks (the /Outlines tree), page labels (/PageLabels)
//! and Link annotations, read from and written to a PDF through the lopdf object model.
//!
//! Behaviour spec: `docs/navigation.md` "Document structure". All of this is untrusted input
//! from arbitrary PDFs: every walk is bounded (visited sets, node / depth / size caps) and
//! none of it panics or indexes unchecked. Only GoTo (a destination) and URI actions are
//! understood; every other action is reported as unsupported, never run and never written.
//!
//! Layout: [`uri`] (the one URL allowlist), [`outline`] (bookmarks), [`labels`] (page
//! labels), [`links`] (Link annotations), [`edit`] (the edit / undo layer the commands call).

use std::collections::HashMap;

use anyhow::{bail, Result};
use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use crate::geometry::{display_to_true, true_to_display, PdfPoint};

pub mod edit;
pub mod labels;
pub mod links;
pub mod outline;
pub mod uri;

/// Longest text (title, label prefix) read from a file, in characters.
pub const MAX_TEXT_CHARS: usize = 1024;

/// Upper bound on hops through /Parent or indirect-reference chains.
pub(crate) const MAX_HOPS: usize = 64;

// ---------------------------------------------------------------------------
// Destinations and targets
// ---------------------------------------------------------------------------

/// How a destination positions the page (ISO 32000-1 Table 151). Coordinates are in the
/// viewer's page space (PDFium "display" space, the same frame mark-up geometry uses), not
/// the file's raw user space; [`PageSpace`] converts at the read / write boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageView {
    Xyz {
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
    },
    Fit,
    FitH {
        top: Option<f64>,
    },
    FitV {
        left: Option<f64>,
    },
    FitR {
        left: f64,
        bottom: f64,
        right: f64,
        top: f64,
    },
    FitB,
    FitBH {
        top: Option<f64>,
    },
    FitBV {
        left: Option<f64>,
    },
}

impl PageView {
    /// "Inherit zoom": show the page, keep the current position and zoom.
    pub fn inherit() -> Self {
        PageView::Xyz {
            left: None,
            top: None,
            zoom: None,
        }
    }
}

/// What a bookmark or link does when activated, as far as redline understands it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// No destination and no action.
    None,
    /// Go to a page (0-based) of this document.
    Page { page: u32, view: PageView },
    /// Open an address (http, https or mailto only; the user is asked first).
    Uri { uri: String },
    /// A URI action whose address is not allowed (other scheme, control characters, too long).
    Blocked { reason: String },
    /// A destination whose page is not in the document, or a name that does not resolve.
    Broken,
    /// Any other action type (Launch, JavaScript, GoToR, Named, ...). Listed, never run.
    Unsupported { action: String },
}

/// A target the user chose (what the frontend sends when creating or re-pointing).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetInput {
    Page { page: u32, view: PageView },
    Uri { uri: String },
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/// Page object id <-> 0-based index, built once per read.
pub struct PageIndex {
    ids: Vec<ObjectId>,
    by_id: HashMap<ObjectId, u32>,
}

impl PageIndex {
    pub(crate) fn build(doc: &Document) -> Self {
        let ids: Vec<ObjectId> = doc.page_iter().collect();
        let by_id = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i as u32))
            .collect();
        Self { ids, by_id }
    }

    pub(crate) fn count(&self) -> u32 {
        self.ids.len() as u32
    }

    pub(crate) fn index_of(&self, id: ObjectId) -> Option<u32> {
        self.by_id.get(&id).copied()
    }

    pub(crate) fn id_of(&self, idx: u32) -> Option<ObjectId> {
        self.ids.get(idx as usize).copied()
    }
}

/// A page's true-space frame: `/Rotate` plus the `/MediaBox` size and origin. Converts
/// between the file's raw user space and the viewer's display space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageSpace {
    pub rotation: i32,
    pub w0: f64,
    pub h0: f64,
    pub ox: f64,
    pub oy: f64,
}

impl PageSpace {
    pub(crate) const IDENTITY: PageSpace = PageSpace {
        rotation: 0,
        w0: 0.0,
        h0: 0.0,
        ox: 0.0,
        oy: 0.0,
    };

    pub(crate) fn is_identity(&self) -> bool {
        self.rotation.rem_euclid(360) == 0 && self.ox == 0.0 && self.oy == 0.0
    }

    pub(crate) fn to_display(self, x: f64, y: f64) -> (f64, f64) {
        let p = true_to_display(
            PdfPoint { x, y },
            self.rotation,
            self.w0,
            self.h0,
            self.ox,
            self.oy,
        );
        (p.x, p.y)
    }

    pub(crate) fn to_true(self, x: f64, y: f64) -> (f64, f64) {
        let p = display_to_true(
            PdfPoint { x, y },
            self.rotation,
            self.w0,
            self.h0,
            self.ox,
            self.oy,
        );
        (p.x, p.y)
    }
}

/// Resolve a page's space, walking /Parent for the inheritable /Rotate and /MediaBox.
/// A page whose box cannot be read gets the identity frame (never an error: a damaged page
/// must not stop the rest of the structure from loading).
pub(crate) fn page_space(doc: &Document, page_id: ObjectId) -> PageSpace {
    let mut rotation: Option<i32> = None;
    let mut media: Option<(f64, f64, f64, f64)> = None;
    let mut current = Some(page_id);
    let mut hops = 0usize;
    while let Some(id) = current {
        hops += 1;
        if hops > MAX_HOPS {
            break;
        }
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if rotation.is_none() {
            if let Some(r) = dict
                .get(b"Rotate")
                .ok()
                .and_then(|o| deref(doc, o))
                .and_then(|o| o.as_i64().ok())
            {
                rotation = Some((r.rem_euclid(360)) as i32);
            }
        }
        if media.is_none() {
            if let Some(arr) = dict
                .get(b"MediaBox")
                .ok()
                .and_then(|o| deref(doc, o))
                .and_then(|o| o.as_array().ok())
            {
                let nums: Vec<f64> = arr
                    .iter()
                    .filter_map(|o| deref(doc, o).and_then(num))
                    .collect();
                if nums.len() == 4 {
                    media = Some((nums[0], nums[1], nums[2], nums[3]));
                }
            }
        }
        if rotation.is_some() && media.is_some() {
            break;
        }
        current = match dict.get(b"Parent") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        };
    }
    match media {
        Some((x0, y0, x1, y1)) => PageSpace {
            rotation: rotation.unwrap_or(0),
            w0: (x1 - x0).abs(),
            h0: (y1 - y0).abs(),
            ox: x0.min(x1),
            oy: y0.min(y1),
        },
        None => PageSpace {
            rotation: rotation.unwrap_or(0),
            ..PageSpace::IDENTITY
        },
    }
}

/// Lazily resolved [`PageSpace`] per page index.
pub(crate) struct SpaceCache<'a> {
    doc: &'a Document,
    pages: &'a PageIndex,
    cache: HashMap<u32, PageSpace>,
}

impl<'a> SpaceCache<'a> {
    pub(crate) fn new(doc: &'a Document, pages: &'a PageIndex) -> Self {
        Self {
            doc,
            pages,
            cache: HashMap::new(),
        }
    }

    pub(crate) fn get(&mut self, page: u32) -> PageSpace {
        if let Some(s) = self.cache.get(&page) {
            return *s;
        }
        let s = self
            .pages
            .id_of(page)
            .map(|id| page_space(self.doc, id))
            .unwrap_or(PageSpace::IDENTITY);
        self.cache.insert(page, s);
        s
    }
}

// ---------------------------------------------------------------------------
// Safe removal of replaced structure
// ---------------------------------------------------------------------------

/// Most nodes (objects, array items, dictionary values, bytes tokens) the reachability scan
/// may visit. Past it nothing is deleted.
pub(crate) const MAX_REACH_NODES: usize = 200_000_000;

/// Over-approximate `n g R` tokens in raw bytes (an object stream's contents, decoded or not):
/// anything that merely looks like a reference counts as one. False positives only keep
/// objects alive.
fn scan_ref_tokens(
    bytes: &[u8],
    out: &mut std::collections::HashSet<ObjectId>,
    budget: &mut usize,
) {
    let mut i = 0;
    while i < bytes.len() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if bytes[i].is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_digit()) {
            let (mut j, mut num) = (i, 0u64);
            while j < bytes.len() && bytes[j].is_ascii_digit() && j - i < 10 {
                num = num * 10 + u64::from(bytes[j] - b'0');
                j += 1;
            }
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            let g_start = k;
            let mut gen = 0u64;
            while k < bytes.len() && bytes[k].is_ascii_digit() && k - g_start < 6 {
                gen = gen * 10 + u64::from(bytes[k] - b'0');
                k += 1;
            }
            if k > g_start && k < bytes.len() {
                let mut r = k;
                while r < bytes.len() && bytes[r].is_ascii_whitespace() {
                    r += 1;
                }
                if r < bytes.len()
                    && bytes[r] == b'R'
                    && num <= u64::from(u32::MAX)
                    && gen <= u64::from(u16::MAX)
                {
                    out.insert((num as u32, gen as u16));
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Collect every reference inside `root` (iteratively, all depths, stream dictionaries, and the
/// contents of object streams) into `refs`. `false` when the budget ran out.
fn collect_refs(
    root: &Object,
    refs: &mut std::collections::HashSet<ObjectId>,
    left: &mut usize,
) -> bool {
    let mut stack: Vec<&Object> = vec![root];
    while let Some(o) = stack.pop() {
        if *left == 0 {
            return false;
        }
        *left -= 1;
        match o {
            Object::Reference(r) => {
                refs.insert(*r);
            }
            Object::Array(a) => stack.extend(a.iter()),
            Object::Dictionary(d) => stack.extend(d.iter().map(|(_, v)| v)),
            Object::Stream(s) => {
                stack.extend(s.dict.iter().map(|(_, v)| v));
                let is_objstm = s
                    .dict
                    .get(b"Type")
                    .ok()
                    .and_then(|t| t.as_name().ok())
                    .map(|n| n == b"ObjStm")
                    .unwrap_or(false);
                if is_objstm {
                    // Raw bytes, and the decoded bytes when they decode.
                    scan_ref_tokens(&s.content, refs, left);
                    if let Ok(dec) = s.decompressed_content_with_limit(64 * 1024 * 1024) {
                        scan_ref_tokens(&dec, refs, left);
                    }
                    if *left == 0 {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// A /Type value no navigation edit owns. Candidates are outline items (no /Type), the outline
/// root (/Outlines) and link annotations (/Annot); a page, page-tree node, catalog, font, XObject
/// or anything else that merely sits in a hostile structure is never a deletion candidate.
fn is_ours_to_delete(obj: &Object) -> bool {
    let dict = match obj {
        Object::Dictionary(d) => d,
        _ => return false,
    };
    match dict.get(b"Type") {
        // No /Type: an outline item or an untyped link, unless it looks like page structure.
        Err(_) => ![&b"Kids"[..], b"Contents", b"MediaBox", b"Resources", b"Count"]
            .iter()
            .any(|k| dict.has(k) && !dict.has(b"Title")),
        Ok(Object::Name(n)) => n == b"Outlines" || n == b"Annot",
        Ok(_) => false,
    }
}

/// Of `candidates` (objects a structure we are replacing used), the ones that are safe to
/// delete: nothing outside the candidates references them, no kept candidate references them,
/// and they are objects this feature owns. References are matched by object NUMBER alone (any
/// generation) and a candidate whose number is also used by another object is kept, so the
/// answer cannot differ from how another reader or lopdf's writer resolves ids. Run it on the
/// same `Document` instance that is saved straight afterwards.
///
/// Fails CLOSED: if the scan cannot complete within its budget, nothing is returned and
/// nothing is deleted (orphans are harmless; a deleted live object is not).
pub(crate) fn unreferenced_among(doc: &Document, candidates: &[ObjectId]) -> Vec<ObjectId> {
    unreferenced_among_with_budget(doc, candidates, MAX_REACH_NODES)
}

pub(crate) fn unreferenced_among_with_budget(
    doc: &Document,
    candidates: &[ObjectId],
    budget: usize,
) -> Vec<ObjectId> {
    use std::collections::{HashMap, HashSet};
    let cand_nums: HashSet<u32> = candidates.iter().map(|c| c.0).collect();
    let mut left = budget;

    // References from everything that is not a candidate: the trailer and every other object.
    let mut outside: HashSet<ObjectId> = HashSet::new();
    for (_, v) in doc.trailer.iter() {
        if !collect_refs(v, &mut outside, &mut left) {
            return Vec::new();
        }
    }
    // Per-candidate outgoing references, and numbers shared with a non-candidate object.
    let mut out_of: HashMap<u32, HashSet<ObjectId>> = HashMap::new();
    let mut shared_number: HashSet<u32> = HashSet::new();
    let cand_ids: HashSet<ObjectId> = candidates.iter().copied().collect();
    for (id, obj) in doc.objects.iter() {
        if cand_ids.contains(id) {
            let e = out_of.entry(id.0).or_default();
            if !collect_refs(obj, e, &mut left) {
                return Vec::new();
            }
        } else {
            // Not a candidate: an outside source, even when it shares a candidate's number
            // (another generation). Its references must keep their targets alive.
            if cand_nums.contains(&id.0) {
                shared_number.insert(id.0);
            }
            if !collect_refs(obj, &mut outside, &mut left) {
                return Vec::new();
            }
        }
    }
    // Live candidates: referenced from outside, or from a live candidate (a kept item's
    // /Next, /Parent or /First must not be left pointing at a deleted object).
    let mut live: HashSet<u32> = outside
        .iter()
        .map(|r| r.0)
        .filter(|n| cand_nums.contains(n))
        .collect();
    // Candidates that are kept for any other reason also seed liveness, so what they point at
    // is kept too: a number shared with another generation, or a type this feature does not
    // own (an untyped link with /Contents, a page, ...).
    live.extend(shared_number.iter().copied());
    live.extend(
        candidates
            .iter()
            .filter(|id| !doc.objects.get(id).is_some_and(is_ours_to_delete))
            .map(|id| id.0),
    );
    let mut queue: Vec<u32> = live.iter().copied().collect();
    while let Some(n) = queue.pop() {
        if let Some(outs) = out_of.get(&n) {
            for r in outs {
                if cand_nums.contains(&r.0) && live.insert(r.0) {
                    queue.push(r.0);
                }
            }
        }
    }
    candidates
        .iter()
        .copied()
        .filter(|id| {
            !live.contains(&id.0)
                && !shared_number.contains(&id.0)
                && doc.objects.get(id).is_some_and(is_ours_to_delete)
        })
        .collect()
}

/// Every reference in the document (trailer and all objects) whose exact (number, generation)
/// is not an object in `doc.objects`, i.e. what lopdf would write as a dangling reference.
/// `None` when the budget ran out.
fn dangling_targets(doc: &Document, budget: usize) -> Option<std::collections::HashSet<ObjectId>> {
    let mut refs = std::collections::HashSet::new();
    let mut left = budget;
    for (_, v) in doc.trailer.iter() {
        if !collect_refs(v, &mut refs, &mut left) {
            return None;
        }
    }
    for obj in doc.objects.values() {
        if !collect_refs(obj, &mut refs, &mut left) {
            return None;
        }
    }
    Some(refs.into_iter().filter(|r| !doc.objects.contains_key(r)).collect())
}

/// Delete the candidates that [`unreferenced_among`] clears, then verify in code that the
/// deletion created no reference to a missing (number, generation); if it did (or the check
/// cannot finish) put everything back. Fails closed: the worst outcome is an orphan.
/// Returns the ids actually deleted.
pub(crate) fn delete_unreferenced(doc: &mut Document, candidates: &[ObjectId]) -> Vec<ObjectId> {
    delete_unreferenced_with_budget(doc, candidates, MAX_REACH_NODES)
}

pub(crate) fn delete_unreferenced_with_budget(
    doc: &mut Document,
    candidates: &[ObjectId],
    budget: usize,
) -> Vec<ObjectId> {
    let doomed = unreferenced_among_with_budget(doc, candidates, budget);
    if doomed.is_empty() {
        return doomed;
    }
    let Some(before) = dangling_targets(doc, budget) else {
        return Vec::new();
    };
    let removed: Vec<(ObjectId, Object)> = doomed
        .iter()
        .filter_map(|id| doc.objects.remove(id).map(|o| (*id, o)))
        .collect();
    let ok = dangling_targets(doc, budget).is_some_and(|after| after.is_subset(&before));
    if ok {
        return removed.into_iter().map(|(id, _)| id).collect();
    }
    for (id, obj) in removed {
        doc.objects.insert(id, obj);
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Small object helpers
// ---------------------------------------------------------------------------

/// Follow an indirect reference (bounded); `None` for a dangling or looping chain.
pub(crate) fn deref<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    doc.dereference(obj).ok().map(|(_, o)| o)
}

/// A finite number from an Integer or Real.
pub(crate) fn num(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) if r.is_finite() => Some(f64::from(*r)),
        _ => None,
    }
}

/// A PDF text string as display text: decoded, control characters dropped, capped.
pub(crate) fn text_of(bytes: &[u8], max_chars: usize) -> String {
    crate::markup::replies::decode_text(bytes)
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(max_chars)
        .collect()
}

/// Encode text for /Title, /P and similar: control characters removed, capped.
pub(crate) fn clean_input_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

pub(crate) fn name_str(obj: &Object) -> Option<String> {
    obj.as_name()
        .ok()
        .map(|b| String::from_utf8_lossy(b).chars().take(32).collect())
}

// ---------------------------------------------------------------------------
// Destination resolution
// ---------------------------------------------------------------------------

/// Resolve a destination object (explicit array, or a name / string looked up in the
/// document's named destinations) to a page and view.
pub(crate) fn resolve_dest(
    doc: &Document,
    obj: &Object,
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
) -> Target {
    let Some(obj) = deref(doc, obj) else {
        return Target::Broken;
    };
    match obj {
        Object::Array(arr) => explicit_dest(doc, arr, pages, spaces),
        Object::Name(n) => named_dest(doc, n, pages, spaces),
        Object::String(s, _) => named_dest(doc, s, pages, spaces),
        // A named destination's value may be a dictionary with a /D entry.
        Object::Dictionary(d) => match d.get(b"D") {
            Ok(inner) => match deref(doc, inner) {
                Some(Object::Array(arr)) => explicit_dest(doc, arr, pages, spaces),
                _ => Target::Broken,
            },
            Err(_) => Target::Broken,
        },
        _ => Target::Broken,
    }
}

fn named_dest(
    doc: &Document,
    name: &[u8],
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
) -> Target {
    match links::lookup_named_dest(doc, name) {
        Some(Object::Array(arr)) => explicit_dest(doc, &arr, pages, spaces),
        Some(Object::Dictionary(d)) => match d.get(b"D") {
            Ok(Object::Array(arr)) => explicit_dest(doc, arr, pages, spaces),
            Ok(Object::Reference(r)) => match doc.get_object(*r) {
                Ok(Object::Array(arr)) => explicit_dest(doc, arr, pages, spaces),
                _ => Target::Broken,
            },
            _ => Target::Broken,
        },
        _ => Target::Broken,
    }
}

fn explicit_dest(
    doc: &Document,
    arr: &[Object],
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
) -> Target {
    let Some(first) = arr.first() else {
        return Target::Broken;
    };
    // The page is an indirect reference to a page object (an integer is the remote form).
    let Object::Reference(page_ref) = first else {
        return Target::Broken;
    };
    let Some(page) = pages.index_of(*page_ref) else {
        return Target::Broken;
    };
    let space = spaces.get(page);
    let arg = |i: usize| -> Option<f64> { arr.get(i).and_then(|o| deref(doc, o)).and_then(num) };
    let kind = arr
        .get(1)
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"");
    let identity = space.is_identity();
    // One-sided coordinates cannot be mapped through a rotated / shifted frame; drop them.
    let one = |v: Option<f64>| if identity { v } else { None };
    let view = match kind {
        b"XYZ" => {
            let zoom = arg(4).filter(|z| *z > 0.0);
            match (arg(2), arg(3)) {
                (Some(l), Some(t)) => {
                    let (l, t) = space.to_display(l, t);
                    PageView::Xyz {
                        left: Some(l),
                        top: Some(t),
                        zoom,
                    }
                }
                (l, t) => PageView::Xyz {
                    left: one(l),
                    top: one(t),
                    zoom,
                },
            }
        }
        b"Fit" => PageView::Fit,
        b"FitH" => PageView::FitH { top: one(arg(2)) },
        b"FitV" => PageView::FitV { left: one(arg(2)) },
        b"FitB" => PageView::FitB,
        b"FitBH" => PageView::FitBH { top: one(arg(2)) },
        b"FitBV" => PageView::FitBV { left: one(arg(2)) },
        b"FitR" => match (arg(2), arg(3), arg(4), arg(5)) {
            (Some(l), Some(b), Some(r), Some(t)) => {
                let (x0, y0) = space.to_display(l, b);
                let (x1, y1) = space.to_display(r, t);
                PageView::FitR {
                    left: x0.min(x1),
                    bottom: y0.min(y1),
                    right: x0.max(x1),
                    top: y0.max(y1),
                }
            }
            _ => PageView::Fit,
        },
        _ => return Target::Broken,
    };
    Target::Page { page, view }
}

/// Build the destination array for `/Dest` or an action's `/D` from a chosen target page
/// and view. Display-space coordinates are mapped back to the file's user space.
pub(crate) fn build_dest(page_ref: ObjectId, view: &PageView, space: &PageSpace) -> Result<Object> {
    let n = |v: f64| -> Result<Object> {
        if !v.is_finite() {
            bail!("destination coordinates must be finite numbers");
        }
        Ok(Object::Real(v as f32))
    };
    let opt = |v: Option<f64>| -> Result<Object> {
        match v {
            Some(x) => n(x),
            None => Ok(Object::Null),
        }
    };
    let mut out = vec![Object::Reference(page_ref)];
    match view {
        PageView::Xyz { left, top, zoom } => {
            out.push(Object::Name(b"XYZ".to_vec()));
            let (l, t) = match (left, top) {
                (Some(l), Some(t)) => {
                    let (l, t) = space.to_true(*l, *t);
                    (Some(l), Some(t))
                }
                // One-sided: only valid on an unrotated, unshifted page.
                (l, t) if space.is_identity() => (*l, *t),
                _ => (None, None),
            };
            out.push(opt(l)?);
            out.push(opt(t)?);
            match zoom {
                Some(z) if z.is_finite() && *z > 0.0 => out.push(n(*z)?),
                _ => out.push(Object::Null),
            }
        }
        PageView::Fit => out.push(Object::Name(b"Fit".to_vec())),
        PageView::FitB => out.push(Object::Name(b"FitB".to_vec())),
        PageView::FitH { top } | PageView::FitBH { top } => {
            let name: &[u8] = if matches!(view, PageView::FitH { .. }) {
                b"FitH"
            } else {
                b"FitBH"
            };
            out.push(Object::Name(name.to_vec()));
            out.push(if space.is_identity() {
                opt(*top)?
            } else {
                Object::Null
            });
        }
        PageView::FitV { left } | PageView::FitBV { left } => {
            let name: &[u8] = if matches!(view, PageView::FitV { .. }) {
                b"FitV"
            } else {
                b"FitBV"
            };
            out.push(Object::Name(name.to_vec()));
            out.push(if space.is_identity() {
                opt(*left)?
            } else {
                Object::Null
            });
        }
        PageView::FitR {
            left,
            bottom,
            right,
            top,
        } => {
            out.push(Object::Name(b"FitR".to_vec()));
            let (x0, y0) = space.to_true(*left, *bottom);
            let (x1, y1) = space.to_true(*right, *top);
            out.push(n(x0.min(x1))?);
            out.push(n(y0.min(y1))?);
            out.push(n(x0.max(x1))?);
            out.push(n(y0.max(y1))?);
        }
    }
    Ok(Object::Array(out))
}

/// Resolve the target of an object that carries `/Dest` (a destination) or `/A` (an
/// action): an outline item or a Link annotation. A GoTo action is the same as a `/Dest`;
/// a URI action is checked against the allowlist; anything else is unsupported.
pub(crate) fn target_of(
    doc: &Document,
    dict: &lopdf::Dictionary,
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
) -> Target {
    if let Ok(dest) = dict.get(b"Dest") {
        return resolve_dest(doc, dest, pages, spaces);
    }
    let Ok(action) = dict.get(b"A") else {
        return Target::None;
    };
    let Some(Object::Dictionary(a)) = deref(doc, action) else {
        return Target::Unsupported {
            action: "unreadable".into(),
        };
    };
    let kind = a
        .get(b"S")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(name_str)
        .unwrap_or_default();
    match kind.as_str() {
        "GoTo" => match a.get(b"D") {
            Ok(d) => resolve_dest(doc, d, pages, spaces),
            Err(_) => Target::Broken,
        },
        "URI" => match a
            .get(b"URI")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_str().ok())
        {
            Some(bytes) => match uri::validate_uri(bytes) {
                Ok(u) => Target::Uri { uri: u },
                Err(reason) => Target::Blocked { reason },
            },
            None => Target::Blocked {
                reason: "no address".into(),
            },
        },
        "" => Target::Unsupported {
            action: "unknown".into(),
        },
        other => Target::Unsupported {
            action: other.to_string(),
        },
    }
}

/// Build the `/Dest` or `/A` entries for a chosen target (never anything but GoTo / URI).
pub(crate) fn target_entries(
    doc: &Document,
    target: &TargetInput,
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
) -> Result<(&'static str, Object)> {
    match target {
        TargetInput::Page { page, view } => {
            let Some(page_ref) = pages.id_of(*page) else {
                bail!(
                    "page {} is outside this {}-page document",
                    page + 1,
                    pages.count()
                );
            };
            let _ = doc;
            let space = spaces.get(*page);
            Ok(("Dest", build_dest(page_ref, view, &space)?))
        }
        TargetInput::Uri { uri } => {
            let ok = uri::validate_uri(uri.as_bytes()).map_err(anyhow::Error::msg)?;
            let action = dictionary! {
                "Type" => "Action",
                "S" => "URI",
                "URI" => Object::string_literal(ok),
            };
            Ok(("A", Object::Dictionary(action)))
        }
    }
}

#[cfg(test)]
mod tests;
