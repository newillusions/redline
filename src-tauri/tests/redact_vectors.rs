//! Redaction must not leak through any secondary vector. Every case asserts BOTH
//! `pdftotext` and a raw grep over every (decompressed) object of the saved file.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use redline_lib::docops::{redact_annotations, redact_regions, RedactRegion};
use std::process::Command;

const FONT_W: i64 = 500;

fn have_pdftotext() -> bool {
    Command::new("pdftotext").arg("-v").output().is_ok()
}

fn region(x: f64, y: f64, w: f64, h: f64) -> RedactRegion {
    RedactRegion {
        page_index: 0,
        x,
        y,
        width: w,
        height: h,
    }
}

struct Fx {
    doc: Document,
    pages_id: ObjectId,
    font_id: ObjectId,
}

impl Fx {
    fn new() -> Fx {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let widths: Vec<Object> = (32..=126).map(|_| Object::Integer(FONT_W)).collect();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "FirstChar" => 32, "LastChar" => 126, "Widths" => widths,
        });
        Fx {
            doc,
            pages_id,
            font_id,
        }
    }
    fn stream(&mut self, dict: Dictionary, c: &str) -> ObjectId {
        self.doc
            .add_object(Stream::new(dict, c.as_bytes().to_vec()))
    }
    fn form(&mut self, c: &str, extra_res: Option<Dictionary>) -> ObjectId {
        let mut res = dictionary! { "Font" => dictionary! { "F1" => self.font_id } };
        if let Some(x) = extra_res {
            res.set("XObject", x);
        }
        self.stream(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 300.into(), 200.into()], "Resources" => res,
            },
            c,
        )
    }
    fn page(
        &mut self,
        contents: Object,
        resources: Option<Dictionary>,
        annots: Vec<Object>,
    ) -> ObjectId {
        let mut d = dictionary! {
            "Type" => "Page", "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Contents" => contents,
        };
        if let Some(r) = resources {
            d.set("Resources", r);
        }
        if !annots.is_empty() {
            d.set("Annots", Object::Array(annots));
        }
        self.doc.add_object(d)
    }
    fn finish(
        mut self,
        kids: Vec<ObjectId>,
        pages_res: Option<Dictionary>,
        catalog_extra: Dictionary,
    ) -> Document {
        let mut p = dictionary! {
            "Type" => "Pages",
            "Kids" => kids.iter().map(|k| Object::Reference(*k)).collect::<Vec<_>>(),
            "Count" => kids.len() as i64,
        };
        if let Some(r) = pages_res {
            p.set("Resources", r);
        }
        self.doc
            .objects
            .insert(self.pages_id, Object::Dictionary(p));
        let mut cat = dictionary! { "Type" => "Catalog", "Pages" => self.pages_id };
        for (k, v) in catalog_extra.iter() {
            cat.set(k.clone(), v.clone());
        }
        let cat_id = self.doc.add_object(cat);
        self.doc.trailer.set("Root", cat_id);
        self.doc
    }
    fn res(&self) -> Dictionary {
        dictionary! { "Font" => dictionary! { "F1" => self.font_id } }
    }
}

fn to_bytes(mut d: Document) -> Vec<u8> {
    let mut out = Vec::new();
    d.save_to(&mut out).unwrap();
    out
}

fn redact_bytes(src: &[u8], regions: &[RedactRegion]) -> Result<Vec<u8>, String> {
    let mut doc = Document::load_mem(src).unwrap();
    redact_regions(&mut doc, regions).map_err(|e| format!("{e:#}"))?;
    Ok(to_bytes(doc))
}

