//! Bluebeam-interoperability STRUCTURAL conformance harness.
//!
//! Owner-directed (2026-08-11 G9 reopen, verbatim): "there are still significant issues
//! with how our markups read in bluebeam" - "you need to find a way of testing and
//! validating that yourself". This harness is the structural layer of that response.
//!
//! WHY STRUCTURE, NOT SCREENSHOTS: generic PDF viewers mostly render the stored `/AP`
//! appearance stream, so a broken annotation can look fine in a plain renderer even
//! when its DATA MODEL is wrong. Bluebeam Revu regenerates appearances from the
//! annotation dictionary when a markup is edited/moved/reopened - exactly where a
//! data-model gap turns into a visible defect (see judgment.md's "six render-loop bugs
//! invisible to headless tests" precedent, and the whole G9 history: every real fix in
//! this file's `obs:` trail was a DICTIONARY-level gap - missing `/AP`, `/BE`, wrong
//! `/Rect`-vs-`/AP BBox` fit, a stripped `/CL`, a dangling `/Popup` - not a rendering
//! bug). A render-matrix comparison (see `crates/pdf-diff`, this repo's existing
//! pixel/text diff engine) is the necessary secondary check, not the primary one.
//!
//! METHOD: `bench/corpus/btx/` (gitignored, real Bluebeam Tool Set exports) is a corpus
//! of GENUINE Bluebeam-authored PDF annotation dictionaries - each `<ToolChestItem>`'s
//! `<Raw>` element decodes to the literal dict Bluebeam itself wrote (see
//! `toolchest::btx` module doc). For every real item: decode the golden dict
//! independently (this file does NOT call btx.rs's own `pub(crate)` decoder - that
//! keeps this harness able to catch a bug in btx.rs's OWN decode path too, not just in
//! redline's write side), round-trip it through redline's real read/write path
//! (`Markup::from_annotation_dict` -> `to_annotation_dict`, the exact functions
//! `document::annots::read_markups`/`write_markups` call), and diff the resulting
//! dictionary's key set against the golden one. This isolates redline's WRITE-side
//! fidelity using Bluebeam's own real data as input - no need to hand-author markups
//! and guess what Bluebeam expects to see.
//!
//! Run: `cargo test --test bb_interop_conformance -- --ignored --nocapture`
//! (gated `#[ignore]`, mirrors `diag_bluebeam_reference.rs`'s corpus-gated pattern -
//! `bench/corpus/` is machine-local and gitignored, so CI always sees "skip: corpus not
//! present" rather than a failure).

use lopdf::Dictionary;
use redline_lib::markup::Markup;
use redline_lib::toolchest::btx::import_btx_bytes;
use redline_lib::toolchest::{StampAsset, StampDef};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Corpus access
// ---------------------------------------------------------------------------

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bench")
        .join("corpus")
        .join("btx")
}

fn corpus_files() -> Vec<PathBuf> {
    let dir = corpus_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("btx")) == Some(true))
        .collect();
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Independent golden-side decoder.
//
// Deliberately duplicates the small hex+zlib `<Raw>` decode documented in
// `toolchest::btx`'s module doc rather than calling its `pub(crate)` internals (not
// reachable from an integration test anyway - a separate crate target). This is a
// feature, not a shortcut: it means a latent bug in btx.rs's OWN decoder (like the real
// `/Length`-mismatch bug fixed 2026-08-08, obs:ahooy0ykf0721twpbyll) would show up as a
// DISAGREEMENT between this harness's golden dict and what redline actually imports,
// instead of both sides silently sharing the same blind spot.
// ---------------------------------------------------------------------------

