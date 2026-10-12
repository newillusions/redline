//! Link annotations (ISO 32000-1 12.5.6.5): reading every link, creating, deleting and
//! re-pointing them, and named-destination lookup.
//!
//! Foreign Link annotations are never rewritten by the mark-up save path (they carry no
//! `/RLType`), so a link written here and any link another program wrote survive a mark-up
//! save untouched. Redline only ever *authors* two kinds of action: a destination (`/Dest`)
//! and a URI action whose address passed the allowlist ([`super::uri`]). Links already in the
//! file with other actions (Launch, JavaScript, GoToR, ...) are listed as unsupported, never
//! run, and left exactly as they are unless the user deletes or re-points them.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};
use serde::Serialize;

use super::{
    deref, num, page_space, target_entries, target_of, PageIndex, SpaceCache, Target, TargetInput,
};

/// Most Link annotations read from one page.
pub const MAX_LINKS_PER_PAGE: usize = 5_000;
/// Most Link annotations read from one document.
pub const MAX_LINKS_TOTAL: usize = 50_000;
/// Smallest side of a link rectangle, in points (a click, not a drag, creates nothing).
pub const MIN_LINK_SIDE: f64 = 2.0;

const MAX_NAME_TREE_DEPTH: usize = 16;
const MAX_NAME_TREE_STEPS: usize = 100_000;

#[derive(Debug, Clone, Serialize)]
pub struct LinkView {
    /// `obj:<num>:<gen>` for an indirect annotation (editable); `None` for an inline one.
    pub id: Option<String>,
    pub page: u32,
    /// `[left, bottom, right, top]` in the viewer's page space.
    pub rect: [f64; 4],
    pub target: Target,
}

pub fn link_id(id: ObjectId) -> String {
    format!("obj:{}:{}", id.0, id.1)
}

pub fn parse_link_id(s: &str) -> Result<ObjectId> {
    let mut it = s.split(':');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some("obj"), Some(n), Some(g), None) => {
            let n: u32 = n.parse().context("invalid link id")?;
            let g: u16 = g.parse().context("invalid link id")?;
            Ok((n, g))
        }
        _ => bail!("invalid link id {s:?}"),
    }
}

// ---------------------------------------------------------------------------
// Named destinations
// ---------------------------------------------------------------------------

/// Look a destination name up in the catalog's /Dests dictionary and the /Names /Dests name
/// tree. Bounded; returns the (dereferenced) value.
pub fn lookup_named_dest(doc: &Document, name: &[u8]) -> Option<Object> {
    let catalog = doc.catalog().ok()?;
    if let Some(Object::Dictionary(dests)) = catalog.get(b"Dests").ok().and_then(|o| deref(doc, o))
    {
        if let Ok(v) = dests.get(name) {
            return deref(doc, v).cloned();
        }
    }
    let names = catalog
        .get(b"Names")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_dict().ok())?;
    let root = names.get(b"Dests").ok()?;
    let mut visited = HashSet::new();
    let mut steps = 0usize;
    name_tree_find(doc, root, name, 0, &mut visited, &mut steps)
}

