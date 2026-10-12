//! The edit layer the commands call: one [`NavEdit`] in, the previous state out as a
//! [`NavUndo`] that, applied, restores it (and returns the inverse for redo).
//!
//! Everything here runs on an in-memory lopdf `Document`; the caller owns loading, the
//! stale-file guard, the atomic write and the render-engine reload (the same path every page
//! operation uses, `commands::document::apply_page_edit`).

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::labels::{self, LabelRange, LabelStyle};
use super::links::{self, LinkView};
use super::outline::{self, OutlineView, Place, RawNode};
use super::{target_entries, target_of, PageIndex, PageView, SpaceCache, Target, TargetInput};

/// Everything the Navigator panels need, in one read.
#[derive(Debug, Clone, Serialize)]
pub struct NavigationInfo {
    pub page_count: u32,
    pub outline: OutlineView,
    /// Label text per page; `""` where a page has none.
    pub labels: Vec<String>,
    pub links: Vec<LinkView>,
    pub links_truncated: bool,
    /// Set when the whole file cannot be edited (a password-protected PDF), with the reason.
    pub read_only: Option<String>,
    /// Set when the page-label tree could not be read completely, so label edits are refused.
    pub labels_read_only: Option<String>,
    /// Steps available to Undo / Redo (filled in by the command layer from the history).
    pub undo_depth: usize,
    pub redo_depth: usize,
}

/// Read bookmarks, labels and links from `doc`.
pub fn read_info(doc: &Document) -> NavigationInfo {
    let pages = PageIndex::build(doc);
    let read = outline::read_outline(doc);
    let outline = outline::build_view(doc, &read, &pages);
    let ranges = labels::read_ranges(doc);
    let (links, links_truncated) = links::read_links(doc, &pages);
    NavigationInfo {
        page_count: pages.count(),
        outline,
        labels: labels::page_labels(&ranges, pages.count()),
        links,
        links_truncated,
        read_only: None,
        labels_read_only: labels::read_only_reason(doc),
        undo_depth: 0,
        redo_depth: 0,
    }
}

/// An edit the user asked for. Bookmark ids are the index paths from [`OutlineView`]; `expect`
/// is the title the panel showed for that id, checked so a stale panel cannot edit the wrong
/// bookmark.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NavEdit {
    BookmarkAdd {
        anchor: Option<String>,
        anchor_expect: Option<String>,
        place: Place,
        title: String,
        target: TargetInput,
    },
    BookmarkRename {
        id: String,
        expect: String,
        title: String,
    },
    BookmarkDelete {
        id: String,
        expect: String,
    },
    BookmarkMove {
        id: String,
        expect: String,
        anchor: String,
        anchor_expect: String,
        place: Place,
    },
    BookmarkSetTarget {
        id: String,
        expect: String,
        target: TargetInput,
    },
    BookmarkSetStyle {
        id: String,
        expect: String,
        color: Option<[f64; 3]>,
        bold: bool,
        italic: bool,
    },
    /// Save the panel's expand / collapse state: `open` lists the ids of the open bookmarks.
    BookmarkSetOpen {
        open: Vec<String>,
    },
    /// One top-level bookmark per page, titled with the page's label.
    BookmarksFromLabels {
        pages: Vec<u32>,
    },
    LabelsNumber {
        pages: Vec<u32>,
        style: Option<LabelStyle>,
        prefix: String,
        start: u32,
    },
    LabelRename {
        page: u32,
        text: String,
    },
    /// Pages with a bookmark take the bookmark's title as a text-only label.
    LabelsFromBookmarks {
        pages: Vec<u32>,
    },
    LinkCreate {
        page: u32,
        rect: [f64; 4],
        target: TargetInput,
    },
    LinkDelete {
        page: u32,
        id: String,
    },
    LinkSetTarget {
        page: u32,
        id: String,
        target: TargetInput,
    },
}

/// The state to restore. Applying one returns its inverse.
#[derive(Debug, Clone)]
pub enum NavUndo {
    Outline(Vec<RawNode>),
    Labels(Vec<LabelRange>),
    LinkRemove {
        page: u32,
        id: ObjectId,
    },
    LinkInsert {
        page: u32,
        index: usize,
        id: ObjectId,
        dict: Dictionary,
    },
    LinkReplace {
        page: u32,
        id: ObjectId,
        dict: Dictionary,
    },
}

const STALE: &str = "The bookmarks changed since the panel was drawn; it has been refreshed.";

