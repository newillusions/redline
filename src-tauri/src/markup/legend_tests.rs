//! Legends through the PDF: the saved table, reopening, three-cycle idempotence (also with a
//! Revu-style 16-character /NM), foreign look-alikes, hostile definitions, and agreement of
//! the numbers with the exports. Behaviour spec: docs/legends.md.

use super::legend::{
    compute_rows, key_of, LegendColumn, LegendEntry, LegendProps, LegendScope, MAX_ENTRIES,
};
use super::*;
use crate::document::annots::tests::one_page_doc;
use crate::document::annots::{page_annots, read_markups, write_markups};
use crate::geometry::PdfPoint;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

fn user() -> UserRef {
    UserRef {
        user_id: uuid::Uuid::nil(),
        display_name: "T".into(),
    }
}

fn set(name: &str, color: &str, symbol: CountSymbol) -> CountSet {
    CountSet {
        id: uuid::Uuid::new_v4(),
        name: name.into(),
        color: color.into(),
        symbol,
    }
}

fn counted(page: u32, cs: &CountSet, n: u32, group: Option<uuid::Uuid>) -> Markup {
    let a = Appearance {
        color: cs.color.clone(),
        ..Appearance::default()
    };
    let mut m = Markup::new(
        MarkupType::MeasurementCount,
        page,
        MarkupGeometry::Point(PdfPoint { x: 50.0, y: 50.0 }),
        a,
        user(),
    );
    m.count_set = Some(cs.clone());
    m.count_group = group;
    m.measurement = Some(Measurement {
        scale_ref: None,
        raw_measure: f64::from(n),
        unit: "ea".into(),
        computed_quantity: f64::from(n),
        depth: None,
        count_value: Some(n),
        custom_columns: Default::default(),
        rise_drop: Vec::new(),
    });
    m
}

fn legend_for(entries_from: &[&Markup], scope: LegendScope) -> Markup {
    let mut m = Markup::new(
        MarkupType::Legend,
        0,
        MarkupGeometry::Rect {
            min: PdfPoint { x: 40.0, y: 500.0 },
            max: PdfPoint { x: 340.0, y: 700.0 },
        },
        Appearance {
            fill: Some("#ffffff".into()),
            color: "#000000".into(),
            ..Appearance::default()
        },
        user(),
    );
    m.subject = Some("Legend".into());
    m.legend = Some(
        LegendProps {
            title: "Luminaire key".into(),
            scope,
            columns: vec![
                LegendColumn::Symbol,
                LegendColumn::Subject,
                LegendColumn::Quantity,
                LegendColumn::Unit,
            ],
            entries: entries_from
                .iter()
                .map(|e| LegendEntry {
                    key: key_of(e),
                    label: None,
                    description: None,
                    hidden: false,
                    symbol: None,
                    set_name: None,
                })
                .collect(),
            ..LegendProps::default()
        }
        .sanitized(),
    );
    m
}

fn reload(doc: &mut Document) -> Document {
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    Document::load_mem(&buf).unwrap()
}

fn annots(doc: &Document) -> Vec<(Option<ObjectId>, Dictionary)> {
    let page = *doc.get_pages().values().next().unwrap();
    page_annots(doc, page).unwrap()
}

fn legend_dict(doc: &Document) -> Dictionary {
    let all: Vec<_> = annots(doc)
        .into_iter()
        .filter(|(_, d)| {
            d.has(b"RLLegend")
                || d.get(b"RLType").ok().and_then(|o| o.as_name().ok()) == Some(b"Legend")
        })
        .collect();
    assert_eq!(all.len(), 1, "expected one legend annotation");
    all.into_iter().next().unwrap().1
}

fn ap_text(doc: &Document, d: &Dictionary) -> String {
    let ap = d.get(b"AP").unwrap().as_dict().unwrap();
    let n = ap.get(b"N").unwrap().as_reference().unwrap();
    let s = doc.get_object(n).unwrap().as_stream().unwrap();
    String::from_utf8_lossy(
        &s.decompressed_content()
            .unwrap_or_else(|_| s.content.clone()),
    )
    .into_owned()
}

fn floats(d: &Dictionary, key: &[u8]) -> Vec<f64> {
    d.get(key)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| f64::from(o.as_float().unwrap()))
        .collect()
}

fn cycle(doc: &mut Document, edit: impl FnOnce(&mut Vec<Markup>)) {
    let mut ms = read_markups(doc).unwrap();
    edit(&mut ms);
    write_markups(doc, &ms).unwrap();
    *doc = reload(doc);
}

fn legends(ms: &[Markup]) -> Vec<&Markup> {
    ms.iter()
        .filter(|m| m.markup_type == MarkupType::Legend)
        .collect()
}