fn name_tree_find(
    doc: &Document,
    node: &Object,
    key: &[u8],
    depth: usize,
    visited: &mut HashSet<ObjectId>,
    steps: &mut usize,
) -> Option<Object> {
    if depth > MAX_NAME_TREE_DEPTH || *steps >= MAX_NAME_TREE_STEPS {
        return None;
    }
    *steps += 1;
    let (id, obj) = doc.dereference(node).ok()?;
    if let Some(id) = id {
        if !visited.insert(id) {
            return None;
        }
    }
    let d = obj.as_dict().ok()?;
    if let Some(Object::Array(names)) = d.get(b"Names").ok().and_then(|o| deref(doc, o)) {
        for pair in names.chunks_exact(2) {
            if let Some(k) = deref(doc, &pair[0]).and_then(|o| o.as_str().ok()) {
                if k == key {
                    return deref(doc, &pair[1]).cloned();
                }
            }
        }
    }
    if let Some(Object::Array(kids)) = d.get(b"Kids").ok().and_then(|o| deref(doc, o)) {
        for kid in kids {
            // /Limits [least greatest] lets whole subtrees be skipped.
            let outside = deref(doc, kid)
                .and_then(|o| o.as_dict().ok())
                .and_then(|kd| kd.get(b"Limits").ok())
                .and_then(|l| deref(doc, l))
                .and_then(|l| l.as_array().ok())
                .and_then(|a| {
                    let lo = deref(doc, a.first()?)?.as_str().ok()?;
                    let hi = deref(doc, a.get(1)?)?.as_str().ok()?;
                    Some(key < lo || key > hi)
                })
                .unwrap_or(false);
            if outside {
                continue;
            }
            if let Some(v) = name_tree_find(doc, kid, key, depth + 1, visited, steps) {
                return Some(v);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Page /Annots access
// ---------------------------------------------------------------------------

/// The page's /Annots entries (cloned), whether the array is direct or an indirect object.
fn annots_of(doc: &Document, page_id: ObjectId) -> Vec<Object> {
    let Ok(page) = doc.get_dictionary(page_id) else {
        return Vec::new();
    };
    match page.get(b"Annots").ok().and_then(|o| deref(doc, o)) {
        Some(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// Replace the page's /Annots array, keeping it where it lives (direct, or its indirect
/// array object) so other references to that array stay valid.
fn set_annots(doc: &mut Document, page_id: ObjectId, entries: Vec<Object>) -> Result<()> {
    let target = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|o| o.as_reference().ok());
    if let Some(arr_id) = target {
        if let Some(Object::Array(a)) = doc.objects.get_mut(&arr_id) {
            *a = entries;
            return Ok(());
        }
    }
    doc.get_dictionary_mut(page_id)
        .context("page dictionary")?
        .set("Annots", Object::Array(entries));
    Ok(())
}

fn is_link(d: &Dictionary) -> bool {
    d.get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n == b"Link")
        .unwrap_or(false)
}

/// ISO 32000-1 Table 165 annotation flags: Locked (bit 8) and LockedContents (bit 10). A locked
/// link is the author's way of saying "do not change this"; redline honours it like it does
/// for mark-ups (`Markup::is_locked`).
const LOCKED_FLAGS: i64 = 128 | 512;

/// Fails CLOSED: an absent /F is "not locked"; a /F that cannot be read as an integer (a real
/// number, a string, a reference that does not resolve, a loop) is treated as locked, as is
/// any state where the link dictionary itself cannot be read.
fn ensure_unlocked(doc: &Document, id: ObjectId) -> Result<()> {
    let locked = || anyhow::anyhow!("that link is locked, so it cannot be changed or deleted");
    let dict = doc
        .get_dictionary(id)
        .map_err(|_| anyhow::anyhow!("that link cannot be read, so it is treated as locked"))?;
    let flags = match dict.get(b"F") {
        Err(_) => 0,
        Ok(o) => match deref(doc, o) {
            Some(Object::Integer(i)) => *i,
            Some(Object::Real(r)) if r.is_finite() && r.fract() == 0.0 && r.abs() < 1e15 => {
                *r as i64
            }
            _ => bail!("that link's flags cannot be read, so it is treated as locked"),
        },
    };
    if flags & LOCKED_FLAGS != 0 {
        return Err(locked());
    }
    Ok(())
}

fn rect_of(doc: &Document, d: &Dictionary) -> Option<[f64; 4]> {
    let arr = d
        .get(b"Rect")
        .ok()
        .and_then(|o| deref(doc, o))?
        .as_array()
        .ok()?;
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f64> = arr
        .iter()
        .filter_map(|o| deref(doc, o).and_then(num))
        .collect();
    if v.len() != 4 {
        return None;
    }
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Every Link annotation in the document, in page order, plus whether a cap cut the list.
pub fn read_links(doc: &Document, pages: &PageIndex) -> (Vec<LinkView>, bool) {
    let mut spaces = SpaceCache::new(doc, pages);
    let mut out = Vec::new();
    let mut truncated = false;
    'pages: for page in 0..pages.count() {
        let Some(page_id) = pages.id_of(page) else {
            continue;
        };
        let space = spaces.get(page);
        let mut on_page = 0usize;
        for entry in annots_of(doc, page_id) {
            let (id, dict) = match &entry {
                Object::Reference(r) => match doc.get_dictionary(*r) {
                    Ok(d) => (Some(*r), d),
                    Err(_) => continue,
                },
                Object::Dictionary(d) => (None, d),
                _ => continue,
            };
            if !is_link(dict) {
                continue;
            }
            if on_page >= MAX_LINKS_PER_PAGE || out.len() >= MAX_LINKS_TOTAL {
                truncated = true;
                if out.len() >= MAX_LINKS_TOTAL {
                    break 'pages;
                }
                break;
            }
            let Some(rect) = rect_of(doc, dict) else {
                continue;
            };
            let (x0, y0) = space.to_display(rect[0], rect[1]);
            let (x1, y1) = space.to_display(rect[2], rect[3]);
            let target = target_of(doc, dict, pages, &mut spaces);
            on_page += 1;
            out.push(LinkView {
                id: id.map(link_id),
                page,
                rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)],
                target,
            });
        }
    }
    (out, truncated)
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

fn validated_rect(rect: [f64; 4]) -> Result<[f64; 4]> {
    if rect.iter().any(|v| !v.is_finite()) {
        bail!("the link area is not valid");
    }
    let r = [
        rect[0].min(rect[2]),
        rect[1].min(rect[3]),
        rect[0].max(rect[2]),
        rect[1].max(rect[3]),
    ];
    if r[2] - r[0] < MIN_LINK_SIDE || r[3] - r[1] < MIN_LINK_SIDE {
        bail!("drag a larger area for the link");
    }
    Ok(r)
}

/// Create a Link annotation on `page` (0-based) covering `rect` (viewer page space). The
/// annotation is hidden (`/Border [0 0 0]`); Revu and Acrobat show it as a clickable region.
pub fn create_link(
    doc: &mut Document,
    page: u32,
    rect: [f64; 4],
    target: &TargetInput,
) -> Result<ObjectId> {
    let rect = validated_rect(rect)?;
    let pages = PageIndex::build(doc);
    let page_id = pages.id_of(page).with_context(|| {
        format!(
            "page {} is outside this {}-page document",
            page + 1,
            pages.count()
        )
    })?;
    let (entry_key, entry_val) = {
        let mut spaces = SpaceCache::new(doc, &pages);
        target_entries(doc, target, &pages, &mut spaces)?
    };
    let space = page_space(doc, page_id);
    let (x0, y0) = space.to_true(rect[0], rect[1]);
    let (x1, y1) = space.to_true(rect[2], rect[3]);
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => vec![
            Object::Real(x0.min(x1) as f32),
            Object::Real(y0.min(y1) as f32),
            Object::Real(x0.max(x1) as f32),
            Object::Real(y0.max(y1) as f32),
        ],
        "Border" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(0)],
        "F" => 4,
    };
    d.set(entry_key, entry_val);
    let id = doc.add_object(Object::Dictionary(d));
    let mut entries = annots_of(doc, page_id);
    entries.push(Object::Reference(id));
    set_annots(doc, page_id, entries)?;
    Ok(id)
}

/// The position of `id` in the page's /Annots, checking it is an editable Link there.
fn locate(doc: &Document, page_id: ObjectId, id: ObjectId) -> Result<usize> {
    let entries = annots_of(doc, page_id);
    let idx = entries
        .iter()
        .position(|e| matches!(e, Object::Reference(r) if *r == id))
        .context("that link no longer exists")?;
    match doc.get_dictionary(id) {
        Ok(d) if is_link(d) => Ok(idx),
        _ => bail!("that annotation is not a link"),
    }
}

/// Remove a link; returns its position and dictionary so it can be restored.
pub fn delete_link(doc: &mut Document, page: u32, id: ObjectId) -> Result<(usize, Dictionary)> {
    let pages = PageIndex::build(doc);
    let page_id = pages.id_of(page).context("that page no longer exists")?;
    let idx = locate(doc, page_id, id)?;
    ensure_unlocked(doc, id)?;
    let mut entries = annots_of(doc, page_id);
    entries.remove(idx);
    set_annots(doc, page_id, entries)?;
    let dict = match doc.objects.get(&id) {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => bail!("that link no longer exists"),
    };
    // Only drop the object when nothing else in the file points at it (a link shared with
    // another page, an outline /A, or a hostile dictionary that is really something else);
    // otherwise it just leaves this page's /Annots. Fails closed: an incomplete scan keeps it.
    super::delete_unreferenced(doc, &[id]);
    Ok((idx, dict))
}

/// Put a removed link back at `index`. Reuses `id` when it is still free, else a fresh id.
pub fn restore_link(
    doc: &mut Document,
    page: u32,
    index: usize,
    dict: Dictionary,
    id: ObjectId,
) -> Result<ObjectId> {
    let pages = PageIndex::build(doc);
    let page_id = pages.id_of(page).context("that page no longer exists")?;
    // The object survived the delete (still referenced elsewhere): point this page back at it.
    let still_there = matches!(doc.objects.get(&id), Some(Object::Dictionary(d)) if *d == dict);
    let new_id = if still_there {
        id
    } else if doc.objects.contains_key(&id) {
        doc.new_object_id()
    } else {
        id
    };
    if new_id.0 > doc.max_id {
        doc.max_id = new_id.0;
    }
    if !still_there {
        doc.objects.insert(new_id, Object::Dictionary(dict));
    }
    let mut entries = annots_of(doc, page_id);
    let at = index.min(entries.len());
    entries.insert(at, Object::Reference(new_id));
    set_annots(doc, page_id, entries)?;
    Ok(new_id)
}

/// Replace a link's whole dictionary; returns the previous one.
pub fn replace_link_dict(
    doc: &mut Document,
    page: u32,
    id: ObjectId,
    dict: Dictionary,
) -> Result<Dictionary> {
    let pages = PageIndex::build(doc);
    let page_id = pages.id_of(page).context("that page no longer exists")?;
    locate(doc, page_id, id)?;
    ensure_unlocked(doc, id)?;
    match doc.objects.insert(id, Object::Dictionary(dict)) {
        Some(Object::Dictionary(old)) => Ok(old),
        _ => bail!("that link no longer exists"),
    }
}

/// Re-point a link: drops its /Dest and /A and writes the new (GoTo / URI only) entry.
/// Returns the previous dictionary.
pub fn set_link_target(
    doc: &mut Document,
    page: u32,
    id: ObjectId,
    target: &TargetInput,
) -> Result<Dictionary> {
    let pages = PageIndex::build(doc);
    let page_id = pages.id_of(page).context("that page no longer exists")?;
    locate(doc, page_id, id)?;
    let (key, val) = {
        let mut spaces = SpaceCache::new(doc, &pages);
        target_entries(doc, target, &pages, &mut spaces)?
    };
    let old = doc.get_dictionary(id).context("link")?.clone();
    let mut new = old.clone();
    new.remove(b"Dest");
    new.remove(b"A");
    new.set(key, val);
    replace_link_dict(doc, page, id, new)?;
    Ok(old)
}
