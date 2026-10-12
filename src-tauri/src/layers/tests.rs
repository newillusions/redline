//! Tests for `layers`: the model reader (incl. hostile input), the view overlay (byte
//! level, and rendered by PDFium when `PDFIUM_DYNAMIC_LIB_PATH` is set), and the edits.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use super::edit::{add_layer, set_default_state};
use super::overlay::OverlayBase;
use super::*;

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

fn name(n: &str) -> Object {
    Object::Name(n.as_bytes().to_vec())
}

fn refs(ids: &[ObjectId]) -> Object {
    Object::Array(ids.iter().map(|i| Object::Reference(*i)).collect())
}

fn ocg(doc: &mut Document, n: &str) -> ObjectId {
    doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal(n) })
}

/// A 200x100 page: a red square (x 0..100) on layer "Red" and a blue one (x 100..200) on
/// layer "Blue". The caller installs `/OCProperties`.
pub(crate) struct Fx {
    pub doc: Document,
    pub red: ObjectId,
    pub blue: ObjectId,
}

pub(crate) fn fixture() -> Fx {
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let red = ocg(&mut doc, "Red");
    let blue = ocg(&mut doc, "Blue");
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"/OC /L1 BDC 1 0 0 rg 0 0 100 100 re f EMC /OC /L2 BDC 0 0 1 rg 100 0 100 100 re f EMC"
            .to_vec(),
    ));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
        "Contents" => content,
        "Resources" => dictionary! { "Properties" => dictionary! { "L1" => red, "L2" => blue } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
        ),
    );
    let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", root);
    Fx { doc, red, blue }
}

impl Fx {
    /// Install `/OCProperties << /OCGs [red blue] /D d >>` directly in the catalog.
    fn with_d(mut self, d: Dictionary) -> Self {
        let ocgs = refs(&[self.red, self.blue]);
        self.doc
            .catalog_mut()
            .unwrap()
            .set("OCProperties", dictionary! { "OCGs" => ocgs, "D" => d });
        self
    }

    fn save_classic(&mut self, dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("classic.pdf");
        // lopdf writes a cross-reference stream unless told to write a table.
        self.doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
        self.doc.save(&p).unwrap();
        p
    }

    fn save_plain_stream(&mut self, dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("plain-stream.pdf");
        self.doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceStream;
        self.doc.save(&p).unwrap();
        p
    }

    fn save_xref_stream(&mut self, dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("stream.pdf");
        let mut f = std::fs::File::create(&p).unwrap();
        self.doc.save_modern(&mut f).unwrap();
        p
    }
}

fn model_of(path: &std::path::Path) -> LayerModel {
    read_model(&Document::load(path).unwrap())
}

fn off(ids: &[ObjectId]) -> BTreeSet<u32> {
    ids.iter().map(|i| i.0).collect()
}

fn layer(m: &LayerModel, id: ObjectId) -> &Layer {
    m.layers.iter().find(|l| l.id == id.0).unwrap()
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

#[test]
fn model_reads_names_and_the_default_configuration() {
    let fx = fixture();
    let blue = fx.blue;
    let fx = fx.with_d(dictionary! { "OFF" => refs(&[blue]) });
    let m = read_model(&fx.doc);
    assert!(m.has_ocproperties);
    assert_eq!(m.layers.len(), 2);
    assert_eq!(layer(&m, fx.red).name, "Red");
    assert!(layer(&m, fx.red).default_on);
    assert!(!layer(&m, fx.blue).default_on);
}

#[test]
fn model_base_state_off_turns_on_only_the_on_list() {
    let fx = fixture();
    let red = fx.red;
    let fx = fx.with_d(dictionary! { "BaseState" => name("OFF"), "ON" => refs(&[red]) });
    let m = read_model(&fx.doc);
    assert!(layer(&m, fx.red).default_on);
    assert!(!layer(&m, fx.blue).default_on);
}

#[test]
fn model_no_optional_content_is_an_empty_model() {
    let m = read_model(&fixture().doc);
    assert!(!m.has_ocproperties);
    assert!(m.layers.is_empty() && m.tree.is_empty() && m.ocmds.is_empty());
}

#[test]
fn model_locked_and_design_intent_are_flagged() {
    let mut fx = fixture();
    fx.doc
        .get_dictionary_mut(fx.blue)
        .unwrap()
        .set("Intent", name("Design"));
    let red = fx.red;
    let fx = fx.with_d(dictionary! { "Locked" => refs(&[red]) });
    let m = read_model(&fx.doc);
    assert!(layer(&m, fx.red).locked);
    assert!(!layer(&m, fx.blue).locked);
    assert!(layer(&m, fx.red).view_intent);
    assert!(!layer(&m, fx.blue).view_intent);
}

#[test]
fn model_usage_view_state_outranks_the_default_configuration() {
    let mut fx = fixture();
    fx.doc.get_dictionary_mut(fx.blue).unwrap().set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => name("OFF") } },
    );
    let fx = fx.with_d(dictionary! {});
    let m = read_model(&fx.doc);
    assert_eq!(layer(&m, fx.blue).usage_view_state, Some(false));
    assert!(!layer(&m, fx.blue).default_on);
}

#[test]
fn model_order_gives_labelled_groups_and_sublayers_and_appends_unlisted() {
    let mut fx = fixture();
    let sub = ocg(&mut fx.doc, "Sub");
    let lone = ocg(&mut fx.doc, "Lone");
    let (red, blue) = (fx.red, fx.blue);
    // Order: [ (Folder) red [ sub ] ]  and  blue with no children; `lone` is not listed.
    let order = Object::Array(vec![
        Object::Array(vec![
            Object::String(b"Folder".to_vec(), StringFormat::Literal),
            Object::Reference(red),
            Object::Array(vec![Object::Reference(sub)]),
        ]),
        Object::Reference(blue),
    ]);
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! {
            "OCGs" => refs(&[red, blue, sub, lone]),
            "D" => dictionary! { "Order" => order },
        },
    );
    let m = read_model(&fx.doc);
    assert_eq!(
        m.tree,
        vec![
            LayerNode::Group {
                label: "Folder".into(),
                children: vec![LayerNode::Layer {
                    id: red.0,
                    children: vec![LayerNode::Layer {
                        id: sub.0,
                        children: vec![]
                    }]
                }]
            },
            LayerNode::Layer {
                id: blue.0,
                children: vec![]
            },
            LayerNode::Layer {
                id: lone.0,
                children: vec![]
            },
        ]
    );
    assert_eq!(m.unlisted, 1);
}

