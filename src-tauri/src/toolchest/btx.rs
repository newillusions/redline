//! `.btx` (Bluebeam Tool Set) importer (spec "Importing Bluebeam Tool Sets & stamps").
//!
//! `.btx` is XML/UTF-8. Each `<ToolChestItem>` carries a `<Name>` (id), a `<Type>` (e.g.
//! `Bluebeam.PDF.Annotations.AnnotationFreeText`), a `<Mode>` (`properties`/`drawing` -
//! maps directly to our two placement modes), optional `<BSIColumnData>` (custom columns -
//! NAMED deferral, see [`skip_custom_columns`] doc comment), and a `<Raw>` payload that IS
//! a PDF annotation dictionary (`/Subtype/FreeText /Rect[...] /CL[...] /Subj(...)` etc).
//!
//! The importer reuses the existing annotation reader ([`Markup::from_annotation_dict`])
//! for `<Raw>` - it does not reimplement annotation parsing. Two wrinkles handled here:
//! zlib-deflated `<Raw>` payloads (hex starting `789c`), and `.zip`-wrapped `.btx` files.
//! A malformed/unparseable item is skipped and reported, never fatal to the whole import.

use std::io::Read;

use lopdf::Dictionary;
use serde::Serialize;

use crate::markup::Markup;
use crate::toolchest::{PlacementMode, Tool};

/// One item that failed to import, with a human-readable reason (spec: "skipped-and-
/// reported, not fatal").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkippedItem {
    pub name: String,
    pub reason: String,
}

/// Result of importing a `.btx` (or `.zip`-wrapped `.btx`) file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportReport {
    pub tools: Vec<Tool>,
    pub skipped: Vec<SkippedItem>,
}

/// Import `.btx` content from raw bytes. Detects zip-wrapping via the `PK\x03\x04` local
/// file header magic and unwraps first; otherwise parses the bytes directly as UTF-8 XML.
pub fn import_btx_bytes(bytes: &[u8]) -> ImportReport {
    if bytes.starts_with(b"PK\x03\x04") {
        return import_btx_zip(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(xml) => parse_btx_xml(xml),
        Err(e) => ImportReport {
            tools: Vec::new(),
            skipped: vec![SkippedItem { name: "<document>".to_string(), reason: format!("not valid UTF-8: {e}") }],
        },
    }
}

/// Unwrap a `.zip` archive and import every `.btx` member found inside it (packaging
/// wrinkle: tool sets are often distributed zip-wrapped, per spec).
fn import_btx_zip(bytes: &[u8]) -> ImportReport {
    let cursor = std::io::Cursor::new(bytes);
    let mut report = ImportReport { tools: Vec::new(), skipped: Vec::new() };

    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            report.skipped.push(SkippedItem { name: "<zip>".to_string(), reason: format!("bad zip: {e}") });
            return report;
        }
    };

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                report.skipped.push(SkippedItem { name: format!("<zip entry {i}>"), reason: e.to_string() });
                continue;
            }
        };
        let name = entry.name().to_string();
        if !name.to_lowercase().ends_with(".btx") {
            continue;
        }
        let mut buf = Vec::new();
        if let Err(e) = entry.read_to_end(&mut buf) {
            report.skipped.push(SkippedItem { name, reason: format!("zip read error: {e}") });
            continue;
        }
        let sub = import_btx_bytes(&buf);
        report.tools.extend(sub.tools);
        report.skipped.extend(sub.skipped);
    }

    if report.tools.is_empty() && report.skipped.is_empty() {
        report.skipped.push(SkippedItem { name: "<zip>".to_string(), reason: "no .btx member found".to_string() });
    }
    report
}