fn check_page_list(pages: &[u32], count: u32) -> Result<Vec<u32>> {
    let mut v: Vec<u32> = pages.to_vec();
    v.sort_unstable();
    v.dedup();
    if v.is_empty() {
        bail!("no pages selected");
    }
    if let Some(p) = v.iter().find(|p| **p >= count) {
        bail!("page {} is outside this {count}-page document", p + 1);
    }
    Ok(v)
}

fn expect_title(forest: &[RawNode], path: &[usize], expect: &str) -> Result<()> {
    match outline::get(forest, path) {
        Some(n) if n.title() == expect => Ok(()),
        _ => bail!(STALE),
    }
}

/// Run `f` on the outline forest and write the result back. Refuses a damaged outline.
fn edit_outline(
    doc: &mut Document,
    f: impl FnOnce(&mut Document, &mut Vec<RawNode>) -> Result<()>,
) -> Result<NavUndo> {
    let read = outline::read_outline(doc);
    if let Some(reason) = &read.damaged {
        bail!("The bookmarks are read-only: {reason}.");
    }
    let before = read.nodes.clone();
    let mut forest = read.nodes;
    f(doc, &mut forest)?;
    outline::write_outline(doc, &forest, &read.item_ids, read.root_id)?;
    Ok(NavUndo::Outline(before))
}

fn target_pair(doc: &Document, target: &TargetInput) -> Result<(&'static str, Object)> {
    let pages = PageIndex::build(doc);
    let mut spaces = SpaceCache::new(doc, &pages);
    target_entries(doc, target, &pages, &mut spaces)
}

