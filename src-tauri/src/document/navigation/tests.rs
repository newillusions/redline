//! Tests for bookmarks, page labels and links: structure, hostile input, edit / undo, and the
//! file round trips (including the mark-up save path leaving all of it alone).

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use super::edit::{
    apply_edit, apply_undo, carry_labels, map_delete, map_insert, map_reorder, read_info, NavEdit,
    NavHistory, NavUndo,
};
use super::labels::{self, LabelRange, LabelStyle};
use super::links::{self, parse_link_id};
use super::outline::{self, Place, RawNode};
use super::{PageIndex, PageView, SpaceCache, Target, TargetInput};
use crate::document::page_ops::tests::n_page_doc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn reload(doc: &mut Document) -> Document {
    let mut buf = Vec::new();
    doc.save_to(&mut buf).expect("save to memory");
    Document::load_mem(&buf).expect("reload")
}

fn dest_to(page: ObjectId) -> (&'static str, Object) {
    (
        "Dest",
        Object::Array(vec![Object::Reference(page), Object::Name(b"Fit".to_vec())]),
    )
}

fn node(title: &str, page: Option<ObjectId>, open: bool, kids: Vec<RawNode>) -> RawNode {
    let mut n = outline::new_node(title, page.map(dest_to)).unwrap();
    n.open = open;
    n.children = kids;
    n
}

fn put(doc: &mut Document, forest: &[RawNode]) {
    let read = outline::read_outline(doc);
    outline::write_outline(doc, forest, &read.item_ids, read.root_id).unwrap();
}

fn titles(forest: &[RawNode]) -> Vec<String> {
    forest.iter().map(RawNode::title).collect()
}

fn sample_forest(p: &[ObjectId]) -> Vec<RawNode> {
    vec![
        node(
            "A",
            Some(p[0]),
            true,
            vec![
                node("B", Some(p[1]), false, vec![]),
                node(
                    "C",
                    Some(p[2]),
                    false,
                    vec![node("D", Some(p[3]), false, vec![])],
                ),
            ],
        ),
        node("E", Some(p[4]), false, vec![]),
    ]
}

fn item_ids(doc: &Document) -> Vec<ObjectId> {
    outline::read_outline(doc).item_ids
}

fn dict(doc: &Document, id: ObjectId) -> Dictionary {
    doc.get_dictionary(id).unwrap().clone()
}

fn json(doc: &Document) -> serde_json::Value {
    serde_json::to_value(read_info(doc)).unwrap()
}

// ---------------------------------------------------------------------------
// Outline: structure
// ---------------------------------------------------------------------------

#[test]
fn outline_writes_spec_counts_and_links() {
    let (mut doc, pages) = n_page_doc(5);
    put(&mut doc, &sample_forest(&pages));
    let ids = item_ids(&doc);
    assert_eq!(ids.len(), 5);
    let (a, b, c, d, e) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let rd = dict(&doc, root);
    // A open with B and C visible (D hidden under closed C); E: 4 visible items in all.
    assert_eq!(rd.get(b"Count").unwrap().as_i64().unwrap(), 4);
    assert_eq!(rd.get(b"First").unwrap().as_reference().unwrap(), a);
    assert_eq!(rd.get(b"Last").unwrap().as_reference().unwrap(), e);
    assert_eq!(dict(&doc, a).get(b"Count").unwrap().as_i64().unwrap(), 2);
    assert_eq!(dict(&doc, c).get(b"Count").unwrap().as_i64().unwrap(), -1);
    assert!(dict(&doc, b).get(b"Count").is_err(), "a leaf has no /Count");
    assert_eq!(
        dict(&doc, a).get(b"Next").unwrap().as_reference().unwrap(),
        e
    );
    assert_eq!(
        dict(&doc, e).get(b"Prev").unwrap().as_reference().unwrap(),
        a
    );
    assert_eq!(
        dict(&doc, b).get(b"Next").unwrap().as_reference().unwrap(),
        c
    );
    assert_eq!(
        dict(&doc, d)
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap(),
        c
    );
    assert_eq!(
        dict(&doc, a)
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap(),
        root
    );
}

#[test]
fn outline_round_trips_through_a_file_three_cycles() {
    let (mut doc, pages) = n_page_doc(5);
    put(&mut doc, &sample_forest(&pages));
    let mut cur = reload(&mut doc);
    let first = outline::read_outline(&cur);
    assert!(first.damaged.is_none());
    assert_eq!(titles(&first.nodes), vec!["A", "E"]);
    for cycle in 0..3 {
        let r = outline::read_outline(&cur);
        assert_eq!(
            r.nodes, first.nodes,
            "cycle {cycle}: structure and open state"
        );
        put(&mut cur, &r.nodes);
        cur = reload(&mut cur);
    }
    assert_eq!(outline::read_outline(&cur).nodes, first.nodes);
    // The rewrite left no orphan item objects behind.
    assert_eq!(item_ids(&cur).len(), 5);
}

#[test]
fn removing_every_bookmark_removes_the_outline() {
    let (mut doc, pages) = n_page_doc(5);
    put(&mut doc, &sample_forest(&pages));
    let objects_with = doc.objects.len();
    put(&mut doc, &[]);
    assert!(doc.catalog().unwrap().get(b"Outlines").is_err());
    assert!(outline::read_outline(&doc).nodes.is_empty());
    assert!(
        doc.objects.len() < objects_with,
        "item and root objects are gone"
    );
}

#[test]
fn a_rewrite_keeps_unknown_keys_and_unsupported_actions() {
    let (mut doc, pages) = n_page_doc(2);
    let mut n = node("Remote", None, false, vec![]);
    n.dict.set(
        "A",
        dictionary! { "S" => "GoToR", "F" => Object::string_literal("other.pdf"), "D" => vec![0.into(), Object::Name(b"Fit".to_vec())] },
    );
    n.dict.set(
        "C",
        vec![Object::Real(1.0), Object::Real(0.0), Object::Real(0.0)],
    );
    n.dict.set("BBPrivate", Object::string_literal("keep me"));
    put(
        &mut doc,
        &[n.clone(), node("Plain", Some(pages[1]), false, vec![])],
    );

    let mut f = outline::read_outline(&doc).nodes;
    outline::set_title(&mut f[1], "Renamed").unwrap();
    put(&mut doc, &f);
    let again = outline::read_outline(&doc).nodes;
    assert_eq!(
        again[0].dict, n.dict,
        "the foreign node is carried verbatim"
    );
    assert_eq!(titles(&again), vec!["Remote", "Renamed"]);

    // And it is reported as unsupported, never as something runnable.
    let info = read_info(&doc);
    assert!(matches!(
        &info.outline.nodes[0].target,
        Target::Unsupported { action } if action == "GoToR"
    ));
}

// ---------------------------------------------------------------------------
// Outline: forest edits
// ---------------------------------------------------------------------------

fn simple(titles: &[&str]) -> Vec<RawNode> {
    titles
        .iter()
        .map(|t| node(t, None, false, vec![]))
        .collect()
}

#[test]
fn add_before_after_child_and_with_no_selection() {
    let mut f = simple(&["A", "B"]);
    // No selection: After appends, Before goes to the top, Child is refused.
    outline::insert_at(&mut f, None, Place::After, node("end", None, false, vec![])).unwrap();
    outline::insert_at(
        &mut f,
        None,
        Place::Before,
        node("top", None, false, vec![]),
    )
    .unwrap();
    assert_eq!(titles(&f), vec!["top", "A", "B", "end"]);
    assert!(
        outline::insert_at(&mut f, None, Place::Child, node("x", None, false, vec![])).is_err()
    );
    // Relative to a selection.
    let p = outline::insert_at(
        &mut f,
        Some(&[1]),
        Place::Before,
        node("before A", None, false, vec![]),
    )
    .unwrap();
    assert_eq!(p, vec![1]);
    let p = outline::insert_at(
        &mut f,
        Some(&[2]),
        Place::After,
        node("after A", None, false, vec![]),
    )
    .unwrap();
    assert_eq!(p, vec![3]);
    let p = outline::insert_at(
        &mut f,
        Some(&[2]),
        Place::Child,
        node("kid", None, false, vec![]),
    )
    .unwrap();
    assert_eq!(p, vec![2, 0]);
    assert!(
        f[2].open,
        "adding a child opens its parent so it is visible"
    );
    assert_eq!(
        titles(&f),
        vec!["top", "before A", "A", "after A", "B", "end"]
    );
}

#[test]
fn move_up_down_indent_outdent() {
    let mut f = simple(&["A", "B", "C", "D"]);
    // Move C above B.
    outline::move_node(&mut f, &[2], &[1], Place::Before).unwrap();
    assert_eq!(titles(&f), vec!["A", "C", "B", "D"]);
    // Move A below D.
    outline::move_node(&mut f, &[0], &[3], Place::After).unwrap();
    assert_eq!(titles(&f), vec!["C", "B", "D", "A"]);
    // Indent B under C (becomes the last child of the previous sibling).
    let p = outline::move_node(&mut f, &[1], &[0], Place::Child).unwrap();
    assert_eq!(p, vec![0, 0]);
    assert_eq!(titles(&f), vec!["C", "D", "A"]);
    assert_eq!(titles(&f[0].children), vec!["B"]);
    // Outdent B: becomes a sibling after its parent.
    let p = outline::move_node(&mut f, &[0, 0], &[0], Place::After).unwrap();
    assert_eq!(p, vec![1]);
    assert_eq!(titles(&f), vec!["C", "B", "D", "A"]);
}

#[test]
fn a_move_into_its_own_subtree_is_refused_and_changes_nothing() {
    let mut f = vec![node(
        "A",
        None,
        true,
        vec![node("B", None, false, vec![node("C", None, false, vec![])])],
    )];
    let before = f.clone();
    assert!(outline::move_node(&mut f, &[0], &[0, 0, 0], Place::Child).is_err());
    assert!(outline::move_node(&mut f, &[0], &[0], Place::After).is_err());
    assert!(outline::move_node(&mut f, &[0, 0], &[0, 0, 0], Place::Before).is_err());
    assert_eq!(f, before);
}

#[test]
fn empty_paths_are_refused_not_panicked_on() {
    let mut f = simple(&["A", "B"]);
    let n = || node("x", None, false, vec![]);
    assert!(outline::insert_at(&mut f, Some(&[]), Place::After, n()).is_err());
    assert!(outline::insert_at(&mut f, Some(&[]), Place::Child, n()).is_err());
    assert!(outline::move_node(&mut f, &[], &[0], Place::After).is_err());
    assert!(outline::move_node(&mut f, &[0], &[], Place::After).is_err());
    assert!(outline::remove_at(&mut f, &[]).is_err());
    assert!(outline::parse_path("").is_err());
    assert!(outline::parse_path("1..2").is_err());
    assert!(outline::parse_path("-1").is_err());
    assert_eq!(titles(&f), vec!["A", "B"]);
}

#[test]
fn delete_removes_the_whole_subtree() {
    let mut f = vec![
        node(
            "A",
            None,
            true,
            vec![
                node("B", None, false, vec![]),
                node("C", None, false, vec![]),
            ],
        ),
        node("D", None, false, vec![]),
    ];
    let removed = outline::remove_at(&mut f, &[0]).unwrap();
    assert_eq!(removed.children.len(), 2);
    assert_eq!(titles(&f), vec!["D"]);
    assert!(outline::remove_at(&mut f, &[5]).is_err());
}

#[test]
fn depth_and_size_caps_apply_to_edits() {
    // 32 levels is allowed, a 33rd is refused.
    let mut f = vec![node("L0", None, true, vec![])];
    let mut path = vec![0usize];
    for i in 1..outline::MAX_OUTLINE_DEPTH {
        path = outline::insert_at(
            &mut f,
            Some(&path),
            Place::Child,
            node(&format!("L{i}"), None, true, vec![]),
        )
        .unwrap();
    }
    assert_eq!(path.len(), outline::MAX_OUTLINE_DEPTH);
    assert!(outline::insert_at(
        &mut f,
        Some(&path),
        Place::Child,
        node("too deep", None, false, vec![])
    )
    .is_err());
    let (mut doc, _) = n_page_doc(1);
    outline::write_outline(&mut doc, &f, &[], None).unwrap();
    assert!(
        outline::read_outline(&doc).damaged.is_none(),
        "32 levels reads back whole"
    );
}

#[test]
fn collapse_state_is_saved_per_node_with_children() {
    let mut f = vec![node(
        "A",
        None,
        true,
        vec![node("B", None, true, vec![node("C", None, false, vec![])])],
    )];
    let open: std::collections::HashSet<String> = ["0".to_string()].into_iter().collect();
    outline::set_open_states(&mut f, &open);
    assert!(f[0].open);
    assert!(!f[0].children[0].open);
    let (mut doc, _) = n_page_doc(1);
    outline::write_outline(&mut doc, &f, &[], None).unwrap();
    let ids = item_ids(&doc);
    assert_eq!(
        dict(&doc, ids[0]).get(b"Count").unwrap().as_i64().unwrap(),
        1
    );
    assert_eq!(
        dict(&doc, ids[1]).get(b"Count").unwrap().as_i64().unwrap(),
        -1
    );
}

// ---------------------------------------------------------------------------
// Outline: hostile input
// ---------------------------------------------------------------------------

fn three_items(doc: &mut Document) -> Vec<ObjectId> {
    put(doc, &simple(&["A", "B", "C"]));
    item_ids(doc)
}

fn assert_damaged_and_read_only(doc: &mut Document) {
    let r = outline::read_outline(doc);
    assert!(
        r.damaged.is_some(),
        "expected the outline to be flagged damaged"
    );
    let edit = NavEdit::BookmarkRename {
        id: "0".into(),
        expect: "A".into(),
        title: "x".into(),
    };
    let err = apply_edit(doc, &edit).unwrap_err().to_string();
    assert!(err.contains("read-only"), "{err}");
    // Viewing still works and reports why.
    let info = read_info(doc);
    assert!(info.outline.read_only.is_some());
}

