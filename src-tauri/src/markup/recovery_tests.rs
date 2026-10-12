//! Recovery from a PDF that changed on disk under redline: touch-only changes do not refuse,
//! Save As (rebuild) writes the user's mark-ups from the loaded bytes, reload-and-keep
//! re-applies unsaved edits, History restore refreshes reader-owned originals, an oversized
//! key rebuilds the markup, and an edited foreign polylength keeps /IT and /Measure.

use super::*;
use crate::document::annots::tests::{one_page_doc, redline_markup};
use crate::document::annots::{page_annots, read_markups, write_markups};
use crate::document::recover::{merge_unsaved, rebuild_inputs};
use crate::document::save::{load_markups_from, save_with_markups_removing};
use crate::document::store::{MarkupStore, ERR_DISK_CHANGED};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::path::{Path, PathBuf};

const ORIGINAL_AP: &[u8] = b"q 1 0 0 RG 5 5 m 50 5 l 50 40 l S Q ORIGINAL-REVU-AP";

fn form(doc: &mut Document, content: &[u8]) -> ObjectId {
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
        },
        content.to_vec(),
    ))
}

fn revu_cloud(doc: &mut Document, nm: &str) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Polygon", "F" => 4,
        "NM" => Object::string_literal(nm),
        "IT" => "PolygonCloud",
        "BE" => dictionary! { "S" => "C", "I" => 1 },
        "Rect" => vec![10.into(), 10.into(), 100.into(), 80.into()],
        "Vertices" => vec![10.into(), 10.into(), 100.into(), 10.into(), 100.into(), 80.into(), 10.into(), 80.into()],
        "Contents" => Object::string_literal("cloud"),
        "AP" => dictionary! { "N" => ap },
    })
}

fn revu_square(doc: &mut Document, nm: &str) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    // "NONM-<n>": a foreign square with no /NM, told apart by its position and /Contents.
    let mut d = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "Rect" => vec![200.into(), 200.into(), 260.into(), 240.into()],
        "AP" => dictionary! { "N" => ap },
    };
    if let Some(n) = nm.strip_prefix("NONM-") {
        let x: i64 = n.parse::<i64>().unwrap() * 100;
        d.set("Rect", vec![x.into(), 300.into(), (x + 50).into(), 340.into()]);
        d.set("Contents", Object::string_literal(nm));
    } else {
        d.set("NM", Object::string_literal(nm));
    }
    doc.add_object(d)
}

fn set_annots(doc: &mut Document, page: ObjectId, ids: &[ObjectId]) {
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("Annots", Object::Array(ids.iter().map(|i| (*i).into()).collect()));
}

/// A one-page file whose object numbers are shifted by `pad` unrelated streams created
/// before the annotations, with the given Revu-style annotations.
fn write_pdf(path: &Path, pad: usize, annots: &[&str]) {
    let (mut doc, page) = one_page_doc();
    for i in 0..pad {
        form(&mut doc, format!("IMPOSTER-STREAM-{i}").as_bytes());
    }
    let ids: Vec<ObjectId> = annots
        .iter()
        .map(|nm| {
            if nm.starts_with("CLOUD") {
                revu_cloud(&mut doc, nm)
            } else {
                revu_square(&mut doc, nm)
            }
        })
        .collect();
    set_annots(&mut doc, page, &ids);
    doc.save(path).unwrap();
}

fn page_dicts(path: &Path) -> Vec<Dictionary> {
    let doc = Document::load(path).unwrap();
    let page = *doc.get_pages().values().next().unwrap();
    page_annots(&doc, page).unwrap().into_iter().map(|(_, d)| d).collect()
}

fn nm_of(d: &Dictionary) -> String {
    String::from_utf8_lossy(d.get(b"NM").unwrap().as_str().unwrap()).into_owned()
}

fn loaded_store(path: &Path) -> MarkupStore {
    let s = MarkupStore::default();
    s.register("d", path.to_path_buf(), None);
    s.seed_loaded("d", load_markups_from(path, None).unwrap()).unwrap();
    s
}