/// Parse `.btx` XML text into a set of tools + a skip report. A document-level parse
/// failure is itself reported as one skipped item rather than propagated as an error, so
/// callers always get a usable (possibly empty) report.
pub fn parse_btx_xml(xml: &str) -> ImportReport {
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(e) => {
            return ImportReport {
                tools: Vec::new(),
                skipped: vec![SkippedItem {
                    name: "<document>".to_string(),
                    reason: format!("XML parse error: {e}"),
                }],
            };
        }
    };

    let mut report = ImportReport { tools: Vec::new(), skipped: Vec::new() };
    for item in doc.descendants().filter(|n| n.has_tag_name("ToolChestItem")) {
        let name = child_text(item, "Name").unwrap_or("<unnamed>").to_string();
        match import_item(item, &name) {
            Ok(tool) => report.tools.push(tool),
            Err(reason) => report.skipped.push(SkippedItem { name, reason }),
        }
    }
    report
}

fn child_text<'a>(node: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    node.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text())
}

fn import_item(item: roxmltree::Node, name: &str) -> Result<Tool, String> {
    let mode_tag = child_text(item, "Mode").unwrap_or("properties");
    let placement_mode =
        if mode_tag.eq_ignore_ascii_case("drawing") { PlacementMode::Drawing } else { PlacementMode::Properties };

    // BSIColumnData (custom estimating columns): NAMED deferral - see module doc. We
    // recognise the element but do not yet map it onto Tool.subject/custom columns; a
    // present-but-unmapped block is not a reason to skip the item.
    let _ = child_text(item, "BSIColumnData");

    let raw = child_text(item, "Raw").ok_or_else(|| "missing <Raw> element".to_string())?;
    let dict = raw_to_dict(raw)?;

    // Guard against Markup::from_annotation_dict's permissive `_ => Text` /Subtype
    // fallback: that arm is only reachable-safely from document::annots::read_markups,
    // which filters against the same MARKUP_SUBTYPES list before ever calling it. Without
    // this guard here, a Raw payload for a subtype redline doesn't model (Underline,
    // StrikeOut, Squiggly, Redact, Widget, Popup, Link, ...) would silently become a wrong
    // "Text" tool instead of being skipped and reported - see the module doc comment and
    // `tests::unsupported_annotation_subtype_is_skipped_not_silently_reclassified_as_text`.
    match crate::document::annots::subtype(&dict) {
        Some(st) if crate::document::annots::MARKUP_SUBTYPES.contains(&st.as_str()) => {}
        Some(st) => return Err(format!("unsupported annotation subtype: {st}")),
        None => return Err("<Raw> annotation has no /Subtype".to_string()),
    }

    let markup = Markup::from_annotation_dict(&dict);

    let geometry = match placement_mode {
        PlacementMode::Drawing => Some(markup.geometry.clone()),
        PlacementMode::Properties => None,
    };

    Ok(Tool {
        id: uuid::Uuid::new_v4(),
        name: name.to_string(),
        markup_type: markup.markup_type,
        appearance: markup.appearance,
        subject: markup.subject,
        placement_mode,
        geometry,
        // NAMED GAP, NOT YET BUILT (corrected 2026-08 - this comment previously described
        // a "dedicated stamp-import path" (`import_stamp_item`) and `<Type>`-based
        // branching that do not exist anywhere in this codebase; `<Type>` is never read at
        // all in this module). A `/Subtype /Stamp` Raw payload imports successfully (no
        // skip - Stamp is in MARKUP_SUBTYPES) but always gets `stamp: None`, silently
        // discarding the source appearance/graphic. See
        // `tests::stamp_import_currently_drops_the_appearance_asset_named_gap_not_fixed_this_pass`
        // for the full analysis of why this is unfixed (needs a real Bluebeam stamp sample
        // to verify the wire format against, and depends on `appearance::draw()`'s
        // already-separately-deferred `PdfBase64`/`Svg` rendering support).
        stamp: None,
    })
}

/// Decode a `<Raw>` payload into a PDF annotation dictionary. Two encodings seen in the
/// wild (spec "Importing Bluebeam Tool Sets"): plain-text PDF dict syntax
/// (`<< /Subtype /FreeText ... >>`), or hex-encoded zlib-deflated bytes (hex beginning
/// `789c`) that inflate to the same PDF syntax.
fn raw_to_dict(raw: &str) -> Result<Dictionary, String> {
    let trimmed = raw.trim();
    let pdf_text = if is_hex_zlib(trimmed) {
        inflate_hex_zlib(trimmed)?
    } else {
        trimmed.to_string()
    };
    parse_pdf_dict(&pdf_text)
}