#[test]
fn a_next_loop_is_damaged_not_a_hang() {
    let (mut doc, _) = n_page_doc(1);
    let ids = three_items(&mut doc);
    doc.get_dictionary_mut(ids[2])
        .unwrap()
        .set("Next", Object::Reference(ids[0]));
    assert_damaged_and_read_only(&mut doc);
    assert_eq!(
        outline::read_outline(&doc).nodes.len(),
        3,
        "what was readable is still shown"
    );
}

#[test]
fn a_first_loop_through_an_ancestor_is_damaged() {
    let (mut doc, _) = n_page_doc(1);
    put(
        &mut doc,
        &[node("A", None, true, vec![node("B", None, false, vec![])])],
    );
    let ids = item_ids(&doc);
    doc.get_dictionary_mut(ids[1])
        .unwrap()
        .set("First", Object::Reference(ids[0]));
    assert_damaged_and_read_only(&mut doc);
}

#[test]
fn an_entry_shared_by_two_parents_is_damaged() {
    let (mut doc, _) = n_page_doc(1);
    put(
        &mut doc,
        &[
            node("A", None, true, vec![node("X", None, false, vec![])]),
            node("B", None, false, vec![]),
        ],
    );
    let ids = item_ids(&doc);
    let x = ids[1];
    let b = ids[2];
    doc.get_dictionary_mut(b)
        .unwrap()
        .set("First", Object::Reference(x));
    assert_damaged_and_read_only(&mut doc);
}

#[test]
fn structural_garbage_is_damaged_without_panicking() {
    // /First as an inline dictionary.
    let (mut doc, _) = n_page_doc(1);
    three_items(&mut doc);
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.get_dictionary_mut(root)
        .unwrap()
        .set("First", Object::Dictionary(dictionary! {}));
    assert!(outline::read_outline(&doc).damaged.is_some());

    // /First pointing at nothing.
    let (mut doc, _) = n_page_doc(1);
    three_items(&mut doc);
    let root = doc
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.get_dictionary_mut(root)
        .unwrap()
        .set("First", Object::Reference((9999, 0)));
    assert!(outline::read_outline(&doc).damaged.is_some());

    // /Next pointing at a non-dictionary.
    let (mut doc, _) = n_page_doc(1);
    let ids = three_items(&mut doc);
    let junk = doc.add_object(Object::Integer(7));
    doc.get_dictionary_mut(ids[0])
        .unwrap()
        .set("Next", Object::Reference(junk));
    assert!(outline::read_outline(&doc).damaged.is_some());

    // /Outlines that is not a dictionary: nothing to show, nothing to break.
    let (mut doc, _) = n_page_doc(1);
    doc.catalog_mut()
        .unwrap()
        .set("Outlines", Object::Integer(3));
    let r = outline::read_outline(&doc);
    assert!(r.nodes.is_empty());
}

#[test]
fn depth_and_node_caps_flag_the_outline() {
    let (mut doc, _) = n_page_doc(1);
    put(
        &mut doc,
        &[node(
            "1",
            None,
            true,
            vec![node(
                "2",
                None,
                true,
                vec![node("3", None, true, vec![node("4", None, false, vec![])])],
            )],
        )],
    );
    assert!(outline::read_outline_with_limits(&doc, 100, 4)
        .damaged
        .is_none());
    let shallow = outline::read_outline_with_limits(&doc, 100, 3);
    assert!(shallow.damaged.unwrap().contains("deeper"));
    let small = outline::read_outline_with_limits(&doc, 3, 32);
    assert!(small.damaged.unwrap().contains("more than"));
}

#[test]
fn a_long_flat_outline_reads_whole() {
    let (mut doc, _) = n_page_doc(1);
    let flat: Vec<RawNode> = (0..2_000)
        .map(|i| node(&format!("S{i}"), None, false, vec![]))
        .collect();
    put(&mut doc, &flat);
    let r = outline::read_outline(&doc);
    assert!(r.damaged.is_none());
    assert_eq!(r.nodes.len(), 2_000);
}

// ---------------------------------------------------------------------------
// Destinations and actions
// ---------------------------------------------------------------------------

fn target_of_dict(doc: &Document, d: &Dictionary) -> Target {
    let pages = PageIndex::build(doc);
    let mut spaces = SpaceCache::new(doc, &pages);
    super::target_of(doc, d, &pages, &mut spaces)
}

#[test]
fn explicit_destinations_resolve_to_a_page_and_view() {
    let (doc, p) = n_page_doc(3);
    let xyz = dictionary! { "Dest" => vec![p[2].into(), Object::Name(b"XYZ".to_vec()), 10.into(), 700.into(), Object::Real(1.5)] };
    assert_eq!(
        target_of_dict(&doc, &xyz),
        Target::Page {
            page: 2,
            view: PageView::Xyz {
                left: Some(10.0),
                top: Some(700.0),
                zoom: Some(1.5)
            }
        }
    );
    let nulls = dictionary! { "Dest" => vec![p[1].into(), Object::Name(b"XYZ".to_vec()), Object::Null, Object::Null, 0.into()] };
    assert_eq!(
        target_of_dict(&doc, &nulls),
        Target::Page {
            page: 1,
            view: PageView::inherit()
        },
        "null coordinates and a zero zoom mean unchanged"
    );
    let fit = dictionary! { "Dest" => vec![p[0].into(), Object::Name(b"Fit".to_vec())] };
    assert_eq!(
        target_of_dict(&doc, &fit),
        Target::Page {
            page: 0,
            view: PageView::Fit
        }
    );
    let fith =
        dictionary! { "Dest" => vec![p[0].into(), Object::Name(b"FitH".to_vec()), 500.into()] };
    assert_eq!(
        target_of_dict(&doc, &fith),
        Target::Page {
            page: 0,
            view: PageView::FitH { top: Some(500.0) }
        }
    );
    let fitr = dictionary! { "Dest" => vec![p[0].into(), Object::Name(b"FitR".to_vec()), 50.into(), 40.into(), 200.into(), 300.into()] };
    assert_eq!(
        target_of_dict(&doc, &fitr),
        Target::Page {
            page: 0,
            view: PageView::FitR {
                left: 50.0,
                bottom: 40.0,
                right: 200.0,
                top: 300.0
            }
        }
    );
}

#[test]
fn a_goto_action_is_the_same_as_a_dest() {
    let (doc, p) = n_page_doc(2);
    let d = dictionary! { "A" => dictionary! { "S" => "GoTo", "D" => vec![p[1].into(), Object::Name(b"Fit".to_vec())] } };
    assert_eq!(
        target_of_dict(&doc, &d),
        Target::Page {
            page: 1,
            view: PageView::Fit
        }
    );
}

#[test]
fn destinations_to_nowhere_are_broken() {
    let (doc, p) = n_page_doc(2);
    // A page object that is not in the page tree.
    let stranger =
        dictionary! { "Dest" => vec![Object::Reference((777, 0)), Object::Name(b"Fit".to_vec())] };
    assert_eq!(target_of_dict(&doc, &stranger), Target::Broken);
    // The catalog is not a page.
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let not_a_page = dictionary! { "Dest" => vec![root.into(), Object::Name(b"Fit".to_vec())] };
    assert_eq!(target_of_dict(&doc, &not_a_page), Target::Broken);
    // Unknown view type, empty array, remote (integer) page, unresolved name.
    assert_eq!(
        target_of_dict(
            &doc,
            &dictionary! { "Dest" => vec![p[0].into(), Object::Name(b"Spin".to_vec())] }
        ),
        Target::Broken
    );
    assert_eq!(
        target_of_dict(&doc, &dictionary! { "Dest" => Vec::<Object>::new() }),
        Target::Broken
    );
    assert_eq!(
        target_of_dict(
            &doc,
            &dictionary! { "Dest" => vec![3.into(), Object::Name(b"Fit".to_vec())] }
        ),
        Target::Broken
    );
    assert_eq!(
        target_of_dict(
            &doc,
            &dictionary! { "Dest" => Object::string_literal("nope") }
        ),
        Target::Broken
    );
    // No destination and no action is simply "none".
    assert_eq!(target_of_dict(&doc, &dictionary! {}), Target::None);
}

#[test]
fn named_destinations_resolve_through_dests_and_the_name_tree() {
    let (mut doc, p) = n_page_doc(4);
    let dests = doc.add_object(dictionary! {
        "Legacy" => vec![p[1].into(), Object::Name(b"Fit".to_vec())],
    });
    // Name tree: root with /Kids; leaves hold sorted /Names; /Limits guide the search.
    let leaf_a = doc.add_object(dictionary! {
        "Limits" => vec![Object::string_literal("a1"), Object::string_literal("m1")],
        "Names" => vec![
            Object::string_literal("a1"), vec![p[0].into(), Object::Name(b"Fit".to_vec())].into(),
            Object::string_literal("m1"), dictionary! { "D" => vec![p[2].into(), Object::Name(b"Fit".to_vec())] }.into(),
        ],
    });
    let leaf_b = doc.add_object(dictionary! {
        "Limits" => vec![Object::string_literal("n1"), Object::string_literal("z9")],
        "Names" => vec![Object::string_literal("z9"), vec![p[3].into(), Object::Name(b"Fit".to_vec())].into()],
    });
    let tree = doc.add_object(dictionary! { "Kids" => vec![leaf_a.into(), leaf_b.into()] });
    let names = doc.add_object(dictionary! { "Dests" => tree });
    {
        let cat = doc.catalog_mut().unwrap();
        cat.set("Dests", dests);
        cat.set("Names", names);
    }
    let by_name = |n: &str, as_name: bool| {
        let v = if as_name {
            Object::Name(n.as_bytes().to_vec())
        } else {
            Object::string_literal(n)
        };
        target_of_dict(&doc, &dictionary! { "Dest" => v })
    };
    assert_eq!(
        by_name("Legacy", true),
        Target::Page {
            page: 1,
            view: PageView::Fit
        }
    );
    assert_eq!(
        by_name("a1", false),
        Target::Page {
            page: 0,
            view: PageView::Fit
        }
    );
    assert_eq!(
        by_name("m1", false),
        Target::Page {
            page: 2,
            view: PageView::Fit
        },
        "a dict value with /D"
    );
    assert_eq!(
        by_name("z9", false),
        Target::Page {
            page: 3,
            view: PageView::Fit
        }
    );
    assert_eq!(by_name("missing", false), Target::Broken);
}

#[test]
fn a_cyclic_name_tree_terminates() {
    let (mut doc, _) = n_page_doc(1);
    let a = doc.new_object_id();
    let b = doc.new_object_id();
    doc.objects.insert(
        a,
        Object::Dictionary(dictionary! { "Kids" => vec![b.into()] }),
    );
    doc.objects.insert(
        b,
        Object::Dictionary(dictionary! { "Kids" => vec![a.into()] }),
    );
    let names = doc.add_object(dictionary! { "Dests" => a });
    doc.catalog_mut().unwrap().set("Names", names);
    assert_eq!(
        target_of_dict(&doc, &dictionary! { "Dest" => Object::string_literal("x") }),
        Target::Broken
    );
}