// --- the saved table -----------------------------------------------------------------

#[test]
fn saved_legend_is_a_stamp_with_the_table_in_its_appearance() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let b = set("Track", "#0000ff", CountSymbol::Square);
    let ms = vec![
        counted(0, &a, 2, None),
        counted(0, &a, 5, None),
        counted(0, &b, 1, None),
        legend_for(
            &[&counted(0, &a, 1, None), &counted(0, &b, 1, None)],
            LegendScope::AllPages,
        ),
    ];
    write_markups(&mut doc, &ms).unwrap();
    let doc = reload(&mut doc);
    let d = legend_dict(&doc);
    assert_eq!(d.get(b"Subtype").unwrap().as_name().unwrap(), b"Stamp");
    assert!(
        !d.has(b"Contents"),
        "a stamp's contents would show as a comment note"
    );
    let text = ap_text(&doc, &d);
    for needle in [
        "(Luminaire key) Tj",
        "(Downlight) Tj",
        "(7) Tj",
        "(Track) Tj",
        "(1) Tj",
        "(ea) Tj",
    ] {
        assert!(
            text.contains(needle),
            "missing {needle} in appearance:\n{text}"
        );
    }
    // /Rect equals the appearance /BBox (identity BBox-to-Rect fit) and the authored top edge holds.
    let ap = d.get(b"AP").unwrap().as_dict().unwrap();
    let n = ap.get(b"N").unwrap().as_reference().unwrap();
    let bbox = floats(
        &doc.get_object(n).unwrap().as_stream().unwrap().dict,
        b"BBox",
    );
    let rect = floats(&d, b"Rect");
    for (r, b) in rect.iter().zip(&bbox) {
        assert!((r - b).abs() < 1e-3, "Rect {rect:?} != BBox {bbox:?}");
    }
    assert!(
        (rect[3] - 700.0).abs() < 1e-3,
        "top edge is the user's: {rect:?}"
    );
    assert!(rect[1] < 700.0 - 30.0, "bottom follows the rows: {rect:?}");
}

#[test]
fn reopening_restores_the_definition_and_recomputes_the_numbers() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let l = legend_for(&[&counted(0, &a, 1, None)], LegendScope::AllPages);
    let before = l.legend.clone().unwrap();
    write_markups(&mut doc, &[counted(0, &a, 4, None), l]).unwrap();
    let doc = reload(&mut doc);
    let ms = read_markups(&doc).unwrap();
    let l = legends(&ms)[0];
    assert_eq!(l.legend.as_ref().unwrap(), &before);
    let rows = compute_rows(l.page, l.legend.as_ref().unwrap(), &ms);
    assert_eq!(rows[0].quantity_text, "4");
    assert_eq!(rows[0].subject, "Downlight");
}

#[test]
fn the_appearance_follows_every_add_edit_and_delete_across_saves() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let first = counted(0, &a, 1, None);
    let l = legend_for(&[&first], LegendScope::AllPages);
    write_markups(&mut doc, &[first, l]).unwrap();
    let mut doc = reload(&mut doc);
    assert!(ap_text(&doc, &legend_dict(&doc)).contains("(1) Tj"));
    cycle(&mut doc, |ms| ms.push(counted(0, &a, 3, None))); // add
    assert!(ap_text(&doc, &legend_dict(&doc)).contains("(4) Tj"));
    cycle(&mut doc, |ms| {
        let c = ms
            .iter_mut()
            .find(|m| m.markup_type == MarkupType::MeasurementCount)
            .unwrap();
        c.measurement.as_mut().unwrap().count_value = Some(10); // edit
    });
    assert!(ap_text(&doc, &legend_dict(&doc)).contains("(13) Tj"));
    cycle(&mut doc, |ms| {
        let i = ms
            .iter()
            .position(|m| m.markup_type == MarkupType::MeasurementCount)
            .unwrap();
        ms.remove(i); // delete
    });
    assert!(ap_text(&doc, &legend_dict(&doc)).contains("(3) Tj"));
}

#[test]
fn three_cycles_are_idempotent_with_a_revu_style_sixteen_char_nm() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Star);
    let l = legend_for(&[&counted(0, &a, 1, None)], LegendScope::AllPages);
    write_markups(&mut doc, &[counted(0, &a, 6, None), l]).unwrap();
    let mut doc = reload(&mut doc);
    // Rename the legend annotation the way Revu names things.
    let page = *doc.get_pages().values().next().unwrap();
    let (oid, _) = annots(&doc)
        .into_iter()
        .find(|(_, d)| d.has(b"RLLegend"))
        .unwrap();
    doc.get_dictionary_mut(oid.unwrap())
        .unwrap()
        .set("NM", Object::string_literal("LEGENDREVU000001"));
    let _ = page;
    let mut snapshots: Vec<(String, Vec<f64>)> = Vec::new();
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = legend_dict(&doc);
        assert_eq!(
            d.get(b"NM").unwrap().as_str().unwrap(),
            b"LEGENDREVU000001",
            "cycle {n}: /NM kept"
        );
        assert_eq!(
            annots(&doc).len(),
            2,
            "cycle {n}: nothing duplicated or lost"
        );
        snapshots.push((ap_text(&doc, &d), floats(&d, b"Rect")));
    }
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[1], snapshots[2]);
    assert!(snapshots[0].0.contains("(6) Tj"));
}