/// Apply an edit; returns the undo entry that restores the previous state.
pub fn apply_edit(doc: &mut Document, edit: &NavEdit) -> Result<NavUndo> {
    match edit {
        NavEdit::BookmarkAdd {
            anchor,
            anchor_expect,
            place,
            title,
            target,
        } => {
            let entry = target_pair(doc, target)?;
            let node = outline::new_node(title, Some(entry))?;
            let anchor = anchor.as_deref().map(outline::parse_path).transpose()?;
            edit_outline(doc, |_, forest| {
                if let (Some(a), Some(e)) = (&anchor, anchor_expect) {
                    expect_title(forest, a, e)?;
                }
                outline::insert_at(forest, anchor.as_deref(), *place, node)?;
                Ok(())
            })
        }
        NavEdit::BookmarkRename { id, expect, title } => {
            let path = outline::parse_path(id)?;
            edit_outline(doc, |_, forest| {
                expect_title(forest, &path, expect)?;
                let n = outline::get_mut(forest, &path).context(STALE)?;
                outline::set_title(n, title)
            })
        }
        NavEdit::BookmarkDelete { id, expect } => {
            let path = outline::parse_path(id)?;
            edit_outline(doc, |_, forest| {
                expect_title(forest, &path, expect)?;
                outline::remove_at(forest, &path)?;
                Ok(())
            })
        }
        NavEdit::BookmarkMove {
            id,
            expect,
            anchor,
            anchor_expect,
            place,
        } => {
            let path = outline::parse_path(id)?;
            let anchor = outline::parse_path(anchor)?;
            edit_outline(doc, |_, forest| {
                expect_title(forest, &path, expect)?;
                expect_title(forest, &anchor, anchor_expect)?;
                outline::move_node(forest, &path, &anchor, *place)?;
                Ok(())
            })
        }
        NavEdit::BookmarkSetTarget { id, expect, target } => {
            let path = outline::parse_path(id)?;
            let entry = target_pair(doc, target)?;
            edit_outline(doc, |_, forest| {
                expect_title(forest, &path, expect)?;
                let n = outline::get_mut(forest, &path).context(STALE)?;
                outline::set_target(n, entry);
                Ok(())
            })
        }
        NavEdit::BookmarkSetStyle {
            id,
            expect,
            color,
            bold,
            italic,
        } => {
            let path = outline::parse_path(id)?;
            edit_outline(doc, |_, forest| {
                expect_title(forest, &path, expect)?;
                let n = outline::get_mut(forest, &path).context(STALE)?;
                outline::set_style(n, *color, *bold, *italic);
                Ok(())
            })
        }
        NavEdit::BookmarkSetOpen { open } => {
            let open: HashSet<String> = open.iter().cloned().collect();
            edit_outline(doc, |_, forest| {
                outline::set_open_states(forest, &open);
                Ok(())
            })
        }
        NavEdit::BookmarksFromLabels { pages } => {
            let page_count = PageIndex::build(doc).count();
            let pages = check_page_list(pages, page_count)?;
            let ranges = labels::read_ranges(doc);
            let texts = labels::page_labels(&ranges, page_count);
            let mut new_nodes = Vec::with_capacity(pages.len());
            for p in &pages {
                let label = texts.get(*p as usize).cloned().unwrap_or_default();
                let title = if label.trim().is_empty() {
                    format!("Page {}", p + 1)
                } else {
                    label
                };
                let entry = target_pair(
                    doc,
                    &TargetInput::Page {
                        page: *p,
                        view: PageView::inherit(),
                    },
                )?;
                new_nodes.push(outline::new_node(&title, Some(entry))?);
            }
            edit_outline(doc, |_, forest| {
                for n in new_nodes {
                    outline::insert_at(forest, None, Place::After, n)?;
                }
                Ok(())
            })
        }
        NavEdit::LabelsNumber {
            pages,
            style,
            prefix,
            start,
        } => {
            let page_count = PageIndex::build(doc).count();
            let pages = check_page_list(pages, page_count)?;
            let before = labels::read_ranges_for_edit(doc)?;
            let new = labels::apply_numbering(&before, page_count, &pages, *style, prefix, *start)?;
            labels::write_ranges(doc, &new)?;
            Ok(NavUndo::Labels(before))
        }
        NavEdit::LabelRename { page, text } => {
            let page_count = PageIndex::build(doc).count();
            let before = labels::read_ranges_for_edit(doc)?;
            let new = labels::rename_label(&before, page_count, *page, text)?;
            labels::write_ranges(doc, &new)?;
            Ok(NavUndo::Labels(before))
        }
        NavEdit::LabelsFromBookmarks { pages } => {
            let page_count = PageIndex::build(doc).count();
            let wanted: HashSet<u32> = check_page_list(pages, page_count)?.into_iter().collect();
            let titles = bookmark_titles_by_page(doc);
            let before = labels::read_ranges_for_edit(doc)?;
            let mut specs = labels::expand(&before, page_count);
            let mut changed = 0usize;
            for (page, title) in titles {
                if wanted.contains(&page) {
                    if let Some(slot) = specs.get_mut(page as usize) {
                        *slot = Some(labels::PageSpec {
                            style: None,
                            prefix: super::clean_input_text(&title, labels::MAX_LABEL_CHARS),
                            number: 1,
                        });
                        changed += 1;
                    }
                }
            }
            if changed == 0 {
                bail!("none of those pages has a bookmark");
            }
            labels::write_ranges(doc, &labels::compress(&specs))?;
            Ok(NavUndo::Labels(before))
        }
        NavEdit::LinkCreate { page, rect, target } => {
            let id = links::create_link(doc, *page, *rect, target)?;
            Ok(NavUndo::LinkRemove { page: *page, id })
        }
        NavEdit::LinkDelete { page, id } => {
            let id = links::parse_link_id(id)?;
            let (index, dict) = links::delete_link(doc, *page, id)?;
            Ok(NavUndo::LinkInsert {
                page: *page,
                index,
                id,
                dict,
            })
        }
        NavEdit::LinkSetTarget { page, id, target } => {
            let id = links::parse_link_id(id)?;
            let old = links::set_link_target(doc, *page, id, target)?;
            Ok(NavUndo::LinkReplace {
                page: *page,
                id,
                dict: old,
            })
        }
    }
}

/// The first bookmark title (outline order) for every page a bookmark points at.
fn bookmark_titles_by_page(doc: &Document) -> Vec<(u32, String)> {
    let read = outline::read_outline(doc);
    let pages = PageIndex::build(doc);
    let mut spaces = SpaceCache::new(doc, &pages);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    fn go(
        doc: &Document,
        nodes: &[RawNode],
        pages: &PageIndex,
        spaces: &mut SpaceCache<'_>,
        seen: &mut HashSet<u32>,
        out: &mut Vec<(u32, String)>,
    ) {
        for n in nodes {
            if let Target::Page { page, .. } = target_of(doc, &n.dict, pages, spaces) {
                let t = n.title();
                if !t.trim().is_empty() && seen.insert(page) {
                    out.push((page, t));
                }
            }
            go(doc, &n.children, pages, spaces, seen, out);
        }
    }
    go(doc, &read.nodes, &pages, &mut spaces, &mut seen, &mut out);
    out
}

