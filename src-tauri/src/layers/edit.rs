//! Edits to the layer structure of a file (run through `apply_page_edit`, so they are
//! atomic and re-read by the renderer): adding a layer, and saving the current view state
//! as the file's default configuration. Nothing else writes `/OCProperties`.

use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};

use super::{clean_text, read_model, MAX_ORDER_DEPTH};

/// Where a value lives: inline in its parent dictionary, or as an indirect object.
#[derive(Clone, Copy)]
struct Slot(Option<ObjectId>);

fn load(doc: &Document, parent: &Dictionary, key: &[u8]) -> (Option<Object>, Slot) {
    match parent.get(key) {
        Ok(Object::Reference(r)) => (doc.get_object(*r).ok().cloned(), Slot(Some(*r))),
        Ok(o) => (Some(o.clone()), Slot(None)),
        Err(_) => (None, Slot(None)),
    }
}

fn store(doc: &mut Document, parent: &mut Dictionary, key: &str, slot: Slot, value: Object) {
    match slot.0 {
        Some(id) => {
            doc.set_object(id, value);
        }
        None => parent.set(key, value),
    }
}

fn is_ref_to(o: &Object, num: u32) -> bool {
    matches!(o, Object::Reference((n, _)) if *n == num)
}

/// Put `child` under `parent` in an `/Order` array: in the unlabelled array right after the
/// parent's entry (made if absent). False when `parent` is not found in direct arrays.
fn insert_child(order: &mut Vec<Object>, parent: u32, child: ObjectId, depth: usize) -> bool {
    if depth > MAX_ORDER_DEPTH {
        return false;
    }
    let mut i = 0;
    while i < order.len() {
        if is_ref_to(&order[i], parent) {
            let has_sub = matches!(order.get(i + 1), Some(Object::Array(a)) if !matches!(a.first(), Some(Object::String(..))));
            if has_sub {
                if let Some(Object::Array(a)) = order.get_mut(i + 1) {
                    a.push(Object::Reference(child));
                }
            } else {
                order.insert(i + 1, Object::Array(vec![Object::Reference(child)]));
            }
            return true;
        }
        if let Object::Array(inner) = &mut order[i] {
            if insert_child(inner, parent, child, depth + 1) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Add a blank layer named `name` (optionally as a sub-layer of layer `parent`). It is ON in
/// the default configuration. Returns the new group's object number.
pub fn add_layer(doc: &mut Document, name: &str, parent: Option<u32>) -> Result<u32> {
    let name = clean_text(name);
    if name.is_empty() {
        bail!("a layer needs a name");
    }
    let root = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .context("document has no catalog")?;
    let mut catalog = doc
        .get_dictionary(root)
        .context("catalog is not a dictionary")?
        .clone();

    let (ocp_obj, ocp_slot) = load(doc, &catalog, b"OCProperties");
    let mut ocp = match ocp_obj {
        Some(Object::Dictionary(d)) => d,
        Some(_) => bail!("the file's optional content properties are unreadable"),
        None => Dictionary::new(),
    };
    let (ocgs_obj, ocgs_slot) = load(doc, &ocp, b"OCGs");
    let mut ocgs = match ocgs_obj {
        Some(Object::Array(a)) => a,
        _ => Vec::new(),
    };
    let (d_obj, d_slot) = load(doc, &ocp, b"D");
    let mut d = match d_obj {
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    let (order_obj, order_slot) = load(doc, &d, b"Order");
    let mut order = match order_obj {
        Some(Object::Array(a)) => a,
        _ => Vec::new(),
    };

    let mut g = Dictionary::new();
    g.set("Type", Object::Name(b"OCG".to_vec()));
    g.set("Name", lopdf::text_string(&name));
    let id = doc.add_object(Object::Dictionary(g));
    let existing: Vec<Object> = ocgs
        .iter()
        .filter(|o| matches!(o, Object::Reference(_)))
        .cloned()
        .collect();
    ocgs.push(Object::Reference(id));

    // An absent or empty /Order presents nothing in a strict reader: list the existing
    // groups first so adding one does not hide the rest.
    if order.is_empty() {
        order = existing;
    }
    let placed = parent.is_some_and(|p| insert_child(&mut order, p, id, 0));
    if !placed {
        order.push(Object::Reference(id));
    }

    // With BaseState OFF a group is on only when listed in /ON.
    if matches!(d.get(b"BaseState"), Ok(Object::Name(n)) if n == b"OFF") {
        let (on_obj, on_slot) = load(doc, &d, b"ON");
        let mut on = match on_obj {
            Some(Object::Array(a)) => a,
            _ => Vec::new(),
        };
        on.push(Object::Reference(id));
        store(doc, &mut d, "ON", on_slot, Object::Array(on));
    }

    store(doc, &mut d, "Order", order_slot, Object::Array(order));
    store(doc, &mut ocp, "OCGs", ocgs_slot, Object::Array(ocgs));
    store(doc, &mut ocp, "D", d_slot, Object::Dictionary(d));
    store(
        doc,
        &mut catalog,
        "OCProperties",
        ocp_slot,
        Object::Dictionary(ocp),
    );
    doc.set_object(root, Object::Dictionary(catalog));
    Ok(id.0)
}

/// Set a group's own `/Usage /View /ViewState`, following indirect `/Usage` and `/View`.
fn set_view_state(doc: &mut Document, ocg: ObjectId, on: bool) {
    fn dict_id(doc: &Document, holder: ObjectId, key: &[u8]) -> Option<ObjectId> {
        match doc.get_dictionary(holder).ok()?.get(key).ok()? {
            Object::Reference(r) => Some(*r),
            _ => None,
        }
    }
    let state = Object::Name(if on { b"ON".to_vec() } else { b"OFF".to_vec() });
    let usage_id = dict_id(doc, ocg, b"Usage");
    let usage_holder = usage_id.unwrap_or(ocg);
    let view_id = if usage_id.is_some() {
        dict_id(doc, usage_holder, b"View")
    } else {
        // /Usage is direct inside the group.
        match doc
            .get_dictionary(ocg)
            .ok()
            .and_then(|g| g.get(b"Usage").ok())
        {
            Some(Object::Dictionary(u)) => match u.get(b"View") {
                Ok(Object::Reference(r)) => Some(*r),
                _ => None,
            },
            _ => None,
        }
    };
    if let Some(v) = view_id {
        if let Ok(view) = doc.get_dictionary_mut(v) {
            view.set("ViewState", state);
        }
        return;
    }
    let Ok(holder) = doc.get_dictionary_mut(usage_holder) else {
        return;
    };
    let view = if usage_id.is_some() {
        holder
            .get_mut(b"View")
            .ok()
            .and_then(|o| o.as_dict_mut().ok())
    } else {
        holder
            .get_mut(b"Usage")
            .ok()
            .and_then(|o| o.as_dict_mut().ok())
            .and_then(|u| u.get_mut(b"View").ok())
            .and_then(|o| o.as_dict_mut().ok())
    };
    if let Some(view) = view {
        view.set("ViewState", state);
    }
}

/// Save a view state as the file's default configuration: `off` are the groups hidden.
/// Locked groups and groups whose intent excludes View keep their current state.
pub fn set_default_state(doc: &mut Document, off: &BTreeSet<u32>) -> Result<()> {
    let model = read_model(doc);
    if !model.has_ocproperties {
        bail!("the file has no layers");
    }
    if model.layers_truncated {
        // The configuration is rebuilt from the groups read: the rest would be switched on.
        bail!("this file has more layers than redline reads, so its default cannot be saved here");
    }
    let mut hidden: Vec<ObjectId> = Vec::new();
    let mut fix: Vec<(ObjectId, bool)> = Vec::new();
    for l in &model.layers {
        let want_on = if l.locked || !l.view_intent {
            l.default_on
        } else {
            !off.contains(&l.id)
        };
        if !want_on {
            hidden.push((l.id, l.gen));
        }
        if l.usage_view_state.is_some_and(|v| v != want_on) {
            fix.push(((l.id, l.gen), want_on));
        }
    }

    let root = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .context("document has no catalog")?;
    let mut catalog = doc
        .get_dictionary(root)
        .context("catalog is not a dictionary")?
        .clone();
    let (ocp_obj, ocp_slot) = load(doc, &catalog, b"OCProperties");
    let Some(Object::Dictionary(mut ocp)) = ocp_obj else {
        bail!("the file's optional content properties are unreadable");
    };
    let (d_obj, d_slot) = load(doc, &ocp, b"D");
    let mut d = match d_obj {
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    d.remove(b"BaseState");
    d.remove(b"ON");
    if hidden.is_empty() {
        d.remove(b"OFF");
    } else {
        d.set(
            "OFF",
            Object::Array(hidden.into_iter().map(Object::Reference).collect()),
        );
    }
    store(doc, &mut ocp, "D", d_slot, Object::Dictionary(d));
    store(
        doc,
        &mut catalog,
        "OCProperties",
        ocp_slot,
        Object::Dictionary(ocp),
    );
    doc.set_object(root, Object::Dictionary(catalog));
    for (id, on) in fix {
        set_view_state(doc, id, on);
    }
    Ok(())
}
