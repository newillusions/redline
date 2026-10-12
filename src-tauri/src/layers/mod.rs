//! PDF layers (optional content groups, ISO 32000-1 section 8.11): read the document's
//! layer model, and (in the submodules) render a per-view visibility state and edit the
//! file's layer structure. Behaviour spec: `docs/layers.md`.
//!
//! Everything here reads UNTRUSTED input (a PDF's `/OCProperties`): every walk is bounded in
//! count and depth, cycles in `/Order` are cut, strings become text (never markup), and
//! nothing panics on a malformed structure - a damaged entry is skipped, never fatal.

pub mod edit;
pub mod overlay;

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

/// Most layers (OCGs) read from one document; the rest are ignored and `truncated` is set.
pub const MAX_LAYERS: usize = 5_000;
/// Most `/Order` elements walked.
pub const MAX_ORDER_NODES: usize = 20_000;
/// Deepest `/Order` nesting walked.
pub const MAX_ORDER_DEPTH: usize = 16;
/// Longest layer / group name kept, in characters.
pub const MAX_NAME_CHARS: usize = 256;
/// Most membership dictionaries (OCMD) read.
pub const MAX_OCMDS: usize = 2_000;
/// Deepest `/VE` visibility expression read.
pub const MAX_VE_DEPTH: usize = 8;
/// Most nodes in one `/VE` visibility expression.
pub const MAX_VE_NODES: usize = 64;
/// Most radio-button groups, and most members per group.
pub const MAX_RB_GROUPS: usize = 256;
pub const MAX_RB_MEMBERS: usize = 256;

/// One optional content group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    /// The group's object number (the key every other structure uses).
    pub id: u32,
    pub gen: u16,
    pub name: String,
    /// Visible when the file is opened, as the default configuration (and the group's own
    /// `/Usage /View /ViewState`) give it.
    pub default_on: bool,
    /// Listed in the default configuration's `/Locked`: its state is not changeable in the UI.
    pub locked: bool,
    /// False when the group's `/Intent` excludes `View`: it does not affect what is shown.
    pub view_intent: bool,
    /// The group's own `/Usage /View /ViewState` when present (it outranks the default
    /// configuration in the renderer; see `docs/layers.md`).
    pub usage_view_state: Option<bool>,
}

/// One row of the layer list, from `/Order`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LayerNode {
    /// A layer; `children` are its sub-layers (an unlabelled nested array after it).
    Layer { id: u32, children: Vec<LayerNode> },
    /// A labelled collection (a nested array whose first element is a string): not a layer,
    /// no visibility toggle.
    Group {
        label: String,
        children: Vec<LayerNode>,
    },
}

/// A visibility expression (`/VE`), ISO 32000-1 table 99.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OcExpr {
    Group(u32),
    And(Vec<OcExpr>),
    Or(Vec<OcExpr>),
    Not(Box<OcExpr>),
}

/// An optional content membership dictionary: content can belong to one of these instead of
/// a single group. Evaluated by the frontend (`src/lib/layers.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ocmd {
    pub id: u32,
    /// `AllOn`, `AnyOn` (default), `AnyOff` or `AllOff`.
    pub policy: String,
    pub ocgs: Vec<u32>,
    pub ve: Option<OcExpr>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerModel {
    pub layers: Vec<Layer>,
    pub tree: Vec<LayerNode>,
    pub ocmds: Vec<Ocmd>,
    /// Radio-button groups: turning one member on turns the others off.
    pub rb_groups: Vec<Vec<u32>>,
    pub has_ocproperties: bool,
    /// Some structure was cut at a bound.
    pub truncated: bool,
    /// Groups past [`MAX_LAYERS`] were not read: the configuration cannot be rewritten
    /// without switching them on.
    pub layers_truncated: bool,
    /// Layers not listed in `/Order`, appended to the list (the spec would hide them; see
    /// `docs/layers.md`).
    pub unlisted: usize,
    /// Per-view visibility can be rendered for this file (set by the caller).
    pub view_supported: bool,
    pub view_unsupported_reason: Option<String>,
    pub encrypted: bool,
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, o)?.as_dict().ok()
}

fn array_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Vec<Object>> {
    deref(doc, o)?.as_array().ok()
}