#[test]
fn model_ocmd_policy_and_visibility_expression() {
    let mut fx = fixture();
    let (red, blue) = (fx.red, fx.blue);
    let ocmd = fx.doc.add_object(dictionary! {
        "Type" => "OCMD", "OCGs" => refs(&[red, blue]), "P" => name("AllOn")
    });
    let ve = fx.doc.add_object(dictionary! {
        "Type" => "OCMD",
        "VE" => Object::Array(vec![
            name("Or"),
            Object::Reference(red),
            Object::Array(vec![name("Not"), Object::Reference(blue)]),
        ])
    });
    let fx = fx.with_d(dictionary! {});
    let m = read_model(&fx.doc);
    let a = m.ocmds.iter().find(|o| o.id == ocmd.0).unwrap();
    assert_eq!(
        (a.policy.as_str(), a.ocgs.clone()),
        ("AllOn", vec![red.0, blue.0])
    );
    let b = m.ocmds.iter().find(|o| o.id == ve.0).unwrap();
    assert_eq!(b.policy, "AnyOn", "P defaults to AnyOn");
    assert_eq!(
        b.ve,
        Some(OcExpr::Or(vec![
            OcExpr::Group(red.0),
            OcExpr::Not(Box::new(OcExpr::Group(blue.0)))
        ]))
    );
}

// --- hostile input ---------------------------------------------------------

#[test]
fn hostile_order_that_references_itself_terminates() {
    let mut fx = fixture();
    let (red, blue) = (fx.red, fx.blue);
    let a = fx.doc.new_object_id();
    // a = [ red a blue ]: the array contains a reference to itself.
    fx.doc.objects.insert(
        a,
        Object::Array(vec![
            Object::Reference(red),
            Object::Reference(a),
            Object::Reference(blue),
        ]),
    );
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! { "OCGs" => refs(&[red, blue]), "D" => dictionary! { "Order" => Object::Reference(a) } },
    );
    let m = read_model(&fx.doc);
    let mut ids = Vec::new();
    fn collect(n: &LayerNode, ids: &mut Vec<u32>) {
        if let LayerNode::Layer { id, children } = n {
            ids.push(*id);
            children.iter().for_each(|c| collect(c, ids));
        }
    }
    m.tree.iter().for_each(|n| collect(n, &mut ids));
    ids.sort_unstable();
    assert_eq!(ids, vec![red.0, blue.0], "each layer is listed once");
    assert!(m.truncated, "the cut cycle is reported");
}

#[test]
fn hostile_deeply_nested_order_is_cut_at_the_depth_bound() {
    let mut fx = fixture();
    let (red, blue) = (fx.red, fx.blue);
    let mut nested = Object::Array(vec![Object::Reference(blue)]);
    for _ in 0..200 {
        nested = Object::Array(vec![nested]);
    }
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! {
            "OCGs" => refs(&[red, blue]),
            "D" => dictionary! { "Order" => Object::Array(vec![Object::Reference(red), nested]) }
        },
    );
    let m = read_model(&fx.doc);
    assert!(m.truncated);
    // Both layers are still listed (the deep one falls back to the appended list).
    assert_eq!(m.layers.len(), 2);
    let mut seen = BTreeSet::new();
    fn walk(n: &LayerNode, seen: &mut BTreeSet<u32>) {
        match n {
            LayerNode::Layer { id, children } => {
                assert!(seen.insert(*id), "a layer is listed once");
                children.iter().for_each(|c| walk(c, seen));
            }
            LayerNode::Group { children, .. } => children.iter().for_each(|c| walk(c, seen)),
        }
    }
    m.tree.iter().for_each(|n| walk(n, &mut seen));
    assert_eq!(seen.len(), 2);
}

#[test]
fn hostile_ocgs_entries_that_are_not_groups_are_skipped() {
    let mut fx = fixture();
    let notdict = fx.doc.add_object(Object::Integer(7));
    let (red, blue) = (fx.red, fx.blue);
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! {
            "OCGs" => Object::Array(vec![
                Object::Integer(3), Object::Reference((9999, 0)), Object::Reference(notdict),
                Object::Reference(red), Object::Reference(red), Object::Reference(blue),
            ]),
            "D" => dictionary! {}
        },
    );
    let m = read_model(&fx.doc);
    assert_eq!(
        m.layers.iter().map(|l| l.id).collect::<Vec<_>>(),
        vec![red.0, blue.0]
    );
}

#[test]
fn hostile_names_are_clean_bounded_text() {
    let mut fx = fixture();
    fx.doc.get_dictionary_mut(fx.red).unwrap().set(
        "Name",
        Object::String(
            format!("<b>A\u{0}\u{7}B\n{}</b>", "x".repeat(2000)).into_bytes(),
            StringFormat::Literal,
        ),
    );
    fx.doc.get_dictionary_mut(fx.blue).unwrap().remove(b"Name");
    let fx = fx.with_d(dictionary! {});
    let m = read_model(&fx.doc);
    let n = &layer(&m, fx.red).name;
    assert!(n.chars().count() <= MAX_NAME_CHARS);
    assert!(
        n.starts_with("<b>AB"),
        "controls are dropped, markup kept as plain text: {n}"
    );
    assert!(!n.chars().any(|c| c.is_control()));
    assert_eq!(layer(&m, fx.blue).name, "(unnamed layer)");
}

#[test]
fn hostile_more_groups_than_the_bound_are_truncated() {
    let mut fx = fixture();
    let mut all = vec![fx.red, fx.blue];
    for i in 0..(MAX_LAYERS + 50) {
        all.push(ocg(&mut fx.doc, &format!("L{i}")));
    }
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! { "OCGs" => refs(&all), "D" => dictionary! {} },
    );
    let m = read_model(&fx.doc);
    assert_eq!(m.layers.len(), MAX_LAYERS);
    assert!(m.truncated);
}