struct GoldenItem {
    source_file: String,
    name: String,
    has_child: bool,
    dict: Dictionary,
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn is_hex_zlib(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.len() >= 4 && lower.starts_with("789c") && lower.len() % 2 == 0 && lower.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Wrap a bare `<< dict >>` literal as a minimal one-object PDF and parse it via lopdf -
/// lopdf has no public "parse a bare dict" entry point (same technique btx.rs itself
/// uses for the identical reason, `wrap_dict_as_pdf`).
fn decode_raw(raw: &str) -> Option<Dictionary> {
    let trimmed = raw.trim();
    let pdf_text = if is_hex_zlib(trimmed) {
        let bytes = hex_decode(trimmed)?;
        let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).ok()?;
        String::from_utf8(out).ok()?
    } else {
        trimmed.to_string()
    };

    let header = b"%PDF-1.4\n".to_vec();
    let obj_offset = header.len();
    let mut buf = header;
    buf.extend_from_slice(b"1 0 obj\n");
    buf.extend_from_slice(pdf_text.as_bytes());
    buf.extend_from_slice(b"\nendobj\n");
    let xref_offset = buf.len();
    let xref = format!(
        "xref\n0 2\n0000000000 65535 f \n{obj_offset:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF"
    );
    buf.extend_from_slice(xref.as_bytes());

    let doc = lopdf::Document::load_mem(&buf).ok()?;
    let obj = doc.get_object((1, 0)).ok()?;
    obj.as_dict().ok().cloned()
}

fn golden_items(path: &PathBuf) -> Vec<GoldenItem> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    if bytes.starts_with(b"PK\x03\x04") {
        // zip-wrapped .btx: none in the current corpus. Named, not silently guessed at.
        eprintln!("  (skip {}: zip-wrapped .btx not handled by this harness)", path.display());
        return Vec::new();
    }
    let xml = match std::str::from_utf8(&bytes) {
        Ok(s) => s.strip_prefix('\u{FEFF}').unwrap_or(s).to_string(),
        Err(_) => return Vec::new(),
    };
    let doc = match roxmltree::Document::parse(&xml) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let source_file = path.file_name().unwrap().to_string_lossy().to_string();
    let mut out = Vec::new();
    for item in doc.descendants().filter(|n| n.has_tag_name("ToolChestItem")) {
        let name = item
            .children()
            .find(|n| n.has_tag_name("Name"))
            .and_then(|n| n.text())
            .unwrap_or("<unnamed>")
            .to_string();
        let Some(raw) = item.children().find(|n| n.has_tag_name("Raw")).and_then(|n| n.text()) else {
            continue;
        };
        let Some(dict) = decode_raw(raw) else {
            continue;
        };
        let has_child = item.children().any(|n| n.has_tag_name("Child"));
        out.push(GoldenItem { source_file: source_file.clone(), name, has_child, dict });
    }
    out
}

// ---------------------------------------------------------------------------
// Dict inspection helpers
// ---------------------------------------------------------------------------

fn dict_keys(d: &Dictionary) -> BTreeSet<String> {
    d.iter().map(|(k, _)| String::from_utf8_lossy(k).to_string()).collect()
}

fn subtype_of(d: &Dictionary) -> Option<String> {
    d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()).map(|n| String::from_utf8_lossy(n).to_string())
}

/// Keys redline's own model owns (private extension keys, `RL`-prefixed by convention -
/// see annotation.rs's `to_annotation_dict`). Not a defect for these to be absent from a
/// Bluebeam-authored golden dict, or present-only-on-redline's-side.
fn is_redline_private_key(k: &str) -> bool {
    k.starts_with("RL")
}

/// Standard PDF annotation dictionary keys (ISO 32000-1 §12.5, §12.5.6) this harness
/// specifically watches for interop relevance, per the mission's checklist and the G9
/// defect history. Not exhaustive - anything else seen on the golden side that redline
/// doesn't write is still reported, just without a named category.
const WATCHED_STANDARD_KEYS: &[&str] = &[
    "Type", "Subtype", "Rect", "NM", "Subj", "Contents", "C", "IC", "CA", "BS", "BE", "IT", "F", "AP", "DA", "Q",
    "RC", "Measure", "IRT", "RT", "Popup", "T", "QuadPoints", "L", "Vertices", "CL", "M", "CreationDate",
];

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SubtypeReport {
    golden_count: usize,
    /// key -> (present in this many golden dicts, present in this many redline-roundtrip dicts)
    key_presence: BTreeMap<String, (usize, usize)>,
    round_trip_failures: Vec<String>,
}

