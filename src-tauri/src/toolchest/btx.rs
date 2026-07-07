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
        // Stamp mapping (static-import-direct, dynamic-field-recognition-with-fallback) is
        // handled by the dedicated stamp-import path once markup_type is Stamp/StampDynamic -
        // see `import_stamp_item` used by the command layer for `<Type>` values containing
        // "Stamp". Plain-annotation items (the common case) never carry a StampDef.
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
    fn drawing_mode_item_carries_fixed_geometry() {
        let xml = PLAIN_ITEM.replace("<Mode>properties</Mode>", "<Mode>drawing</Mode>");
        let report = parse_btx_xml(&xml);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].placement_mode, PlacementMode::Drawing);
        assert!(report.tools[0].geometry.is_some());
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
}