/// Text a PDF string carries, cleaned for display: control characters dropped, bounded.
pub fn clean_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn text_of(o: &Object) -> Option<String> {
    let Object::String(bytes, _) = o else {
        return None;
    };
    Some(clean_text(&lopdf::decode_text_string(o).unwrap_or_else(
        |_| String::from_utf8_lossy(bytes).into_owned(),
    )))
}

/// The object numbers of the references in a (possibly indirect) array, bounded.
fn ref_ids(doc: &Document, o: Option<&Object>, cap: usize) -> Vec<ObjectId> {
    let Some(arr) = o.and_then(|o| array_of(doc, o)) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| e.as_reference().ok())
        .take(cap)
        .collect()
}

fn name_is(o: Option<&Object>, want: &[u8]) -> bool {
    matches!(o, Some(Object::Name(n)) if n.as_slice() == want)
}

/// True when the group's `/Intent` includes View (absent = View; `All` counts).
fn has_view_intent(doc: &Document, ocg: &Dictionary) -> bool {
    match ocg.get(b"Intent").ok().and_then(|o| deref(doc, o)) {
        None => true,
        Some(Object::Name(n)) => n == b"View" || n == b"All",
        Some(Object::Array(a)) => a
            .iter()
            .any(|e| matches!(e, Object::Name(n) if n == b"View" || n == b"All")),
        Some(_) => true,
    }
}

/// `/Usage /View /ViewState` of a group (`ON` -> true, `OFF` -> false).
pub fn usage_view_state(doc: &Document, ocg: &Dictionary) -> Option<bool> {
    let usage = dict_of(doc, ocg.get(b"Usage").ok()?)?;
    let view = dict_of(doc, usage.get(b"View").ok()?)?;
    match deref(doc, view.get(b"ViewState").ok()?)? {
        Object::Name(n) if n == b"ON" => Some(true),
        Object::Name(n) if n == b"OFF" => Some(false),
        _ => None,
    }
}

struct Walk<'a> {
    doc: &'a Document,
    layer_ids: &'a HashSet<u32>,
    placed: HashSet<u32>,
    budget: usize,
    truncated: bool,
}

impl Walk<'_> {
    /// Rows for the elements of one `/Order` array. `path` holds the indirect arrays being
    /// walked, so a reference cycle is cut instead of followed.
    fn rows(&mut self, items: &[Object], depth: usize, path: &mut Vec<ObjectId>) -> Vec<LayerNode> {
        let mut out: Vec<LayerNode> = Vec::new();
        for el in items {
            if self.budget == 0 {
                self.truncated = true;
                break;
            }
            self.budget -= 1;
            let (rid, target) = match el {
                Object::Reference(r) => match self.doc.get_object(*r) {
                    Ok(t) => (Some(*r), t),
                    Err(_) => continue,
                },
                other => (None, other),
            };
            match target {
                Object::Dictionary(_) => {
                    if let Some(r) = rid {
                        if self.layer_ids.contains(&r.0) && self.placed.insert(r.0) {
                            out.push(LayerNode::Layer {
                                id: r.0,
                                children: Vec::new(),
                            });
                        }
                    }
                }
                Object::Array(inner) => {
                    if depth >= MAX_ORDER_DEPTH {
                        self.truncated = true;
                        continue;
                    }
                    if let Some(r) = rid {
                        if path.contains(&r) {
                            self.truncated = true;
                            continue;
                        }
                        path.push(r);
                    }
                    let label = inner.first().and_then(text_of);
                    let rest = if label.is_some() {
                        &inner[1..]
                    } else {
                        &inner[..]
                    };
                    let children = self.rows(rest, depth + 1, path);
                    if rid.is_some() {
                        path.pop();
                    }
                    match label {
                        Some(label) => out.push(LayerNode::Group { label, children }),
                        // An unlabelled array lists the sub-layers of the element before it.
                        None => match out.last_mut() {
                            Some(LayerNode::Layer { children: c, .. })
                            | Some(LayerNode::Group { children: c, .. }) => c.extend(children),
                            None => out.extend(children),
                        },
                    }
                }
                _ => {}
            }
        }
        out
    }
}