fn set_mtime(path: &Path, secs_ahead: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs_ahead))
        .unwrap();
}

// --- N8 (a): an mtime-only change is not a change ----------------------------------------

#[test]
fn a_touch_or_sync_rewrite_with_the_same_bytes_does_not_refuse_but_a_real_change_does() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = loaded_store(&p);
    assert_eq!(s.check_disk_unchanged("d"), Ok(()));

    set_mtime(&p, 60); // touch: same bytes, new time
    assert_eq!(s.check_disk_unchanged("d"), Ok(()), "same content is not a change");
    assert_eq!(s.check_disk_unchanged("d"), Ok(()), "and the time was re-baselined");

    // Same size, different content, new time: caught by the hash.
    let mut bytes = std::fs::read(&p).unwrap();
    let at = bytes.windows(8).position(|w| w == b"ORIGINAL").unwrap();
    bytes[at..at + 8].copy_from_slice(b"CHANGED!");
    std::fs::write(&p, &bytes).unwrap();
    set_mtime(&p, 120);
    assert_eq!(s.check_disk_unchanged("d"), Err(ERR_DISK_CHANGED.to_string()));
    assert!(ERR_DISK_CHANGED.contains("Save As (rebuild)"), "{ERR_DISK_CHANGED}");
}

// --- N8 (b): Save As (rebuild) -------------------------------------------------------------

#[test]
fn rebuild_writes_the_users_markups_from_the_loaded_bytes_not_the_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001", "SQUAREREVU000001"]);
    let loaded_bytes = std::fs::read(&p).unwrap();
    let s = loaded_store(&p);
    // The user adds a mark-up and deletes the Revu square.
    s.add("d", redline_markup(0)).unwrap();
    let square = s
        .list("d")
        .unwrap()
        .into_iter()
        .find(|m| m.source_nm.as_deref() == Some("SQUAREREVU000001"))
        .unwrap();
    s.delete("d", square.id()).unwrap();

    // Another program rewrites the file with shifted object numbers.
    write_pdf(&p, 3, &["CLOUDREVU0000001", "SQUAREREVU000001", "OTHERREVU0000001"]);
    set_mtime(&p, 30);
    let changed_bytes = std::fs::read(&p).unwrap();
    assert_eq!(s.check_disk_unchanged("d"), Err(ERR_DISK_CHANGED.to_string()));

    let copy = s.original_copy("d").expect("a private copy of the loaded bytes");
    assert_eq!(std::fs::read(&copy).unwrap(), loaded_bytes, "the copy is the file as loaded");

    let (ms, removed) = rebuild_inputs(&s.list("d").unwrap(), &s.removed_sources("d"));
    assert!(ms.iter().all(|m| m.source_dict.is_none() && !m.is_unedited()));
    let out = dir.path().join("rebuilt.pdf");
    save_with_markups_removing(&copy, &out, &ms, &removed).unwrap();

    let got = page_dicts(&out);
    assert_eq!(got.len(), 2, "the cloud (rebuilt once) and the new mark-up; the square is gone");
    let cloud = got.iter().find(|d| nm_of(d) == "CLOUDREVU0000001").unwrap();
    assert!(cloud.has(b"RLType"), "rebuilt from the model, not written back");
    assert!(!cloud.has(crate::markup::foreign::PRESERVED_MARKER.as_bytes()));
    assert!(got.iter().all(|d| nm_of(d) != "SQUAREREVU000001" && nm_of(d) != "OTHERREVU0000001"));
    let raw = std::fs::read(&out).unwrap();
    assert!(
        !raw.windows(8).any(|w| w == b"IMPOSTER"),
        "nothing from the changed file leaks into the rebuilt one"
    );
    assert_eq!(std::fs::read(&p).unwrap(), changed_bytes, "the changed file is untouched");

    // Once written, the old file's names mean nothing.
    s.forget_source("d");
    assert!(s
        .list("d")
        .unwrap()
        .iter()
        .all(|m| m.source_ref.is_none() && m.source_dict.is_none() && m.source_fp.is_none()));
    assert!(s.removed_sources("d").is_empty());
}