#[test]
fn uri_actions_are_allowlisted_and_other_actions_are_unsupported() {
    let (doc, _) = n_page_doc(1);
    let uri = |u: &str| {
        target_of_dict(
            &doc,
            &dictionary! { "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal(u) } },
        )
    };
    assert_eq!(
        uri("https://example.com/x"),
        Target::Uri {
            uri: "https://example.com/x".into()
        }
    );
    assert!(matches!(uri("javascript:alert(1)"), Target::Blocked { .. }));
    assert!(matches!(uri("file:///etc/passwd"), Target::Blocked { .. }));
    assert!(matches!(
        uri("https://exa mple.com"),
        Target::Blocked { .. }
    ));
    for action in [
        "Launch",
        "JavaScript",
        "GoToR",
        "Named",
        "SubmitForm",
        "ImportData",
        "SetOCGState",
    ] {
        let d = dictionary! { "A" => dictionary! { "S" => action } };
        assert!(
            matches!(target_of_dict(&doc, &d), Target::Unsupported { action: ref a } if a == action),
            "{action} must be listed as unsupported"
        );
    }
    // A malformed action.
    assert!(matches!(
        target_of_dict(&doc, &dictionary! { "A" => dictionary! {} }),
        Target::Unsupported { .. }
    ));
    assert!(matches!(
        target_of_dict(&doc, &dictionary! { "A" => Object::Integer(1) }),
        Target::Unsupported { .. }
    ));
}

#[test]
fn a_destination_page_that_was_removed_is_counted_broken() {
    let (mut doc, p) = n_page_doc(3);
    put(
        &mut doc,
        &[
            node("ok", Some(p[0]), false, vec![]),
            node("gone", Some(p[2]), false, vec![]),
        ],
    );
    crate::document::page_ops::delete_page(&mut doc, 2).unwrap();
    let info = read_info(&doc);
    assert_eq!(info.outline.broken, 1);
    assert!(matches!(info.outline.nodes[1].target, Target::Broken));
}

#[test]
fn rotated_and_offset_pages_convert_destinations_both_ways() {
    let (mut doc, p) = n_page_doc(1);
    {
        let page = doc.get_dictionary_mut(p[0]).unwrap();
        page.set("Rotate", 90);
        page.set(
            "MediaBox",
            vec![10.into(), 20.into(), 622.into(), 812.into()],
        );
    }
    let input = TargetInput::Page {
        page: 0,
        view: PageView::Xyz {
            left: Some(100.0),
            top: Some(200.0),
            zoom: Some(2.0),
        },
    };
    let pages = PageIndex::build(&doc);
    let mut spaces = SpaceCache::new(&doc, &pages);
    let (key, arr) = super::target_entries(&doc, &input, &pages, &mut spaces).unwrap();
    assert_eq!(key, "Dest");
    let d = dictionary! { "Dest" => arr };
    // Read back through the same frame: the display coordinates survive the trip.
    match target_of_dict(&doc, &d) {
        Target::Page {
            page: 0,
            view:
                PageView::Xyz {
                    left: Some(l),
                    top: Some(t),
                    zoom: Some(z),
                },
        } => {
            assert!(
                (l - 100.0).abs() < 0.01 && (t - 200.0).abs() < 0.01 && (z - 2.0).abs() < 0.001,
                "{l} {t} {z}"
            );
        }
        other => panic!("{other:?}"),
    }
    // The file holds the true-space point, which differs from the display point.
    let arr = d.get(b"Dest").unwrap().as_array().unwrap();
    let raw_left = arr[2].as_float().unwrap();
    assert!(
        (raw_left - 100.0).abs() > 1.0,
        "the file stores user-space coordinates"
    );
}

// ---------------------------------------------------------------------------
// Page labels in a document
// ---------------------------------------------------------------------------

fn label_dict(style: Option<&str>, prefix: Option<&str>, st: Option<i64>) -> Object {
    let mut d = Dictionary::new();
    if let Some(s) = style {
        d.set("S", Object::Name(s.as_bytes().to_vec()));
    }
    if let Some(p) = prefix {
        d.set("P", Object::string_literal(p));
    }
    if let Some(n) = st {
        d.set("St", n);
    }
    Object::Dictionary(d)
}

#[test]
fn labels_are_read_from_a_nested_number_tree() {
    let (mut doc, _) = n_page_doc(9);
    let leaf1 = doc.add_object(dictionary! {
        "Limits" => vec![0.into(), 4.into()],
        "Nums" => vec![0.into(), label_dict(Some("r"), None, None), 4.into(), label_dict(Some("D"), None, None)],
    });
    let leaf2 = doc.add_object(dictionary! {
        "Limits" => vec![7.into(), 7.into()],
        "Nums" => vec![7.into(), label_dict(Some("D"), Some("A-"), Some(8))],
    });
    let root = doc.add_object(dictionary! { "Kids" => vec![leaf1.into(), leaf2.into()] });
    doc.catalog_mut().unwrap().set("PageLabels", root);
    let info = read_info(&doc);
    assert_eq!(
        info.labels,
        vec!["i", "ii", "iii", "iv", "1", "2", "3", "A-8", "A-9"]
    );
}

#[test]
fn hostile_label_trees_are_bounded() {
    // A /Kids cycle.
    let (mut doc, _) = n_page_doc(3);
    let a = doc.new_object_id();
    let b = doc.new_object_id();
    doc.objects.insert(a, Object::Dictionary(dictionary! { "Kids" => vec![b.into()], "Nums" => vec![0.into(), label_dict(Some("D"), None, None)] }));
    doc.objects.insert(
        b,
        Object::Dictionary(dictionary! { "Kids" => vec![a.into()] }),
    );
    doc.catalog_mut().unwrap().set("PageLabels", a);
    assert_eq!(read_info(&doc).labels, vec!["1", "2", "3"]);

    // Nesting far beyond the depth cap.
    let (mut doc, _) = n_page_doc(2);
    let mut inner: Object = Object::Dictionary(
        dictionary! { "Nums" => vec![0.into(), label_dict(Some("D"), None, None)] },
    );
    for _ in 0..200 {
        inner = Object::Dictionary(dictionary! { "Kids" => vec![inner] });
    }
    doc.catalog_mut().unwrap().set("PageLabels", inner);
    assert_eq!(
        read_info(&doc).labels,
        vec!["", ""],
        "too deep to reach: no labels, no panic"
    );

    // Garbage everywhere.
    let (mut doc, _) = n_page_doc(2);
    doc.catalog_mut().unwrap().set(
        "PageLabels",
        dictionary! { "Nums" => vec![Object::string_literal("x"), 1.into(), (-5).into(), label_dict(None, None, None), 0.into(), Object::Integer(3), 1.into()] },
    );
    assert_eq!(read_info(&doc).labels.len(), 2);

    // More ranges than the cap.
    let (mut doc, _) = n_page_doc(2);
    let mut nums = Vec::new();
    for i in 0..(labels::MAX_LABEL_RANGES as i64 + 500) {
        nums.push(Object::Integer(i));
        nums.push(label_dict(None, Some("x"), None));
    }
    doc.catalog_mut()
        .unwrap()
        .set("PageLabels", dictionary! { "Nums" => nums });
    assert_eq!(labels::read_ranges(&doc).len(), labels::MAX_LABEL_RANGES);
}

#[test]
fn labels_round_trip_and_always_have_a_page_zero_entry() {
    let (mut doc, _) = n_page_doc(6);
    let before = labels::read_ranges(&doc);
    assert!(before.is_empty());
    let new = labels::apply_numbering(&before, 6, &[2, 3, 4], Some(LabelStyle::Decimal), "E-", 101)
        .unwrap();
    labels::write_ranges(&mut doc, &new).unwrap();
    let mut cur = reload(&mut doc);
    for cycle in 0..3 {
        let r = labels::read_ranges(&cur);
        assert_eq!(r[0].start, 0, "cycle {cycle}: ISO needs page index 0");
        assert_eq!(
            labels::page_labels(&r, 6),
            vec!["", "", "E-101", "E-102", "E-103", ""]
        );
        labels::write_ranges(&mut cur, &r).unwrap();
        cur = reload(&mut cur);
    }
    // Removing them all removes the entry and the tree.
    labels::write_ranges(&mut cur, &[]).unwrap();
    assert!(cur.catalog().unwrap().get(b"PageLabels").is_err());
}

#[test]
fn non_ascii_label_prefixes_are_stored_as_utf16() {
    let (mut doc, _) = n_page_doc(2);
    let ranges = vec![LabelRange {
        start: 0,
        style: Some(LabelStyle::Decimal),
        prefix: "Seite \u{00e4}-".into(),
        first: 1,
    }];
    labels::write_ranges(&mut doc, &ranges).unwrap();
    let mut cur = reload(&mut doc);
    assert_eq!(
        labels::page_labels(&labels::read_ranges(&cur), 2),
        vec!["Seite \u{00e4}-1", "Seite \u{00e4}-2"]
    );
    let _ = &mut cur;
}

#[test]
fn page_operations_carry_labels_with_their_pages() {
    let (mut doc, _) = n_page_doc(4);
    let r =
        labels::apply_numbering(&[], 4, &[0, 1, 2, 3], Some(LabelStyle::Decimal), "E-", 1).unwrap();
    labels::write_ranges(&mut doc, &r).unwrap();

    carry_labels(&mut doc, &map_delete(4, 1)).unwrap();
    crate::document::page_ops::delete_page(&mut doc, 1).unwrap();
    assert_eq!(read_info(&doc).labels, vec!["E-1", "E-3", "E-4"]);

    carry_labels(&mut doc, &map_reorder(&[2, 0, 1])).unwrap();
    crate::document::page_ops::reorder_pages(&mut doc, vec![2, 0, 1]).unwrap();
    assert_eq!(read_info(&doc).labels, vec!["E-4", "E-1", "E-3"]);

    carry_labels(&mut doc, &map_insert(3, 1)).unwrap();
    crate::document::page_ops::insert_blank_page(&mut doc, 1, 100.0, 100.0).unwrap();
    assert_eq!(read_info(&doc).labels, vec!["E-4", "", "E-1", "E-3"]);
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

fn add_link(doc: &mut Document, page: ObjectId, d: Dictionary) -> ObjectId {
    let id = doc.add_object(Object::Dictionary(d));
    let mut annots = match doc.get_dictionary(page).unwrap().get(b"Annots") {
        Ok(Object::Array(a)) => a.clone(),
        _ => vec![],
    };
    annots.push(Object::Reference(id));
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("Annots", Object::Array(annots));
    id
}

fn link_dict(rect: [i64; 4], extra: (&str, Object)) -> Dictionary {
    let mut d = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>(),
    };
    d.set(extra.0, extra.1);
    d
}

#[test]
fn every_link_is_listed_with_its_target() {
    let (mut doc, p) = n_page_doc(3);
    add_link(
        &mut doc,
        p[0],
        link_dict(
            [10, 10, 110, 40],
            (
                "Dest",
                vec![p[2].into(), Object::Name(b"Fit".to_vec())].into(),
            ),
        ),
    );
    add_link(
        &mut doc,
        p[0],
        link_dict([10, 50, 110, 80], ("A", dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com") }.into())),
    );
    add_link(
        &mut doc,
        p[1],
        link_dict(
            [0, 0, 50, 50],
            (
                "A",
                dictionary! { "S" => "Launch", "F" => Object::string_literal("calc.exe") }.into(),
            ),
        ),
    );
    add_link(
        &mut doc,
        p[1],
        link_dict(
            [0, 60, 50, 90],
            (
                "A",
                dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert(1)") }
                    .into(),
            ),
        ),
    );
    // An inline (non-indirect) link is listed but cannot be edited.
    let mut annots = doc
        .get_dictionary(p[2])
        .unwrap()
        .get(b"Annots")
        .cloned()
        .unwrap_or(Object::Array(vec![]));
    if let Object::Array(a) = &mut annots {
        a.push(Object::Dictionary(link_dict(
            [1, 1, 20, 20],
            (
                "Dest",
                vec![p[0].into(), Object::Name(b"Fit".to_vec())].into(),
            ),
        )));
    }
    doc.get_dictionary_mut(p[2]).unwrap().set("Annots", annots);
    // A non-link annotation is ignored.
    add_link(
        &mut doc,
        p[2],
        dictionary! { "Type" => "Annot", "Subtype" => "Square", "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
    );

    let (links, truncated) = links::read_links(&doc, &PageIndex::build(&doc));
    assert!(!truncated);
    assert_eq!(links.len(), 5);
    assert!(matches!(links[0].target, Target::Page { page: 2, .. }));
    assert!(links[0].id.is_some());
    assert!(matches!(&links[1].target, Target::Uri { uri } if uri == "https://example.com/"));
    assert!(matches!(&links[2].target, Target::Unsupported { action } if action == "Launch"));
    assert!(matches!(&links[3].target, Target::Unsupported { action } if action == "JavaScript"));
    assert!(links[4].id.is_none(), "an inline link has no editable id");
    assert_eq!(links[4].page, 2);
}

#[test]
fn link_rectangles_are_reported_in_display_space() {
    let (mut doc, p) = n_page_doc(1);
    {
        let page = doc.get_dictionary_mut(p[0]).unwrap();
        page.set("Rotate", 90);
    }
    add_link(
        &mut doc,
        p[0],
        link_dict(
            [100, 200, 160, 220],
            (
                "Dest",
                vec![p[0].into(), Object::Name(b"Fit".to_vec())].into(),
            ),
        ),
    );
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    // Rotate 90 on a 612 x 792 page: display (x, y) = (y, w0 - x).
    let r = links[0].rect;
    assert_eq!(r, [200.0, 612.0 - 160.0, 220.0, 612.0 - 100.0]);
}

#[test]
fn a_link_cap_truncates_instead_of_stalling() {
    let (mut doc, p) = n_page_doc(1);
    let mut ids = Vec::new();
    for i in 0..(links::MAX_LINKS_PER_PAGE as i64 + 20) {
        let id = doc.add_object(Object::Dictionary(link_dict(
            [i % 500, 0, i % 500 + 5, 5],
            (
                "Dest",
                vec![p[0].into(), Object::Name(b"Fit".to_vec())].into(),
            ),
        )));
        ids.push(Object::Reference(id));
    }
    doc.get_dictionary_mut(p[0])
        .unwrap()
        .set("Annots", Object::Array(ids));
    let (links, truncated) = links::read_links(&doc, &PageIndex::build(&doc));
    assert!(truncated);
    assert_eq!(links.len(), links::MAX_LINKS_PER_PAGE);
}

#[test]
fn creating_a_link_writes_a_hidden_standard_link_annotation() {
    let (mut doc, p) = n_page_doc(3);
    let id = links::create_link(
        &mut doc,
        0,
        [50.0, 60.0, 150.0, 90.0],
        &TargetInput::Page {
            page: 2,
            view: PageView::Fit,
        },
    )
    .unwrap();
    let d = dict(&doc, id);
    assert_eq!(d.get(b"Subtype").unwrap().as_name().unwrap(), b"Link");
    assert_eq!(d.get(b"Border").unwrap().as_array().unwrap().len(), 3);
    assert!(
        d.get(b"A").is_err(),
        "a page link uses /Dest, not an action"
    );
    let dest = d.get(b"Dest").unwrap().as_array().unwrap();
    assert_eq!(dest[0].as_reference().unwrap(), p[2]);
    // It reads back as a link to page 3 at the same rectangle.
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert_eq!(links[0].rect, [50.0, 60.0, 150.0, 90.0]);
    assert!(matches!(
        links[0].target,
        Target::Page {
            page: 2,
            view: PageView::Fit
        }
    ));
    assert_eq!(links[0].id.as_deref(), Some(links::link_id(id).as_str()));

    let url = links::create_link(
        &mut doc,
        1,
        [0.0, 0.0, 30.0, 30.0],
        &TargetInput::Uri {
            uri: "mailto:me@example.com".into(),
        },
    )
    .unwrap();
    let u = dict(&doc, url);
    assert_eq!(
        u.get(b"A")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"S")
            .unwrap()
            .as_name()
            .unwrap(),
        b"URI"
    );
    assert!(u.get(b"Dest").is_err());
}

#[test]
fn addresses_are_normalised_everywhere_they_are_read_or_written() {
    let (mut doc, p) = n_page_doc(1);
    // Read: a foreign link's address is shown normalised (lower-case host, resolved dots).
    add_link(
        &mut doc,
        p[0],
        link_dict([0, 0, 50, 50], ("A", dictionary! { "S" => "URI", "URI" => Object::string_literal("HTTPS://EXAMPLE.com/a/../b") }.into())),
    );
    // A hostile foreign one (user info) is listed as blocked, not as an openable address.
    add_link(
        &mut doc,
        p[0],
        link_dict([0, 60, 50, 90], ("A", dictionary! { "S" => "URI", "URI" => Object::string_literal("https://trusted.example@evil.example/") }.into())),
    );
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert!(matches!(&links[0].target, Target::Uri { uri } if uri == "https://example.com/b"));
    assert!(matches!(&links[1].target, Target::Blocked { .. }));
    // Write: the normalised form is what lands in the file, for links and bookmarks.
    let id = links::create_link(
        &mut doc,
        0,
        [0.0, 0.0, 30.0, 30.0],
        &TargetInput::Uri {
            uri: "HTTP://Example.COM".into(),
        },
    )
    .unwrap();
    let a = dict(&doc, id).get(b"A").unwrap().as_dict().unwrap().clone();
    assert_eq!(
        a.get(b"URI").unwrap().as_str().unwrap(),
        b"http://example.com/"
    );
    for bad in [
        "https://user:pw@example.com/",
        "//example.com",
        "https:example.com",
        "https://\u{0430}pple.com",
        "mailto:a@b.example?x=%0d%0aBcc:c@d.example",
    ] {
        assert!(
            links::create_link(
                &mut doc,
                0,
                [0.0, 0.0, 30.0, 30.0],
                &TargetInput::Uri { uri: bad.into() }
            )
            .is_err(),
            "{bad}"
        );
        assert!(
            apply_edit(
                &mut doc,
                &NavEdit::BookmarkAdd {
                    anchor: None,
                    anchor_expect: None,
                    place: Place::After,
                    title: "x".into(),
                    target: TargetInput::Uri { uri: bad.into() }
                }
            )
            .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn bad_link_requests_are_refused() {
    let (mut doc, _) = n_page_doc(2);
    let page = TargetInput::Page {
        page: 1,
        view: PageView::Fit,
    };
    // Too small (a click), not finite, reversed corners are fixed, page out of range.
    assert!(links::create_link(&mut doc, 0, [10.0, 10.0, 11.0, 11.0], &page).is_err());
    assert!(links::create_link(&mut doc, 0, [f64::NAN, 0.0, 10.0, 10.0], &page).is_err());
    assert!(links::create_link(&mut doc, 5, [0.0, 0.0, 10.0, 10.0], &page).is_err());
    assert!(links::create_link(
        &mut doc,
        0,
        [0.0, 0.0, 10.0, 10.0],
        &TargetInput::Page {
            page: 9,
            view: PageView::Fit
        }
    )
    .is_err());
    // Executable and local schemes never get written.
    for bad in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,x",
        "ftp://example.com",
        "https://a b",
    ] {
        assert!(
            links::create_link(
                &mut doc,
                0,
                [0.0, 0.0, 10.0, 10.0],
                &TargetInput::Uri { uri: bad.into() }
            )
            .is_err(),
            "{bad}"
        );
    }
    assert!(
        links::create_link(&mut doc, 0, [10.0, 10.0, 0.0, 0.0], &page).is_ok(),
        "reversed corners are normalised"
    );
    // Nothing was written by the refused calls.
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert_eq!(links.len(), 1);
}

#[test]
fn deleting_a_link_only_touches_links() {
    let (mut doc, p) = n_page_doc(2);
    let sq = add_link(
        &mut doc,
        p[0],
        dictionary! { "Type" => "Annot", "Subtype" => "Square", "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
    );
    let id = links::create_link(
        &mut doc,
        0,
        [0.0, 0.0, 20.0, 20.0],
        &TargetInput::Page {
            page: 1,
            view: PageView::Fit,
        },
    )
    .unwrap();
    assert!(
        links::delete_link(&mut doc, 0, sq).is_err(),
        "a Square is not a link"
    );
    assert!(links::delete_link(&mut doc, 1, id).is_err(), "wrong page");
    assert!(
        links::delete_link(&mut doc, 0, (4242, 0)).is_err(),
        "unknown id"
    );
    let (idx, _) = links::delete_link(&mut doc, 0, id).unwrap();
    assert_eq!(idx, 1);
    assert!(
        !doc.objects.contains_key(&id),
        "the annotation object is gone, not orphaned"
    );
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert!(links.is_empty());
    assert!(doc.objects.contains_key(&sq));
}

#[test]
fn links_work_when_annots_is_an_indirect_array() {
    let (mut doc, p) = n_page_doc(2);
    let arr = doc.add_object(Object::Array(vec![]));
    doc.get_dictionary_mut(p[0]).unwrap().set("Annots", arr);
    let id = links::create_link(
        &mut doc,
        0,
        [0.0, 0.0, 30.0, 30.0],
        &TargetInput::Page {
            page: 1,
            view: PageView::Fit,
        },
    )
    .unwrap();
    // The array object itself was updated; the page still points at it.
    assert_eq!(
        doc.get_dictionary(p[0])
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_reference()
            .unwrap(),
        arr
    );
    assert_eq!(doc.get_object(arr).unwrap().as_array().unwrap().len(), 1);
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert_eq!(links.len(), 1);
    links::delete_link(&mut doc, 0, id).unwrap();
    assert!(doc.get_object(arr).unwrap().as_array().unwrap().is_empty());
}

#[test]
fn an_inherited_rotation_is_honoured_for_link_rectangles() {
    let (mut doc, p) = n_page_doc(1);
    let parent = doc
        .get_dictionary(p[0])
        .unwrap()
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.get_dictionary_mut(parent).unwrap().set("Rotate", 90);
    add_link(
        &mut doc,
        p[0],
        link_dict(
            [100, 200, 160, 220],
            (
                "Dest",
                vec![p[0].into(), Object::Name(b"Fit".to_vec())].into(),
            ),
        ),
    );
    let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
    assert_eq!(
        links[0].rect,
        [200.0, 452.0, 220.0, 512.0],
        "rotation set on /Pages applies to the page"
    );
}

#[test]
fn link_ids_parse_strictly() {
    assert_eq!(parse_link_id("obj:12:0").unwrap(), (12, 0));
    for bad in [
        "",
        "12",
        "obj:x:0",
        "obj:1",
        "obj:1:2:3",
        "xobj:1:0",
        "obj:-1:0",
    ] {
        assert!(parse_link_id(bad).is_err(), "{bad}");
    }
}

// ---------------------------------------------------------------------------
// The edit / undo layer
// ---------------------------------------------------------------------------

fn page_target(page: u32) -> TargetInput {
    TargetInput::Page {
        page,
        view: PageView::inherit(),
    }
}

fn do_edit(doc: &mut Document, hist: &mut NavHistory, e: NavEdit) {
    let undo = apply_edit(doc, &e).unwrap_or_else(|err| panic!("{e:?}: {err:#}"));
    hist.record(undo);
}

fn undo_once(doc: &mut Document, hist: &mut NavHistory) {
    let e = hist.undo.pop().expect("something to undo");
    let inv = apply_undo(doc, &e).unwrap();
    hist.redo.push(inv);
}

fn redo_once(doc: &mut Document, hist: &mut NavHistory) {
    let e = hist.redo.pop().expect("something to redo");
    let inv = apply_undo(doc, &e).unwrap();
    hist.undo.push(inv);
}

#[test]
fn bookmark_edits_undo_and_redo_exactly() {
    let (mut doc, _) = n_page_doc(4);
    let mut h = NavHistory::default();
    let s0 = json(&doc);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "Plans".into(),
            target: page_target(0),
        },
    );
    let s1 = json(&doc);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: Some("0".into()),
            anchor_expect: Some("Plans".into()),
            place: Place::Child,
            title: "Level 1".into(),
            target: page_target(1),
        },
    );
    let s2 = json(&doc);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkRename {
            id: "0.0".into(),
            expect: "Level 1".into(),
            title: "Ground".into(),
        },
    );
    let s3 = json(&doc);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkDelete {
            id: "0".into(),
            expect: "Plans".into(),
        },
    );
    let s4 = json(&doc);
    assert_eq!(s4["outline"]["total"], 0);

    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s3, "undo delete restores the subtree");
    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s2);
    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s1);
    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s0);
    assert!(doc.catalog().unwrap().get(b"Outlines").is_err());
    redo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s1);
    redo_once(&mut doc, &mut h);
    redo_once(&mut doc, &mut h);
    redo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s4);
}