fn parse_ve(doc: &Document, o: &Object, depth: usize, nodes: &mut usize) -> Option<OcExpr> {
    if depth > MAX_VE_DEPTH || *nodes >= MAX_VE_NODES {
        return None;
    }
    *nodes += 1;
    if let Object::Reference(r) = o {
        // A reference is a group unless it points at an array (an indirect sub-expression).
        match doc.get_object(*r).ok()? {
            Object::Array(_) => {}
            _ => return Some(OcExpr::Group(r.0)),
        }
    }
    let arr = array_of(doc, o)?;
    let op = match arr.first()? {
        Object::Name(n) => n.clone(),
        _ => return None,
    };
    let mut args = Vec::new();
    for a in &arr[1..] {
        args.push(parse_ve(doc, a, depth + 1, nodes)?);
    }
    match op.as_slice() {
        b"And" if !args.is_empty() => Some(OcExpr::And(args)),
        b"Or" if !args.is_empty() => Some(OcExpr::Or(args)),
        b"Not" if args.len() == 1 => Some(OcExpr::Not(Box::new(args.remove(0)))),
        _ => None,
    }
}

fn read_ocmd(doc: &Document, id: u32, d: &Dictionary) -> Ocmd {
    let ocgs: Vec<u32> = match d.get(b"OCGs").ok() {
        Some(Object::Reference(r)) if matches!(doc.get_object(*r), Ok(Object::Dictionary(_))) => {
            vec![r.0]
        }
        Some(o) => ref_ids(doc, Some(o), MAX_LAYERS)
            .into_iter()
            .map(|r| r.0)
            .collect(),
        None => Vec::new(),
    };
    let policy = match d.get(b"P").ok().and_then(|o| deref(doc, o)) {
        Some(Object::Name(n))
            if matches!(n.as_slice(), b"AllOn" | b"AnyOn" | b"AnyOff" | b"AllOff") =>
        {
            String::from_utf8_lossy(n).into_owned()
        }
        _ => "AnyOn".to_string(),
    };
    let ve = d.get(b"VE").ok().and_then(|o| parse_ve(doc, o, 0, &mut 0));
    Ocmd {
        id,
        policy,
        ocgs,
        ve,
    }
}