#[test]
fn hostile_visibility_expression_cycles_and_depth_are_bounded() {
    let mut fx = fixture();
    // An indirect array that contains itself, and a very deep direct expression.
    let a = fx.doc.new_object_id();
    fx.doc
        .objects
        .insert(a, Object::Array(vec![name("And"), Object::Reference(a)]));
    let cyc = fx
        .doc
        .add_object(dictionary! { "Type" => "OCMD", "VE" => Object::Reference(a) });
    let mut deep = Object::Reference(fx.red);
    for _ in 0..100 {
        deep = Object::Array(vec![name("Not"), deep]);
    }
    let dp = fx
        .doc
        .add_object(dictionary! { "Type" => "OCMD", "VE" => deep });
    let fx = fx.with_d(dictionary! {});
    let m = read_model(&fx.doc);
    assert_eq!(m.ocmds.iter().find(|o| o.id == cyc.0).unwrap().ve, None);
    assert_eq!(m.ocmds.iter().find(|o| o.id == dp.0).unwrap().ve, None);
}

#[test]
fn hostile_malformed_vectors_do_not_panic() {
    let mut fx = fixture();
    // /OCProperties of the wrong type, /D a number, /Order a string, /OCMD /OCGs garbage.
    fx.doc
        .catalog_mut()
        .unwrap()
        .set("OCProperties", Object::Integer(1));
    let _ = read_model(&fx.doc);
    let red = fx.red;
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! { "OCGs" => refs(&[red]), "D" => Object::Integer(5) },
    );
    let _ = read_model(&fx.doc);
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! {
            "OCGs" => Object::String(b"x".to_vec(), StringFormat::Literal),
            "D" => dictionary! { "Order" => Object::String(b"x".to_vec(), StringFormat::Literal),
                                  "OFF" => Object::Integer(1), "RBGroups" => Object::Integer(2) }
        },
    );
    fx.doc.add_object(dictionary! { "Type" => "OCMD", "OCGs" => Object::Integer(1), "VE" => Object::Integer(2), "P" => Object::Integer(3) });
    let m = read_model(&fx.doc);
    assert!(m.layers.is_empty());
}

// ---------------------------------------------------------------------------
// The overlay (bytes)
// ---------------------------------------------------------------------------

fn overlay_model(path: &std::path::Path, off_ids: &BTreeSet<u32>) -> LayerModel {
    let doc = Document::load(path).unwrap();
    let model = read_model(&doc);
    let base = OverlayBase::new(&doc, &model, path).unwrap();
    let mut bytes = std::fs::read(path).unwrap();
    bytes.extend_from_slice(&base.tail(off_ids).unwrap());
    read_model(&Document::load_mem(&bytes).expect("the base file plus the update must parse"))
}

fn hides_blue_only(m: &LayerModel, fx_blue: ObjectId, fx_red: ObjectId) {
    assert!(layer(m, fx_red).default_on);
    assert!(!layer(m, fx_blue).default_on);
}

#[test]
fn overlay_classic_xref_hides_the_chosen_group_and_keeps_the_rest() {
    let mut fx = fixture().with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    assert!(matches!(
        Document::load(&p)
            .unwrap()
            .reference_table
            .cross_reference_type,
        lopdf::xref::XrefType::CrossReferenceTable
    ));
    let m = overlay_model(&p, &off(&[fx.blue]));
    hides_blue_only(&m, fx.blue, fx.red);
}

#[test]
fn overlay_xref_stream_file_gets_an_xref_stream_update() {
    let mut fx = fixture().with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_xref_stream(dir.path());
    assert!(matches!(
        Document::load(&p)
            .unwrap()
            .reference_table
            .cross_reference_type,
        lopdf::xref::XrefType::CrossReferenceStream
    ));
    let m = overlay_model(&p, &off(&[fx.blue]));
    hides_blue_only(&m, fx.blue, fx.red);
}

#[test]
fn overlay_can_show_a_group_the_file_hides() {
    let fx = fixture();
    let blue = fx.blue;
    let mut fx = fx.with_d(dictionary! { "BaseState" => name("OFF"), "ON" => refs(&[]) });
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    assert!(!layer(&model_of(&p), blue).default_on);
    let m = overlay_model(&p, &off(&[fx.red]));
    assert!(
        layer(&m, fx.blue).default_on,
        "BaseState is dropped, so unlisted groups are on"
    );
    assert!(!layer(&m, fx.red).default_on);
}

#[test]
fn overlay_works_when_d_and_ocproperties_are_indirect() {
    for variant in 0..3 {
        let mut fx = fixture();
        let (red, blue) = (fx.red, fx.blue);
        let d = dictionary! { "OFF" => refs(&[red]) };
        let ocgs = refs(&[red, blue]);
        match variant {
            0 => {
                let d_id = fx.doc.add_object(d);
                fx.doc
                    .catalog_mut()
                    .unwrap()
                    .set("OCProperties", dictionary! { "OCGs" => ocgs, "D" => d_id });
            }
            1 => {
                let p = fx.doc.add_object(dictionary! { "OCGs" => ocgs, "D" => d });
                fx.doc.catalog_mut().unwrap().set("OCProperties", p);
            }
            _ => {
                let d_id = fx.doc.add_object(d);
                let p = fx
                    .doc
                    .add_object(dictionary! { "OCGs" => ocgs, "D" => d_id });
                fx.doc.catalog_mut().unwrap().set("OCProperties", p);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let p = fx.save_classic(dir.path());
        let m = overlay_model(&p, &off(&[blue]));
        assert!(
            layer(&m, red).default_on,
            "variant {variant}: red shown by the view"
        );
        assert!(
            !layer(&m, blue).default_on,
            "variant {variant}: blue hidden by the view"
        );
    }
}

#[test]
fn overlay_rewrites_a_group_whose_view_state_disagrees() {
    let mut fx = fixture();
    fx.doc.get_dictionary_mut(fx.blue).unwrap().set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => name("OFF") } },
    );
    let mut fx = fx.with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    assert!(!layer(&model_of(&p), fx.blue).default_on);
    let m = overlay_model(&p, &BTreeSet::new());
    assert!(layer(&m, fx.blue).default_on, "view state brought to ON");
    assert_eq!(layer(&m, fx.blue).usage_view_state, Some(true));
}

