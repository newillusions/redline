//! The document outline (bookmarks): /Outlines, ISO 32000-1 12.3.3.
//!
//! The file stores a doubly linked list per level (/First /Last /Prev /Next /Parent) with a
//! signed /Count. That shape is awkward to edit in place and easy to corrupt, so edits work on
//! a plain in-memory forest ([`RawNode`]) and the whole outline is rewritten from it
//! ([`write_outline`]). Each node keeps its original dictionary (minus the structural keys),
//! so everything redline does not understand - /C, /F, /SE, Bluebeam-private keys, GoToR and
//! other actions - is carried through an edit untouched.
//!
//! Reading is bounded (visited set, node and depth caps). A cyclic, shared-node, oversized or
//! too deeply nested outline is *damaged*: it is shown (as far as it could be read) but never
//! rewritten, because a rewrite from a partial read would silently drop bookmarks.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::{
    clean_input_text, deref, num, target_of, text_of, PageIndex, SpaceCache, Target, MAX_TEXT_CHARS,
};

/// Most outline items read or kept.
pub const MAX_OUTLINE_NODES: usize = 20_000;
/// Deepest outline nesting read or written (levels, top level = 1).
pub const MAX_OUTLINE_DEPTH: usize = 32;

/// Keys the structure owns; they are rebuilt on write, never carried in [`RawNode::dict`].
const STRUCTURAL_KEYS: [&[u8]; 6] = [b"Parent", b"Prev", b"Next", b"First", b"Last", b"Count"];

/// One bookmark and its children, detached from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct RawNode {
    /// The item dictionary without the structural keys (/Title, /Dest or /A, /C, /F, ...).
    pub dict: Dictionary,
    /// Expanded in the panel (sign of /Count).
    pub open: bool,
    pub children: Vec<RawNode>,
}

impl RawNode {
    pub fn title(&self) -> String {
        title_of(&self.dict)
    }
}

pub(crate) fn title_of(d: &Dictionary) -> String {
    d.get(b"Title")
        .ok()
        .and_then(|o| o.as_str().ok())
        .map(|b| text_of(b, MAX_TEXT_CHARS))
        .unwrap_or_default()
}

/// The outcome of reading /Outlines.
#[derive(Debug, Clone)]
pub struct OutlineRead {
    pub nodes: Vec<RawNode>,
    /// Object ids of every item read, preorder (removed when the outline is rewritten).
    pub item_ids: Vec<ObjectId>,
    /// The /Outlines root dictionary, when it is an indirect object.
    pub root_id: Option<ObjectId>,
    /// Why the outline cannot be safely edited, if it cannot.
    pub damaged: Option<String>,
}

struct ReadState {
    visited: HashSet<ObjectId>,
    ids: Vec<ObjectId>,
    count: usize,
    damaged: Option<String>,
    max_nodes: usize,
    max_depth: usize,
}

/// Read the outline with the default caps.
pub fn read_outline(doc: &Document) -> OutlineRead {
    read_outline_with_limits(doc, MAX_OUTLINE_NODES, MAX_OUTLINE_DEPTH)
}

/// Read the outline with explicit caps (the tests use tiny ones).
pub fn read_outline_with_limits(doc: &Document, max_nodes: usize, max_depth: usize) -> OutlineRead {
    let mut st = ReadState {
        visited: HashSet::new(),
        ids: Vec::new(),
        count: 0,
        damaged: None,
        max_nodes,
        max_depth,
    };
    let mut root_id = None;
    let mut nodes = Vec::new();
    if let Ok(catalog) = doc.catalog() {
        if let Ok(root_obj) = catalog.get(b"Outlines") {
            if let Ok((id, Object::Dictionary(root))) = doc.dereference(root_obj) {
                root_id = id;
                if let Ok(first) = root.get(b"First") {
                    nodes = read_level(doc, first, 0, &mut st);
                }
            }
        }
    }
    OutlineRead {
        nodes,
        item_ids: st.ids,
        root_id,
        damaged: st.damaged,
    }
}