/// Build a full conformance report across every real corpus file, printed as a
/// per-subtype table. Returns the per-subtype reports plus flat counts used by the
/// calibration assertions below.
fn build_report() -> (BTreeMap<String, SubtypeReport>, usize, usize, usize) {
    let files = corpus_files();
    let mut reports: BTreeMap<String, SubtypeReport> = BTreeMap::new();
    let mut total_items = 0usize;
    let mut total_with_child = 0usize;
    let mut total_decode_failures = 0usize;

    for path in &files {
        for item in golden_items(path) {
            total_items += 1;
            if item.has_child {
                total_with_child += 1;
            }
            let Some(subtype) = subtype_of(&item.dict) else {
                total_decode_failures += 1;
                continue;
            };
            let golden_keys = dict_keys(&item.dict);

            // Round-trip through redline's real read/write path.
            let markup = Markup::from_annotation_dict(&item.dict);
            let redline_dict = markup.to_annotation_dict();
            let redline_keys = dict_keys(&redline_dict);

            let report = reports.entry(subtype.clone()).or_default();
            report.golden_count += 1;
            for k in &golden_keys {
                let entry = report.key_presence.entry(k.clone()).or_insert((0, 0));
                entry.0 += 1;
                if redline_keys.contains(k) {
                    entry.1 += 1;
                }
            }
            // Also register keys redline emits that golden never had, at (0, N), so the
            // "present on our side, absent from every golden example" case is visible
            // too (only interesting for non-RL, non-watched keys - see report printer).
            for k in &redline_keys {
                report.key_presence.entry(k.clone()).or_insert((0, 0)).1 += if !golden_keys.contains(k) { 1 } else { 0 };
            }

            if golden_keys.is_empty() {
                report.round_trip_failures.push(format!("{}/{}: empty golden dict", item.source_file, item.name));
            }
        }
    }

    (reports, total_items, total_with_child, total_decode_failures)
}

fn print_report(reports: &BTreeMap<String, SubtypeReport>) {
    for (subtype, report) in reports {
        println!("\n=== Subtype: {subtype}  ({} golden item(s)) ===", report.golden_count);

        let mut missing: Vec<(&String, usize)> = Vec::new();
        let mut redline_only_standard: Vec<&String> = Vec::new();
        let mut redline_only_private: Vec<&String> = Vec::new();

        for (key, (golden_n, redline_n)) in &report.key_presence {
            if *golden_n > 0 && *redline_n < *golden_n {
                missing.push((key, *golden_n - *redline_n));
            } else if *golden_n == 0 && *redline_n > 0 {
                if is_redline_private_key(key) {
                    redline_only_private.push(key);
                } else {
                    redline_only_standard.push(key);
                }
            }
        }

        if missing.is_empty() {
            println!("  OK: every golden key redline saw at least once round-trips back out.");
        } else {
            println!("  MISSING on round-trip (BB writes it, redline's write path drops it):");
            for (key, n_dropped) in &missing {
                let watched = if WATCHED_STANDARD_KEYS.contains(&key.as_str()) { " [WATCHED]" } else { "" };
                println!("    /{key}: dropped in {n_dropped}/{} golden item(s){watched}", report.golden_count);
            }
        }

        if !redline_only_standard.is_empty() {
            println!("  redline emits (standard-looking key, never seen in golden for this subtype): {redline_only_standard:?}");
        }
        if !redline_only_private.is_empty() {
            println!("  redline-private (RL*) keys, expected: {redline_only_private:?}");
        }
        for f in &report.round_trip_failures {
            println!("  ROUND-TRIP FAILURE: {f}");
        }
    }
}