#[test]
fn a_stale_bookmark_id_is_refused_not_applied_to_the_wrong_bookmark() {
    let (mut doc, _) = n_page_doc(2);
    let mut h = NavHistory::default();
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "One".into(),
            target: page_target(0),
        },
    );
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "Two".into(),
            target: page_target(1),
        },
    );
    let s = json(&doc);
    // The panel thinks bookmark 0 is "Two" (it was reordered elsewhere).
    let err = apply_edit(
        &mut doc,
        &NavEdit::BookmarkDelete {
            id: "0".into(),
            expect: "Two".into(),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("changed"), "{err}");
    assert!(apply_edit(
        &mut doc,
        &NavEdit::BookmarkDelete {
            id: "9".into(),
            expect: "x".into()
        }
    )
    .is_err());
    assert!(apply_edit(
        &mut doc,
        &NavEdit::BookmarkDelete {
            id: "not a path".into(),
            expect: "x".into()
        }
    )
    .is_err());
    assert_eq!(json(&doc), s, "refused edits change nothing");
}

#[test]
fn move_properties_target_and_collapse_through_the_edit_layer() {
    let (mut doc, _) = n_page_doc(4);
    let mut h = NavHistory::default();
    for (t, p) in [("A", 0), ("B", 1), ("C", 2)] {
        do_edit(
            &mut doc,
            &mut h,
            NavEdit::BookmarkAdd {
                anchor: None,
                anchor_expect: None,
                place: Place::After,
                title: t.into(),
                target: page_target(p),
            },
        );
    }
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkMove {
            id: "2".into(),
            expect: "C".into(),
            anchor: "0".into(),
            anchor_expect: "A".into(),
            place: Place::Before,
        },
    );
    let nodes = &read_info(&doc).outline.nodes;
    assert_eq!(
        nodes.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(),
        vec!["C", "A", "B"]
    );

    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkSetStyle {
            id: "0".into(),
            expect: "C".into(),
            color: Some([1.0, 0.0, 0.0]),
            bold: true,
            italic: true,
        },
    );
    let n = &read_info(&doc).outline.nodes[0];
    assert!(n.bold && n.italic);
    assert_eq!(n.color, Some([1.0, 0.0, 0.0]));

    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkSetTarget {
            id: "1".into(),
            expect: "A".into(),
            target: TargetInput::Uri {
                uri: "https://example.com".into(),
            },
        },
    );
    assert!(matches!(
        read_info(&doc).outline.nodes[1].target,
        Target::Uri { .. }
    ));
    // A URL bookmark has /A and no /Dest; re-pointing back removes the /A.
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkSetTarget {
            id: "1".into(),
            expect: "A".into(),
            target: page_target(3),
        },
    );
    let ids = item_ids(&doc);
    let a = dict(&doc, ids[1]);
    assert!(a.get(b"A").is_err() && a.get(b"Dest").is_ok());

    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: Some("1".into()),
            anchor_expect: None,
            place: Place::Child,
            title: "kid".into(),
            target: page_target(3),
        },
    );
    do_edit(&mut doc, &mut h, NavEdit::BookmarkSetOpen { open: vec![] });
    assert!(!read_info(&doc).outline.nodes[1].open);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkSetOpen {
            open: vec!["1".into()],
        },
    );
    assert!(read_info(&doc).outline.nodes[1].open);
}

#[test]
fn bookmarks_can_be_created_from_page_labels() {
    let (mut doc, _) = n_page_doc(4);
    let mut h = NavHistory::default();
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LabelsNumber {
            pages: vec![0, 1, 2],
            style: Some(LabelStyle::Decimal),
            prefix: "E-".into(),
            start: 101,
        },
    );
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarksFromLabels {
            pages: vec![1, 2, 3],
        },
    );
    let t: Vec<String> = read_info(&doc)
        .outline
        .nodes
        .iter()
        .map(|n| n.title.clone())
        .collect();
    assert_eq!(
        t,
        vec!["E-102", "E-103", "Page 4"],
        "an unlabelled page falls back to its number"
    );
    assert!(matches!(
        read_info(&doc).outline.nodes[0].target,
        Target::Page { page: 1, .. }
    ));
    assert!(apply_edit(&mut doc, &NavEdit::BookmarksFromLabels { pages: vec![] }).is_err());
    assert!(apply_edit(&mut doc, &NavEdit::BookmarksFromLabels { pages: vec![9] }).is_err());
}

#[test]
fn label_edits_and_labels_from_bookmarks_undo() {
    let (mut doc, _) = n_page_doc(4);
    let mut h = NavHistory::default();
    let s0 = json(&doc);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LabelsNumber {
            pages: vec![0, 1, 2, 3],
            style: Some(LabelStyle::UpperAlpha),
            prefix: "".into(),
            start: 1,
        },
    );
    assert_eq!(read_info(&doc).labels, vec!["A", "B", "C", "D"]);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LabelRename {
            page: 2,
            text: "Legend".into(),
        },
    );
    assert_eq!(read_info(&doc).labels, vec!["A", "B", "Legend", "D"]);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "Cover sheet".into(),
            target: page_target(0),
        },
    );
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LabelsFromBookmarks { pages: vec![0, 1] },
    );
    assert_eq!(
        read_info(&doc).labels,
        vec!["Cover sheet", "B", "Legend", "D"]
    );
    assert!(
        apply_edit(&mut doc, &NavEdit::LabelsFromBookmarks { pages: vec![3] }).is_err(),
        "no bookmark on that page"
    );
    undo_once(&mut doc, &mut h);
    assert_eq!(read_info(&doc).labels, vec!["A", "B", "Legend", "D"]);
    undo_once(&mut doc, &mut h);
    undo_once(&mut doc, &mut h);
    assert_eq!(read_info(&doc).labels, vec!["A", "B", "C", "D"]);
    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s0);
}