fn read_level(doc: &Document, first: &Object, depth: usize, st: &mut ReadState) -> Vec<RawNode> {
    let mut out = Vec::new();
    let mut cur = match first {
        Object::Reference(r) => Some(*r),
        _ => {
            st.damaged
                .get_or_insert_with(|| "an outline entry is not an indirect object".into());
            return out;
        }
    };
    while let Some(id) = cur {
        if st.damaged.is_some() {
            break;
        }
        if depth >= st.max_depth {
            st.damaged = Some(format!(
                "the outline is nested deeper than {} levels",
                st.max_depth
            ));
            break;
        }
        if st.count >= st.max_nodes {
            st.damaged = Some(format!(
                "the outline has more than {} entries",
                st.max_nodes
            ));
            break;
        }
        if !st.visited.insert(id) {
            st.damaged = Some("the outline loops back on itself or shares an entry".into());
            break;
        }
        let Some(Object::Dictionary(d)) = doc.objects.get(&id) else {
            st.damaged = Some("an outline entry is missing or is not a dictionary".into());
            break;
        };
        st.count += 1;
        st.ids.push(id);

        let count = d
            .get(b"Count")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let mut dict = Dictionary::new();
        for (k, v) in d.iter() {
            if !STRUCTURAL_KEYS.contains(&k.as_slice()) {
                dict.set(k.clone(), v.clone());
            }
        }
        let children = match d.get(b"First") {
            Ok(f) => read_level(doc, f, depth + 1, st),
            Err(_) => Vec::new(),
        };
        out.push(RawNode {
            dict,
            open: count > 0,
            children,
        });
        cur = match d.get(b"Next") {
            Ok(Object::Reference(r)) => Some(*r),
            _ => None,
        };
    }
    out
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

struct IdNode {
    id: ObjectId,
    children: Vec<IdNode>,
}

fn assign(nodes: &[RawNode], ids: &mut impl Iterator<Item = ObjectId>) -> Vec<IdNode> {
    nodes
        .iter()
        .filter_map(|n| {
            let id = ids.next()?;
            Some(IdNode {
                id,
                children: assign(&n.children, ids),
            })
        })
        .collect()
}

/// Insert one level's dictionaries; returns the number of entries visible at this level
/// (each node, plus the visible descendants of every open node).
fn emit_level(doc: &mut Document, nodes: &[RawNode], ids: &[IdNode], parent: ObjectId) -> i64 {
    let mut visible = 0i64;
    for (k, (n, idn)) in nodes.iter().zip(ids.iter()).enumerate() {
        let child_visible = emit_level(doc, &n.children, &idn.children, idn.id);
        let mut d = n.dict.clone();
        d.set("Parent", Object::Reference(parent));
        if k > 0 {
            d.set("Prev", Object::Reference(ids[k - 1].id));
        }
        if k + 1 < ids.len() {
            d.set("Next", Object::Reference(ids[k + 1].id));
        }
        if let (Some(first), Some(last)) = (idn.children.first(), idn.children.last()) {
            d.set("First", Object::Reference(first.id));
            d.set("Last", Object::Reference(last.id));
            d.set(
                "Count",
                Object::Integer(if n.open {
                    child_visible
                } else {
                    -child_visible
                }),
            );
        }
        doc.objects.insert(idn.id, Object::Dictionary(d));
        visible += 1 + if n.open { child_visible } else { 0 };
    }
    visible
}

fn count_nodes(nodes: &[RawNode]) -> usize {
    nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
}

fn height(n: &RawNode) -> usize {
    1 + n.children.iter().map(height).max().unwrap_or(0)
}

/// Replace the document's outline with `forest`. `old_ids` / `root_id` come from the
/// [`OutlineRead`] the forest was derived from; old objects are removed only when nothing else in the file references them.
pub fn write_outline(
    doc: &mut Document,
    forest: &[RawNode],
    old_ids: &[ObjectId],
    root_id: Option<ObjectId>,
) -> Result<()> {
    if count_nodes(forest) > MAX_OUTLINE_NODES {
        bail!("an outline can hold at most {MAX_OUTLINE_NODES} bookmarks");
    }
    if forest.iter().map(height).max().unwrap_or(0) > MAX_OUTLINE_DEPTH {
        bail!("bookmarks can be nested at most {MAX_OUTLINE_DEPTH} levels deep");
    }
    // The new root is always a fresh object: the old one may be anything a hostile file
    // pointed /Outlines at (a page, a shared object), so it is never overwritten.
    let mut old: Vec<ObjectId> = old_ids.to_vec();
    old.extend(root_id);
    if forest.is_empty() {
        doc.catalog_mut().context("catalog")?.remove(b"Outlines");
        super::delete_unreferenced(doc, &old);
        return Ok(());
    }
    let root = doc.new_object_id();
    let total = count_nodes(forest);
    let new_ids: Vec<ObjectId> = (0..total).map(|_| doc.new_object_id()).collect();
    let mut it = new_ids.into_iter();
    let id_tree = assign(forest, &mut it);
    let visible = emit_level(doc, forest, &id_tree, root);

    let mut rd = Dictionary::new();
    rd.set("Type", Object::Name(b"Outlines".to_vec()));
    if let (Some(f), Some(l)) = (id_tree.first(), id_tree.last()) {
        rd.set("First", Object::Reference(f.id));
        rd.set("Last", Object::Reference(l.id));
    }
    if visible > 0 {
        rd.set("Count", Object::Integer(visible));
    }
    doc.objects.insert(root, Object::Dictionary(rd));
    doc.catalog_mut()
        .context("catalog")?
        .set("Outlines", Object::Reference(root));
    // Only now (the catalog points at the new tree) drop old objects that nothing else uses.
    super::delete_unreferenced(doc, &old);
    Ok(())
}

// ---------------------------------------------------------------------------
// Paths and forest edits
// ---------------------------------------------------------------------------

/// Index path from the top level to a node, `[2, 0, 5]` = third top-level item's first
/// child's sixth child.
pub type Path = Vec<usize>;

pub fn parse_path(s: &str) -> Result<Path> {
    if s.is_empty() {
        bail!("empty bookmark id");
    }
    s.split('.')
        .map(|p| {
            p.parse::<usize>()
                .map_err(|_| anyhow::anyhow!("invalid bookmark id {s:?}"))
        })
        .collect()
}

pub fn path_string(p: &[usize]) -> String {
    p.iter().map(usize::to_string).collect::<Vec<_>>().join(".")
}

pub fn get<'a>(forest: &'a [RawNode], path: &[usize]) -> Option<&'a RawNode> {
    let (first, rest) = path.split_first()?;
    let node = forest.get(*first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        get(&node.children, rest)
    }
}