// --- foreign and hostile ---------------------------------------------------------------

const REVU_AP: &[u8] = b"q 0 0 1 RG 5 5 90 40 re S Q REVU-LEGEND-AP";

fn foreign_stamp(doc: &mut Document, forged: bool) -> ObjectId {
    let ap = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 50.into()],
        },
        REVU_AP.to_vec(),
    ));
    let mut d = dictionary! {
        "Type" => "Annot", "Subtype" => "Stamp", "F" => 4,
        "NM" => Object::string_literal("REVULEGEND00001"),
        "Subj" => Object::string_literal("Legend"),
        "Rect" => vec![100.into(), 100.into(), 200.into(), 150.into()],
        "BSIColumnData" => vec![Object::string_literal("col")],
        "AP" => dictionary! { "N" => ap },
    };
    if forged {
        d.set(
            "RLLegend",
            Object::string_literal(r##"{"title":"forged","entries":[],"scope":"AllPages"}"##),
        );
    }
    doc.add_object(d)
}

#[test]
fn a_revu_legend_look_alike_is_left_exactly_alone_for_three_cycles() {
    for forged in [false, true] {
        let (mut doc, page) = one_page_doc();
        let f = foreign_stamp(&mut doc, forged);
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Annots", Object::Array(vec![f.into()]));
        for n in 1..=3 {
            cycle(&mut doc, |ms| {
                assert_eq!(ms.len(), 1);
                assert_eq!(
                    ms[0].markup_type,
                    MarkupType::Stamp,
                    "never read as a redline legend"
                );
                assert!(
                    ms[0].legend.is_none(),
                    "a forged /RLLegend is not interpreted"
                );
            });
            let all = annots(&doc);
            assert_eq!(all.len(), 1, "cycle {n}");
            let d = &all[0].1;
            assert_eq!(d.get(b"NM").unwrap().as_str().unwrap(), b"REVULEGEND00001");
            assert!(d.has(b"BSIColumnData"), "cycle {n}: foreign keys kept");
            let ap = d.get(b"AP").unwrap().as_dict().unwrap();
            let n_ref = ap.get(b"N").unwrap().as_reference().unwrap();
            let s = doc.get_object(n_ref).unwrap().as_stream().unwrap();
            assert_eq!(
                s.decompressed_content()
                    .unwrap_or_else(|_| s.content.clone()),
                REVU_AP,
                "cycle {n}: original appearance byte-for-byte (forged={forged})"
            );
        }
    }
}