fn text_of(pdf: &[u8]) -> String {
    let p = std::env::temp_dir().join(format!("rl-vec-{}-{}.pdf", std::process::id(), pdf.len()));
    std::fs::write(&p, pdf).unwrap();
    let o = Command::new("pdftotext")
        .arg("-layout")
        .arg(&p)
        .arg("-")
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// Secret absent from pdftotext AND from every object of the file (streams inflated).
fn assert_gone(pdf: &[u8], secrets: &[&str]) {
    if have_pdftotext() {
        let t = text_of(pdf);
        for s in secrets {
            assert!(!t.contains(s), "{s} still in pdftotext output:\n{t}");
        }
    }
    let mut d = Document::load_mem(pdf).unwrap();
    d.decompress();
    let raw = to_bytes(d);
    let hay = String::from_utf8_lossy(&raw);
    for s in secrets {
        assert!(!hay.contains(s), "{s} still present in a file object");
    }
}

const R: fn() -> RedactRegion = || region(10.0, 140.0, 280.0, 30.0); // covers baseline y=150 text

#[test]
fn contents_array_second_stream() {
    let mut f = Fx::new();
    let a = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 50 Td (KEEPLINE) Tj ET");
    let b = f.stream(
        Dictionary::new(),
        "BT /F1 12 Tf 20 150 Td (ARRAYSECRET) Tj ET",
    );
    let res = f.res();
    let p = f.page(Object::Array(vec![a.into(), b.into()]), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    assert!(!have_pdftotext() || text_of(&pdf).contains("ARRAYSECRET"));
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(&out, &["ARRAYSECRET"]);
    if have_pdftotext() {
        assert!(text_of(&out).contains("KEEPLINE"));
    }
}

#[test]
fn text_split_across_show_operators_and_tj_arrays() {
    let mut f = Fx::new();
    let c = "BT /F1 12 Tf 20 150 Td (ACCT SPLI) Tj (TSECRET 99) Tj ET\n\
             BT /F1 12 Tf 20 120 Td [(TJARR) -300 (AYSECRET) 50 (TAIL)] TJ ET\n\
             BT /F1 12 Tf 20 100 Td (X) ' (QUOTESECRET) ' ET";
    let s = f.stream(Dictionary::new(), c);
    let res = f.res();
    let p = f.page(s.into(), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    let out = redact_bytes(&pdf, &[region(10.0, 90.0, 280.0, 80.0)]).unwrap();
    assert_gone(
        &out,
        &[
            "SPLITSECRET",
            "TJARRAYSECRET",
            "QUOTESECRET",
            "ACCT",
            "TAIL",
        ],
    );
}

#[test]
fn nested_forms() {
    let mut f = Fx::new();
    let inner = f.form("BT /F1 12 Tf 20 150 Td (INNERSECRET) Tj ET", None);
    let outer = f.form(
        "BT /F1 12 Tf 20 60 Td (OUTERKEEP) Tj ET\n/Fi Do",
        Some(dictionary! { "Fi" => inner }),
    );
    let s = f.stream(Dictionary::new(), "/Fo Do");
    let mut res = f.res();
    res.set("XObject", dictionary! { "Fo" => outer });
    let p = f.page(s.into(), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(&out, &["INNERSECRET"]);
    if have_pdftotext() {
        assert!(text_of(&out).contains("OUTERKEEP"));
    }
}

#[test]
fn annotations_popups_replies_and_form_fields() {
    let mut f = Fx::new();
    let s = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 20 Td (PAGEKEEP) Tj ET");
    let ap = f.form("BT /F1 12 Tf 2 2 Td (APPEARSECRET) Tj ET", None);
    let popup = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Popup",
        "Rect" => vec![200.into(), 10.into(), 290.into(), 40.into()],
        "Contents" => Object::string_literal("POPUPSECRET"),
    });
    let freetext = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText",
        "Rect" => vec![20.into(), 140.into(), 120.into(), 165.into()],
        "Contents" => Object::string_literal("ANNOTSECRET"),
        "AP" => dictionary! { "N" => ap }, "Popup" => popup,
    });
    // Popup's /Parent set after both exist.
    if let Ok(Object::Dictionary(d)) = f.doc.get_object_mut(popup) {
        d.set("Parent", freetext);
    }
    let reply = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text",
        "Rect" => vec![250.into(), 100.into(), 270.into(), 120.into()],
        "Contents" => Object::string_literal("REPLYSECRET"), "IRT" => freetext,
    });
    let widget = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("f1"),
        "V" => Object::string_literal("FIELDSECRET"),
        "Rect" => vec![30.into(), 145.into(), 100.into(), 160.into()],
    });
    let res = f.res();
    let p = f.page(
        s.into(),
        Some(res),
        vec![freetext.into(), popup.into(), reply.into(), widget.into()],
    );
    let acro =
        dictionary! { "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(widget)] } };
    let pdf = to_bytes(f.finish(vec![p], None, acro));
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(
        &out,
        &[
            "ANNOTSECRET",
            "APPEARSECRET",
            "POPUPSECRET",
            "REPLYSECRET",
            "FIELDSECRET",
        ],
    );
    if have_pdftotext() {
        assert!(text_of(&out).contains("PAGEKEEP"));
    }
}