pub fn get_mut<'a>(forest: &'a mut [RawNode], path: &[usize]) -> Option<&'a mut RawNode> {
    let (first, rest) = path.split_first()?;
    let node = forest.get_mut(*first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        get_mut(&mut node.children, rest)
    }
}

/// Where a new or moved node goes relative to the anchor node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Before,
    After,
    Child,
}

/// The siblings list holding `path`'s parent level.
fn level_mut<'a>(forest: &'a mut Vec<RawNode>, parent: &[usize]) -> Option<&'a mut Vec<RawNode>> {
    match parent.split_first() {
        None => Some(forest),
        Some((first, rest)) => {
            let node = forest.get_mut(*first)?;
            level_mut(&mut node.children, rest)
        }
    }
}

/// Insert `node` relative to `anchor`. With no anchor, `Before` goes to the top of the list
/// and `After` to the end; `Child` needs an anchor. Returns the node's new path.
pub fn insert_at(
    forest: &mut Vec<RawNode>,
    anchor: Option<&[usize]>,
    place: Place,
    node: RawNode,
) -> Result<Path> {
    if matches!(anchor, Some(a) if a.is_empty()) {
        bail!("empty bookmark id");
    }
    let (parent, index): (Path, usize) = match (anchor, place) {
        (None, Place::Before) => (vec![], 0),
        (None, Place::After) => (vec![], forest.len()),
        (None, Place::Child) => bail!("select a bookmark to add a child to"),
        (Some(a), Place::Before) => (a[..a.len() - 1].to_vec(), a[a.len() - 1]),
        (Some(a), Place::After) => (a[..a.len() - 1].to_vec(), a[a.len() - 1] + 1),
        (Some(a), Place::Child) => {
            let n = get(forest, a).context("that bookmark no longer exists")?;
            (a.to_vec(), n.children.len())
        }
    };
    let mut new_path = parent.clone();
    new_path.push(index);
    if new_path.len() - 1 + height(&node) > MAX_OUTLINE_DEPTH {
        bail!("bookmarks can be nested at most {MAX_OUTLINE_DEPTH} levels deep");
    }
    if count_nodes(forest) + count_nodes(std::slice::from_ref(&node)) > MAX_OUTLINE_NODES {
        bail!("an outline can hold at most {MAX_OUTLINE_NODES} bookmarks");
    }
    if place == Place::Child {
        if let Some(a) = anchor {
            if let Some(n) = get_mut(forest, a) {
                n.open = true;
            }
        }
    }
    let level = level_mut(forest, &parent).context("that bookmark no longer exists")?;
    if index > level.len() {
        bail!("that bookmark no longer exists");
    }
    level.insert(index, node);
    Ok(new_path)
}