#[test]
fn link_edits_undo_and_redo_including_the_ids() {
    let (mut doc, _) = n_page_doc(3);
    let mut h = NavHistory::default();
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LinkCreate {
            page: 0,
            rect: [0.0, 0.0, 40.0, 40.0],
            target: page_target(1),
        },
    );
    let s1 = json(&doc);
    let id = read_info(&doc).links[0].id.clone().unwrap();
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LinkSetTarget {
            page: 0,
            id: id.clone(),
            target: TargetInput::Uri {
                uri: "https://example.com".into(),
            },
        },
    );
    let s2 = json(&doc);
    assert!(matches!(
        read_info(&doc).links[0].target,
        Target::Uri { .. }
    ));
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::LinkDelete {
            page: 0,
            id: id.clone(),
        },
    );
    assert!(read_info(&doc).links.is_empty());

    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s2, "the deleted link is back, at the same id");
    undo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s1);
    undo_once(&mut doc, &mut h);
    assert!(read_info(&doc).links.is_empty());
    redo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s1);
    redo_once(&mut doc, &mut h);
    assert_eq!(json(&doc), s2);
}

#[test]
fn history_is_capped_and_a_new_edit_clears_redo() {
    let (mut doc, _) = n_page_doc(2);
    let mut h = NavHistory::default();
    for i in 0..(super::edit::MAX_HISTORY + 10) {
        do_edit(
            &mut doc,
            &mut h,
            NavEdit::BookmarkAdd {
                anchor: None,
                anchor_expect: None,
                place: Place::After,
                title: format!("b{i}"),
                target: page_target(0),
            },
        );
    }
    assert_eq!(h.undo.len(), super::edit::MAX_HISTORY);
    undo_once(&mut doc, &mut h);
    assert_eq!(h.redo.len(), 1);
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "new".into(),
            target: page_target(0),
        },
    );
    assert!(h.redo.is_empty());
}

#[test]
fn a_damaged_outline_cannot_be_undone_into() {
    let (mut doc, _) = n_page_doc(1);
    let mut h = NavHistory::default();
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "A".into(),
            target: page_target(0),
        },
    );
    do_edit(
        &mut doc,
        &mut h,
        NavEdit::BookmarkAdd {
            anchor: None,
            anchor_expect: None,
            place: Place::After,
            title: "B".into(),
            target: page_target(0),
        },
    );
    let ids = item_ids(&doc);
    doc.get_dictionary_mut(ids[1])
        .unwrap()
        .set("Next", Object::Reference(ids[0]));
    let e = h.undo.pop().unwrap();
    assert!(matches!(e, NavUndo::Outline(_)));
    assert!(apply_undo(&mut doc, &e).is_err());
}

// ---------------------------------------------------------------------------
// File-level: the write path other features share, and the mark-up save leaving it alone
// ---------------------------------------------------------------------------

mod on_disk {
    use super::*;
    use crate::commands::document::apply_edit_and_save_scaled;
    use crate::document::annots::read_markups;
    use crate::document::save::{load_markups_from, save_with_markups};
    use std::collections::HashSet;

    /// A 4-page file with a mark-up, a foreign Link and (optionally) navigation structure.
    fn fixture(dir: &std::path::Path) -> std::path::PathBuf {
        let (mut doc, p) = n_page_doc(4);
        add_link(
            &mut doc,
            p[0],
            link_dict([5, 5, 95, 35], ("A", dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com/spec") }.into())),
        );
        let path = dir.join("nav.pdf");
        doc.save(&path).unwrap();
        save_with_markups(
            &path,
            &path,
            &[crate::document::annots::tests::redline_markup(1)],
        )
        .unwrap();
        path
    }

    fn edit_on_disk(path: &std::path::Path, edits: Vec<NavEdit>) {
        let markups = load_markups_from(path, None).unwrap();
        apply_edit_and_save_scaled(path, &markups, &HashSet::new(), &[], move |doc| {
            for e in &edits {
                apply_edit(doc, e)?;
            }
            Ok(())
        })
        .unwrap();
    }

    fn info_of(path: &std::path::Path) -> serde_json::Value {
        json(&Document::load(path).unwrap())
    }

    #[test]
    fn navigation_edits_ride_the_page_edit_write_path_and_keep_markups_and_foreign_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        edit_on_disk(
            &path,
            vec![
                NavEdit::BookmarkAdd {
                    anchor: None,
                    anchor_expect: None,
                    place: Place::After,
                    title: "Plans".into(),
                    target: page_target(1),
                },
                NavEdit::LabelsNumber {
                    pages: vec![0, 1, 2, 3],
                    style: Some(LabelStyle::Decimal),
                    prefix: "E-".into(),
                    start: 1,
                },
                NavEdit::LinkCreate {
                    page: 2,
                    rect: [0.0, 0.0, 50.0, 50.0],
                    target: page_target(0),
                },
            ],
        );
        let doc = Document::load(&path).unwrap();
        let info = read_info(&doc);
        assert_eq!(info.outline.nodes[0].title, "Plans");
        assert_eq!(info.labels, vec!["E-1", "E-2", "E-3", "E-4"]);
        assert_eq!(info.links.len(), 2, "the foreign Link and the new one");
        assert!(
            matches!(&info.links[0].target, Target::Uri { uri } if uri == "https://example.com/spec")
        );
        let markups = read_markups(&doc).unwrap();
        assert_eq!(
            markups.len(),
            1,
            "the mark-up survived the navigation write"
        );
        assert_eq!(markups[0].page, 1);
    }

    #[test]
    fn the_markup_save_path_never_changes_navigation_structure_three_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        edit_on_disk(
            &path,
            vec![
                NavEdit::BookmarkAdd {
                    anchor: None,
                    anchor_expect: None,
                    place: Place::After,
                    title: "A".into(),
                    target: page_target(0),
                },
                NavEdit::BookmarkAdd {
                    anchor: Some("0".into()),
                    anchor_expect: None,
                    place: Place::Child,
                    title: "B".into(),
                    target: page_target(1),
                },
                NavEdit::LabelsNumber {
                    pages: vec![0, 1, 2, 3],
                    style: Some(LabelStyle::LowerRoman),
                    prefix: "".into(),
                    start: 1,
                },
                NavEdit::LinkCreate {
                    page: 3,
                    rect: [10.0, 10.0, 60.0, 40.0],
                    target: TargetInput::Uri {
                        uri: "https://example.com".into(),
                    },
                },
            ],
        );
        let before = info_of(&path);
        for cycle in 0..3 {
            let markups = load_markups_from(&path, None).unwrap();
            save_with_markups(&path, &path, &markups).unwrap();
            assert_eq!(info_of(&path), before, "mark-up save, cycle {cycle}");
            // And a page-edit write with no navigation edit at all.
            let markups = load_markups_from(&path, None).unwrap();
            apply_edit_and_save_scaled(&path, &markups, &HashSet::new(), &[], |_| Ok(())).unwrap();
            assert_eq!(info_of(&path), before, "page-edit write, cycle {cycle}");
        }
    }