#[test]
fn a_hostile_definition_reads_as_an_empty_legend_and_saves_without_panic() {
    let (mut doc, _) = one_page_doc();
    let a = set("A", "#ff0000", CountSymbol::Circle);
    write_markups(
        &mut doc,
        &[legend_for(
            &[&counted(0, &a, 1, None)],
            LegendScope::AllPages,
        )],
    )
    .unwrap();
    let mut doc = reload(&mut doc);
    let (oid, _) = annots(&doc)
        .into_iter()
        .find(|(_, d)| d.has(b"RLLegend"))
        .unwrap();
    // Huge definition, then unparseable, then absurd-but-valid JSON.
    let attacks = [
        "x".repeat(super::legend::MAX_DEF_BYTES + 10),
        "{broken".to_string(),
        format!(
            r##"{{"title":"{}","font_size_pt":1e300,"columns":[],"scope":{{"Pages":[[0,0],[9,1]]}},"entries":[{}]}}"##,
            "T".repeat(100_000),
            (0..3000)
                .map(|i| format!(
                    r##"{{"key":{{"markup_type":"Cloud","color":"#{i:06x}"}},"label":"{}"}}"##,
                    "L".repeat(5000)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ];
    for (i, atk) in attacks.iter().enumerate() {
        doc.get_dictionary_mut(oid.unwrap())
            .unwrap()
            .set("RLLegend", Object::string_literal(atk.as_str()));
        let ms = read_markups(&doc).unwrap();
        let l = legends(&ms);
        assert_eq!(l.len(), 1, "attack {i}");
        if let Some(p) = &l[0].legend {
            assert!(p.entries.len() <= MAX_ENTRIES, "attack {i}");
            assert!(p.title.chars().count() <= super::legend::MAX_TITLE_CHARS);
            assert!(
                (super::legend::MIN_FONT_PT..=super::legend::MAX_FONT_PT).contains(&p.font_size_pt)
            );
        }
        // And a save of whatever was read must not panic or blow up, and the legend stays
        // visible: a refused definition leaves an outlined box, not a blank annotation.
        let mut again = doc.clone();
        write_markups(&mut again, &ms).unwrap();
        let again = reload(&mut again);
        let reread = read_markups(&again).unwrap();
        assert_eq!(legends(&reread).len(), 1, "attack {i}");
        if l[0].legend.is_none() {
            let d = annots(&again)
                .into_iter()
                .find(|(_, d)| {
                    d.get(b"RLType").ok().and_then(|o| o.as_name().ok()) == Some(b"Legend")
                })
                .unwrap()
                .1;
            assert!(
                ap_text(&again, &d).contains(" re\nS"),
                "attack {i}: outlined box"
            );
        }
    }
}

#[test]
fn a_full_legend_of_256_rows_saves_with_a_bounded_appearance() {
    let (mut doc, _) = one_page_doc();
    let sets: Vec<CountSet> = (0..256)
        .map(|i| set(&format!("S{i}"), "#ff0000", CountSymbol::Hexagon))
        .collect();
    let mut ms: Vec<Markup> = sets.iter().map(|s| counted(0, s, 1, None)).collect();
    let keys: Vec<Markup> = ms.clone();
    let key_refs: Vec<&Markup> = keys.iter().collect();
    ms.push(legend_for(&key_refs, LegendScope::AllPages));
    let t = std::time::Instant::now();
    write_markups(&mut doc, &ms).unwrap();
    assert!(t.elapsed().as_secs() < 20);
    let doc = reload(&mut doc);
    let d = legend_dict(&doc);
    assert!(ap_text(&doc, &d).len() < 2_000_000);
    let ms = read_markups(&doc).unwrap();
    assert_eq!(legends(&ms)[0].legend.as_ref().unwrap().entries.len(), 256);
}

// --- agreement with the other quantity surfaces -----------------------------------------

#[test]
fn legend_numbers_equal_the_quantities_export_including_after_recalibration() {
    use crate::commands::takeoff::{export_markup_list_with_scales, ExportFormat};
    use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleTarget};
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let g1 = uuid::Uuid::new_v4();
    let g2 = uuid::Uuid::new_v4();
    let mut ms = vec![
        counted(0, &a, 1, Some(g1)),
        counted(0, &a, 1, Some(g1)),
        counted(0, &a, 1, Some(g1)),
        counted(0, &a, 1, Some(g2)),
        counted(2, &a, 1, None),
    ];
    let mut len = Markup::new(
        MarkupType::MeasurementLength,
        0,
        MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 1000.0, y: 0.0 },
        ]),
        Appearance::default(),
        user(),
    );
    len.measurement = Some(Measurement {
        scale_ref: Some("old".into()),
        raw_measure: 1000.0,
        unit: "m".into(),
        computed_quantity: 1.0,
        depth: None,
        count_value: None,
        custom_columns: Default::default(),
        rise_drop: vec![RiseDrop {
            segment: 0,
            amount: 2.0,
            unit: "m".into(),
        }],
    });
    ms.push(len);
    let scale = ScaleRecord::new(
        ScaleTarget::DocumentDefault,
        ScaleMethod::TwoPoint,
        0.005,
        "m".into(),
        "recal".into(),
        2,
    );
    // The app recomputes on recalibration (#138); the export recomputes itself.
    let mut app = ms.clone();
    crate::takeoff::recompute_all(&mut app, std::slice::from_ref(&scale));
    let entries = [&app[0], &app[5]];
    let legend = legend_for(&entries, LegendScope::AllPages);
    let rows = compute_rows(0, legend.legend.as_ref().unwrap(), &app);

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("q.csv");
    export_markup_list_with_scales(&ms, &[scale], &out, ExportFormat::Csv).unwrap();
    let mut rdr = csv::Reader::from_path(&out).unwrap();
    let recs: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();
    // Count groups collapse to one export row each; the loose count is a row of its own.
    let count_total: f64 = recs
        .iter()
        .filter(|r| &r[8] != "7.00")
        .map(|r| r[8].parse::<f64>().unwrap())
        .sum();
    let length_row = recs
        .iter()
        .find(|r| &r[8] == "7.00")
        .expect("1000 x 0.005 + 2 m rise");
    assert_eq!(
        rows[0].quantity as u64 as f64, count_total,
        "counts: legend {rows:?} vs export {recs:?}"
    );
    assert_eq!(rows[0].quantity_text, "5");
    assert_eq!(
        rows[1].quantity_text, &length_row[8],
        "length after recalibration"
    );
    assert_eq!(rows[1].unit, "m");
    // And the Rust group totals the panels read.
    let totals = crate::takeoff::count::group_totals(&app);
    let grouped: u64 = totals.values().sum();
    assert_eq!(
        grouped + 1,
        rows[0].quantity as u64,
        "grouped totals + the ungrouped item"
    );
}