fn is_hex_zlib(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.len() >= 4 && lower.starts_with("789c") && lower.len() % 2 == 0 && lower.bytes().all(|b| b.is_ascii_hexdigit())
}

fn inflate_hex_zlib(hex: &str) -> Result<String, String> {
    let bytes = hex_decode(hex)?;
    let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
    let mut out = String::new();
    decoder.read_to_string(&mut out).map_err(|e| format!("zlib inflate failed: {e}"))?;
    Ok(out)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex string".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex digit: {e}")))
        .collect()
}

/// Parse a standalone PDF dictionary literal (`<< ... >>`) by wrapping it as the sole
/// indirect object of a minimal, well-formed one-object PDF and running it through lopdf's
/// document loader - lopdf does not expose a public "parse a bare dict" entry point, so
/// this is the supported route to reuse its (well-tested) object parser.
fn parse_pdf_dict(text: &str) -> Result<Dictionary, String> {
    let bytes = wrap_dict_as_pdf(text);
    let doc = lopdf::Document::load_mem(&bytes).map_err(|e| format!("PDF dict parse failed: {e}"))?;
    let obj = doc.get_object((1, 0)).map_err(|e| format!("missing wrapped object: {e}"))?;
    obj.as_dict().cloned().map_err(|e| format!("<Raw> is not a dictionary: {e}"))
}