#[test]
fn rebuilt_markups_are_stable_over_three_save_reload_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = loaded_store(&p);
    let (ms, removed) = rebuild_inputs(&s.list("d").unwrap(), &s.removed_sources("d"));
    let mut cur = dir.path().join("c0.pdf");
    save_with_markups_removing(&p, &cur, &ms, &removed).unwrap();
    for n in 1..=3 {
        let next = dir.path().join(format!("c{n}.pdf"));
        let ms = load_markups_from(&cur, None).unwrap();
        save_with_markups_removing(&cur, &next, &ms, &Default::default()).unwrap();
        cur = next;
        let got = page_dicts(&cur);
        assert_eq!(got.len(), 1, "cycle {n}");
        assert_eq!(nm_of(&got[0]), "CLOUDREVU0000001", "cycle {n}");
    }
}

// --- N8 (c): reload and keep ----------------------------------------------------------------

#[test]
fn unsaved_work_is_classified_against_the_file_as_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001", "SQUAREREVU000001"]);
    let s = loaded_store(&p);
    let (none, _, _) = s.begin_reload("d").unwrap().into_parts();
    assert!(none.added.is_empty() && none.modified.is_empty() && none.deleted.is_empty());

    let mut ms = s.list("d").unwrap();
    let cloud_pos = ms.iter().position(|m| m.source_nm.as_deref() == Some("CLOUDREVU0000001")).unwrap();
    ms[cloud_pos].contents = Some("edited".into());
    s.update("d", ms[cloud_pos].clone()).unwrap();
    let sq = ms.iter().find(|m| m.source_nm.as_deref() == Some("SQUAREREVU000001")).unwrap().id();
    s.delete("d", sq).unwrap();
    s.add("d", redline_markup(0)).unwrap();

    let (u, _, _) = s.begin_reload("d").unwrap().into_parts();
    assert_eq!((u.added.len(), u.modified.len(), u.deleted.len()), (1, 1, 1));
    assert_eq!(u.deleted, vec![sq]);

    // A save makes the file the baseline again.
    s.rebaseline_after_write("d");
    let (u, _, _) = s.begin_reload("d").unwrap().into_parts();
    assert!(u.added.is_empty() && u.modified.is_empty() && u.deleted.is_empty());
}

#[test]
fn reload_keeps_unsaved_edits_where_their_targets_still_exist_and_reports_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001", "SQUAREREVU000001"]);
    let s = loaded_store(&p);
    let ms = s.list("d").unwrap();
    let mut cloud = ms.iter().find(|m| m.source_nm.as_deref() == Some("CLOUDREVU0000001")).unwrap().clone();
    cloud.contents = Some("edited by me".into());
    s.update("d", cloud.clone()).unwrap();
    let sq = ms.iter().find(|m| m.source_nm.as_deref() == Some("SQUAREREVU000001")).unwrap().id();
    s.delete("d", sq).unwrap();
    let mine = redline_markup(0);
    let mine_id = mine.id();
    s.add("d", mine).unwrap();
    // The changed file: ids shifted, the square still there, a new Revu square from elsewhere.
    write_pdf(&p, 4, &["CLOUDREVU0000001", "SQUAREREVU000001", "OTHERREVU0000001"]);
    set_mtime(&p, 30);
    let txn = s.begin_reload("d").unwrap();
    let loaded = load_markups_from(&p, None).unwrap();
    let report = s.commit_reload("d", txn, loaded.clone(), 1).unwrap();
    assert_eq!(report.applied, 3, "{report:?}");
    assert!(report.skipped.is_empty(), "{report:?}");
    let merged = s.list("d").unwrap();
    assert!(merged.iter().any(|m| m.id() == mine_id));
    assert!(!merged.iter().any(|m| m.id() == sq), "the deletion was re-applied");
    assert!(merged.iter().any(|m| m.source_nm.as_deref() == Some("OTHERREVU0000001")));
    let kept = merged.iter().find(|m| m.id() == cloud.id()).unwrap();
    assert_eq!(kept.contents.as_deref(), Some("edited by me"));
    let fresh = loaded.iter().find(|m| m.id() == cloud.id()).unwrap();
    assert_eq!(kept.source_ref, fresh.source_ref, "matched against the reloaded file's objects");
    assert!(!kept.is_unedited(), "an edited markup is rebuilt");
    assert_eq!(s.removed_sources("d").len(), 1);

    // Installed and saved over the changed file: one copy of everything, nothing stale.
    assert_eq!(s.check_disk_unchanged("d"), Ok(()), "stamp and copy moved to the reloaded file");
    let out = dir.path().join("out.pdf");
    save_with_markups_removing(&p, &out, &s.list("d").unwrap(), &s.removed_sources("d")).unwrap();
    let names: Vec<String> = page_dicts(&out).iter().map(nm_of).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"CLOUDREVU0000001".to_string()));
    assert!(names.contains(&"OTHERREVU0000001".to_string()));
    assert!(!names.contains(&"SQUAREREVU000001".to_string()));
}