/// Read the layer model of `doc`. Never fails: a document with no (or a broken)
/// `/OCProperties` yields an empty model.
pub fn read_model(doc: &Document) -> LayerModel {
    let mut model = LayerModel {
        encrypted: doc.is_encrypted(),
        ..Default::default()
    };
    let Ok(catalog) = doc.catalog() else {
        return model;
    };
    let Some(ocp) = catalog
        .get(b"OCProperties")
        .ok()
        .and_then(|o| dict_of(doc, o))
    else {
        return model;
    };
    model.has_ocproperties = true;

    // The configuration the file opens in.
    let d = ocp.get(b"D").ok().and_then(|o| dict_of(doc, o));
    let base_on =
        !d.is_some_and(|d| name_is(d.get(b"BaseState").ok().and_then(|o| deref(doc, o)), b"OFF"));
    let on_set: HashSet<u32> = d.map_or_else(HashSet::new, |d| {
        ref_ids(doc, d.get(b"ON").ok(), MAX_LAYERS)
            .into_iter()
            .map(|r| r.0)
            .collect()
    });
    let off_set: HashSet<u32> = d.map_or_else(HashSet::new, |d| {
        ref_ids(doc, d.get(b"OFF").ok(), MAX_LAYERS)
            .into_iter()
            .map(|r| r.0)
            .collect()
    });
    let locked: HashSet<u32> = d.map_or_else(HashSet::new, |d| {
        ref_ids(doc, d.get(b"Locked").ok(), MAX_LAYERS)
            .into_iter()
            .map(|r| r.0)
            .collect()
    });

    // Every group, in /OCGs order.
    let listed = ocp.get(b"OCGs").ok().and_then(|o| array_of(doc, o));
    let mut seen: HashSet<u32> = HashSet::new();
    for el in listed.into_iter().flatten() {
        let Ok(r) = el.as_reference() else { continue };
        if !seen.insert(r.0) {
            continue;
        }
        if model.layers.len() >= MAX_LAYERS {
            model.truncated = true;
            model.layers_truncated = true;
            break;
        }
        let Ok(Object::Dictionary(g)) = doc.get_object(r) else {
            continue;
        };
        let name = g
            .get(b"Name")
            .ok()
            .and_then(|o| deref(doc, o))
            .and_then(text_of)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "(unnamed layer)".to_string());
        let usage = usage_view_state(doc, g);
        let mut default_on = base_on;
        if on_set.contains(&r.0) {
            default_on = true;
        }
        if off_set.contains(&r.0) {
            default_on = false;
        }
        if let Some(v) = usage {
            default_on = v;
        }
        model.layers.push(Layer {
            id: r.0,
            gen: r.1,
            name,
            default_on,
            locked: locked.contains(&r.0),
            view_intent: has_view_intent(doc, g),
            usage_view_state: usage,
        });
    }

    // The presentation tree.
    let layer_ids: HashSet<u32> = model.layers.iter().map(|l| l.id).collect();
    let order = d
        .and_then(|d| d.get(b"Order").ok())
        .and_then(|o| array_of(doc, o));
    let mut walk = Walk {
        doc,
        layer_ids: &layer_ids,
        placed: HashSet::new(),
        budget: MAX_ORDER_NODES,
        truncated: false,
    };
    let mut tree = match order {
        Some(items) if !items.is_empty() => walk.rows(items, 0, &mut Vec::new()),
        _ => Vec::new(),
    };
    for l in &model.layers {
        if walk.placed.insert(l.id) {
            tree.push(LayerNode::Layer {
                id: l.id,
                children: Vec::new(),
            });
            model.unlisted += 1;
        }
    }
    model.truncated |= walk.truncated;
    model.tree = tree;

    // Radio-button groups.
    if let Some(groups) = d
        .and_then(|d| d.get(b"RBGroups").ok())
        .and_then(|o| array_of(doc, o))
    {
        for g in groups.iter().take(MAX_RB_GROUPS) {
            let members: Vec<u32> = ref_ids(doc, Some(g), MAX_RB_MEMBERS)
                .into_iter()
                .map(|r| r.0)
                .filter(|id| layer_ids.contains(id))
                .collect();
            if members.len() > 1 {
                model.rb_groups.push(members);
            }
        }
    }

    // Membership dictionaries anywhere in the file.
    let mut ocmds: Vec<(u32, Ocmd)> = Vec::new();
    for (&(num, _), obj) in doc.objects.iter() {
        let Object::Dictionary(d) = obj else { continue };
        if !name_is(d.get(b"Type").ok(), b"OCMD") {
            continue;
        }
        if ocmds.len() >= MAX_OCMDS {
            model.truncated = true;
            break;
        }
        ocmds.push((num, read_ocmd(doc, num, d)));
    }
    ocmds.sort_by_key(|(n, _)| *n);
    model.ocmds = ocmds.into_iter().map(|(_, o)| o).collect();
    model
}

/// Whether object `r` is a group listed in the file's `/OCProperties /OCGs`.
fn is_listed_group(doc: &Document, r: ObjectId) -> bool {
    let Some(ocp) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"OCProperties").ok())
        .and_then(|o| dict_of(doc, o))
    else {
        return false;
    };
    ocp.get(b"OCGs")
        .ok()
        .and_then(|o| array_of(doc, o))
        .is_some_and(|a| {
            a.iter()
                .any(|e| matches!(e, Object::Reference(x) if *x == r))
        })
}

/// Whether an annotation's `/OC` may point at object `r`: a group (`/Type /OCG`, or any
/// dictionary the file lists in `/OCGs`, since `read_model` presents those as layers even
/// without a `/Type`) or a membership dictionary (`/Type /OCMD`). A missing or non-dictionary
/// target, or a dictionary that is neither, is not optional content.
pub fn is_oc_target(doc: &Document, r: ObjectId) -> bool {
    let Ok(Object::Dictionary(d)) = doc.get_object(r) else {
        return false;
    };
    name_is(d.get(b"Type").ok(), b"OCG")
        || name_is(d.get(b"Type").ok(), b"OCMD")
        || is_listed_group(doc, r)
}

/// The display name of the group `num`, for filling a mark-up's `layer` name from its `/OC`
/// reference (an OCMD has no single name).
pub fn name_of_oc(doc: &Document, num: u32, gen: u16) -> Option<String> {
    let Ok(Object::Dictionary(d)) = doc.get_object((num, gen)) else {
        return None;
    };
    if name_is(d.get(b"Type").ok(), b"OCMD") || !is_oc_target(doc, (num, gen)) {
        return None;
    }
    d.get(b"Name")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(text_of)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
pub(crate) mod tests;