#[test]
fn marked_content_actualtext_and_struct_tree() {
    let mut f = Fx::new();
    let s = f.stream(
        Dictionary::new(),
        "/Span << /ActualText (MCIDSECRET) >> BDC BT /F1 12 Tf 20 150 Td (visible) Tj ET EMC",
    );
    let res = f.res();
    let p = f.page(s.into(), Some(res), vec![]);
    let elem = f.doc.add_object(dictionary! {
        "Type" => "StructElem", "S" => "P", "ActualText" => Object::string_literal("STRUCTSECRET"),
    });
    let root = f
        .doc
        .add_object(dictionary! { "Type" => "StructTreeRoot", "K" => elem });
    let pdf = to_bytes(f.finish(vec![p], None, dictionary! { "StructTreeRoot" => root }));
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(&out, &["MCIDSECRET", "STRUCTSECRET"]);
}

#[test]
fn shared_inherited_resources_do_not_keep_the_original_form() {
    // Resources (with the secret form) live on the /Pages node; page 2 never uses it.
    let mut f = Fx::new();
    let form = f.form("BT /F1 12 Tf 20 150 Td (INHERITSECRET) Tj ET", None);
    let mut shared = f.res();
    shared.set("XObject", dictionary! { "Fsec" => form });
    let s1 = f.stream(Dictionary::new(), "/Fsec Do");
    let s2 = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 20 Td (PAGE2KEEP) Tj ET");
    let p1 = f.page(s1.into(), None, vec![]);
    let p2 = f.page(s2.into(), None, vec![]);
    let pdf = to_bytes(f.finish(vec![p1, p2], Some(shared), Dictionary::new()));
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(&out, &["INHERITSECRET"]);
}

#[test]
fn image_pixels_under_region_are_blanked_and_original_image_is_gone() {
    let mut f = Fx::new();
    let mut px = vec![0u8; 8 * 8 * 3];
    for (i, b) in px.iter_mut().enumerate() {
        *b = 150 + (i % 50) as u8; // never zero
    }
    let img = f.doc.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 8, "Height" => 8,
        "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        px.clone(),
    ));
    let s = f.stream(Dictionary::new(), "q 80 0 0 80 100 60 cm /Im Do Q");
    let mut res = f.res();
    res.set("XObject", dictionary! { "Im" => img });
    let p = f.page(s.into(), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    // Region covers the left half of the image.
    let out = redact_bytes(&pdf, &[region(100.0, 60.0, 40.0, 80.0)]).unwrap();
    let mut d = Document::load_mem(&out).unwrap();
    d.decompress();
    let mut images = 0;
    for o in d.objects.values() {
        if let Object::Stream(st) = o {
            let is_img = matches!(st.dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Image");
            // (the 1x1 black overlay the redactor draws is not the picture under test)
            if is_img && st.dict.get(b"Width").ok().and_then(|w| w.as_i64().ok()) == Some(8) {
                images += 1;
                assert_ne!(st.content, px, "unblanked original image still in the file");
                assert_eq!(&st.content[0..3], &[0, 0, 0]);
            }
        }
    }
    assert_eq!(images, 1, "exactly the blanked clone must remain");
}

#[test]
fn unreachable_to_clean_original_makes_redaction_refuse() {
    // The secret form is also referenced from a catalog key the redactor cannot rewrite:
    // success would be a lie, so it must refuse.
    let mut f = Fx::new();
    let form = f.form("BT /F1 12 Tf 20 150 Td (PINNEDSECRET) Tj ET", None);
    let s = f.stream(Dictionary::new(), "/Fm Do");
    let mut res = f.res();
    res.set("XObject", dictionary! { "Fm" => form });
    let p = f.page(s.into(), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, dictionary! { "Extra" => form }));
    let err = redact_bytes(&pdf, &[R()]).expect_err("must refuse, not report success");
    assert!(err.contains("redaction refused"), "{err}");
}