#[test]
fn reload_reports_edits_it_could_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001", "SQUAREREVU000001"]);
    let s = loaded_store(&p);
    let mut cloud = s
        .list("d")
        .unwrap()
        .into_iter()
        .find(|m| m.source_nm.as_deref() == Some("CLOUDREVU0000001"))
        .unwrap();
    cloud.contents = Some("edited".into());
    s.update("d", cloud).unwrap();
    s.add("d", redline_markup(0)).unwrap();
    let (unsaved, baseline, keys) = s.begin_reload("d").unwrap().into_parts();

    // The changed file no longer has the cloud, and has no page for the added mark-up.
    write_pdf(&p, 0, &["SQUAREREVU000001"]);
    let loaded = load_markups_from(&p, None).unwrap();
    let (merged, _, report) = merge_unsaved(loaded, &baseline, &keys, unsaved, 0);
    assert_eq!(report.applied, 0);
    assert_eq!(report.skipped.len(), 2, "{report:?}");
    assert!(report.skipped.iter().any(|l| l.contains("could not be matched")));
    assert!(report.skipped.iter().any(|l| l.contains("page is no longer")));
    assert_eq!(merged.len(), 1, "only the file's own square");
}

// --- N7: History restore refreshes reader-owned state ---------------------------------------

#[test]
fn a_restored_file_replaces_the_stored_originals_of_markups_it_shares() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = loaded_store(&p);
    let before = s.list("d").unwrap().remove(0);

    // The restored file has the same markup at other object numbers.
    let q = dir.path().join("restored.pdf");
    write_pdf(&q, 5, &["CLOUDREVU0000001"]);
    let restored = load_markups_from(&q, None).unwrap();
    let fresh = restored[0].clone();
    assert_ne!(before.source_ref, fresh.source_ref);
    let merged = s.seed_loaded("d", restored).unwrap();

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].source_ref, fresh.source_ref, "object id of the restored file");
    assert_eq!(merged[0].source_dict, fresh.source_dict, "original of the restored file");
    assert!(merged[0].is_unedited());
}

#[test]
fn a_user_edit_survives_a_reseed_and_still_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = loaded_store(&p);
    let mut m = s.list("d").unwrap().remove(0);
    m.contents = Some("mine".into());
    s.update("d", m).unwrap();
    let merged = s.seed_loaded("d", load_markups_from(&p, None).unwrap()).unwrap();
    assert_eq!(merged[0].contents.as_deref(), Some("mine"));
    assert!(!merged[0].is_unedited());
}

// --- N10: an over-budget key rebuilds, never a partial original -----------------------------