/// Restore a previous state; returns the entry that undoes this restore (for redo).
pub fn apply_undo(doc: &mut Document, entry: &NavUndo) -> Result<NavUndo> {
    match entry {
        NavUndo::Outline(state) => {
            let read = outline::read_outline(doc);
            if let Some(reason) = &read.damaged {
                bail!("The bookmarks are read-only: {reason}.");
            }
            let current = read.nodes.clone();
            outline::write_outline(doc, state, &read.item_ids, read.root_id)?;
            Ok(NavUndo::Outline(current))
        }
        NavUndo::Labels(state) => {
            let current = labels::read_ranges_for_edit(doc)?;
            labels::write_ranges(doc, state)?;
            Ok(NavUndo::Labels(current))
        }
        NavUndo::LinkRemove { page, id } => {
            let (index, dict) = links::delete_link(doc, *page, *id)?;
            Ok(NavUndo::LinkInsert {
                page: *page,
                index,
                id: *id,
                dict,
            })
        }
        NavUndo::LinkInsert {
            page,
            index,
            id,
            dict,
        } => {
            let new_id = links::restore_link(doc, *page, *index, dict.clone(), *id)?;
            Ok(NavUndo::LinkRemove {
                page: *page,
                id: new_id,
            })
        }
        NavUndo::LinkReplace { page, id, dict } => {
            let old = links::replace_link_dict(doc, *page, *id, dict.clone())?;
            Ok(NavUndo::LinkReplace {
                page: *page,
                id: *id,
                dict: old,
            })
        }
    }
}

/// Per-document undo / redo stacks.
#[derive(Debug, Default)]
pub struct NavHistory {
    pub undo: Vec<NavUndo>,
    pub redo: Vec<NavUndo>,
}

/// Most undo steps kept per document.
pub const MAX_HISTORY: usize = 30;

impl NavHistory {
    pub fn record(&mut self, entry: NavUndo) {
        self.undo.push(entry);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

// ---------------------------------------------------------------------------
// Page operations
// ---------------------------------------------------------------------------

/// Carry page labels through a page operation (call before the operation, while the page
/// count is still the old one). `map[new_index]` is the old index of the page that ends up at
/// `new_index`, or `None` for a new blank page. Rotate does not change the page order, so it
/// needs no call.
pub fn carry_labels(doc: &mut Document, map: &[Option<u32>]) -> Result<()> {
    // A partly read tree is never rewritten: leave /PageLabels as it is.
    if labels::read_only_reason(doc).is_some() {
        return Ok(());
    }
    let ranges = labels::read_ranges(doc);
    if ranges.is_empty() {
        return Ok(());
    }
    let old_count = PageIndex::build(doc).count();
    let new = labels::remap(&ranges, old_count, map);
    labels::write_ranges(doc, &new)
}

/// The map for deleting page `idx` from a `count`-page document.
pub fn map_delete(count: u32, idx: u32) -> Vec<Option<u32>> {
    (0..count).filter(|i| *i != idx).map(Some).collect()
}

/// The map for inserting a blank page at `at` in a `count`-page document.
pub fn map_insert(count: u32, at: u32) -> Vec<Option<u32>> {
    let at = at.min(count);
    (0..at)
        .map(Some)
        .chain(std::iter::once(None))
        .chain((at..count).map(Some))
        .collect()
}

/// The map for `reorder_pages(new_order)`: new page `j` is old page `new_order[j]`.
pub fn map_reorder(new_order: &[u32]) -> Vec<Option<u32>> {
    new_order.iter().map(|o| Some(*o)).collect()
}

/// [`carry_labels`] for `delete_page(idx)`.
pub fn carry_labels_delete(doc: &mut Document, idx: u32) -> Result<()> {
    let n = PageIndex::build(doc).count();
    carry_labels(doc, &map_delete(n, idx))
}

/// [`carry_labels`] for `insert_blank_page(at)`.
pub fn carry_labels_insert(doc: &mut Document, at: u32) -> Result<()> {
    let n = PageIndex::build(doc).count();
    carry_labels(doc, &map_insert(n, at))
}

/// [`carry_labels`] for `reorder_pages(new_order)`.
pub fn carry_labels_reorder(doc: &mut Document, new_order: &[u32]) -> Result<()> {
    carry_labels(doc, &map_reorder(new_order))
}