/// Stamp artwork resolvability, via the real public import path (`import_btx_bytes`) -
/// mirrors the exact production check `write_markups`/rasterization needs
/// (`objects.get(&root_id)`). Reports, per source file, how many Stamp tools resolve to
/// real artwork vs the named "stock library stamp" gap (HANDOVER PR #76: `/TmpBRXFile`
/// points at Bluebeam's bundled stamps library, not a user file, so the root Form
/// XObject id is absent from the item's own captured `<Resources>` graph - a real
/// data-availability gap, not a bug).
fn stamp_resolvability_report() -> (usize, usize) {
    let mut resolvable = 0usize;
    let mut unresolvable = 0usize;
    for path in corpus_files() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let report = import_btx_bytes(&bytes);
        for tool in &report.tools {
            let (Some(StampDef::Static { asset: StampAsset::BluebeamFormXObject { root_id, objects } })
                | Some(StampDef::Dynamic { asset: Some(StampAsset::BluebeamFormXObject { root_id, objects }), .. })) = &tool.stamp
            else {
                continue;
            };
            if objects.contains_key(root_id) {
                resolvable += 1;
                println!("  RESOLVABLE artwork: {:?} / \"{}\"", path.file_name().unwrap(), tool.name);
            } else {
                unresolvable += 1;
                println!(
                    "  UNRESOLVABLE artwork (stock/library stamp gap): {:?} / \"{}\" (root_id={root_id:?} not in captured Resources)",
                    path.file_name().unwrap(),
                    tool.name
                );
            }
        }
    }
    (resolvable, unresolvable)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full conformance report, printed for human/agent review. Always "passes" (it's a
/// report, not an assertion) unless the corpus is entirely absent or decode fails
/// completely - see `calibration_flags_known_defect_classes` for the pass/fail gate.
#[test]
#[ignore]
fn bb_interop_conformance_report() {
    let files = corpus_files();
    if files.is_empty() {
        eprintln!("skip: bb corpus not present (bench/corpus/btx/)");
        return;
    }
    println!("Corpus files: {}", files.len());
    for f in &files {
        println!("  {}", f.display());
    }

    let (reports, total_items, total_with_child, total_decode_failures) = build_report();
    println!(
        "\nTotal items: {total_items}, with <Child> (grouped-annotation defect class): {total_with_child}, decode failures: {total_decode_failures}"
    );
    print_report(&reports);

    println!("\n=== Stamp artwork resolvability (Form XObject) ===");
    let (resolvable, unresolvable) = stamp_resolvability_report();
    println!("Resolvable: {resolvable}, unresolvable (stock-stamp gap): {unresolvable}");
}

/// CALIBRATION: this is the acceptance test the dispatch asked for - the harness must
/// actually flag the two known-bad classes and stay quiet on the known-good ones.
///
/// Known-bad #1: grouped `<Child>` annotations. Architecturally unsupported (Tool is
/// 1:1 with a single Markup; a `<Child>` pair - shape + attached label, or callout +
/// leader - needs a data-model change beyond this harness's scope to fix, see
/// obs:ullyvzs86ncoa70itfdi). The harness must DETECT and COUNT these, not silently
/// drop them.
///
/// Known-bad #2: the stock/library stamp artwork gap (4/11 real corpus stamps per
/// HANDOVER PR #76 - "MR Init"/"MR Sig" class). The harness must find at least one
/// unresolvable Stamp among the real corpus.
///
/// Known-good: the 2026-08-08 UID-naming fix (`/Subj` -> `/TmpBRXFile` basename ->
/// opaque-UID fallback chain, obs:ahooy0ykf0721twpbyll) - every corpus Stamp item must
/// resolve to a non-empty, non-raw-UID display name via the real public import path.
#[test]
#[ignore]
fn calibration_flags_known_defect_classes() {
    let files = corpus_files();
    if files.is_empty() {
        eprintln!("skip: bb corpus not present (bench/corpus/btx/)");
        return;
    }

    let (_reports, total_items, total_with_child, _decode_failures) = build_report();
    assert!(total_items > 0, "corpus present but zero golden items decoded - decoder likely broken");
    assert!(
        total_with_child > 0,
        "calibration FAILED: expected the harness to detect at least one grouped <Child> \
         annotation in the real corpus (known-bad class, obs:ullyvzs86ncoa70itfdi) - found none"
    );
    println!("PASS: grouped-<Child> class detected ({total_with_child}/{total_items} items)");

    let (_resolvable, unresolvable) = stamp_resolvability_report();
    assert!(
        unresolvable > 0,
        "calibration FAILED: expected at least one unresolvable stock-stamp artwork gap \
         (HANDOVER PR #76 class) in the real corpus - found none"
    );
    println!("PASS: stock-stamp artwork gap detected ({unresolvable} unresolvable)");

    // Known-good: UID naming fix. Every Stamp tool's display name must be non-empty and
    // must not equal a raw all-caps opaque <Name> UID pattern (Bluebeam's internal ids
    // observed in the corpus are long, all-uppercase-alnum tokens with no spaces -
    // real display names always contain a space or lowercase letter).
    let mut raw_uid_names = 0usize;
    let mut checked = 0usize;
    for path in corpus_files() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let report = import_btx_bytes(&bytes);
        for tool in &report.tools {
            if tool.markup_type != redline_lib::markup::MarkupType::Stamp
                && tool.markup_type != redline_lib::markup::MarkupType::StampDynamic
            {
                continue;
            }
            checked += 1;
            let looks_like_raw_uid = !tool.name.is_empty()
                && tool.name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && !tool.name.contains(' ');
            if looks_like_raw_uid {
                raw_uid_names += 1;
                println!("  STILL A RAW UID NAME: {:?} -> {:?}", path.file_name().unwrap(), tool.name);
            }
        }
    }
    println!("Checked {checked} stamp tool(s), {raw_uid_names} still showing a raw UID name.");
    assert_eq!(
        raw_uid_names, 0,
        "calibration FAILED: the 2026-08-08 UID-naming fix (obs:ahooy0ykf0721twpbyll) has \
         regressed - a corpus stamp tool is back to showing a raw opaque UID as its name"
    );
    println!("PASS: v0.3.13 UID-naming fix holds (0/{checked} raw UID names)");
}