#[test]
fn a_key_over_the_inlining_budget_rebuilds_the_whole_markup() {
    let (mut doc, page) = one_page_doc();
    let big = doc.add_object(Object::Array((0..25_000).map(Object::Integer).collect()));
    let small = doc.add_object(Object::Array(vec![1.into(), 2.into()]));
    let ap = form(&mut doc, ORIGINAL_AP);
    let sq = |doc: &mut Document, nm: &str, key: ObjectId, ap: ObjectId| {
        doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "F" => 4,
            "NM" => Object::string_literal(nm),
            "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
            "Custom" => key,
            "AP" => dictionary! { "N" => ap },
        })
    };
    let a = sq(&mut doc, "BIGKEYREVU000001", big, ap);
    let b = sq(&mut doc, "SMALLKEYREVU0001", small, ap);
    set_annots(&mut doc, page, &[a, b]);
    let ms = read_markups(&doc).unwrap();
    let find = |nm: &str| ms.iter().find(|m| m.source_nm.as_deref() == Some(nm)).unwrap();
    assert!(find("BIGKEYREVU000001").source_dict.is_none(), "rebuilt, no partial original");
    assert!(find("SMALLKEYREVU0001").source_dict.is_some(), "within budget: preserved");
}

// --- polylength: an edited foreign one keeps /IT and /Measure --------------------------------

fn revu_polylength(doc: &mut Document, nm: &str, measure_indirect: bool) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    let measure = dictionary! {
        "Type" => "Measure", "Subtype" => "RL",
        "R" => Object::string_literal("1 in = 1 ft"),
        "X" => vec![dictionary! { "Type" => "NumberFormat", "U" => Object::string_literal("ft"), "C" => 1, "F" => "D" }.into()],
        "D" => vec![dictionary! { "Type" => "NumberFormat", "U" => Object::string_literal("ft"), "C" => 1 }.into()],
    };
    let measure: Object = if measure_indirect {
        doc.add_object(measure).into()
    } else {
        measure.into()
    };
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "PolyLine", "F" => 4,
        "NM" => Object::string_literal(nm),
        "IT" => "PolyLineDimension",
        "Rect" => vec![10.into(), 10.into(), 200.into(), 90.into()],
        "Vertices" => vec![10.into(), 10.into(), 100.into(), 50.into(), 200.into(), 90.into()],
        "Measure" => measure,
        "AP" => dictionary! { "N" => ap },
    })
}

fn edited_polylength_cycles(indirect: bool) {
    let (mut doc, page) = one_page_doc();
    let p = revu_polylength(&mut doc, "POLYLENREVU0001", indirect);
    set_annots(&mut doc, page, &[p]);
    for n in 1..=3 {
        let mut ms = read_markups(&doc).unwrap();
        assert_eq!(ms.len(), 1, "cycle {n}");
        ms[0].contents = Some(format!("edit {n}")); // an edit: rebuilt, not written back
        write_markups(&mut doc, &ms).unwrap();
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        doc = Document::load_mem(&buf).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let all = page_annots(&doc, page).unwrap();
        assert_eq!(all.len(), 1, "cycle {n}");
        let d = &all[0].1;
        assert!(d.has(b"RLType"), "cycle {n}: rebuilt");
        assert_eq!(d.get(b"IT").unwrap().as_name().unwrap(), b"PolyLineDimension", "cycle {n}");
        let m = d.get(b"Measure").unwrap().as_dict().unwrap();
        assert_eq!(m.get(b"Subtype").unwrap().as_name().unwrap(), b"RL", "cycle {n}");
        assert_eq!(m.get(b"R").unwrap().as_str().unwrap(), b"1 in = 1 ft", "cycle {n}");
        assert!(m.get(b"X").is_ok() && m.get(b"D").is_ok(), "cycle {n}: number formats kept");
        assert_eq!(d.get(b"Subtype").unwrap().as_name().unwrap(), b"PolyLine", "cycle {n}");
    }
}