    #[test]
    fn a_failed_navigation_edit_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        let bytes_before = std::fs::read(&path).unwrap();
        let markups = load_markups_from(&path, None).unwrap();
        let err = apply_edit_and_save_scaled(&path, &markups, &HashSet::new(), &[], |doc| {
            apply_edit(
                doc,
                &NavEdit::LinkCreate {
                    page: 0,
                    rect: [0.0, 0.0, 40.0, 40.0],
                    target: TargetInput::Uri {
                        uri: "javascript:alert(1)".into(),
                    },
                },
            )?;
            Ok(())
        });
        assert!(err.is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes_before,
            "the file is byte-identical"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".redline-tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}

// ---------------------------------------------------------------------------
// The JSON contract with the frontend (src/lib/navigation-ipc.ts)
// ---------------------------------------------------------------------------

mod contract {
    use super::*;
    use serde_json::json;

    fn edit(v: serde_json::Value) -> Result<NavEdit, serde_json::Error> {
        serde_json::from_value(v)
    }

    #[test]
    fn every_edit_the_frontend_can_send_parses() {
        let page = json!({"kind":"page","page":1,"view":{"kind":"xyz","left":null,"top":null,"zoom":null}});
        let cases = vec![
            json!({"op":"bookmark_add","anchor":null,"anchor_expect":null,"place":"after","title":"t","target":page}),
            json!({"op":"bookmark_add","anchor":"0.1","anchor_expect":"x","place":"child","title":"t","target":{"kind":"uri","uri":"https://example.com"}}),
            json!({"op":"bookmark_rename","id":"0","expect":"a","title":"b"}),
            json!({"op":"bookmark_delete","id":"0","expect":"a"}),
            json!({"op":"bookmark_move","id":"0","expect":"a","anchor":"1","anchor_expect":"b","place":"before"}),
            json!({"op":"bookmark_set_target","id":"0","expect":"a","target":page}),
            json!({"op":"bookmark_set_style","id":"0","expect":"a","color":[1.0,0.0,0.0],"bold":true,"italic":false}),
            json!({"op":"bookmark_set_style","id":"0","expect":"a","color":null,"bold":false,"italic":false}),
            json!({"op":"bookmark_set_open","open":["0","0.1"]}),
            json!({"op":"bookmarks_from_labels","pages":[0,1]}),
            json!({"op":"labels_number","pages":[0,1],"style":"D","prefix":"E-","start":101}),
            json!({"op":"labels_number","pages":[0],"style":"r","prefix":"","start":1}),
            json!({"op":"labels_number","pages":[0],"style":null,"prefix":"Cover","start":1}),
            json!({"op":"label_rename","page":2,"text":"x"}),
            json!({"op":"labels_from_bookmarks","pages":[3]}),
            json!({"op":"link_create","page":0,"rect":[1.0,2.0,30.0,40.0],"target":page}),
            json!({"op":"link_delete","page":0,"id":"obj:9:0"}),
            json!({"op":"link_set_target","page":0,"id":"obj:9:0","target":{"kind":"uri","uri":"mailto:me@example.com"}}),
        ];
        for c in cases {
            edit(c.clone()).unwrap_or_else(|e| panic!("{c}: {e}"));
        }
    }

    #[test]
    fn every_view_the_frontend_can_send_parses() {
        for v in [
            json!({"kind":"xyz","left":1.0,"top":2.0,"zoom":1.5}),
            json!({"kind":"fit"}),
            json!({"kind":"fit_h","top":null}),
            json!({"kind":"fit_v","left":3.0}),
            json!({"kind":"fit_r","left":1.0,"bottom":2.0,"right":3.0,"top":4.0}),
            json!({"kind":"fit_b"}),
            json!({"kind":"fit_b_h","top":null}),
            json!({"kind":"fit_b_v","left":null}),
        ] {
            let _: PageView =
                serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
        }
    }

    #[test]
    fn a_target_the_user_cannot_author_is_not_accepted() {
        for bad in [
            json!({"op":"link_create","page":0,"rect":[0,0,9,9],"target":{"kind":"unsupported","action":"Launch"}}),
            json!({"op":"link_create","page":0,"rect":[0,0,9,9],"target":{"kind":"launch","file":"calc.exe"}}),
            json!({"op":"link_create","page":0,"rect":[0,0,9,9],"target":{"kind":"blocked","reason":"x"}}),
            json!({"op":"link_create","page":0,"rect":[0,0,9,9],"target":{"kind":"none"}}),
            json!({"op":"nope"}),
            json!({"op":"bookmark_delete","id":"0"}),
            json!({"op":"bookmark_add","anchor":null,"anchor_expect":null,"place":"inside","title":"t","target":{"kind":"uri","uri":"https://a.example"}}),
            json!({"op":"labels_number","pages":[0],"style":"X","prefix":"","start":1}),
        ] {
            assert!(edit(bad.clone()).is_err(), "{bad} must not parse");
        }
    }

    #[test]
    fn the_state_the_frontend_receives_has_the_documented_shape() {
        let (mut doc, p) = n_page_doc(3);
        put(
            &mut doc,
            &[node(
                "A",
                Some(p[1]),
                true,
                vec![node("B", Some(p[2]), false, vec![])],
            )],
        );
        labels::write_ranges(
            &mut doc,
            &[LabelRange {
                start: 0,
                style: Some(LabelStyle::Decimal),
                prefix: "E-".into(),
                first: 1,
            }],
        )
        .unwrap();
        links::create_link(
            &mut doc,
            0,
            [0.0, 0.0, 20.0, 20.0],
            &TargetInput::Page {
                page: 2,
                view: PageView::Fit,
            },
        )
        .unwrap();
        let v = serde_json::to_value(read_info(&doc)).unwrap();
        assert_eq!(v["page_count"], 3);
        assert_eq!(v["labels"], json!(["E-1", "E-2", "E-3"]));
        assert_eq!(v["outline"]["nodes"][0]["id"], "0");
        assert_eq!(v["outline"]["nodes"][0]["children"][0]["id"], "0.0");
        assert_eq!(
            v["outline"]["nodes"][0]["target"],
            json!({"kind":"page","page":1,"view":{"kind":"fit"}})
        );
        assert_eq!(v["outline"]["nodes"][0]["open"], true);
        assert_eq!(v["outline"]["read_only"], serde_json::Value::Null);
        assert_eq!(v["links"][0]["target"]["kind"], "page");
        assert_eq!(v["links"][0]["rect"], json!([0.0, 0.0, 20.0, 20.0]));
        assert!(v["links"][0]["id"].as_str().unwrap().starts_with("obj:"));
        for k in ["links_truncated", "read_only", "undo_depth", "redo_depth"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        // The other target kinds as the frontend's NavTarget type spells them.
        assert_eq!(
            serde_json::to_value(Target::None).unwrap(),
            json!({"kind":"none"})
        );
        assert_eq!(
            serde_json::to_value(Target::Broken).unwrap(),
            json!({"kind":"broken"})
        );
        assert_eq!(
            serde_json::to_value(Target::Uri {
                uri: "https://a.example".into()
            })
            .unwrap(),
            json!({"kind":"uri","uri":"https://a.example"})
        );
        assert_eq!(
            serde_json::to_value(Target::Unsupported {
                action: "Launch".into()
            })
            .unwrap(),
            json!({"kind":"unsupported","action":"Launch"})
        );
        assert_eq!(
            serde_json::to_value(Target::Blocked { reason: "x".into() }).unwrap(),
            json!({"kind":"blocked","reason":"x"})
        );
        assert_eq!(
            serde_json::to_value(PageView::FitBH { top: None }).unwrap(),
            json!({"kind":"fit_b_h","top":null})
        );
    }
}

// ---------------------------------------------------------------------------
// A sample file for opening in Revu / Acrobat (owner check) and in other readers
// ---------------------------------------------------------------------------

/// Writes `navigation-sample.pdf`: a six-page set with page labels (i, ii, E-101..E-104),
/// nested bookmarks (one web address), links (page, web address, and a foreign Launch link
/// that redline must carry but never run) and a redline mark-up. Run with:
///
/// ```text
/// REDLINE_NAV_SAMPLE_OUT=/tmp/navigation-sample.pdf \
///   cargo test --lib navigation_sample -- --ignored
/// ```
#[test]
#[ignore = "writes a sample PDF for manual checks; set REDLINE_NAV_SAMPLE_OUT"]
fn navigation_sample_for_manual_checks() {
    use crate::commands::document::apply_edit_and_save_scaled;
    use crate::document::save::{load_markups_from, save_with_markups};
    use lopdf::Stream;
    use std::collections::HashSet;

    let Some(out) = std::env::var_os("REDLINE_NAV_SAMPLE_OUT") else {
        eprintln!("REDLINE_NAV_SAMPLE_OUT is not set; nothing to write");
        return;
    };
    let out = std::path::PathBuf::from(out);

    let (mut doc, p) = n_page_doc(6);
    let font = doc.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let names = [
        "Front matter i",
        "Front matter ii",
        "Sheet E-101 Lighting plan",
        "Sheet E-102 Power plan",
        "Sheet E-103 Details",
        "Sheet E-104 Schedules",
    ];
    for (i, id) in p.iter().enumerate() {
        let content = format!("BT /F1 28 Tf 72 700 Td ({}) Tj ET", names[i]);
        let c = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page = doc.get_dictionary_mut(*id).unwrap();
        page.set("Contents", c);
        page.set(
            "Resources",
            dictionary! { "Font" => dictionary! { "F1" => font } },
        );
    }
    let path = out.clone();
    doc.save(&path).unwrap();

    let markups = load_markups_from(&path, None).unwrap();
    apply_edit_and_save_scaled(&path, &markups, &HashSet::new(), &[], |doc| {
        let page = |p: u32| TargetInput::Page {
            page: p,
            view: PageView::inherit(),
        };
        // Labels: i, ii, then E-101 ...
        apply_edit(
            doc,
            &NavEdit::LabelsNumber {
                pages: vec![0, 1],
                style: Some(LabelStyle::LowerRoman),
                prefix: "".into(),
                start: 1,
            },
        )?;
        apply_edit(
            doc,
            &NavEdit::LabelsNumber {
                pages: vec![2, 3, 4, 5],
                style: Some(LabelStyle::Decimal),
                prefix: "E-".into(),
                start: 101,
            },
        )?;
        // Bookmarks.
        let add = |doc: &mut Document,
                   anchor: Option<&str>,
                   place: Place,
                   title: &str,
                   target: TargetInput| {
            apply_edit(
                doc,
                &NavEdit::BookmarkAdd {
                    anchor: anchor.map(str::to_string),
                    anchor_expect: None,
                    place,
                    title: title.into(),
                    target,
                },
            )
        };
        add(doc, None, Place::After, "Cover", page(0))?;
        add(doc, None, Place::After, "Electrical", page(2))?;
        add(
            doc,
            Some("1"),
            Place::Child,
            "Lighting plan E-101",
            TargetInput::Page {
                page: 2,
                view: PageView::Xyz {
                    left: Some(72.0),
                    top: Some(720.0),
                    zoom: Some(1.0),
                },
            },
        )?;
        add(doc, Some("1"), Place::Child, "Power plan E-102", page(3))?;
        add(doc, None, Place::After, "Details", page(4))?;
        add(doc, Some("2"), Place::Child, "E-103", page(4))?;
        add(doc, Some("2"), Place::Child, "E-104", page(5))?;
        add(
            doc,
            None,
            Place::After,
            "Specification (web)",
            TargetInput::Uri {
                uri: "https://example.com/spec".into(),
            },
        )?;
        apply_edit(
            doc,
            &NavEdit::BookmarkSetOpen {
                open: vec!["1".into()],
            },
        )?;
        // Links.
        apply_edit(
            doc,
            &NavEdit::LinkCreate {
                page: 2,
                rect: [60.0, 690.0, 380.0, 740.0],
                target: TargetInput::Page {
                    page: 4,
                    view: PageView::Fit,
                },
            },
        )?;
        apply_edit(
            doc,
            &NavEdit::LinkCreate {
                page: 2,
                rect: [60.0, 600.0, 380.0, 640.0],
                target: TargetInput::Uri {
                    uri: "https://example.com/spec".into(),
                },
            },
        )?;
        apply_edit(
            doc,
            &NavEdit::LinkCreate {
                page: 3,
                rect: [60.0, 690.0, 380.0, 740.0],
                target: page(2),
            },
        )?;
        // A foreign Launch link, as another program might have written: carried, never run.
        let pages = PageIndex::build(doc);
        let pid = pages.id_of(3).unwrap();
        let launch = doc.add_object(Object::Dictionary(dictionary! {
            "Type" => "Annot", "Subtype" => "Link",
            "Rect" => vec![60.into(), 500.into(), 380.into(), 540.into()],
            "A" => dictionary! { "S" => "Launch", "F" => Object::string_literal("calc.exe") },
        }));
        let mut annots = match doc.get_dictionary(pid).unwrap().get(b"Annots") {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![],
        };
        annots.push(Object::Reference(launch));
        doc.get_dictionary_mut(pid)
            .unwrap()
            .set("Annots", Object::Array(annots));
        Ok(())
    })
    .unwrap();
    // And a redline mark-up on top, through the normal save path.
    save_with_markups(
        &path,
        &path,
        &[crate::document::annots::tests::redline_markup(2)],
    )
    .unwrap();
    eprintln!("wrote {}", path.display());
}

// ---------------------------------------------------------------------------
// Writing must never destroy objects it does not own (hostile /PageLabels, /Outlines)
// ---------------------------------------------------------------------------

mod hostile_references {
    use super::*;

    /// A document with a page, an annotation on it, and an object other things share.
    fn doc_with_victims() -> (Document, Vec<ObjectId>, ObjectId, ObjectId) {
        let (mut doc, p) = n_page_doc(2);
        let annot = add_link(
            &mut doc,
            p[0],
            dictionary! { "Type" => "Annot", "Subtype" => "Square", "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
        );
        let shared = doc.add_object(dictionary! { "Shared" => "yes", "Nums" => vec![0.into(), label_dict(Some("D"), None, None)] });
        // Something else in the file refers to the shared object.
        doc.get_dictionary_mut(p[1])
            .unwrap()
            .set("SharedThing", shared);
        (doc, p, annot, shared)
    }

    fn snapshot(doc: &Document, ids: &[ObjectId]) -> Vec<Object> {
        ids.iter()
            .map(|id| doc.objects.get(id).cloned().expect("object exists"))
            .collect()
    }

    #[test]
    fn writing_labels_over_a_hostile_page_labels_entry_deletes_nothing() {
        for pointing_at in 0..3 {
            let (mut doc, p, annot, shared) = doc_with_victims();
            let victims = [p[0], p[1], annot, shared];
            // /PageLabels is (or points through /Kids at) a page, an annotation, a shared object.
            let target = [p[0], annot, shared][pointing_at];
            let entry: Object = if pointing_at == 2 {
                Object::Reference(target)
            } else {
                dictionary! { "Kids" => vec![Object::Reference(target)] }.into()
            };
            doc.catalog_mut().unwrap().set("PageLabels", entry);
            let before = snapshot(&doc, &victims);

            let ranges = labels::apply_numbering(
                &labels::read_ranges(&doc),
                2,
                &[0, 1],
                Some(LabelStyle::Decimal),
                "E-",
                1,
            )
            .unwrap();
            labels::write_ranges(&mut doc, &ranges).unwrap();
            assert_eq!(
                snapshot(&doc, &victims),
                before,
                "case {pointing_at}: no object was removed or changed"
            );
            assert_eq!(
                labels::page_labels(&labels::read_ranges(&doc), 2),
                vec!["E-1", "E-2"]
            );
            // Removing the labels altogether is just as safe.
            labels::write_ranges(&mut doc, &[]).unwrap();
            assert_eq!(snapshot(&doc, &victims), before);
            assert_eq!(PageIndex::build(&doc).count(), 2, "the page tree is intact");
        }
    }

    #[test]
    fn a_label_tree_with_a_shared_node_is_left_in_place() {
        let (mut doc, _p, _a, shared) = doc_with_victims();
        doc.catalog_mut()
            .unwrap()
            .set("PageLabels", Object::Reference(shared));
        let ranges = labels::read_ranges(&doc);
        labels::write_ranges(&mut doc, &ranges).unwrap();
        assert!(
            doc.objects.contains_key(&shared),
            "the object another part of the file refers to survives"
        );
    }

    #[test]
    fn writing_the_outline_over_a_hostile_outlines_entry_deletes_and_overwrites_nothing() {
        // /Outlines is a page; a page's /First chain is irrelevant, but the page must survive.
        let (mut doc, p, annot, shared) = doc_with_victims();
        let victims = [p[0], p[1], annot, shared];
        doc.catalog_mut()
            .unwrap()
            .set("Outlines", Object::Reference(p[0]));
        let before = snapshot(&doc, &victims);
        put(&mut doc, &[node("Mine", Some(p[1]), false, vec![])]);
        assert_eq!(
            snapshot(&doc, &victims),
            before,
            "the page the entry pointed at is untouched"
        );
        assert_eq!(read_info(&doc).outline.nodes[0].title, "Mine");

        // The outline's first item is an annotation, the next a shared object, then a page:
        // all of them are referenced elsewhere, so a rewrite must leave every one in place.
        let (mut doc, p, annot, shared) = doc_with_victims();
        let victims = [p[0], p[1], annot, shared];
        let root =
            doc.add_object(dictionary! { "Type" => "Outlines", "First" => annot, "Last" => annot });
        doc.get_dictionary_mut(annot).unwrap().set("Next", shared);
        doc.get_dictionary_mut(shared).unwrap().set("Next", p[1]);
        doc.catalog_mut().unwrap().set("Outlines", root);
        let before = snapshot(&doc, &[p[0]]);
        let read = outline::read_outline(&doc);
        assert!(!read.item_ids.is_empty());
        let nodes = read.nodes.clone();
        outline::write_outline(&mut doc, &nodes, &read.item_ids, read.root_id).unwrap();
        for id in victims {
            assert!(doc.objects.contains_key(&id), "{id:?} was deleted");
        }
        assert_eq!(snapshot(&doc, &[p[0]]), before);
        assert_eq!(PageIndex::build(&doc).count(), 2);
    }

    #[test]
    fn replaced_outline_objects_that_nothing_else_uses_are_still_removed() {
        let (mut doc, p) = n_page_doc(2);
        put(&mut doc, &sample_forest(&[p[0], p[1], p[0], p[1], p[0]]));
        let n = doc.objects.len();
        put(&mut doc, &[node("Only", Some(p[0]), false, vec![])]);
        assert!(
            doc.objects.len() < n,
            "old items and root were dropped, no orphans left behind"
        );
    }

    #[test]
    fn a_locked_link_cannot_be_deleted_or_re_pointed() {
        let (mut doc, p) = n_page_doc(2);
        let id = add_link(&mut doc, p[0], {
            let mut d = link_dict(
                [0, 0, 50, 50],
                (
                    "Dest",
                    vec![p[1].into(), Object::Name(b"Fit".to_vec())].into(),
                ),
            );
            d.set("F", 128); // Locked
            d
        });
        let lid = links::link_id(id);
        let before = doc.objects.get(&id).cloned().unwrap();
        for e in [
            NavEdit::LinkDelete {
                page: 0,
                id: lid.clone(),
            },
            NavEdit::LinkSetTarget {
                page: 0,
                id: lid.clone(),
                target: TargetInput::Uri {
                    uri: "https://example.com".into(),
                },
            },
        ] {
            let err = apply_edit(&mut doc, &e).unwrap_err().to_string();
            assert!(err.contains("locked"), "{err}");
        }
        assert_eq!(doc.objects.get(&id).cloned().unwrap(), before);
        // LockedContents (bit 10) counts too.
        doc.get_dictionary_mut(id).unwrap().set("F", 512);
        assert!(apply_edit(&mut doc, &NavEdit::LinkDelete { page: 0, id: lid }).is_err());
        // An unlocked one is still editable.
        let free = links::create_link(
            &mut doc,
            0,
            [0.0, 0.0, 30.0, 30.0],
            &TargetInput::Page {
                page: 1,
                view: PageView::Fit,
            },
        )
        .unwrap();
        assert!(apply_edit(
            &mut doc,
            &NavEdit::LinkDelete {
                page: 0,
                id: links::link_id(free)
            }
        )
        .is_ok());
    }

    // ---- reachability: nothing still referenced is ever deleted -----------------------

    /// Two old outline items, A (the one we try to keep alive) and B (referenced by nothing
    /// outside the outline). Returns the doc and the two ids.
    fn outline_a_b() -> (Document, ObjectId, ObjectId) {
        let (mut doc, p) = n_page_doc(2);
        put(
            &mut doc,
            &[
                node("A", Some(p[0]), false, vec![]),
                node("B", Some(p[1]), false, vec![]),
            ],
        );
        let ids = item_ids(&doc);
        (doc, ids[0], ids[1])
    }

    /// A reference to a missing object anywhere in the document (objects, nested values, trailer).
    pub(super) fn dangling_refs(doc: &Document) -> Vec<ObjectId> {
        fn walk(o: &Object, doc: &Document, out: &mut Vec<ObjectId>) {
            match o {
                Object::Reference(r) if !doc.objects.contains_key(r) => out.push(*r),
                Object::Array(a) => a.iter().for_each(|x| walk(x, doc, out)),
                Object::Dictionary(d) => d.iter().for_each(|(_, v)| walk(v, doc, out)),
                Object::Stream(s) => s.dict.iter().for_each(|(_, v)| walk(v, doc, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        doc.trailer.iter().for_each(|(_, v)| walk(v, doc, &mut out));
        doc.objects.values().for_each(|o| walk(o, doc, &mut out));
        out
    }

    /// A is referenced from outside the old outline, so it survives; B is only reachable
    /// through A's /Next, so it survives too (deleting it would leave A pointing at nothing).
    /// What must never happen is a dangling reference.
    fn assert_a_kept_b_dropped(mut doc: Document, a: ObjectId, _b: ObjectId, why: &str) {
        put(&mut doc, &[]);
        assert!(
            doc.objects.contains_key(&a),
            "{why}: A is still referenced and must survive"
        );
        assert_eq!(
            dangling_refs(&doc),
            Vec::<ObjectId>::new(),
            "{why}: no reference points at a deleted object"
        );
    }

    #[test]
    fn an_old_item_referenced_from_names_dests_is_kept() {
        let (mut doc, a, b) = outline_a_b();
        let tree =
            doc.add_object(dictionary! { "Names" => vec![Object::string_literal("x"), a.into()] });
        let names = doc.add_object(dictionary! { "Dests" => tree });
        doc.catalog_mut().unwrap().set("Names", names);
        assert_a_kept_b_dropped(doc, a, b, "/Names /Dests");
    }

    #[test]
    fn an_old_item_referenced_from_the_catalog_dests_openaction_or_acroform_is_kept() {
        for key in [
            "Dests",
            "OpenAction",
            "AcroForm",
            "StructTreeRoot",
            "ViewerPreferences",
        ] {
            let (mut doc, a, b) = outline_a_b();
            doc.catalog_mut().unwrap().set(
                key,
                Object::Array(vec![
                    dictionary! { "Deep" => vec![Object::Reference(a)] }.into()
                ]),
            );
            assert_a_kept_b_dropped(doc, a, b, key);
        }
    }

    #[test]
    fn an_old_item_referenced_from_an_annotation_action_dest_or_p_is_kept() {
        let (mut doc, a, b) = outline_a_b();
        let page = *doc.get_pages().get(&1).unwrap();
        add_link(
            &mut doc,
            page,
            dictionary! {
                "Type" => "Annot", "Subtype" => "Link",
                "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()],
                "A" => dictionary! { "S" => "GoTo", "D" => vec![a.into(), Object::Name(b"Fit".to_vec())] },
            },
        );
        assert_a_kept_b_dropped(doc, a, b, "annotation /A /D");
        let (mut doc, a, b) = outline_a_b();
        let page = *doc.get_pages().get(&1).unwrap();
        add_link(
            &mut doc,
            page,
            dictionary! { "Type" => "Annot", "Subtype" => "Text", "P" => a, "Rect" => vec![0.into(), 0.into(), 9.into(), 9.into()] },
        );
        assert_a_kept_b_dropped(doc, a, b, "annotation /P");
    }

    #[test]
    fn an_old_item_referenced_only_from_the_trailer_is_kept() {
        for key in ["Info", "Custom", "Prev"] {
            let (mut doc, a, b) = outline_a_b();
            doc.trailer.set(key, Object::Reference(a));
            assert_a_kept_b_dropped(doc, a, b, key);
        }
    }

    #[test]
    fn an_old_item_referenced_only_from_inside_an_object_stream_is_kept() {
        use lopdf::Stream;
        // The referring object lives only in the (unexpanded) bytes of an /ObjStm, raw or Flate.
        for compress in [false, true] {
            let (mut doc, a, b) = outline_a_b();
            let header = "500 0 ";
            let body = format!("<< /Dest [ {} {} R /Fit ] >>", a.0, a.1);
            let mut st = Stream::new(
                dictionary! { "Type" => "ObjStm", "N" => 1, "First" => header.len() as i64 },
                format!("{header}{body}").into_bytes(),
            );
            if compress {
                st.compress().unwrap();
            }
            doc.add_object(st);
            assert_a_kept_b_dropped(
                doc,
                a,
                b,
                if compress {
                    "compressed /ObjStm"
                } else {
                    "/ObjStm"
                },
            );
        }
    }

    #[test]
    fn an_old_item_referenced_from_a_xref_style_stream_dictionary_is_kept() {
        use lopdf::Stream;
        let (mut doc, a, b) = outline_a_b();
        doc.add_object(Stream::new(
            dictionary! { "Type" => "XRef", "Root" => a, "Size" => 10 },
            vec![],
        ));
        assert_a_kept_b_dropped(doc, a, b, "xref stream dictionary");
    }

    #[test]
    fn references_nested_deeply_are_found_without_recursion() {
        let (mut doc, a, b) = outline_a_b();
        let mut nested: Object = vec![Object::Reference(a)].into();
        for _ in 0..3000 {
            nested = Object::Array(vec![nested, Object::Dictionary(dictionary! {})]);
        }
        doc.add_object(nested);
        assert_a_kept_b_dropped(doc, a, b, "3000 levels deep");
    }

    #[test]
    fn when_the_scan_cannot_finish_nothing_is_deleted() {
        let (doc, _a, _b) = outline_a_b();
        let read = outline::read_outline(&doc);
        let mut ids = read.item_ids.clone();
        ids.extend(read.root_id);
        assert_eq!(
            super::super::unreferenced_among_with_budget(&doc, &ids, 3),
            Vec::<ObjectId>::new(),
            "budget exhausted: fail closed"
        );
        assert_eq!(
            super::super::unreferenced_among_with_budget(&doc, &ids, 0),
            Vec::<ObjectId>::new()
        );
        let mut detached = doc.clone();
        detached.catalog_mut().unwrap().remove(b"Outlines");
        let full = super::super::unreferenced_among_with_budget(
            &detached,
            &ids,
            super::super::MAX_REACH_NODES,
        );
        assert_eq!(full.len(), ids.len(), "with a real budget and no outside reference everything is removable");
        assert!(
            super::super::unreferenced_among(&doc, &ids).is_empty(),
            "the catalog still reaches the root, which reaches every item"
        );
        // And a rewrite under an exhausted budget leaves the old items orphaned, not deleted.
        let (mut doc, a, b) = outline_a_b();
        let read = outline::read_outline(&doc);
        for id in super::super::unreferenced_among_with_budget(&doc, &read.item_ids, 5) {
            doc.objects.remove(&id);
        }
        assert!(doc.objects.contains_key(&a) && doc.objects.contains_key(&b));
    }

    // ---- authorization checks fail closed --------------------------------------------

    #[test]
    fn a_link_whose_flags_cannot_be_read_is_treated_as_locked() {
        let unreadable: Vec<(&str, Object)> = vec![
            ("real", Object::Real(128.0)),
            ("fractional real", Object::Real(0.5)),
            ("infinite real", Object::Real(f32::INFINITY)),
            ("string", Object::string_literal("128")),
            ("name", Object::Name(b"Locked".to_vec())),
            ("array", Object::Array(vec![Object::Integer(0)])),
            ("dict", Object::Dictionary(dictionary! {})),
            ("null", Object::Null),
            ("dangling ref", Object::Reference((9999, 0))),
            ("negative (all bits)", Object::Integer(-1)),
            ("locked", Object::Integer(128)),
            ("locked contents", Object::Integer(512)),
            ("locked among others", Object::Integer(4 | 128)),
        ];
        for (what, f) in unreadable {
            let (mut doc, p) = n_page_doc(2);
            let id = add_link(&mut doc, p[0], {
                let mut d = link_dict(
                    [0, 0, 50, 50],
                    (
                        "Dest",
                        vec![p[1].into(), Object::Name(b"Fit".to_vec())].into(),
                    ),
                );
                d.set("F", f);
                d
            });
            let before = doc.objects.get(&id).cloned().unwrap();
            let lid = links::link_id(id);
            assert!(
                apply_edit(
                    &mut doc,
                    &NavEdit::LinkDelete {
                        page: 0,
                        id: lid.clone()
                    }
                )
                .is_err(),
                "{what}: delete"
            );
            assert!(
                apply_edit(
                    &mut doc,
                    &NavEdit::LinkSetTarget {
                        page: 0,
                        id: lid,
                        target: TargetInput::Uri {
                            uri: "https://example.com".into()
                        }
                    }
                )
                .is_err(),
                "{what}: re-point"
            );
            assert_eq!(
                doc.objects.get(&id).cloned().unwrap(),
                before,
                "{what}: untouched"
            );
        }
    }

    #[test]
    fn readable_unlocked_flags_still_allow_editing() {
        // Absent, zero, print-only, and an indirect integer without the lock bits.
        for f in [
            None,
            Some(Object::Real(0.0)),
            Some(Object::Real(4.0)),
            Some(Object::Integer(0)),
            Some(Object::Integer(4)),
            Some(Object::Integer(4 | 256)),
        ] {
            let (mut doc, p) = n_page_doc(2);
            let id = add_link(&mut doc, p[0], {
                let mut d = link_dict(
                    [0, 0, 50, 50],
                    (
                        "Dest",
                        vec![p[1].into(), Object::Name(b"Fit".to_vec())].into(),
                    ),
                );
                if let Some(f) = &f {
                    d.set("F", f.clone());
                }
                d
            });
            assert!(
                apply_edit(
                    &mut doc,
                    &NavEdit::LinkDelete {
                        page: 0,
                        id: links::link_id(id)
                    }
                )
                .is_ok(),
                "{f:?}"
            );
        }
        let (mut doc, p) = n_page_doc(2);
        let flags = doc.add_object(Object::Integer(4));
        let id = add_link(&mut doc, p[0], {
            let mut d = link_dict(
                [0, 0, 50, 50],
                (
                    "Dest",
                    vec![p[1].into(), Object::Name(b"Fit".to_vec())].into(),
                ),
            );
            d.set("F", flags);
            d
        });
        assert!(
            apply_edit(
                &mut doc,
                &NavEdit::LinkDelete {
                    page: 0,
                    id: links::link_id(id)
                }
            )
            .is_ok(),
            "indirect unlocked flags"
        );
        // An indirect flags object that holds the lock bit is locked.
        let (mut doc, p) = n_page_doc(2);
        let flags = doc.add_object(Object::Integer(128));
        let id = add_link(&mut doc, p[0], {
            let mut d = link_dict(
                [0, 0, 50, 50],
                (
                    "Dest",
                    vec![p[1].into(), Object::Name(b"Fit".to_vec())].into(),
                ),
            );
            d.set("F", flags);
            d
        });
        assert!(
            apply_edit(
                &mut doc,
                &NavEdit::LinkDelete {
                    page: 0,
                    id: links::link_id(id)
                }
            )
            .is_err(),
            "indirect locked flags"
        );
    }
}

/// Round 5: delete_link keeps shared objects, partial label trees are read-only, mail and host
/// forms, absolute opener path.
mod round5 {
    use super::*;
    use crate::document::navigation::uri::validate_uri;

    fn goto(page: ObjectId) -> Dictionary {
        link_dict([0, 0, 50, 50], dest_to(page))
    }

    fn delete(doc: &mut Document, page: u32, id: ObjectId) -> Result<(), anyhow::Error> {
        apply_edit(
            doc,
            &NavEdit::LinkDelete {
                page,
                id: links::link_id(id),
            },
        )
        .map(|_| ())
    }

    #[test]
    fn a_link_shared_by_two_pages_survives_deletion_from_one() {
        let (mut doc, p) = n_page_doc(2);
        let id = add_link(&mut doc, p[0], goto(p[1]));
        let other = doc.get_dictionary(p[1]).unwrap().get(b"Annots").is_ok();
        assert!(!other);
        doc.get_dictionary_mut(p[1])
            .unwrap()
            .set("Annots", vec![Object::Reference(id)]);
        delete(&mut doc, 0, id).unwrap();
        assert!(doc.objects.contains_key(&id), "still used by page 2");
        let (links, _) = links::read_links(&doc, &PageIndex::build(&doc));
        assert_eq!(links.len(), 1, "page 2 keeps its link");
        // Deleting the last reference now removes the object.
        delete(&mut doc, 1, id).unwrap();
        assert!(!doc.objects.contains_key(&id));
    }

    #[test]
    fn a_link_also_referenced_from_an_outline_action_is_kept() {
        let (mut doc, p) = n_page_doc(2);
        let id = add_link(&mut doc, p[0], goto(p[1]));
        let mut forest = vec![node("B", Some(p[1]), false, vec![])];
        forest[0].dict.set("A", id);
        put(&mut doc, &forest);
        delete(&mut doc, 0, id).unwrap();
        assert!(doc.objects.contains_key(&id), "the outline /A still points at it");
    }

    #[test]
    fn a_page_dictionary_posing_as_a_link_is_never_removed() {
        let (mut doc, p) = n_page_doc(2);
        // Page 1's /Annots names page 2, whose dictionary also says /Subtype /Link.
        doc.get_dictionary_mut(p[1])
            .unwrap()
            .set("Subtype", Object::Name(b"Link".to_vec()));
        doc.get_dictionary_mut(p[0])
            .unwrap()
            .set("Annots", vec![Object::Reference(p[1])]);
        let _ = delete(&mut doc, 0, p[1]);
        assert!(doc.objects.contains_key(&p[1]), "the page object is untouched");
        assert_eq!(PageIndex::build(&doc).count(), 2);
    }

    #[test]
    fn undo_of_a_kept_shared_link_does_not_duplicate_it() {
        let (mut doc, p) = n_page_doc(2);
        let id = add_link(&mut doc, p[0], goto(p[1]));
        doc.get_dictionary_mut(p[1])
            .unwrap()
            .set("Annots", vec![Object::Reference(id)]);
        let mut hist = NavHistory::default();
        do_edit(
            &mut doc,
            &mut hist,
            NavEdit::LinkDelete {
                page: 0,
                id: links::link_id(id),
            },
        );
        undo_once(&mut doc, &mut hist);
        let a0 = doc
            .get_dictionary(p[0])
            .unwrap()
            .get(b"Annots")
            .unwrap()
            .as_array()
            .unwrap()
            .len();
        assert_eq!(a0, 1);
        assert!(doc.objects.contains_key(&id));
    }

    // ---- labels: a partial read is read-only -----------------------------------------

    fn deep_label_doc(levels: usize) -> Document {
        let (mut doc, _) = n_page_doc(3);
        let mut inner: Object = Object::Dictionary(
            dictionary! { "Nums" => vec![0.into(), label_dict(Some("D"), None, None)] },
        );
        for _ in 0..levels {
            inner = Object::Dictionary(dictionary! { "Kids" => vec![inner] });
        }
        doc.catalog_mut().unwrap().set("PageLabels", inner);
        doc
    }

    #[test]
    fn a_label_tree_cut_by_the_depth_cap_is_read_only() {
        let mut doc = deep_label_doc(20);
        let before = doc.catalog().unwrap().get(b"PageLabels").unwrap().clone();
        assert!(read_info(&doc).labels_read_only.is_some());
        for e in [
            NavEdit::LabelsNumber {
                pages: vec![0, 1, 2],
                style: Some(LabelStyle::Decimal),
                prefix: String::new(),
                start: 1,
            },
            NavEdit::LabelRename {
                page: 0,
                text: "x".into(),
            },
        ] {
            assert!(apply_edit(&mut doc, &e).is_err());
        }
        // A page operation does not rewrite the partial tree either.
        super::super::edit::carry_labels_delete(&mut doc, 0).unwrap();
        assert_eq!(doc.catalog().unwrap().get(b"PageLabels").unwrap(), &before);
        // A tree within the cap is editable.
        let ok = deep_label_doc(3);
        assert!(read_info(&ok).labels_read_only.is_none());
    }

    #[test]
    fn a_label_tree_cut_by_the_range_cap_is_read_only() {
        let (mut doc, _) = n_page_doc(2);
        let mut nums = Vec::new();
        for i in 0..(labels::MAX_LABEL_RANGES as i64 + 5) {
            nums.push(Object::Integer(i));
            nums.push(label_dict(None, Some("x"), None));
        }
        doc.catalog_mut()
            .unwrap()
            .set("PageLabels", dictionary! { "Nums" => nums });
        assert!(labels::read_only_reason(&doc).is_some());
        assert!(labels::read_ranges_for_edit(&doc).is_err());
    }

    // ---- mailto keeps RFC 6068 encoding ----------------------------------------------

    #[test]
    fn mailto_keeps_percent_encoding_and_never_turns_spaces_into_plus() {
        let out = validate_uri(b"mailto:a@b.example?subject=Hi%20there&body=A%2BB%20c").unwrap();
        assert_eq!(out, "mailto:a@b.example?subject=Hi%20there&body=A%2BB%20c");
        assert!(!out.contains('+'));
        // A literal plus stays a plus (it is not a space in a mailto).
        let out = validate_uri(b"mailto:a@b.example?subject=a+b").unwrap();
        assert_eq!(out, "mailto:a@b.example?subject=a%2Bb");
        // Normalising twice is stable.
        assert_eq!(validate_uri(out.as_bytes()).unwrap(), out);
    }

    // ---- hosts with underscores ------------------------------------------------------

    #[test]
    fn underscores_are_allowed_in_non_final_host_labels_only() {
        assert_eq!(
            validate_uri(b"https://a_b.example.com/x").unwrap(),
            "https://a_b.example.com/x"
        );
        assert!(validate_uri(b"https://build_box.corp.example/").is_ok());
        assert!(validate_uri(b"https://example.com_/").is_err());
        assert!(validate_uri(b"https://example._com/").is_err());
        assert!(validate_uri(b"https://localhost_/").is_err());
    }
}

/// Round 6: exhaustive, number-based reachability and no dangling references.
mod round6 {
    use super::hostile_references::dangling_refs as dangling;
    use super::*;

    #[test]
    fn a_kept_candidate_keeps_what_it_points_at() {
        // A is referenced from the catalog; its /Next is B. B must survive or A dangles.
        let (mut doc, p) = n_page_doc(2);
        put(
            &mut doc,
            &[
                node("A", Some(p[0]), false, vec![]),
                node("B", Some(p[1]), false, vec![]),
            ],
        );
        let ids = item_ids(&doc);
        doc.catalog_mut().unwrap().set("Custom", ids[0]);
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&ids[0]) && doc.objects.contains_key(&ids[1]));
        assert!(dangling(&doc).is_empty());
    }

    #[test]
    fn another_generation_of_the_same_number_keeps_the_candidate() {
        let (mut doc, p) = n_page_doc(1);
        put(&mut doc, &[node("A", Some(p[0]), false, vec![])]);
        let a = item_ids(&doc)[0];
        // A different generation of the same object number, referenced from the catalog.
        let twin = (a.0, a.1 + 1);
        doc.objects
            .insert(twin, Object::Dictionary(dictionary! { "Title" => Object::string_literal("twin") }));
        doc.catalog_mut().unwrap().set("Custom", twin);
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&a), "same number, other generation: kept");
        assert!(dangling(&doc).is_empty());
        // A reference to (n, other generation) alone also protects the candidate.
        let (mut doc, p) = n_page_doc(1);
        put(&mut doc, &[node("A", Some(p[0]), false, vec![])]);
        let a = item_ids(&doc)[0];
        doc.catalog_mut().unwrap().set("Custom", Object::Reference((a.0, 7)));
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&a));
    }

    #[test]
    fn only_objects_this_feature_owns_are_ever_deleted() {
        let (mut doc, p) = n_page_doc(2);
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1" });
        let cat = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let pages = doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
        let candidates = [p[0], p[1], font, cat, pages];
        // Detach the references the document itself has so only the type guard decides.
        let mut bare = Document::with_version("1.7");
        bare.objects = doc.objects.clone();
        assert!(super::super::unreferenced_among(&bare, &candidates).is_empty());
    }

    #[test]
    fn no_reference_points_at_a_deleted_object_after_any_delete_path() {
        // Outline replaced, links deleted, labels rewritten, undo of each.
        let (mut doc, p) = n_page_doc(3);
        put(
            &mut doc,
            &[node("A", Some(p[0]), false, vec![node("A1", Some(p[1]), false, vec![])]), node("B", Some(p[2]), false, vec![])],
        );
        let l1 = links::create_link(&mut doc, 0, [0.0, 0.0, 9.0, 9.0], &TargetInput::Page { page: 1, view: PageView::Fit }).unwrap();
        let shared = doc.add_object(Object::Dictionary(link_dict([0, 0, 5, 5], dest_to(p[1]))));
        for pg in [p[1], p[2]] {
            doc.get_dictionary_mut(pg).unwrap().set("Annots", vec![Object::Reference(shared)]);
        }
        let mut hist = NavHistory::default();
        do_edit(&mut doc, &mut hist, NavEdit::LinkDelete { page: 0, id: links::link_id(l1) });
        assert!(dangling(&doc).is_empty(), "link delete");
        do_edit(&mut doc, &mut hist, NavEdit::LinkDelete { page: 1, id: links::link_id(shared) });
        assert!(dangling(&doc).is_empty(), "shared link delete");
        do_edit(&mut doc, &mut hist, NavEdit::LabelRename { page: 1, text: "X".into() });
        assert!(dangling(&doc).is_empty(), "labels");
        put(&mut doc, &[node("N", Some(p[0]), false, vec![])]);
        assert!(dangling(&doc).is_empty(), "outline rewrite");
        put(&mut doc, &[]);
        assert!(dangling(&doc).is_empty(), "outline cleared");
        for _ in 0..3 {
            undo_once(&mut doc, &mut hist);
            assert!(dangling(&doc).is_empty(), "undo");
        }
        let reloaded = reload(&mut doc);
        assert!(dangling(&reloaded).is_empty(), "saved file");
    }
}

/// Round 7: generation twins in both directions, and the in-code post-delete check.
mod round7 {
    use super::hostile_references::dangling_refs as dangling;
    use super::*;

    fn two_items() -> (Document, ObjectId, ObjectId) {
        let (mut doc, p) = n_page_doc(2);
        put(
            &mut doc,
            &[
                node("A", Some(p[0]), false, vec![]),
                node("B", Some(p[1]), false, vec![]),
            ],
        );
        let ids = item_ids(&doc);
        (doc, ids[0], ids[1])
    }

    #[test]
    fn a_twin_generation_that_points_at_a_sibling_keeps_the_sibling() {
        // (a.0, 1) is a different object with A's number; it points at B. It is not a candidate,
        // so its reference is an outside reference and B must survive.
        let (mut doc, a, b) = two_items();
        let twin = (a.0, a.1 + 1);
        doc.objects.insert(
            twin,
            Object::Dictionary(dictionary! { "Title" => Object::string_literal("t"), "Next" => b }),
        );
        let before = dangling(&doc);
        // lopdf cannot write two generations of one number: judge the saved file against the
        // saved file of the untouched document, not against the in-memory view.
        let saved_before: std::collections::HashSet<_> =
            dangling(&reload(&mut doc.clone())).into_iter().collect();
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&twin));
        assert!(doc.objects.contains_key(&b), "B is pointed at by the surviving twin");
        assert_eq!(dangling(&doc), before, "no new dangling reference");
        let reloaded = reload(&mut doc);
        assert!(
            dangling(&reloaded).iter().all(|r| saved_before.contains(r)),
            "after save: nothing dangles that did not already dangle in the untouched file"
        );
    }