// --- store paths and IPC boundary -------------------------------------------------------

#[test]
fn deleting_and_undoing_a_legend_restores_its_reader_owned_name() {
    use crate::document::store::MarkupStore;
    let store = MarkupStore::default();
    store.register("d", std::path::PathBuf::from("/nonexistent/x.pdf"), None);
    let a = set("A", "#ff0000", CountSymbol::Circle);
    let mut l = legend_for(&[&counted(0, &a, 1, None)], LegendScope::AllPages);
    l.source_nm = Some("LEGENDREVU000001".into());
    let id = l.id();
    store.seed_loaded("d", vec![l.clone()]).unwrap();
    store.delete("d", id).unwrap();
    assert!(store.list("d").unwrap().is_empty());
    // The undo re-adds what the frontend holds, which never carries reader-owned names.
    let mut back = l.clone();
    back.source_nm = None;
    store.add("d", back).unwrap();
    let again = store.list("d").unwrap();
    assert_eq!(again[0].source_nm.as_deref(), Some("LEGENDREVU000001"));
    assert_eq!(again[0].legend, l.legend);
}

#[test]
fn computed_rows_cannot_be_injected_through_json() {
    let a = set("A", "#ff0000", CountSymbol::Circle);
    let l = legend_for(&[&counted(0, &a, 1, None)], LegendScope::AllPages);
    let mut v = serde_json::to_value(&l).unwrap();
    assert!(
        v["legend"].get("rows").is_none(),
        "rows are never serialised to the frontend"
    );
    v["legend"]["rows"] = serde_json::json!([{
        "entry": 0, "key": {"markup_type": "Cloud"}, "subject": "FORGED", "description": "",
        "quantity": 99.0, "quantity_text": "99", "unit": "ea", "pages": "", "matched": 99,
        "color": "#000000", "fill": null, "symbol": null
    }]);
    let back: Markup = serde_json::from_value(v).unwrap();
    assert!(
        back.legend.unwrap().rows.is_empty(),
        "rows are ignored on the way in"
    );
}

// --- review round 1: hostile colours and WinAnsi text -------------------------------------

#[test]
fn a_non_ascii_count_set_colour_read_from_a_file_cannot_panic_the_save() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let c = counted(0, &a, 2, None);
    let mut l = legend_for(&[&c], LegendScope::AllPages);
    l.legend.as_mut().unwrap().auto_count_sets = true;
    write_markups(&mut doc, &[c, l]).unwrap();
    let mut doc = reload(&mut doc);
    // "#aéaaa" is 6 bytes after '#': the shape that used to slice inside a character.
    let (oid, _) = annots(&doc)
        .into_iter()
        .find(|(_, d)| d.has(b"RLCountSetColor"))
        .unwrap();
    let dict = doc.get_dictionary_mut(oid.unwrap()).unwrap();
    dict.set("RLCountSetColor", Object::string_literal("#a\u{e9}aaa"));
    let ms = read_markups(&doc).unwrap();
    let cs = ms
        .iter()
        .find_map(|m| m.count_set.clone())
        .expect("the count keeps its set");
    assert_eq!(
        cs.color, "#ff0000",
        "an unusable set colour falls back to the annotation's /C"
    );
    // And even a model that carries the hostile colour (IPC/MCP) saves and draws.
    let mut hostile = ms.clone();
    for m in hostile.iter_mut() {
        if let Some(c) = m.count_set.as_mut() {
            c.color = "#a\u{e9}aaa".into();
        }
        m.appearance.color = "#a\u{e9}aaa".into();
        m.appearance.fill = Some("#\u{e9}\u{e9}\u{e9}".into());
    }
    let mut again = doc.clone();
    write_markups(&mut again, &hostile).unwrap();
    let again = reload(&mut again);
    assert!(ap_text(&again, &legend_dict(&again)).contains("(Downlight) Tj"));
}