#[test]
fn an_edited_foreign_polylength_keeps_it_and_measure_over_three_cycles() {
    edited_polylength_cycles(false);
}

#[test]
fn an_edited_foreign_polylength_with_an_indirect_measure_keeps_it_too() {
    edited_polylength_cycles(true);
}

#[test]
fn a_measure_carrying_an_action_key_is_not_carried() {
    let k = foreign::ForeignKey {
        name: "Measure".into(),
        value: foreign::PdfValue::Dict(vec![("JS".into(), foreign::PdfValue::Str(b"x".to_vec(), false))]),
    };
    assert!(foreign::validate(&k).is_err());
    let ok = foreign::ForeignKey {
        name: "Measure".into(),
        value: foreign::PdfValue::Dict(vec![("R".into(), foreign::PdfValue::Str(b"1:1".to_vec(), false))]),
    };
    assert!(foreign::validate(&ok).is_ok());
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}

// --- security: the private copy of the original bytes ---------------------------------------

#[cfg(unix)]
#[test]
fn the_private_copy_is_owner_only_randomly_named_and_unique_per_document() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = MarkupStore::default();
    s.register("d", p.clone(), None);
    s.register("e", p.clone(), None);
    let (c1, c2) = (s.original_copy("d").unwrap(), s.original_copy("e").unwrap());
    assert_ne!(c1, c2, "never shared across documents");
    assert_eq!(std::fs::metadata(&c1).unwrap().permissions().mode() & 0o777, 0o600);
    let parent = c1.parent().unwrap();
    assert_eq!(std::fs::metadata(parent).unwrap().permissions().mode() & 0o777, 0o700);
    let name = c1.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!name.contains(&std::process::id().to_string()), "random, not pid derived: {name}");
    assert_eq!(std::fs::read(&c1).unwrap(), std::fs::read(&p).unwrap());
}

#[cfg(unix)]
#[test]
fn a_symlink_planted_at_a_guessable_path_is_neither_followed_nor_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    // The paths an earlier design (and any guess) would use.
    let tmp = std::env::temp_dir();
    let guesses = [
        tmp.join("redline-originals").join(format!("{}-d.pdf", std::process::id())),
        tmp.join("redline-originals").join("d.pdf"),
        tmp.join(format!("{}-d.pdf", std::process::id())),
    ];
    let mut planted = Vec::new();
    for g in &guesses {
        if std::fs::create_dir_all(g.parent().unwrap()).is_ok()
            && std::os::unix::fs::symlink(&victim, g).is_ok()
        {
            planted.push(g.clone());
        }
    }
    let s = MarkupStore::default();
    s.register("d", p, None);
    s.invalidate_cache(&dir.path().join("a.pdf"));
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    let copy = s.original_copy("d").unwrap();
    assert!(!planted.contains(&copy));
    for g in planted {
        let _ = std::fs::remove_file(g);
    }
}

#[test]
fn the_private_copy_is_deleted_on_close_and_when_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = MarkupStore::default();
    s.register("d", p.clone(), None);
    let first = s.original_copy("d").unwrap();
    assert!(first.exists());
    s.invalidate_cache(&p); // a write re-takes the copy
    let second = s.original_copy("d").unwrap();
    assert_ne!(first, second);
    assert!(!first.exists(), "the replaced copy is deleted");
    s.remove("d");
    assert!(!second.exists(), "closing the document deletes its copy");
}

#[cfg(unix)]
#[test]
fn writing_the_output_never_goes_through_a_symlink_at_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.pdf");
    write_pdf(&src, 0, &["CLOUDREVU0000001"]);
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, b"precious").unwrap();
    let dest = dir.path().join("out.pdf");
    std::os::unix::fs::symlink(&victim, &dest).unwrap();
    let ms = load_markups_from(&src, None).unwrap();
    save_with_markups_removing(&src, &dest, &ms, &Default::default()).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"precious", "the link target is untouched");
    assert!(!std::fs::symlink_metadata(&dest).unwrap().file_type().is_symlink());
    assert_eq!(page_dicts(&dest).len(), 1);
    // No temp file is left beside it.
    let leftovers = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".redline-tmp-"))
        .count();
    assert_eq!(leftovers, 0);
}