pub fn remove_at(forest: &mut Vec<RawNode>, path: &[usize]) -> Result<RawNode> {
    let (last, parent) = path.split_last().context("empty bookmark id")?;
    let level = level_mut(forest, parent).context("that bookmark no longer exists")?;
    if *last >= level.len() {
        bail!("that bookmark no longer exists");
    }
    Ok(level.remove(*last))
}

/// Move a node (and its subtree) next to, or under, another. Atomic: the forest is left
/// unchanged when the move is refused. A move into the node's own subtree is refused.
pub fn move_node(
    forest: &mut Vec<RawNode>,
    path: &[usize],
    anchor: &[usize],
    place: Place,
) -> Result<Path> {
    if path.is_empty() || anchor.is_empty() {
        bail!("empty bookmark id");
    }
    if anchor.starts_with(path) {
        bail!("a bookmark cannot be moved into itself");
    }
    let mut work = forest.clone();
    // Indices after the removed node at its level shift down by one.
    let d = path.len() - 1;
    let mut adj = anchor.to_vec();
    if adj.len() > d && adj[..d] == path[..d] && adj[d] > path[d] {
        adj[d] -= 1;
    }
    let node = remove_at(&mut work, path)?;
    let new_path = insert_at(&mut work, Some(&adj), place, node)?;
    *forest = work;
    Ok(new_path)
}

/// Apply a saved collapse state: every node with children is open iff its path is in `open`.
pub fn set_open_states(forest: &mut [RawNode], open: &HashSet<String>) {
    fn go(nodes: &mut [RawNode], prefix: &mut Vec<usize>, open: &HashSet<String>) {
        for (i, n) in nodes.iter_mut().enumerate() {
            prefix.push(i);
            if !n.children.is_empty() {
                n.open = open.contains(&path_string(prefix));
            }
            go(&mut n.children, prefix, open);
            prefix.pop();
        }
    }
    go(forest, &mut Vec::new(), open);
}