#[test]
fn hex_to_rgb_style_inputs_never_panic() {
    // Through the public drawing path: markups whose colours are arbitrary strings.
    let (mut doc, _) = one_page_doc();
    let mut ms = Vec::new();
    for (i, col) in [
        "#aéaaa",
        "é",
        "#",
        "",
        "#12345",
        "#ééé",
        "\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}",
    ]
    .iter()
    .enumerate()
    {
        let mut m = counted(0, &set("S", "#ff0000", CountSymbol::Circle), 1, None);
        m.appearance.color = (*col).into();
        m.appearance.outline_color = Some((*col).into());
        m.id();
        let _ = i;
        ms.push(m);
    }
    let mut l = legend_for(&[&ms[0]], LegendScope::AllPages);
    l.appearance.color = "#aéaaa".into();
    ms.push(l);
    write_markups(&mut doc, &ms).unwrap();
}

#[test]
fn legend_text_is_written_as_winansi_bytes() {
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let c = counted(0, &a, 1, None);
    let mut l = legend_for(&[&c], LegendScope::AllPages);
    {
        let p = l.legend.as_mut().unwrap();
        p.entries[0].label = Some("\u{d8}75 \u{2013} 3000K \u{b5}\u{b0} \u{3042}".into());
        p.title = "\u{c9}clairage \u{201c}A\u{201d}".into();
    }
    write_markups(&mut doc, &[c, l]).unwrap();
    let doc = reload(&mut doc);
    let d = legend_dict(&doc);
    let raw = {
        let ap = d.get(b"AP").unwrap().as_dict().unwrap();
        let n = ap.get(b"N").unwrap().as_reference().unwrap();
        let s = doc.get_object(n).unwrap().as_stream().unwrap();
        s.decompressed_content()
            .unwrap_or_else(|_| s.content.clone())
    };
    assert!(
        raw.is_ascii(),
        "the content stream stays ASCII (no UTF-8 bytes of a Latin-1 character)"
    );
    let text = String::from_utf8(raw).unwrap();
    // U+00D8 = 0xD8 = \330, en dash = 0x96 = \226, micro = 0xB5 = \265, degree = 0xB0 = \260,
    // E-acute = 0xC9 = \311, curly quotes = 0x93 / 0x94; Hiragana is not WinAnsi -> '?'.
    assert!(
        text.contains("(\\33075 \\226 3000K \\265\\260 ?) Tj"),
        "subject not WinAnsi encoded:\n{text}"
    );
    assert!(
        text.contains("(\\311clairage \\223A\\224) Tj"),
        "title not WinAnsi encoded:\n{text}"
    );
}

// --- review round 1: stored quantities that predate a scale --------------------------------

fn stale_length() -> Markup {
    let mut m = Markup::new(
        MarkupType::MeasurementLength,
        0,
        MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 1000.0, y: 0.0 },
        ]),
        Appearance::default(),
        user(),
    );
    m.measurement = Some(Measurement {
        scale_ref: None,
        raw_measure: 1000.0,
        unit: "pt".into(),
        computed_quantity: 10.0, // stored under an older ratio
        depth: None,
        count_value: None,
        custom_columns: Default::default(),
        rise_drop: Vec::new(),
    });
    m
}

fn new_scale() -> crate::takeoff::ScaleRecord {
    crate::takeoff::ScaleRecord::new(
        crate::takeoff::ScaleTarget::DocumentDefault,
        crate::takeoff::ScaleMethod::TwoPoint,
        0.005,
        "m".into(),
        "s".into(),
        2,
    )
}

fn export_quantity(markups: &[Markup], scales: &[crate::takeoff::ScaleRecord]) -> String {
    use crate::commands::takeoff::{export_markup_list_with_scales, ExportFormat};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("q.csv");
    export_markup_list_with_scales(markups, scales, &out, ExportFormat::Csv).unwrap();
    let mut rdr = csv::Reader::from_path(&out).unwrap();
    let rec = rdr
        .records()
        .map(|r| r.unwrap())
        .find(|r| &r[1] == "MeasurementLength")
        .unwrap();
    rec[8].to_string()
}

#[test]
fn saved_legend_uses_quantities_recomputed_under_the_current_scales() {
    use crate::document::save::{save_with_markups_removing, save_with_markups_scaled};
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.pdf");
    let (mut doc, _) = one_page_doc();
    doc.save(&src).unwrap();
    let len = stale_length();
    let legend = legend_for(&[&len], LegendScope::AllPages);
    let markups = vec![len, legend];
    let scales = vec![new_scale()];
    let want = export_quantity(&markups, &scales);
    assert_eq!(want, "5.00");

    let with = dir.path().join("with.pdf");
    save_with_markups_scaled(&src, &with, &markups, &Default::default(), &scales).unwrap();
    let doc = lopdf::Document::load(&with).unwrap();
    assert!(
        ap_text(&doc, &legend_dict(&doc)).contains(&format!("({want}) Tj")),
        "legend == export"
    );

    // Without scales the stored (stale) value is what a save sees: the old behaviour.
    let without = dir.path().join("without.pdf");
    save_with_markups_removing(&src, &without, &markups, &Default::default()).unwrap();
    let doc = lopdf::Document::load(&without).unwrap();
    assert!(ap_text(&doc, &legend_dict(&doc)).contains("(10.00) Tj"));
}