    #[test]
    fn a_reference_to_a_missing_generation_protects_the_candidate_without_new_dangling() {
        let (mut doc, a, b) = two_items();
        doc.catalog_mut().unwrap().set("Custom", Object::Reference((a.0, 9)));
        let before = dangling(&doc);
        assert!(before.contains(&(a.0, 9)), "the file already dangles there");
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&a), "kept: some generation of its number is referenced");
        assert!(doc.objects.contains_key(&b), "kept: A points at it");
        assert_eq!(dangling(&doc), before, "deleting added nothing");
        let reloaded = reload(&mut doc);
        assert_eq!(dangling(&reloaded), before);
    }

    #[test]
    fn delete_unreferenced_undoes_itself_when_it_cannot_prove_the_result() {
        let (mut doc, _a, _b) = two_items();
        put(&mut doc, &[]);
        // Nothing references these now; with a real budget they go, with none they stay.
        let (mut doc2, _a, _b) = two_items();
        let read = outline::read_outline(&doc2);
        let mut ids = read.item_ids.clone();
        ids.extend(read.root_id);
        doc2.catalog_mut().unwrap().remove(b"Outlines");
        assert!(super::super::delete_unreferenced_with_budget(&mut doc2, &ids, 2).is_empty());
        assert!(ids.iter().all(|i| doc2.objects.contains_key(i)));
        let gone = super::super::delete_unreferenced(&mut doc2, &ids);
        assert_eq!(gone.len(), ids.len());
        assert!(dangling(&doc2).is_empty());
    }

    #[test]
    fn a_candidate_kept_for_its_number_keeps_what_it_points_at() {
        // A=(n,0) has a twin (n,1) referenced from the catalog; A /Next -> B. A is kept, so B is
        // too, or A dangles. Outside refs to each generation in turn.
        for twin_gen in [0u16, 1] {
            let (mut doc, a, b) = two_items();
            let twin = (a.0, a.1 + 1);
            doc.objects.insert(
                twin,
                Object::Dictionary(dictionary! { "Title" => Object::string_literal("t") }),
            );
            let target = if twin_gen == 0 { a } else { twin };
            doc.catalog_mut().unwrap().set("Custom", target);
            let before = dangling(&doc);
            put(&mut doc, &[]);
            assert!(doc.objects.contains_key(&a), "gen {twin_gen}: A kept");
            assert!(doc.objects.contains_key(&b), "gen {twin_gen}: B kept because A points at it");
            assert_eq!(dangling(&doc), before, "gen {twin_gen}: nothing new dangles");
        }
        // No outside reference at all: the twin alone keeps A, and A keeps B.
        let (mut doc, a, b) = two_items();
        doc.objects.insert(
            (a.0, a.1 + 1),
            Object::Dictionary(dictionary! { "Title" => Object::string_literal("t") }),
        );
        let before = dangling(&doc);
        // The reachability answer itself (before the rollback safety net) must already keep B.
        let read = outline::read_outline(&doc);
        let mut ids = read.item_ids.clone();
        ids.extend(read.root_id);
        doc.catalog_mut().unwrap().remove(b"Outlines");
        let clear = super::super::unreferenced_among(&doc, &ids);
        assert!(!clear.contains(&a) && !clear.contains(&b), "{clear:?}");
        put(&mut doc, &[]);
        assert!(doc.objects.contains_key(&a) && doc.objects.contains_key(&b));
        assert_eq!(dangling(&doc), before);
    }

    #[test]
    fn a_candidate_refused_by_the_type_guard_keeps_a_deletable_target() {
        // X is page-shaped (Contents, no Title) so it is never deleted; it points at Y, which
        // would otherwise be deletable.
        let (mut doc, p) = n_page_doc(1);
        let y = doc.add_object(dictionary! { "Title" => Object::string_literal("y") });
        let x = doc.add_object(dictionary! { "Contents" => Object::Reference(y), "Subtype" => "Link" });
        let _ = p;
        assert!(
            super::super::unreferenced_among(&doc, &[x, y]).is_empty(),
            "the reachability answer itself keeps both"
        );
        let gone = super::super::delete_unreferenced(&mut doc, &[x, y]);
        assert!(gone.is_empty(), "X is not ours and keeps Y alive: {gone:?}");
        assert!(doc.objects.contains_key(&x) && doc.objects.contains_key(&y));
        assert!(dangling(&doc).is_empty());
        let reloaded = reload(&mut doc);
        assert!(dangling(&reloaded).is_empty());
    }
}