#[test]
fn overlay_refuses_files_it_cannot_extend() {
    let mut fx = fixture().with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let doc = Document::load(&p).unwrap();
    let model = read_model(&doc);

    // Data before the header.
    let junk = dir.path().join("junk.pdf");
    let mut b = b"JUNK\n".to_vec();
    b.extend(std::fs::read(&p).unwrap());
    std::fs::write(&junk, b).unwrap();
    assert!(OverlayBase::new(&doc, &model, &junk).is_err());

    // No optional content.
    let plain = fixture();
    let mut plain = plain;
    let pp = dir.path().join("plain.pdf");
    plain.doc.save(&pp).unwrap();
    let d2 = Document::load(&pp).unwrap();
    assert!(OverlayBase::new(&d2, &read_model(&d2), &pp).is_err());
}

#[test]
fn overlay_reader_matches_the_concatenated_bytes() {
    let mut fx = fixture().with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let doc = Document::load(&p).unwrap();
    let base = OverlayBase::new(&doc, &read_model(&doc), &p).unwrap();
    let want = {
        let mut v = std::fs::read(&p).unwrap();
        v.extend(base.tail(&off(&[fx.blue])).unwrap());
        v
    };
    let mut r = base.reader(&off(&[fx.blue])).unwrap();
    assert_eq!(r.len(), want.len() as u64);
    let mut all = Vec::new();
    r.read_to_end(&mut all).unwrap();
    assert_eq!(all, want);
    // Random access, across the file/update seam.
    for (start, len) in [
        (0usize, 10usize),
        (want.len() - 20, 20),
        (std::fs::metadata(&p).unwrap().len() as usize - 5, 12),
        (3, 1),
    ] {
        r.seek(SeekFrom::Start(start as u64)).unwrap();
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(buf, &want[start..start + len]);
    }
    assert_eq!(r.seek(SeekFrom::End(-4)).unwrap(), want.len() as u64 - 4);
    assert!(r.seek(SeekFrom::Current(-(want.len() as i64) * 2)).is_err());
    // A changed file is refused.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .set_len(want.len() as u64)
        .ok();
    assert!(base.reader(&BTreeSet::new()).is_err());
}