#[test]
fn an_mcp_add_scale_leaves_no_stale_quantity_for_save_or_export() {
    use crate::commands::takeoff::recompute_stored_measurements;
    use crate::document::save::save_with_markups_scaled;
    use crate::document::store::MarkupStore;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.pdf");
    let (mut doc, _) = one_page_doc();
    doc.save(&src).unwrap();

    let store = MarkupStore::default();
    store.register("d", src.clone(), None);
    let len = stale_length();
    let legend = legend_for(&[&len], LegendScope::AllPages);
    store.seed_loaded("d", vec![len, legend]).unwrap();

    // What `add_scale` does after recording the scale (the MCP tool and the GUI share it).
    let mut scales = crate::takeoff::ScaleStore::default();
    scales.add("d", new_scale());
    assert_eq!(recompute_stored_measurements(&store, &scales, "d"), 1);
    assert!(store.is_dirty("d"));

    let list = store.list("d").unwrap();
    let stored = list
        .iter()
        .find_map(|m| m.measurement.as_ref().map(|x| x.computed_quantity))
        .unwrap();
    assert!(
        (stored - 5.0).abs() < 1e-9,
        "stored quantity is current: {stored}"
    );
    let dest = dir.path().join("dest.pdf");
    save_with_markups_scaled(&src, &dest, &list, &Default::default(), scales.list("d")).unwrap();
    let doc = lopdf::Document::load(&dest).unwrap();
    let export = export_quantity(&list, scales.list("d"));
    assert!(
        ap_text(&doc, &legend_dict(&doc)).contains(&format!("({export}) Tj")),
        "legend == export"
    );
}

// --- through #141's recovery paths -------------------------------------------------------

#[test]
fn a_legend_survives_the_save_as_rebuild_path_once_and_with_current_rows() {
    use crate::document::recover::rebuild_inputs;
    use crate::document::save::{load_markups_from, save_with_markups_scaled};
    let dir = tempfile::tempdir().unwrap();
    let orig = dir.path().join("orig.pdf");
    let (mut doc, _) = one_page_doc();
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let c = counted(0, &a, 3, None);
    let l = legend_for(&[&c], LegendScope::AllPages);
    write_markups(&mut doc, &[c, l]).unwrap();
    doc.save(&orig).unwrap();

    // The markups as the app holds them, then one more count added (an unsaved edit).
    let mut markups = load_markups_from(&orig, None).unwrap();
    markups.push(counted(0, &a, 4, None));
    let (rebuilt, removed) = rebuild_inputs(&markups, &Default::default());
    let dest = dir.path().join("rebuilt.pdf");
    save_with_markups_scaled(&orig, &dest, &rebuilt, &removed, &[]).unwrap();

    let doc = lopdf::Document::load(&dest).unwrap();
    let d = legend_dict(&doc); // asserts exactly one legend annotation
    assert!(
        ap_text(&doc, &d).contains("(7) Tj"),
        "rows computed from the rebuilt set"
    );
    let again = load_markups_from(&dest, None).unwrap();
    assert_eq!(legends(&again).len(), 1);
    assert_eq!(
        again
            .iter()
            .filter(|m| m.markup_type == MarkupType::MeasurementCount)
            .count(),
        2
    );
}

// --- review round 2 (F7): delete_scale and the page-op / extract writers ------------------