#[test]
fn redact_annotations_path_is_equally_thorough() {
    let mut f = Fx::new();
    let form = f.form("BT /F1 12 Tf 20 150 Td (ANNOTPATHSECRET) Tj ET", None);
    let s = f.stream(Dictionary::new(), "/Fm Do");
    let mut res = f.res();
    res.set("XObject", dictionary! { "Fm" => form });
    let mark = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Redact",
        "Rect" => vec![10.into(), 140.into(), 290.into(), 170.into()],
    });
    let p = f.page(s.into(), Some(res), vec![mark.into()]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    let mut doc = Document::load_mem(&pdf).unwrap();
    redact_annotations(&mut doc).unwrap();
    assert_gone(&to_bytes(doc), &["ANNOTPATHSECRET"]);
}

#[test]
fn bookmarks_info_xmp_and_fields_on_other_pages_do_not_repeat_the_text() {
    let mut f = Fx::new();
    let s1 = f.stream(
        Dictionary::new(),
        "BT /F1 12 Tf 20 150 Td (SECRETWORD) Tj ET",
    );
    let s2 = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 20 Td (PAGE2KEEP) Tj ET");
    let res = f.res();
    let p1 = f.page(s1.into(), Some(res.clone()), vec![]);
    // A form field on page 2 (not under the region) whose value repeats the secret.
    let widget = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("f2"),
        "V" => Object::string_literal("see SECRETWORD here"),
        "Rect" => vec![150.into(), 10.into(), 280.into(), 30.into()],
    });
    let p2 = f.page(s2.into(), Some(res), vec![widget.into()]);
    let item_secret = f.doc.add_object(dictionary! {
        "Title" => Object::string_literal("About SECRETWORD"),
        "Dest" => vec![p1.into(), "Fit".into()],
    });
    let item_keep = f.doc.add_object(dictionary! {
        "Title" => Object::string_literal("Unrelated chapter"),
        "Dest" => vec![p2.into(), "Fit".into()],
    });
    if let Ok(Object::Dictionary(d)) = f.doc.get_object_mut(item_secret) {
        d.set("Next", item_keep);
    }
    let outlines = f.doc.add_object(dictionary! {
        "Type" => "Outlines", "First" => item_secret, "Last" => item_keep, "Count" => 2,
    });
    let xmp = f.doc.add_object(Stream::new(
        dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
        b"<x:xmpmeta><dc:title>SECRETWORD memo</dc:title></x:xmpmeta>".to_vec(),
    ));
    let info = f
        .doc
        .add_object(dictionary! { "Title" => Object::string_literal("SECRETWORD report") });
    let extra = dictionary! {
        "Outlines" => outlines, "Metadata" => xmp,
        "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(widget)] },
    };
    let mut doc = f.finish(vec![p1, p2], None, extra);
    doc.trailer.set("Info", info);
    let pdf = to_bytes(doc);
    let out = redact_bytes(&pdf, &[R()]).unwrap();
    assert_gone(&out, &["SECRETWORD"]);
    if have_pdftotext() {
        assert!(text_of(&out).contains("PAGE2KEEP"));
    }
    // Bookmarks are dropped wholesale on redaction.
    let d = Document::load_mem(&out).unwrap();
    assert!(d.catalog().unwrap().get(b"Outlines").is_err());
}

fn one_secret_page(f: &mut Fx) -> ObjectId {
    let s = f.stream(
        Dictionary::new(),
        "BT /F1 12 Tf 20 150 Td (SECRETWORD) Tj ET",
    );
    let res = f.res();
    f.page(s.into(), Some(res), vec![])
}