// ---------------------------------------------------------------------------
// Views for the frontend
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BookmarkView {
    /// Index path, e.g. `"2.0.5"`; valid until the next edit.
    pub id: String,
    pub title: String,
    pub open: bool,
    pub target: Target,
    pub color: Option<[f64; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub children: Vec<BookmarkView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutlineView {
    pub nodes: Vec<BookmarkView>,
    pub total: usize,
    /// Bookmarks whose destination page is gone or unresolvable.
    pub broken: usize,
    /// Set when the outline cannot be edited, with the reason.
    pub read_only: Option<String>,
}

fn color_of(d: &Dictionary, doc: &Document) -> Option<[f64; 3]> {
    let arr = d
        .get(b"C")
        .ok()
        .and_then(|o| deref(doc, o))?
        .as_array()
        .ok()?;
    if arr.len() != 3 {
        return None;
    }
    let v: Vec<f64> = arr
        .iter()
        .filter_map(|o| deref(doc, o).and_then(num))
        .map(|x| x.clamp(0.0, 1.0))
        .collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

fn view_nodes(
    doc: &Document,
    nodes: &[RawNode],
    prefix: &mut Vec<usize>,
    pages: &PageIndex,
    spaces: &mut SpaceCache<'_>,
    broken: &mut usize,
) -> Vec<BookmarkView> {
    let mut out = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        prefix.push(i);
        let target = target_of(doc, &n.dict, pages, spaces);
        if matches!(target, Target::Broken) {
            *broken += 1;
        }
        let flags = n
            .dict
            .get(b"F")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let children = view_nodes(doc, &n.children, prefix, pages, spaces, broken);
        out.push(BookmarkView {
            id: path_string(prefix),
            title: n.title(),
            open: n.open,
            target,
            color: color_of(&n.dict, doc),
            italic: flags & 1 != 0,
            bold: flags & 2 != 0,
            children,
        });
        prefix.pop();
    }
    out
}

/// The panel's view of an outline read.
pub fn build_view(doc: &Document, read: &OutlineRead, pages: &PageIndex) -> OutlineView {
    let mut spaces = SpaceCache::new(doc, pages);
    let mut broken = 0usize;
    let nodes = view_nodes(
        doc,
        &read.nodes,
        &mut Vec::new(),
        pages,
        &mut spaces,
        &mut broken,
    );
    OutlineView {
        nodes,
        total: count_nodes(&read.nodes),
        broken,
        read_only: read.damaged.clone(),
    }
}

// ---------------------------------------------------------------------------
// Node construction
// ---------------------------------------------------------------------------

/// A new bookmark with a title and one target entry (`Dest` or `A`).
pub fn new_node(title: &str, entry: Option<(&'static str, Object)>) -> Result<RawNode> {
    let title = clean_input_text(title, MAX_TEXT_CHARS);
    if title.trim().is_empty() {
        bail!("a bookmark needs a title");
    }
    let mut dict = Dictionary::new();
    dict.set("Title", crate::markup::replies::encode_text(&title));
    if let Some((k, v)) = entry {
        dict.set(k, v);
    }
    Ok(RawNode {
        dict,
        open: false,
        children: Vec::new(),
    })
}

/// Replace a node's target: removes /Dest and /A, then sets the new entry.
pub fn set_target(node: &mut RawNode, entry: (&'static str, Object)) {
    node.dict.remove(b"Dest");
    node.dict.remove(b"A");
    node.dict.set(entry.0, entry.1);
}

/// Set a node's title, colour and style flags.
pub fn set_style(node: &mut RawNode, color: Option<[f64; 3]>, bold: bool, italic: bool) {
    match color {
        Some(c) if c.iter().all(|v| v.is_finite()) => {
            node.dict.set(
                "C",
                Object::Array(
                    c.iter()
                        .map(|v| Object::Real(v.clamp(0.0, 1.0) as f32))
                        .collect(),
                ),
            );
        }
        _ => {
            node.dict.remove(b"C");
        }
    }
    let flags = (i64::from(bold) << 1) | i64::from(italic);
    if flags == 0 {
        node.dict.remove(b"F");
    } else {
        node.dict.set("F", Object::Integer(flags));
    }
}

pub fn set_title(node: &mut RawNode, title: &str) -> Result<()> {
    let title = clean_input_text(title, MAX_TEXT_CHARS);
    if title.trim().is_empty() {
        bail!("a bookmark needs a title");
    }
    node.dict
        .set("Title", crate::markup::replies::encode_text(&title));
    Ok(())
}