#[test]
fn after_an_mcp_delete_scale_save_page_op_and_extract_all_print_what_the_export_prints() {
    use crate::commands::document::{apply_edit_and_save_scaled, extract_pages_to_new_file_scaled};
    use crate::commands::takeoff::recompute_stored_measurements;
    use crate::document::save::save_with_markups_scaled;
    use crate::document::store::MarkupStore;
    use crate::takeoff::{ScaleMethod, ScaleRecord, ScaleStore, ScaleTarget};

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.pdf");
    let (mut doc, _) = one_page_doc();
    doc.save(&src).unwrap();

    // A document default (0.001 m/pt) and a page-0 scale P (0.005 m/pt); the length was
    // measured under P: 1000 pt x 0.005 = 5.00 m.
    let mk_scale = |target, ratio| {
        ScaleRecord::new(
            target,
            ScaleMethod::TwoPoint,
            ratio,
            "m".into(),
            "s".into(),
            2,
        )
    };
    let default_scale = mk_scale(ScaleTarget::DocumentDefault, 0.001);
    let page_scale = mk_scale(ScaleTarget::Page { page: 0 }, 0.005);
    let mut len = stale_length();
    {
        let m = len.measurement.as_mut().unwrap();
        m.unit = "m".into();
        m.computed_quantity = 5.0;
        m.scale_ref = Some(page_scale.id.clone());
    }
    let legend = legend_for(&[&len], LegendScope::AllPages);

    let store = MarkupStore::default();
    store.register("d", src.clone(), None);
    store
        .seed_loaded("d", vec![len.clone(), legend.clone()])
        .unwrap();
    let mut scales = ScaleStore::default();
    scales.add("d", default_scale);
    scales.add("d", page_scale.clone());

    // delete_scale: forget P, then recompute the stored measurements (what the command does).
    assert!(scales.delete("d", &page_scale.id));
    assert_eq!(recompute_stored_measurements(&store, &scales, "d"), 1);
    let list = store.list("d").unwrap();
    let export = export_quantity(&list, scales.list("d"));
    assert_eq!(export, "1.00", "falls back to the document default");
    let needle = format!("({export}) Tj");

    // Main save.
    let saved = dir.path().join("saved.pdf");
    save_with_markups_scaled(&src, &saved, &list, &Default::default(), scales.list("d")).unwrap();
    let d = lopdf::Document::load(&saved).unwrap();
    assert!(ap_text(&d, &legend_dict(&d)).contains(&needle), "main save");

    // Page op (a no-op edit) and extract, from the recomputed store...
    let op_path = dir.path().join("op.pdf");
    std::fs::copy(&src, &op_path).unwrap();
    apply_edit_and_save_scaled(
        &op_path,
        &list,
        &Default::default(),
        scales.list("d"),
        |_| Ok(()),
    )
    .unwrap();
    let d = lopdf::Document::load(&op_path).unwrap();
    assert!(ap_text(&d, &legend_dict(&d)).contains(&needle), "page op");
    let ext = dir.path().join("ext.pdf");
    extract_pages_to_new_file_scaled(
        &src,
        &ext,
        &list,
        &Default::default(),
        scales.list("d"),
        &[0],
    )
    .unwrap();
    let d = lopdf::Document::load(&ext).unwrap();
    assert!(ap_text(&d, &legend_dict(&d)).contains(&needle), "extract");

    // ... and from STALE stored values (5.00 under the deleted scale): the scales passed to the
    // writers are what make the table right, not only the store recompute.
    let stale = vec![len, legend];
    let op2 = dir.path().join("op2.pdf");
    std::fs::copy(&src, &op2).unwrap();
    apply_edit_and_save_scaled(&op2, &stale, &Default::default(), scales.list("d"), |_| {
        Ok(())
    })
    .unwrap();
    let d = lopdf::Document::load(&op2).unwrap();
    assert!(
        ap_text(&d, &legend_dict(&d)).contains(&needle),
        "page op from stale values"
    );
    let ext2 = dir.path().join("ext2.pdf");
    extract_pages_to_new_file_scaled(
        &src,
        &ext2,
        &stale,
        &Default::default(),
        scales.list("d"),
        &[0],
    )
    .unwrap();
    let d = lopdf::Document::load(&ext2).unwrap();
    assert!(
        ap_text(&d, &legend_dict(&d)).contains(&needle),
        "extract from stale values"
    );
}

// --- page labels in the Pages column (docs/navigation.md RL1) -----------------------------

#[test]
fn the_pages_column_lists_page_labels_when_the_document_has_them() {
    use crate::document::navigation::labels::{write_ranges, LabelRange, LabelStyle};
    let a = set("Downlight", "#ff0000", CountSymbol::Circle);
    let make = || {
        let mut l = legend_for(&[&counted(0, &a, 1, None)], LegendScope::AllPages);
        l.legend.as_mut().unwrap().columns = vec![LegendColumn::Subject, LegendColumn::Pages];
        vec![counted(0, &a, 2, None), l]
    };

    // No labels: the page number.
    let (mut doc, _) = one_page_doc();
    write_markups(&mut doc, &make()).unwrap();
    let plain = reload(&mut doc);
    let text = ap_text(&plain, &legend_dict(&plain));
    assert!(text.contains("(1) Tj"), "page number expected:\n{text}");
    assert!(!text.contains("(E-101) Tj"));

    // Labelled: the label.
    let (mut doc, _) = one_page_doc();
    write_ranges(
        &mut doc,
        &[LabelRange {
            start: 0,
            style: Some(LabelStyle::Decimal),
            prefix: "E-".into(),
            first: 101,
        }],
    )
    .unwrap();
    write_markups(&mut doc, &make()).unwrap();
    let labelled = reload(&mut doc);
    let text = ap_text(&labelled, &legend_dict(&labelled));
    assert!(text.contains("(E-101) Tj"), "label expected:\n{text}");
}