#[test]
fn secret_only_in_info_xmp_or_bookmark_each_leaves_nothing() {
    for which in ["info", "xmp", "bookmark"] {
        let mut f = Fx::new();
        let p = one_secret_page(&mut f);
        let mut extra = Dictionary::new();
        let mut info = None;
        match which {
            "info" => {
                info =
                    Some(f.doc.add_object(
                        dictionary! { "Subject" => Object::string_literal("SECRETWORD") },
                    ));
            }
            "xmp" => {
                let x = f.doc.add_object(Stream::new(
                    dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
                    b"<x>SECRETWORD</x>".to_vec(),
                ));
                extra.set("Metadata", x);
            }
            _ => {
                let item = f.doc.add_object(dictionary! { "Title" => Object::string_literal("SECRETWORD"), "Dest" => vec![p.into(), "Fit".into()] });
                let ol = f.doc.add_object(dictionary! { "Type" => "Outlines", "First" => item, "Last" => item, "Count" => 1 });
                extra.set("Outlines", ol);
            }
        }
        let mut doc = f.finish(vec![p], None, extra);
        if let Some(i) = info {
            doc.trailer.set("Info", i);
        }
        let pdf = to_bytes(doc);
        // Sanity: the secret really is recoverable before redaction.
        assert!(
            String::from_utf8_lossy(&pdf).matches("SECRETWORD").count() >= 2,
            "{which}"
        );
        let out = redact_bytes(&pdf, &[R()]).unwrap();
        assert_gone(&out, &["SECRETWORD"]);
    }
}

#[test]
fn field_value_with_undecodable_removed_text_refuses_not_succeeds() {
    let mut f = Fx::new();
    let t0 = type0_font(&mut f);
    let s = f.stream(
        Dictionary::new(),
        "BT /F9 12 Tf 20 150 Td <004100420043> Tj ET",
    );
    let mut res = f.res();
    res.set("Font", dictionary! { "F1" => f.font_id, "F9" => t0 });
    let widget = f.doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("f"),
        "V" => Object::string_literal("ABC"), "Rect" => vec![150.into(), 10.into(), 280.into(), 30.into()],
    });
    let p = f.page(s.into(), Some(res), vec![widget.into()]);
    let acro =
        dictionary! { "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(widget)] } };
    let pdf = to_bytes(f.finish(vec![p], None, acro));
    let e = redact_bytes(&pdf, &[R()]).expect_err("must refuse, never Ok with a possible residual");
    assert!(e.contains("redaction refused"), "{e}");
}

fn type0_font(f: &mut Fx) -> ObjectId {
    let cid = f.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "Arial", "DW" => 600,
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
    });
    f.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Arial",
        "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)],
    })
}

#[test]
fn undecodable_text_is_removed_with_or_without_bookmarks() {
    for with_outline in [true, false] {
        let mut f = Fx::new();
        let t0 = type0_font(&mut f);
        let s = f.stream(
            Dictionary::new(),
            "BT /F9 12 Tf 20 150 Td <004100420043> Tj ET",
        );
        let mut res = f.res();
        res.set("Font", dictionary! { "F1" => f.font_id, "F9" => t0 });
        let p = f.page(s.into(), Some(res), vec![]);
        let mut extra = Dictionary::new();
        if with_outline {
            let item = f.doc.add_object(dictionary! { "Title" => Object::string_literal("Chapter"), "Dest" => vec![p.into(), "Fit".into()] });
            let ol = f.doc.add_object(
                dictionary! { "Type" => "Outlines", "First" => item, "Last" => item, "Count" => 1 },
            );
            extra.set("Outlines", ol);
        }
        let pdf = to_bytes(f.finish(vec![p], None, extra));
        let r = redact_bytes(&pdf, &[R()]);
        {
            let out = r.unwrap();
            let mut d = Document::load_mem(&out).unwrap();
            d.decompress();
            let page = *d.get_pages().values().next().unwrap();
            let c = String::from_utf8_lossy(&d.get_page_content(page)).to_string();
            assert!(!c.contains("<004100420043>") && !c.contains("ABC"), "{c}");
        }
    }
}