// --- review round 1 ---------------------------------------------------------------------------

/// B1: a reload that fails after the snapshot step leaves everything as it was.
#[test]
fn a_failed_reload_leaves_the_stale_refusal_the_old_copy_and_the_old_markups_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let loaded_bytes = std::fs::read(&p).unwrap();
    let s = loaded_store(&p);
    let before = s.list("d").unwrap();

    write_pdf(&p, 3, &["CLOUDREVU0000001"]); // renumbered by another program
    set_mtime(&p, 30);
    assert_eq!(s.check_disk_unchanged("d"), Err(ERR_DISK_CHANGED.to_string()));

    // The reload starts (snapshot of the changed file in a local) and then fails in the parse
    // or the render reopen: the transaction is dropped without a commit.
    let txn = s.begin_reload("d").unwrap();
    drop(txn);

    assert_eq!(s.check_disk_unchanged("d"), Err(ERR_DISK_CHANGED.to_string()), "a plain save still refuses");
    let copy = s.original_copy("d").unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), loaded_bytes, "rebuild would still use the ORIGINAL bytes");
    assert_eq!(s.list("d").unwrap().len(), before.len());
    assert_eq!(s.list("d").unwrap()[0].source_ref, before[0].source_ref, "old object ids kept");

    // And the rebuild from that copy is still correct.
    let (ms, removed) = rebuild_inputs(&s.list("d").unwrap(), &s.removed_sources("d"));
    let out = dir.path().join("rebuilt.pdf");
    save_with_markups_removing(&copy, &out, &ms, &removed).unwrap();
    assert_eq!(page_dicts(&out).len(), 1);
    let raw = std::fs::read(&out).unwrap();
    assert!(!raw.windows(8).any(|w| w == b"IMPOSTER"));
}

#[test]
fn a_committed_reload_moves_stamp_copy_and_markups_together() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = loaded_store(&p);
    write_pdf(&p, 3, &["CLOUDREVU0000001"]);
    set_mtime(&p, 30);
    let changed = std::fs::read(&p).unwrap();
    let txn = s.begin_reload("d").unwrap();
    let loaded = load_markups_from(&p, None).unwrap();
    s.commit_reload("d", txn, loaded.clone(), 1).unwrap();
    assert_eq!(s.check_disk_unchanged("d"), Ok(()));
    assert_eq!(std::fs::read(s.original_copy("d").unwrap()).unwrap(), changed);
    assert_eq!(s.list("d").unwrap()[0].source_ref, loaded[0].source_ref);
}

/// B2: one huge shared /Measure referenced from many annotations is not cloned per annotation,
/// whatever makes it huge: elements, a long name, or a long dictionary key.
#[test]
fn a_huge_shared_indirect_measure_is_refused_without_a_clone_per_annotation() {
    let shapes: Vec<(&str, Object)> = vec![
        (
            "2M elements",
            Object::Dictionary(dictionary! {
                "X" => Object::Array((0..2_000_000).map(Object::Integer).collect())
            }),
        ),
        (
            "20 MB name",
            Object::Dictionary(dictionary! { "Subtype" => Object::Name(vec![b'x'; 20_000_000]) }),
        ),
        ("20 MB key", {
            let mut d = Dictionary::new();
            d.set(vec![b'k'; 20_000_000], 1);
            Object::Dictionary(d)
        }),
    ];
    for (label, big_obj) in shapes {
        let (mut doc, page) = one_page_doc();
        let big = doc.add_object(big_obj);
        let mut ids = Vec::new();
        for i in 0..300 {
            let ap = form(&mut doc, ORIGINAL_AP);
            ids.push(doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "PolyLine", "F" => 4,
                "NM" => Object::string_literal(format!("BIGMEASURE{i:06}")),
                "IT" => "PolyLineDimension",
                "Rect" => vec![10.into(), 10.into(), 200.into(), 90.into()],
                "Vertices" => vec![10.into(), 10.into(), 100.into(), 50.into(), 200.into(), 90.into()],
                "Measure" => big,
                "AP" => dictionary! { "N" => ap },
            }));
        }
        set_annots(&mut doc, page, &ids);
        let t = std::time::Instant::now();
        let ms = read_markups(&doc).unwrap();
        let took = t.elapsed();
        assert_eq!(ms.len(), 300, "{label}");
        assert!(
            ms.iter().all(|m| !m.foreign_keys.iter().any(|k| k.name == "Measure")),
            "{label}: refused"
        );
        assert!(took.as_millis() < 1000, "{label}: read took {took:?}");
    }
}