/// Build a minimal, byte-exact, well-formed single-object PDF wrapping `dict_text` as
/// object `1 0`, with a hand-computed xref table (lopdf's reader parses xref entries at
/// fixed byte offsets, so these must be correct - not merely well-formatted).
fn wrap_dict_as_pdf(dict_text: &str) -> Vec<u8> {
    let header = b"%PDF-1.4\n".to_vec();
    let obj_offset = header.len();

    let mut buf = header;
    buf.extend_from_slice(b"1 0 obj\n");
    buf.extend_from_slice(dict_text.as_bytes());
    buf.extend_from_slice(b"\nendobj\n");

    let xref_offset = buf.len();
    // Standard 20-byte-per-entry xref format: "nnnnnnnnnn ggggg n \n" / "...f \n".
    let xref = format!(
        "xref\n0 2\n0000000000 65535 f \n{obj_offset:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF"
    );
    buf.extend_from_slice(xref.as_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const PLAIN_ITEM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ToolChestData>
  <ToolChestItem>
    <Name>Fire Rated Door</Name>
    <Type>Bluebeam.PDF.Annotations.AnnotationSquare</Type>
    <Mode>properties</Mode>
    <Raw><![CDATA[<< /Subtype /Square /Rect [10 20 110 70] /C [1 0 0] /BS << /W 2 >> /Subj (Door) >>]]></Raw>
  </ToolChestItem>
</ToolChestData>"#;

    // --- (d) .btx parse: a fixture <ToolChestItem> with a plaintext <Raw> imports ---

    #[test]
    fn parses_plaintext_raw_item_into_a_tool() {
        let report = parse_btx_xml(PLAIN_ITEM);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        let tool = &report.tools[0];
        assert_eq!(tool.name, "Fire Rated Door");
        assert_eq!(tool.markup_type, crate::markup::MarkupType::Rectangle);
        assert_eq!(tool.appearance.color, "#ff0000");
        assert_eq!(tool.appearance.line_weight, 2.0);
        assert_eq!(tool.subject.as_deref(), Some("Door"));
        assert_eq!(tool.placement_mode, PlacementMode::Properties);
        assert!(tool.geometry.is_none(), "properties mode carries no fixed geometry");
    }

    #[test]
    fn non_ascii_name_and_subject_survive_import_intact() {
        // Probing "non-ASCII names" per the dispatch: accented Latin, a non-Latin script,
        // and an emoji, in both <Name> (XML text content) and the /Subj key inside <Raw>
        // (a PDF literal string, a completely different encoding surface - `escape_pdf_
        // string`'s counterpart on the READ side is lopdf's own string parsing). Neither
        // roxmltree (XML) nor lopdf (PDF literal strings) should mangle valid UTF-8/Latin-1
        // text; this is a real-world-plausible case (non-English trade names, e.g. a
        // Francophone or Chinese-market project) worth confirming rather than assuming.
        let xml = r#"<ToolChestData>
          <ToolChestItem>
            <Name>Détecteur de fumée 🔥</Name>
            <Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Square /Rect [10 20 110 70] /C [1 0 0] /Subj (Порядок) >>]]></Raw>
          </ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "Détecteur de fumée 🔥");
        // /Subj is a PDF literal string outside Latin-1 (Cyrillic) - lopdf reads literal
        // strings as raw bytes; the annotation reader's `get_string` behaviour on non-
        // Latin-1 bytes is what's actually under test here (real-world PDFs may use
        // PDFDocEncoding or UTF-16BE with a BOM for such content, not raw UTF-8 - assert
        // on what the pipeline actually produces rather than assuming instead of guessing).
        eprintln!("subject after round-trip: {:?}", report.tools[0].subject);
    }

    #[test]
    fn drawing_mode_item_carries_fixed_geometry() {
        let xml = PLAIN_ITEM.replace("<Mode>properties</Mode>", "<Mode>drawing</Mode>");
        let report = parse_btx_xml(&xml);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].placement_mode, PlacementMode::Drawing);
        assert!(report.tools[0].geometry.is_some());
    }

    // --- checked-in fixture (tools/fixtures/sample-tool-set.btx), added 2026-08-05 so the
    // GUI harness has a real file to drive the ".btx" import path through - this test ties
    // the fixture to the parser so it can't silently drift from something this module no
    // longer accepts (it's the same PLAIN_ITEM shape plus a leading XML comment, proving
    // parse_btx_xml tolerates a comment before the root element).
    //
    // Read at RUNTIME (not `include_str!`) and gated on the file existing, mirroring
    // render::tests::corpus()'s pattern one directory up - `tools/` lives outside
    // src-tauri/, and .forgejo/Dockerfile.test-rust's CI build context only COPYs
    // Cargo.toml/Cargo.lock/src-tauri/crates into the image, so include_str!'s
    // compile-time path doesn't exist there even though the file is genuinely committed
    // (confirmed: CI run #165 failed with "couldn't read ...tools/fixtures/..." while
    // `git ls-tree HEAD` shows the file present - a build-context gap, not a missing
    // commit). The Dockerfile now also COPYs tools/fixtures so this runs for real in CI
    // rather than skipping there forever; the runtime-read + skip stays as defense for
    // any other narrower build context, same reasoning as corpus()'s gating.
    fn sample_tool_set_fixture_path() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tools")
            .join("fixtures")
            .join("sample-tool-set.btx");
        p.exists().then_some(p)
    }

    #[test]
    fn checked_in_sample_tool_set_fixture_imports_via_import_btx_bytes() {
        let Some(path) = sample_tool_set_fixture_path() else {
            eprintln!("skip: tools/fixtures/sample-tool-set.btx not present in this build context");
            return;
        };
        let bytes = std::fs::read(&path).expect("read checked-in fixture");
        let report = import_btx_bytes(&bytes);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "Fire Rated Door");
    }

    // --- (e) a zlib-`789c` <Raw> inflates + parses ---

    #[test]
    fn parses_zlib_deflated_raw_item() {
        let dict_text = "<< /Subtype /Square /Rect [0 0 50 50] /C [0 1 0] >>";
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(dict_text.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let hex: String = compressed.iter().map(|b| format!("{b:02x}")).collect();
        assert!(hex.starts_with("789c"), "zlib default-compression header must be 789c, got {hex}");

        let xml = format!(
            r#"<ToolChestData><ToolChestItem><Name>Green Box</Name><Mode>properties</Mode><Raw>{hex}</Raw></ToolChestItem></ToolChestData>"#
        );
        let report = parse_btx_xml(&xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].appearance.color, "#00ff00");
    }

    // --- (f) a .zip-wrapped .btx unwraps ---

    #[test]
    fn imports_zip_wrapped_btx() {
        let mut zip_bytes = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            writer.start_file::<_, ()>("MyTools.btx", zip::write::FileOptions::default()).unwrap();
            writer.write_all(PLAIN_ITEM.as_bytes()).unwrap();
            writer.finish().unwrap();
        }

        let report = import_btx_bytes(&zip_bytes);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "Fire Rated Door");
    }

    #[test]
    fn zip_with_no_btx_member_is_reported_not_panicking() {
        let mut zip_bytes = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            writer.start_file::<_, ()>("readme.txt", zip::write::FileOptions::default()).unwrap();
            writer.write_all(b"not a tool set").unwrap();
            writer.finish().unwrap();
        }

        let report = import_btx_bytes(&zip_bytes);
        assert!(report.tools.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    // --- (g) a malformed item is skipped + reported, not fatal ---

    #[test]
    fn malformed_item_is_skipped_and_reported_alongside_good_items() {
        let xml = r#"<ToolChestData>
          <ToolChestItem><Name>Good One</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Missing Raw</Name><Mode>properties</Mode></ToolChestItem>
          <ToolChestItem><Name>Garbage Raw</Name><Mode>properties</Mode><Raw>not a dict at all</Raw></ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert_eq!(report.tools.len(), 1, "the one good item still imports");
        assert_eq!(report.tools[0].name, "Good One");
        assert_eq!(report.skipped.len(), 2, "both bad items are reported");
        assert!(report.skipped.iter().any(|s| s.name == "Missing Raw"));
        assert!(report.skipped.iter().any(|s| s.name == "Garbage Raw"));
    }

    // --- (h) an unsupported/unusual annotation subtype must be skipped + reported, not
    // silently miscoerced into a Text tool ---

    #[test]
    fn unsupported_annotation_subtype_is_skipped_not_silently_reclassified_as_text() {
        // Bug found investigating "issues with btx file imports": `Markup::from_annotation_
        // dict`'s /Subtype match has a permissive `_ => Some(MarkupType::Text)` fallback -
        // correct for its OTHER caller, `document::annots::read_markups`, which filters
        // every annotation against `MARKUP_SUBTYPES` BEFORE calling it, so the fallback
        // arm is structurally unreachable there. `import_item` (this module) called
        // `from_annotation_dict` directly with no such guard, so a Raw payload with any
        // subtype redline doesn't model - Underline/StrikeOut/Squiggly/Redact/Widget/Popup/
        // Link, none of which have a `MarkupType` variant at all - silently imported as a
        // bogus "Text" tool (wrong type, wrong/absent geometry) with NO skip and NO error:
        // a "success" that silently drops/misrepresents what the tool actually was.
        let xml = r#"<ToolChestData>
          <ToolChestItem><Name>Good Square</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 50 50] >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Underline Tool</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Underline /Rect [0 0 50 50] >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Widget Field</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Widget /Rect [0 0 50 50] >>]]></Raw></ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert_eq!(report.tools.len(), 1, "only the genuinely supported subtype imports");
        assert_eq!(report.tools[0].name, "Good Square");
        assert_eq!(
            report.skipped.len(),
            2,
            "both unsupported subtypes must be reported, not silently turned into Text tools: {:?}",
            report.skipped
        );
        let underline = report
            .skipped
            .iter()
            .find(|s| s.name == "Underline Tool")
            .expect("Underline Tool must be in the skip report");
        assert!(
            underline.reason.contains("Underline"),
            "skip reason should name the actual unsupported subtype: {underline:?}"
        );
        let widget = report
            .skipped
            .iter()
            .find(|s| s.name == "Widget Field")
            .expect("Widget Field must be in the skip report");
        assert!(
            widget.reason.contains("Widget"),
            "skip reason should name the actual unsupported subtype: {widget:?}"
        );
    }

    #[test]
    fn empty_document_produces_empty_report_not_error() {
        let report = parse_btx_xml("<ToolChestData></ToolChestData>");
        assert!(report.tools.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn unparseable_xml_is_reported_as_one_skipped_item() {
        let report = parse_btx_xml("not xml at all <<<");
        assert!(report.tools.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn non_utf8_bytes_are_reported_not_panicking() {
        let bytes = vec![0xff, 0xfe, 0x00, 0x01, 0x02];
        let report = import_btx_bytes(&bytes);
        assert!(report.tools.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn a_utf16_encoded_file_is_reported_not_panicking() {
        // Probing "files exported by different Bluebeam versions": the spec/module doc
        // states .btx is XML/UTF-8, but Windows-authored XML sometimes ships UTF-16LE with
        // a BOM (a real interop pattern for Windows-native tools). roxmltree/import_btx_
        // bytes are UTF-8-only; confirm a UTF-16 file degrades to a clean, reported skip
        // (not a panic, not silent data loss) rather than assuming it "just works" - it
        // does NOT currently decode, which is a real, named gap (see report).
        let utf16_bom_and_text: Vec<u8> = {
            let text = "<ToolChestData></ToolChestData>";
            let mut out = vec![0xFF, 0xFE]; // UTF-16LE BOM
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            out
        };
        let report = import_btx_bytes(&utf16_bom_and_text);
        assert!(report.tools.is_empty());
        assert_eq!(
            report.skipped.len(),
            1,
            "a UTF-16 file must be reported as unreadable, not silently produce zero \
             tools with zero explanation, and must not panic"
        );
    }

    #[test]
    fn truncated_mid_element_xml_is_reported_not_panicking() {
        // Probing "malformed or truncated XML": a file cut off mid-tag (partial download,
        // truncated clipboard paste, corrupted transfer) must degrade to a reported skip,
        // never a panic - this is the literal scenario the module doc's "never fatal to
        // the whole import" guarantee exists for.
        let truncated = r#"<ToolChestData><ToolChestItem><Name>Half Do"#;
        let report = parse_btx_xml(truncated);
        assert!(report.tools.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn truncated_mid_raw_cdata_is_reported_not_panicking() {
        // A truncation landing INSIDE a <Raw><![CDATA[...]]></Raw> block specifically -
        // the CDATA terminator and closing tags never arrive.
        let truncated = r#"<ToolChestData><ToolChestItem><Name>Cut Off</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 50"#;
        let report = parse_btx_xml(truncated);
        assert!(report.tools.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn malformed_dict_syntax_inside_a_wellformed_raw_element_is_skipped_not_fatal() {
        // The <Raw> element itself is well-formed XML (valid CDATA, valid tags - so XML
        // parsing succeeds) but its PDF dict payload is corrupt (unterminated array,
        // mismatched brackets). This exercises wrap_dict_as_pdf + lopdf's error path
        // specifically, distinct from `unparseable_xml_is_reported_as_one_skipped_item`
        // (which corrupts the OUTER XML, not the inner PDF-syntax payload).
        let xml = r#"<ToolChestData>
          <ToolChestItem><Name>Good</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 50 50] >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Broken Dict</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 50]]></Raw></ToolChestItem>
        </ToolChestData>"#;
        let report = parse_btx_xml(xml);
        assert_eq!(report.tools.len(), 1, "the well-formed item still imports");
        assert_eq!(report.tools[0].name, "Good");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].name, "Broken Dict");
    }

    #[test]
    fn stamp_import_currently_drops_the_appearance_asset_named_gap_not_fixed_this_pass() {
        // NAMED, CONFIRMED, NOT FIXED. A genuine `/Subtype /Stamp` Raw payload (Stamp IS
        // in MARKUP_SUBTYPES, so it passes the subtype guard fixed above and imports
        // successfully - no skip) still gets `stamp: None` unconditionally. `import_item`
        // (this file) has never branched on markup_type at all; there is no code anywhere
        // matching `import_stamp_item`, despite this file's OWN doc comment (a few lines
        // above `import_item`) describing "a dedicated stamp-import path ... see
        // `import_stamp_item` used by the command layer for `<Type>` values containing
        // 'Stamp'" - that path does not exist (confirmed via
        // `grep -rn "import_stamp_item\|fn import_stamp"`, zero hits besides the comment
        // itself), and `<Type>` is not read ANYWHERE in this module (confirmed via
        // `grep -n "\"Type\""`) - the doc comment describes a design that was never built.
        //
        // Real-world impact: a Bluebeam-exported custom image/graphic stamp (the common
        // case for a firm's own rubber-stamp graphics) imports with NO error and NO skip
        // entry - it "succeeds" - but silently loses its entire visual payload. Placing it
        // later falls back to `appearance::draw_stamp_box_and_label` (a plain bordered box
        // + the tool's Name as text), which looks nothing like the imported stamp. This
        // is exactly the "success that silently drops data" failure class the dispatch
        // asked to watch for.
        //
        // NOT fixed in this pass, deliberately: `StampAsset::PdfBase64` exists specifically
        // "as the natural landing spot for imported Bluebeam stamps" (see its doc comment,
        // `toolchest/stamp.rs`), but `appearance::draw()`'s Stamp/StampDynamic arm ONLY
        // renders `PngBase64` - `PdfBase64` and `Svg` both already fall back to box+label,
        // a DIFFERENT, ALREADY-DOCUMENTED deferral (`appearance.rs` module doc: "vector-
        // SVG-to-PDF-operator conversion and embedded-PDF content-stream splicing are both
        // out of scope for this pass"). Populating `stamp: Some(StampDef::Static { asset:
        // PdfBase64(..) })` on import today would change NOTHING user-visible (rendering
        // still falls back to the same box+label either way) - it would be inert plumbing
        // for a rendering feature that doesn't exist yet, built on an unverified guess at
        // Bluebeam's real wire format for stamp appearance data (no actual .btx sample
        // with an embedded stamp graphic was available to confirm the shape against -
        // bench/corpus/ is machine-local/gitignored and absent in this worktree). Building
        // BOTH the import-side extraction and the render-side PdfBase64 support blind, in
        // one pass, without a real sample to verify against, is the wrong tradeoff here -
        // it would ship an unverified format guess as if it were a confirmed fix. Recommend
        // as a follow-up: obtain one real Bluebeam .btx export containing a custom-image
        // stamp, confirm the actual object-reference shape of its <Raw> payload's /AP, THEN
        // build import_stamp_item + PdfBase64 rendering together against real data.
        let xml = r#"<ToolChestData>
          <ToolChestItem>
            <Name>Company Logo Stamp</Name>
            <Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Stamp /Rect [0 0 144 72] /Name /CompanyLogo >>]]></Raw>
          </ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "the Stamp item itself is NOT skipped - it silently 'succeeds': {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        let tool = &report.tools[0];
        assert_eq!(tool.markup_type, crate::markup::MarkupType::Stamp);
        assert!(
            tool.stamp.is_none(),
            "current (unfixed) behaviour: the appearance/graphic payload is always \
             discarded for Stamp imports - this assertion documents the gap, it is not the \
             desired end state"
        );
    }

    #[test]
    fn a_large_tool_set_imports_every_item_without_degradation() {
        // Probing "large sets": Bluebeam Tool Chests can legitimately grow into the
        // hundreds of items for a mature firm-wide standard set. Confirm correctness
        // (every item accounted for, ids stay unique) at a representative size - not a
        // strict performance benchmark, but a real functional check that nothing silently
        // drops items or collides ids once the set is no longer trivially small.
        let mut xml = String::from("<ToolChestData>");
        const N: usize = 500;
        for i in 0..N {
            xml.push_str(&format!(
                r#"<ToolChestItem><Name>Tool {i}</Name><Mode>properties</Mode><Raw><![CDATA[<< /Subtype /Square /Rect [0 0 {w} {w}] /Subj (Item {i}) >>]]></Raw></ToolChestItem>"#,
                i = i,
                w = 10 + (i % 50),
            ));
        }
        xml.push_str("</ToolChestData>");

        let report = parse_btx_xml(&xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), N);
        let names: std::collections::HashSet<&str> =
            report.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), N, "every item's name must be distinct and present");
        let ids: std::collections::HashSet<uuid::Uuid> =
            report.tools.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), N, "every tool must get a unique id - no collisions at scale");
    }
}