#[test]
fn untracked_position_after_type3_text_does_not_hide_later_text() {
    let mut f = Fx::new();
    let t3 = f.doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type3",
        "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()],
        "FontMatrix" => vec![0.001.into(), 0.into(), 0.into(), 0.001.into(), 0.into(), 0.into()],
        "CharProcs" => Dictionary::new(), "Encoding" => dictionary! { "Type" => "Encoding", "Differences" => vec![Object::Integer(65), "a".into(), "b".into()] },
        "FirstChar" => 65, "LastChar" => 66, "Widths" => vec![Object::Integer(1000), Object::Integer(1000)],
        "Resources" => Dictionary::new(),
    });
    let s = f.stream(
        Dictionary::new(),
        "BT /F3 12 Tf 20 150 Td (AB) Tj /F1 12 Tf (LATESECRET) Tj ET",
    );
    let mut res = f.res();
    res.set("Font", dictionary! { "F1" => f.font_id, "F3" => t3 });
    let p = f.page(s.into(), Some(res), vec![]);
    let pdf = to_bytes(f.finish(vec![p], None, Dictionary::new()));
    let out = redact_bytes(&pdf, &[region(60.0, 140.0, 200.0, 30.0)]).unwrap();
    let mut d = Document::load_mem(&out).unwrap();
    d.decompress();
    let page = *d.get_pages().values().next().unwrap();
    let c = String::from_utf8_lossy(&d.get_page_content(page)).to_string();
    assert!(!c.contains("LATESECRET"), "{c}");
}

/// A field on page 2 (not under the region) holding the redacted value reformatted.
fn reformatted_field_case(
    page_text: &str,
    field_value: &str,
    ap_text: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut f = Fx::new();
    let s1 = f.stream(
        Dictionary::new(),
        &format!("BT /F1 12 Tf 20 150 Td ({page_text}) Tj ET"),
    );
    let s2 = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 20 Td (PAGE2KEEP) Tj ET");
    let res = f.res();
    let p1 = f.page(s1.into(), Some(res.clone()), vec![]);
    let mut wd = dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("amt"),
        "V" => Object::string_literal(field_value),
        "Rect" => vec![150.into(), 10.into(), 280.into(), 30.into()],
    };
    if let Some(a) = ap_text {
        let ap = f.form(&format!("BT /F1 12 Tf 2 2 Td ({a}) Tj ET"), None);
        wd.set("AP", dictionary! { "N" => ap });
    }
    let widget = f.doc.add_object(wd);
    let p2 = f.page(s2.into(), Some(res), vec![widget.into()]);
    let acro =
        dictionary! { "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(widget)] } };
    let pdf = to_bytes(f.finish(vec![p1, p2], None, acro));
    redact_bytes(&pdf, &[R()])
}

fn field_value_survives(pdf: &[u8]) -> bool {
    let d = Document::load_mem(pdf).unwrap();
    d.objects
        .values()
        .any(|o| matches!(o, Object::Dictionary(dd) if dd.has(b"V") && dd.has(b"FT")))
}

#[test]
fn reformatted_field_values_are_cleared() {
    for (page, field) in [
        ("42", "total 42 units"),
        ("1234567", "1,234,567"),
        ("5551234", "555-1234"),
        ("(555) 123-4567", "5551234567"),
    ] {
        let out = reformatted_field_case(page, field, None).unwrap();
        assert!(
            !field_value_survives(&out),
            "field {field:?} survived redaction of {page:?}"
        );
        assert_gone(&out, &[field]);
    }
}

#[test]
fn field_appearance_text_is_scrubbed_too() {
    let out = reformatted_field_case("1234567", "unrelated", Some("1,234,567")).unwrap();
    assert_gone(&out, &["1,234,567"]);
}

#[test]
fn field_with_no_overlap_is_left_alone() {
    // Nothing in the field repeats the redacted text: kept (the narrow, documented residual).
    let out = reformatted_field_case("1234567", "hello world", None).unwrap();
    assert!(field_value_survives(&out));
}

#[test]
fn unclearable_matching_field_value_refuses() {
    // The field is an inline dictionary inside /Fields: it cannot be edited in place, so a
    // still-matching value must make redaction return Err, never Ok.
    let mut f = Fx::new();
    let s1 = f.stream(Dictionary::new(), "BT /F1 12 Tf 20 150 Td (1234567) Tj ET");
    let res = f.res();
    let p1 = f.page(s1.into(), Some(res), vec![]);
    let inline = dictionary! { "FT" => "Tx", "T" => Object::string_literal("a"), "V" => Object::string_literal("1,234,567") };
    let acro =
        dictionary! { "AcroForm" => dictionary! { "Fields" => vec![Object::Dictionary(inline)] } };
    let pdf = to_bytes(f.finish(vec![p1], None, acro));
    let e = redact_bytes(&pdf, &[R()]).expect_err("must refuse");
    assert!(e.contains("redaction refused"), "{e}");
}