#[test]
fn a_normal_measure_still_fits_the_carry_budget() {
    let m = Object::Dictionary(dictionary! {
        "Type" => "Measure", "Subtype" => "RL",
        "R" => Object::string_literal("1 in = 1 ft"),
        "X" => vec![dictionary! { "U" => Object::string_literal("ft"), "C" => 1 }.into()],
    });
    assert!(foreign::fits_carry_budget(&m));
}

/// N2: the stamp, copy and ids come from one read even if the file changed after register.
#[test]
fn a_file_changed_between_open_and_load_is_loaded_consistently() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["CLOUDREVU0000001"]);
    let s = MarkupStore::default();
    s.register("d", p.clone(), None); // stamp + copy of the OLD bytes
    write_pdf(&p, 3, &["CLOUDREVU0000001"]); // changes before the markups are loaded
    set_mtime(&p, 30);
    let changed = std::fs::read(&p).unwrap();
    let snap = s.begin_first_load("d");
    let loaded = load_markups_from(&p, None).unwrap();
    s.seed_loaded_with("d", loaded.clone(), snap).unwrap();
    assert_eq!(s.check_disk_unchanged("d"), Ok(()), "consistent with the ids just read");
    assert_eq!(std::fs::read(s.original_copy("d").unwrap()).unwrap(), changed);
    // A second load never swaps the snapshot under an already loaded document.
    assert!(s.begin_first_load("d").is_none());
}

/// N1: foreign annotations without /NM are matched by content, not by their random ids.
#[test]
fn reload_matches_unnamed_foreign_markups_by_content_across_a_renumbering() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.pdf");
    write_pdf(&p, 0, &["NONM-1", "NONM-2", "NONM-3"]);
    let s = loaded_store(&p);
    let ms = s.list("d").unwrap();
    let by = |c: &str| ms.iter().find(|m| m.contents.as_deref() == Some(c)).unwrap().clone();
    s.delete("d", by("NONM-1").id()).unwrap();
    let mut b = by("NONM-2");
    b.contents = Some("edited B".into());
    s.update("d", b).unwrap();

    write_pdf(&p, 4, &["NONM-1", "NONM-2", "NONM-3"]); // renumbered, otherwise the same
    set_mtime(&p, 30);
    let txn = s.begin_reload("d").unwrap();
    let loaded = load_markups_from(&p, None).unwrap();
    let report = s.commit_reload("d", txn, loaded, 1).unwrap();
    assert!(report.skipped.is_empty(), "{report:?}");
    assert_eq!(report.applied, 2, "{report:?}");
    let got: Vec<Option<String>> = s.list("d").unwrap().iter().map(|m| m.contents.clone()).collect();
    assert!(!got.contains(&Some("NONM-1".into())), "the deletion was re-applied: {got:?}");
    assert!(got.contains(&Some("edited B".into())) && got.contains(&Some("NONM-3".into())), "{got:?}");
    // Saving removes the deleted original from the file.
    let out = dir.path().join("out.pdf");
    save_with_markups_removing(&p, &out, &s.list("d").unwrap(), &s.removed_sources("d")).unwrap();
    assert_eq!(page_dicts(&out).len(), 2);
}