#[test]
fn write_object_escapes_names_and_hex_encodes_strings() {
    use super::overlay::write_object_for_test as w;
    assert_eq!(w(&name("A B#(")).unwrap(), b"/A#20B#23#28".to_vec());
    assert_eq!(
        w(&Object::String(b"a)\\".to_vec(), StringFormat::Literal)).unwrap(),
        b"<61295C>".to_vec()
    );
    assert_eq!(w(&Object::Real(1.5)).unwrap(), b"1.5".to_vec());
    assert_eq!(w(&Object::Real(2.0)).unwrap(), b"2".to_vec());
    assert!(w(&Object::Real(f32::NAN)).is_err());
    assert!(w(&Object::Stream(Stream::new(Dictionary::new(), vec![]))).is_err());
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

fn reload(doc: &mut Document, dir: &std::path::Path) -> LayerModel {
    let p = dir.join("edited.pdf");
    doc.save(&p).unwrap();
    model_of(&p)
}

#[test]
fn add_layer_to_a_file_with_no_layers_creates_the_structure() {
    let mut fx = fixture();
    let id = add_layer(&mut fx.doc, "Lighting", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert_eq!(m.layers.len(), 1);
    assert_eq!(
        (
            m.layers[0].id,
            m.layers[0].name.as_str(),
            m.layers[0].default_on
        ),
        (id, "Lighting", true)
    );
    assert_eq!(
        m.tree,
        vec![LayerNode::Layer {
            id,
            children: vec![]
        }]
    );
}

#[test]
fn add_layer_keeps_existing_layers_listed_when_order_was_absent() {
    let mut fx = fixture().with_d(dictionary! {});
    let id = add_layer(&mut fx.doc, "New", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert_eq!(m.unlisted, 0, "Order now lists every group");
    assert_eq!(m.layers.len(), 3);
    let order: Vec<u32> = m
        .tree
        .iter()
        .filter_map(|n| {
            if let LayerNode::Layer { id, .. } = n {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(order, vec![fx.red.0, fx.blue.0, id]);
}

#[test]
fn add_layer_as_a_child_goes_under_its_parent() {
    let mut fx = fixture().with_d(dictionary! {});
    let a = add_layer(&mut fx.doc, "Parent", None).unwrap();
    let b = add_layer(&mut fx.doc, "Child", Some(a)).unwrap();
    let c = add_layer(&mut fx.doc, "Child2", Some(a)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    let parent = m
        .tree
        .iter()
        .find_map(|n| match n {
            LayerNode::Layer { id, children } if *id == a => Some(children.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        parent,
        vec![
            LayerNode::Layer {
                id: b,
                children: vec![]
            },
            LayerNode::Layer {
                id: c,
                children: vec![]
            }
        ]
    );
}

#[test]
fn add_layer_with_base_state_off_is_on() {
    let mut fx = fixture().with_d(dictionary! { "BaseState" => name("OFF") });
    let id = add_layer(&mut fx.doc, "New", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert!(m.layers.iter().find(|l| l.id == id).unwrap().default_on);
    assert!(
        !layer(&m, fx.red).default_on,
        "existing layers keep their state"
    );
}

#[test]
fn add_layer_cleans_the_name_and_refuses_an_empty_one() {
    let mut fx = fixture();
    assert!(add_layer(&mut fx.doc, " \u{0}\n ", None).is_err());
    let id = add_layer(&mut fx.doc, "  A\u{7}B  ", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert_eq!(m.layers.iter().find(|l| l.id == id).unwrap().name, "AB");
}

#[test]
fn add_layer_works_through_indirect_order_ocgs_and_d() {
    let mut fx = fixture();
    let (red, blue) = (fx.red, fx.blue);
    let order = fx.doc.add_object(refs(&[red, blue]));
    let ocgs = fx.doc.add_object(refs(&[red, blue]));
    let d = fx.doc.add_object(dictionary! { "Order" => order });
    fx.doc
        .catalog_mut()
        .unwrap()
        .set("OCProperties", dictionary! { "OCGs" => ocgs, "D" => d });
    let id = add_layer(&mut fx.doc, "New", None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert_eq!(m.layers.len(), 3);
    assert!(m
        .tree
        .iter()
        .any(|n| matches!(n, LayerNode::Layer { id: i, .. } if *i == id)));
}

#[test]
fn save_default_state_writes_the_off_list_and_leaves_locked_layers_alone() {
    let mut fx = fixture();
    let (red, blue) = (fx.red, fx.blue);
    fx.doc.get_dictionary_mut(blue).unwrap().set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => name("OFF") } },
    );
    let mut fx = fx.with_d(dictionary! { "Locked" => refs(&[red]), "OFF" => refs(&[red]) });
    // The user's view: red on, blue on. Red is locked, so it keeps its (off) default.
    set_default_state(&mut fx.doc, &BTreeSet::new()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert!(!layer(&m, red).default_on, "locked layer unchanged");
    assert!(
        layer(&m, blue).default_on,
        "blue's ViewState was brought to ON"
    );
    // And hiding blue is saved too.
    set_default_state(&mut fx.doc, &off(&[blue])).unwrap();
    let m = reload(&mut fx.doc, dir.path());
    assert!(!layer(&m, blue).default_on);
}

// ---------------------------------------------------------------------------
// PDFium (what is actually drawn). Gated like the other PDFium tests.
// ---------------------------------------------------------------------------

fn pdfium_available() -> bool {
    if std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
        eprintln!("skip: no PDFIUM_DYNAMIC_LIB_PATH");
        return false;
    }
    true
}

fn px(r: &crate::render::PageRaster, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * r.width_px + x) * 3) as usize;
    [r.rgb[i], r.rgb[i + 1], r.rgb[i + 2]]
}

/// (left square, right square) colours as drawn at 72 dpi.
fn shown(e: &mut crate::render::RenderEngine) -> ([u8; 3], [u8; 3]) {
    let r = e.render_page_full("d", 0, 72.0).unwrap();
    (px(&r, 50, 50), px(&r, 150, 50))
}

fn engine_with(p: &std::path::Path) -> crate::render::RenderEngine {
    let mut e = crate::render::RenderEngine::new().expect("pdfium");
    e.open_document(p.to_path_buf(), "d".into(), None).unwrap();
    e
}

fn view(e: &mut crate::render::RenderEngine, p: &std::path::Path, off_ids: &[ObjectId]) {
    let doc = Document::load(p).unwrap();
    let base = OverlayBase::new(&doc, &read_model(&doc), p).unwrap();
    e.set_layer_view("d", Some(base.reader(&off(off_ids)).unwrap()))
        .unwrap();
}

#[test]
fn pdfium_honours_the_default_configuration_and_the_overlay_both_xref_kinds() {
    if !pdfium_available() {
        return;
    }
    for stream in 0..3 {
        let mut fx = fixture().with_d(dictionary! {});
        let dir = tempfile::tempdir().unwrap();
        let p = match stream {
            0 => fx.save_classic(dir.path()),
            1 => fx.save_plain_stream(dir.path()),
            _ => fx.save_xref_stream(dir.path()),
        };
        let mut e = engine_with(&p);
        assert_eq!(
            shown(&mut e),
            (RED, BLUE),
            "stream={stream}: both shown by default"
        );
        view(&mut e, &p, &[fx.blue]);
        assert_eq!(
            shown(&mut e),
            (RED, WHITE),
            "stream={stream}: blue hidden by the view"
        );
        view(&mut e, &p, &[fx.red]);
        assert_eq!(
            shown(&mut e),
            (WHITE, BLUE),
            "stream={stream}: red hidden by the view"
        );
        view(&mut e, &p, &[fx.red, fx.blue]);
        assert_eq!(
            shown(&mut e),
            (WHITE, WHITE),
            "stream={stream}: both hidden"
        );
        e.set_layer_view("d", None).unwrap();
        assert_eq!(
            shown(&mut e),
            (RED, BLUE),
            "stream={stream}: back to the file as it is"
        );
    }
}

#[test]
fn pdfium_overlay_can_show_what_the_file_hides() {
    if !pdfium_available() {
        return;
    }
    let fx = fixture();
    let blue = fx.blue;
    let mut fx = fx.with_d(dictionary! { "OFF" => refs(&[blue]) });
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let mut e = engine_with(&p);
    assert_eq!(shown(&mut e), (RED, WHITE), "PDFium honours /D /OFF");
    view(&mut e, &p, &[]);
    assert_eq!(shown(&mut e), (RED, BLUE));
}

#[test]
fn pdfium_view_state_outranks_the_configuration_and_the_overlay_overrides_it() {
    if !pdfium_available() {
        return;
    }
    let mut fx = fixture();
    fx.doc.get_dictionary_mut(fx.blue).unwrap().set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => name("OFF") } },
    );
    let mut fx = fx.with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let mut e = engine_with(&p);
    assert_eq!(
        shown(&mut e),
        (RED, WHITE),
        "ViewState OFF hides although /D lists nothing"
    );
    assert!(
        !layer(&model_of(&p), fx.blue).default_on,
        "the model agrees with the renderer"
    );
    view(&mut e, &p, &[]);
    assert_eq!(shown(&mut e), (RED, BLUE), "the overlay turns it on");
    view(&mut e, &p, &[fx.red]);
    assert_eq!(shown(&mut e), (WHITE, BLUE));
}

#[test]
fn pdfium_view_state_on_outranks_d_off() {
    if !pdfium_available() {
        return;
    }
    let mut fx = fixture();
    let blue = fx.blue;
    fx.doc.get_dictionary_mut(blue).unwrap().set(
        "Usage",
        dictionary! { "View" => dictionary! { "ViewState" => name("ON") } },
    );
    let mut fx = fx.with_d(dictionary! { "OFF" => refs(&[blue]) });
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let mut e = engine_with(&p);
    let (_, right) = shown(&mut e);
    let model_says = layer(&model_of(&p), blue).default_on;
    assert_eq!(
        right == BLUE,
        model_says,
        "the model's default for this group matches what is drawn"
    );
    view(&mut e, &p, &[blue]);
    assert_eq!(shown(&mut e).1, WHITE, "hidden by the view regardless");
}

#[test]
fn pdfium_design_intent_groups_do_not_hide() {
    if !pdfium_available() {
        return;
    }
    let mut fx = fixture();
    let blue = fx.blue;
    fx.doc
        .get_dictionary_mut(blue)
        .unwrap()
        .set("Intent", name("Design"));
    let mut fx = fx.with_d(dictionary! { "OFF" => refs(&[blue]) });
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let mut e = engine_with(&p);
    let (_, right) = shown(&mut e);
    let m = model_of(&p);
    assert!(!layer(&m, blue).view_intent);
    assert_eq!(right, BLUE, "a Design-intent group is always drawn");
}

#[test]
fn pdfium_a_failed_view_keeps_the_current_one() {
    if !pdfium_available() {
        return;
    }
    let mut fx = fixture().with_d(dictionary! {});
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let mut e = engine_with(&p);
    view(&mut e, &p, &[fx.blue]);
    assert_eq!(shown(&mut e), (RED, WHITE));
    // A reader over a truncated stream cannot load: the previous view stays.
    let bad = dir.path().join("bad.pdf");
    std::fs::write(&bad, b"%PDF-1.4\nnot a pdf").unwrap();
    let doc = Document::load(&p).unwrap();
    let base = OverlayBase::new(&doc, &read_model(&doc), &p).unwrap();
    // Point the base at the bad file by building the reader from it directly.
    let mut broken = base.clone();
    broken.path = bad;
    assert!(broken.reader(&BTreeSet::new()).is_err());
    assert_eq!(shown(&mut e), (RED, WHITE));
}

// ---------------------------------------------------------------------------
// Mark-ups on layers (the /OC reference through read -> write -> save cycles)
// ---------------------------------------------------------------------------

mod on_layers {
    use super::*;
    use crate::document::annots::{read_markups, write_markups};
    use crate::geometry::PdfPoint;
    use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupType, OptionalContent, UserRef};

    fn layered() -> Fx {
        let fx = fixture();
        let (red, blue) = (fx.red, fx.blue);
        let order = refs(&[red, blue]);
        fx.with_d(dictionary! { "OFF" => refs(&[blue]), "Order" => order })
    }

    fn markup_on(layer_ref: Option<ObjectId>) -> Markup {
        let mut m = Markup::new(
            MarkupType::Cloud,
            0,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 10.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 10.0 },
                PdfPoint { x: 50.0, y: 40.0 },
            ]),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        if let Some(r) = layer_ref {
            m.optional_content = Some(OptionalContent::Reference(r.0, r.1));
            m.layer = Some("Red".into());
        }
        m
    }

    fn add_foreign_square(fx: &mut Fx, layer_ref: ObjectId) -> ObjectId {
        let annot = fx.doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square",
            "Rect" => vec![10.into(), 10.into(), 60.into(), 60.into()],
            "C" => vec![1.into(), 0.into(), 0.into()], "F" => 4,
            "NM" => Object::string_literal("foreign-square-1"),
            "OC" => layer_ref,
        });
        let page = *fx.doc.get_pages().get(&1).unwrap();
        fx.doc
            .get_dictionary_mut(page)
            .unwrap()
            .set("Annots", Object::Array(vec![Object::Reference(annot)]));
        annot
    }

    /// The optional-content structure as data: /OCProperties resolved, plus the groups.
    fn oc_structure(doc: &Document) -> String {
        let cat = doc.catalog().unwrap();
        let ocp = doc
            .dereference(cat.get(b"OCProperties").unwrap())
            .unwrap()
            .1;
        let mut groups = Vec::new();
        for (id, o) in &doc.objects {
            if let Object::Dictionary(d) = o {
                if matches!(d.get(b"Type"), Ok(Object::Name(n)) if n == b"OCG") {
                    groups.push(format!("{id:?}={d:?}"));
                }
            }
        }
        groups.sort();
        format!("{ocp:?}\n{}", groups.join("\n"))
    }

    fn cycle(path: &std::path::Path, edit: impl FnOnce(&mut Vec<Markup>)) -> Document {
        let mut doc = Document::load(path).unwrap();
        let mut ms = read_markups(&doc).unwrap();
        edit(&mut ms);
        write_markups(&mut doc, &ms).unwrap();
        doc.save(path).unwrap();
        Document::load(path).unwrap()
    }

    #[test]
    fn a_markup_on_a_layer_keeps_its_layer_over_three_save_cycles() {
        let mut fx = layered();
        let red = fx.red;
        let m = markup_on(Some(red));
        write_markups(&mut fx.doc, std::slice::from_ref(&m)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let before = oc_structure(&Document::load(&path).unwrap());
        for round in 0..3 {
            let doc = cycle(&path, |_| {});
            let got = read_markups(&doc).unwrap();
            assert_eq!(got.len(), 1, "round {round}");
            assert_eq!(got[0].id(), m.id());
            assert_eq!(
                got[0].optional_content,
                Some(OptionalContent::Reference(red.0, red.1))
            );
            assert_eq!(got[0].layer.as_deref(), Some("Red"));
            assert_eq!(
                oc_structure(&doc),
                before,
                "round {round}: /OCProperties and groups unchanged"
            );
        }
    }

    #[test]
    fn an_unedited_foreign_markup_keeps_its_oc_and_gets_its_layer_name() {
        let mut fx = layered();
        let red = fx.red;
        add_foreign_square(&mut fx, red);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let before = oc_structure(&Document::load(&path).unwrap());

        let ms = read_markups(&Document::load(&path).unwrap()).unwrap();
        assert_eq!(ms.len(), 1);
        assert_eq!(
            ms[0].layer.as_deref(),
            Some("Red"),
            "the layer name comes from the group"
        );
        assert!(
            ms[0].is_unedited(),
            "filling the name must not make a foreign markup 'edited'"
        );

        for round in 0..3 {
            let doc = cycle(&path, |_| {});
            let page = *doc.get_pages().get(&1).unwrap();
            let annots = crate::document::annots::page_annots(&doc, page).unwrap();
            assert_eq!(annots.len(), 1, "round {round}");
            assert_eq!(
                annots[0]
                    .1
                    .get(b"OC")
                    .ok()
                    .and_then(|o| o.as_reference().ok()),
                Some(red),
                "round {round}: the foreign /OC is written back as it was"
            );
            assert_eq!(oc_structure(&doc), before, "round {round}");
        }
    }

    #[test]
    fn deleting_a_markup_leaves_its_layer_and_an_undo_puts_it_back_on_it() {
        let mut fx = layered();
        let red = fx.red;
        let m = markup_on(Some(red));
        write_markups(&mut fx.doc, std::slice::from_ref(&m)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let before = oc_structure(&Document::load(&path).unwrap());

        // Delete: the layer stays, nothing is on it.
        let doc = cycle(&path, |ms| ms.clear());
        assert!(read_markups(&doc).unwrap().is_empty());
        assert_eq!(oc_structure(&doc), before);

        // Undo: the same markup comes back, still on the layer.
        let mut doc = Document::load(&path).unwrap();
        write_markups(&mut doc, std::slice::from_ref(&m)).unwrap();
        doc.save(&path).unwrap();
        let got = read_markups(&Document::load(&path).unwrap()).unwrap();
        assert_eq!(
            got[0].optional_content,
            Some(OptionalContent::Reference(red.0, red.1))
        );
    }

    #[test]
    fn a_dangling_oc_reference_is_dropped_on_write_for_rebuilt_and_preserved_markups() {
        let mut fx = layered();
        // Rebuilt from the model: a number that is not an object.
        let mut m = markup_on(None);
        m.optional_content = Some(OptionalContent::Reference(9999, 0));
        write_markups(&mut fx.doc, std::slice::from_ref(&m)).unwrap();
        let page = *fx.doc.get_pages().get(&1).unwrap();
        let annots = crate::document::annots::page_annots(&fx.doc, page).unwrap();
        assert!(!annots[0].1.has(b"OC"), "no dangling /OC is written");

        // ... and one pointing at an object that is not optional content (a hostile file's
        // /OC aimed at the page) is dropped as well.
        let page = *fx.doc.get_pages().get(&1).unwrap();
        let mut m2 = markup_on(None);
        m2.optional_content = Some(OptionalContent::Reference(page.0, page.1));
        write_markups(&mut fx.doc, std::slice::from_ref(&m2)).unwrap();
        let annots = crate::document::annots::page_annots(&fx.doc, page).unwrap();
        assert!(
            !annots[0].1.has(b"OC"),
            "an /OC at a non-optional-content object is not written"
        );

        // Preserved original pointing at a group that no longer exists.
        let mut fx2 = layered();
        let gone = fx2
            .doc
            .add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Gone") });
        add_foreign_square(&mut fx2, gone);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx2.doc.save(&path).unwrap();
        let mut doc = Document::load(&path).unwrap();
        let ms = read_markups(&doc).unwrap();
        doc.delete_object(gone);
        write_markups(&mut doc, &ms).unwrap();
        let page = *doc.get_pages().get(&1).unwrap();
        let annots = crate::document::annots::page_annots(&doc, page).unwrap();
        assert!(!annots[0].1.has(b"OC"));
    }

    #[test]
    fn an_oc_that_is_an_ocmd_reference_survives_untouched() {
        let mut fx = layered();
        let ocmd = fx.doc.add_object(dictionary! {
            "Type" => "OCMD", "OCGs" => refs(&[fx.red, fx.blue]), "P" => name("AllOn")
        });
        add_foreign_square(&mut fx, ocmd);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let ms = read_markups(&Document::load(&path).unwrap()).unwrap();
        assert_eq!(ms[0].layer, None, "an OCMD has no single layer name");
        let doc = cycle(&path, |_| {});
        let page = *doc.get_pages().get(&1).unwrap();
        let annots = crate::document::annots::page_annots(&doc, page).unwrap();
        assert_eq!(
            annots[0]
                .1
                .get(b"OC")
                .ok()
                .and_then(|o| o.as_reference().ok()),
            Some(ocmd)
        );
    }

    /// A group the file lists in /OCGs but that has no /Type (non-conforming, yet the panel
    /// presents it as a layer).
    fn typeless_group(fx: &mut Fx, name: &str) -> ObjectId {
        let g = fx
            .doc
            .add_object(dictionary! { "Name" => Object::string_literal(name) });
        let (red, blue) = (fx.red, fx.blue);
        fx.doc.catalog_mut().unwrap().set(
            "OCProperties",
            dictionary! { "OCGs" => refs(&[red, blue, g]), "D" => dictionary! {} },
        );
        g
    }

    #[test]
    fn a_typeless_group_listed_in_ocgs_keeps_assignments_and_foreign_oc_over_three_cycles() {
        let mut fx = layered();
        let g = typeless_group(&mut fx, "NoType");
        assert!(
            read_model(&fx.doc).layers.iter().any(|l| l.id == g.0),
            "shown as a layer"
        );
        add_foreign_square(&mut fx, g);
        let mine = markup_on(Some(g));
        write_markups(&mut fx.doc, std::slice::from_ref(&mine)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        for round in 0..3 {
            let doc = cycle(&path, |_| {});
            let page = *doc.get_pages().get(&1).unwrap();
            let annots = crate::document::annots::page_annots(&doc, page).unwrap();
            assert_eq!(annots.len(), 2, "round {round}");
            for (_, a) in &annots {
                assert_eq!(
                    a.get(b"OC").ok().and_then(|o| o.as_reference().ok()),
                    Some(g),
                    "round {round}: /OC to the typeless group is kept"
                );
            }
            let ms = read_markups(&doc).unwrap();
            assert!(
                ms.iter().all(|m| m.layer.as_deref() == Some("NoType")),
                "round {round}"
            );
        }
    }

    #[test]
    fn a_typeless_dictionary_not_listed_in_ocgs_is_still_not_optional_content() {
        let mut fx = layered();
        let stray = fx
            .doc
            .add_object(dictionary! { "Name" => Object::string_literal("Stray") });
        add_foreign_square(&mut fx, stray);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let doc = cycle(&path, |_| {});
        let page = *doc.get_pages().get(&1).unwrap();
        let annots = crate::document::annots::page_annots(&doc, page).unwrap();
        assert!(!annots[0].1.has(b"OC"));
    }

    #[test]
    fn the_groups_current_name_wins_over_a_stale_label() {
        let mut fx = layered();
        let red = fx.red;
        let mut m = markup_on(Some(red));
        m.layer = Some("Old name".into()); // saved as /RLLayer before the group was renamed
        write_markups(&mut fx.doc, std::slice::from_ref(&m)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        for round in 0..3 {
            let doc = cycle(&path, |_| {});
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms[0].layer.as_deref(), Some("Red"), "round {round}");
        }
    }

    #[test]
    fn adding_a_layer_and_saving_the_default_keep_markups_on_their_layers() {
        // The same write path the commands use (markups first, then the structural edit).
        use crate::commands::document::apply_edit_and_save_scaled;
        let mut fx = layered();
        let red = fx.red;
        add_foreign_square(&mut fx, red);
        let mine = markup_on(Some(red));
        write_markups(&mut fx.doc, std::slice::from_ref(&mine)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();
        let mut ms = read_markups(&Document::load(&path).unwrap()).unwrap();
        assert_eq!(ms.len(), 2);

        let removed = std::collections::HashSet::new();
        let mut made = 0;
        apply_edit_and_save_scaled(&path, &ms, &removed, &[], |doc| {
            made = add_layer(doc, "New", Some(red.0))?;
            Ok(())
        })
        .unwrap();
        let doc = Document::load(&path).unwrap();
        let model = read_model(&doc);
        assert!(model.layers.iter().any(|l| l.id == made && l.name == "New"));
        ms = read_markups(&doc).unwrap();
        assert_eq!(ms.len(), 2);
        assert!(ms
            .iter()
            .all(|m| m.optional_content == Some(OptionalContent::Reference(red.0, red.1))));

        // Three default-state saves in a row: nothing drifts.
        let hide: BTreeSet<u32> = [fx.blue.0].into();
        let mut states = Vec::new();
        for _ in 0..3 {
            apply_edit_and_save_scaled(&path, &ms, &removed, &[], |doc| {
                set_default_state(doc, &hide)
            })
            .unwrap();
            let d = Document::load(&path).unwrap();
            states.push(oc_structure(&d));
            ms = read_markups(&d).unwrap();
        }
        assert_eq!(states[0], states[1]);
        assert_eq!(states[1], states[2]);
        assert_eq!(ms.len(), 2);
        assert!(ms
            .iter()
            .all(|m| m.optional_content == Some(OptionalContent::Reference(red.0, red.1))));
    }
}

/// A real AutoCAD-derived, Bluebeam-saved sheet from the machine-local corpus (gitignored,
/// absent in CI): 8 layers, an `/Order` with no nesting. Hiding layers must change what
/// PDFium draws, and releasing the view must restore the original pixels exactly.
#[test]
fn real_bluebeam_reference_file_hides_and_restores_layers() {
    let p = std::path::PathBuf::from(
        "/Volumes/base/dev/claude/redline/bench/corpus/bb-ref/markup-test-original.pdf",
    );
    if !pdfium_available() || !p.exists() {
        eprintln!("skip: no PDFium or no corpus file");
        return;
    }
    let doc = Document::load(&p).unwrap();
    let model = read_model(&doc);
    assert_eq!(model.layers.len(), 8);
    assert!(model
        .layers
        .iter()
        .all(|l| l.default_on && !l.locked && l.view_intent));
    assert_eq!(model.unlisted, 0);
    let base = OverlayBase::new(&doc, &model, &p).unwrap();

    let mut e = crate::render::RenderEngine::new().expect("pdfium");
    e.open_document(p.clone(), "d".into(), None).unwrap();
    let sum = |r: &crate::render::PageRaster| -> u64 { r.rgb.iter().map(|b| u64::from(*b)).sum() };
    let hash = |r: &crate::render::PageRaster| -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        r.rgb.hash(&mut h);
        h.finish()
    };
    let original = e.render_page_full("d", 0, 36.0).unwrap();

    let all: BTreeSet<u32> = model.layers.iter().map(|l| l.id).collect();
    e.set_layer_view("d", Some(base.reader(&all).unwrap()))
        .unwrap();
    let none = e.render_page_full("d", 0, 36.0).unwrap();
    assert!(
        sum(&none) > sum(&original),
        "all layers hidden is lighter than the drawing (content on no layer stays)"
    );

    let wall = model.layers.iter().find(|l| l.name == "Wall").unwrap().id;
    e.set_layer_view("d", Some(base.reader(&[wall].into()).unwrap()))
        .unwrap();
    let no_wall = e.render_page_full("d", 0, 36.0).unwrap();
    assert_ne!(
        hash(&no_wall),
        hash(&original),
        "hiding Wall changes the page"
    );
    assert_ne!(hash(&no_wall), hash(&none), "...but not to blank");

    e.set_layer_view("d", Some(base.reader(&BTreeSet::new()).unwrap()))
        .unwrap();
    assert_eq!(
        hash(&e.render_page_full("d", 0, 36.0).unwrap()),
        hash(&original),
        "nothing hidden = the original"
    );
    e.set_layer_view("d", None).unwrap();
    assert_eq!(
        hash(&e.render_page_full("d", 0, 36.0).unwrap()),
        hash(&original),
        "released = the original"
    );
}

#[test]
fn a_file_with_more_groups_than_are_read_refuses_the_view_and_save_default() {
    let mut fx = fixture();
    let mut all = vec![fx.red, fx.blue];
    for i in 0..(MAX_LAYERS + 5) {
        all.push(ocg(&mut fx.doc, &format!("L{i}")));
    }
    let last = *all.last().unwrap();
    fx.doc.catalog_mut().unwrap().set(
        "OCProperties",
        dictionary! { "OCGs" => refs(&all), "D" => dictionary! { "OFF" => refs(&[last]) } },
    );
    let dir = tempfile::tempdir().unwrap();
    let p = fx.save_classic(dir.path());
    let doc = Document::load(&p).unwrap();
    let m = read_model(&doc);
    assert!(m.layers_truncated);
    assert!(OverlayBase::new(&doc, &m, &p)
        .unwrap_err()
        .contains("more layers"));
    let mut doc = Document::load(&p).unwrap();
    let before = format!("{:?}", doc.catalog().unwrap().get(b"OCProperties"));
    assert!(set_default_state(&mut doc, &BTreeSet::new()).is_err());
    assert_eq!(
        format!("{:?}", doc.catalog().unwrap().get(b"OCProperties")),
        before,
        "nothing written"
    );
    // A structure bound that does not lose groups (a cyclic /Order) does not disable views.
    assert!(!read_model(&fixture().doc).layers_truncated);
}
