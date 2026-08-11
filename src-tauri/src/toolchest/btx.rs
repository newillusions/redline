//! `.btx` (Bluebeam Tool Set) importer (spec "Importing Bluebeam Tool Sets & stamps").
//!
//! Rewritten 2026-08-06 against FOUR real Bluebeam-exported `.btx` files (a 77-item, 4-file
//! corpus - `bench/corpus/btx/`), the first real samples this importer was ever checked
//! against. Several assumptions baked into the original implementation turned out wrong;
//! this doc comment describes the VERIFIED real wire format, not the earlier guess.
//!
//! `.btx` is UTF-8 XML (real exports carry a UTF-8 BOM). The real root element is
//! `<BluebeamRevuToolSet Version="N">` (NOT `<ToolChestData>`, which only ever existed in
//! this module's own test fixtures) - harmless in practice, since item discovery
//! (`doc.descendants().filter(has_tag_name("ToolChestItem"))`) doesn't care about the root
//! tag, but worth recording accurately. A `<Title>` sibling (also hex+zlib, like `<Raw>`)
//! carries the tool set's own display name - not consumed here, it's set-level metadata.
//!
//! Each `<ToolChestItem>` carries:
//! - `<Name>` - a Bluebeam-internal OPAQUE id (real exports: 16-char random-looking codes
//!   like `"XAZASJPZJVLIFSAM"`), NOT a human label. The name Bluebeam actually shows in its
//!   Tool Chest is the annotation's own `/Subj` (Subject) key inside `<Raw>` - this
//!   importer now prefers that, falling back to `<Name>` only when `/Subj` is absent
//!   (measured: present on ~90% of real items). Using `<Name>` as the display name (the
//!   original implementation) is the root cause of the "naming differs from the original
//!   Bluebeam tool" defect Martin reported.
//!
//!   2026-08-08 follow-up: /Subj is absent specifically on custom Stamp items (7/77 real
//!   items, all Stamp-typed) - and for those, `<Name>` alone is STILL what a user saw
//!   ("some imported tools still display UID names"). Every one of those 7 carries a
//!   private `/TmpBRXFile` literal-string path recording the source file Bluebeam built
//!   the stamp from (e.g. `D:\...\Stamps\MR Init.pdf`) - `stamp_source_file_basename`
//!   extracts its basename minus extension ("MR Init") as a second-tier fallback, ahead
//!   of the opaque `<Name>` UID. Measured: closes all 7/7 real UID-name cases in the
//!   sample corpus. `<Name>` remains the final floor for the case neither `/Subj` nor
//!   `/TmpBRXFile` is present (not observed in the sample corpus, but not assumed
//!   impossible).
//! - `<Type>` (e.g. `Bluebeam.PDF.Annotations.AnnotationFreeText`) - informational only,
//!   never read; the `<Raw>` dict's own `/Subtype` already gives the same information in
//!   the form `Markup::from_annotation_dict` actually consumes.
//! - `<Mode>` (`properties`/`drawing` - maps directly to our two placement modes).
//! - `<X>`/`<Y>` - duplicate the geometry's own origin coordinates (verified: e.g. an Ink
//!   tool's `<X>`/`<Y>` exactly match its `/InkList`'s first vertex) and carry no
//!   additional information - not read.
//! - `<Index>` - Bluebeam's AUTHORED display order within the tool set. Real exports do
//!   NOT store items in that order in the XML itself (measured on one sample: `<Index>`
//!   values `139,2,0,24,0,0,12,43,0` against XML document positions `0..8`) - `.btx`
//!   parsing now sorts the final tool list by `<Index>` (stable, ties keep document order)
//!   so imported Tool Chest order matches Bluebeam's, rather than an arbitrary XML order.
//! - A `<Raw>` payload that IS a PDF annotation dictionary
//!   (`/Subtype/FreeText /Rect[...] /CL[...] /Subj(...)` etc), always hex+zlib-encoded in
//!   every real sample seen (the "plain-text `<Raw>`" case this module's tests also cover
//!   is UNVERIFIED against any real Bluebeam export - kept as defensive parsing, not
//!   confirmed to occur in the wild).
//! - Optionally one or more sibling `<Resources>` blocks (see "Stamp artwork" below).
//! - Optionally one or more `<Child>` elements - each a SECOND, paired annotation (e.g. a
//!   shape + its attached text label, or a callout + its leader arrow) that Bluebeam
//!   groups with the parent as one visual unit. **FIXED 2026-08-11** (measured at 33/77,
//!   43%, of real items across the sample corpus - not a rare edge case; some real items
//!   carry up to 20 children, a "compound stamp" tool): `Tool::children: Vec<ToolChild>`
//!   (`toolchest::mod`) captures every `<Child>`, each converted through the same
//!   `Markup::from_annotation_dict` reuse the parent uses. Placement (frontend,
//!   `Viewport.svelte::createPlacedMarkup`) drops one `Markup` per member (this tool +
//!   every child) sharing a single fresh `group_id`, translated by the same click-anchor
//!   delta - see design doc `docs/design/2026-08-11-grouped-markups.md` §4. A child whose
//!   own `<Raw>` fails to decode is dropped, never fatal to the tool. See
//!   `tests::grouped_child_annotation_is_captured_on_tool_children`.
//!
//! Custom estimating columns (`/BSIColumnData`): this module's ORIGINAL doc comment
//! claimed it was a separate `<BSIColumnData>` XML element - WRONG, confirmed against the
//! real corpus (zero such elements exist anywhere in 4 real files). It is a KEY INSIDE the
//! `<Raw>` PDF dictionary itself, e.g. `/BSIColumnData[(01 General)(Stiles)]`. Detected
//! from the right place now (so a future feature can act on it) but still a genuine
//! deferral - Tool Chest has no estimating-columns UI concept yet.
//!
//! **Stamp artwork** (previously "dropped entirely", now fixed for the mechanism real
//! Bluebeam exports actually use): a genuine Bluebeam Stamp/StampDynamic `<Raw>` dict
//! references its artwork INDIRECTLY, via `/AP<</N/BBObjPtr_<id>>>` - a NAME placeholder,
//! not a stream or a real indirect reference. Bluebeam resolves it against sibling
//! `<Resources>` blocks on the SAME `<ToolChestItem>`, each holding a hex+zlib `<ID>`
//! (the id a `/BBObjPtr_<id>` name refers to) and `<Data>` (raw PDF-syntax bytes for that
//! object - typically a `/Subtype/Form` XObject whose OWN `/Resources` dict may itself
//! reference further `<Resources>` blocks by the same mechanism, e.g. an `/ExtGState`,
//! forming a small object graph, not just one flat blob). This importer resolves that
//! graph into [`stamp::StampAsset::BluebeamFormXObject`] (root id + every referenced
//! object's raw bytes, `/BBObjPtr_*` placeholders still unresolved); the actual
//! placeholder->indirect-reference splicing happens at PLACEMENT time in
//! `document::annots::write_markups` (the one place holding `&mut Document`) via
//! [`resolve_bb_objptr_refs`]/[`parse_pdf_object_bytes`] - see `markup::appearance`'s
//! Stamp/StampDynamic draw arm for how the placed annotation references it. Measured:
//! every Stamp-subtype item in the sample corpus (11/11) uses this exact mechanism.
//!
//! The importer reuses the existing annotation reader ([`Markup::from_annotation_dict`])
//! for `<Raw>` - it does not reimplement annotation parsing. Wrinkles handled: hex+zlib
//! `<Raw>` payloads (hex starting `789c`), and `.zip`-wrapped `.btx` files (this second
//! case remains UNVERIFIED against any real sample - none of the 4 real files were
//! zip-wrapped). A malformed/unparseable item is skipped and reported, never fatal to the
//! whole import.

use std::io::Read;

use lopdf::{Dictionary, Object};
use serde::Serialize;

use crate::markup::Markup;
use crate::toolchest::{PlacementMode, StampAsset, StampDef, Tool, ToolChild};

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
///
/// Strips a leading UTF-8 BOM (`U+FEFF`) before handing the text to roxmltree - every real
/// Bluebeam export in the sample corpus carries one immediately before `<?xml ...?>`, which
/// `std::str::from_utf8` happily decodes (the BOM bytes are valid UTF-8). Verified NOT
/// currently load-bearing (roxmltree 0.21 tolerates the BOM character fine on its own,
/// confirmed against the real corpus with this strip removed) - kept anyway as defensive,
/// free-to-add insurance against relying on an undocumented leniency in a third-party
/// parser (see `tests::leading_utf8_bom_does_not_break_xml_parsing`).
pub fn parse_btx_xml(xml: &str) -> ImportReport {
    let xml = xml.strip_prefix('\u{FEFF}').unwrap_or(xml);
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
    // Bluebeam's authored display order (`<Index>`) does NOT match XML document order in
    // real exports (measured: one sample's `<Index>` values were `139,2,0,24,0,0,12,43,0`
    // against document positions `0..8`) - collect `(index, tool)` pairs and sort once at
    // the end, rather than trusting insertion order, so the imported Tool Chest matches
    // Bluebeam's own layout. An item with no/unparseable `<Index>` keeps its natural
    // document position as a stand-in key (a stable sort then leaves it where it would
    // have landed anyway relative to other index-less items).
    let mut indexed: Vec<(i64, Tool)> = Vec::new();
    for (doc_pos, item) in doc.descendants().filter(|n| n.has_tag_name("ToolChestItem")).enumerate() {
        let name = child_text(item, "Name").unwrap_or("<unnamed>").to_string();
        match import_item(item, &name) {
            Ok(tool) => {
                let index = child_text(item, "Index")
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(doc_pos as i64);
                indexed.push((index, tool));
            }
            Err(reason) => report.skipped.push(SkippedItem { name, reason }),
        }
    }
    indexed.sort_by_key(|(index, _)| *index);
    report.tools = indexed.into_iter().map(|(_, tool)| tool).collect();
    report
}

fn child_text<'a>(node: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    node.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text())
}

fn import_item(item: roxmltree::Node, name: &str) -> Result<Tool, String> {
    let mode_tag = child_text(item, "Mode").unwrap_or("properties");
    let placement_mode =
        if mode_tag.eq_ignore_ascii_case("drawing") { PlacementMode::Drawing } else { PlacementMode::Properties };

    let raw = child_text(item, "Raw").ok_or_else(|| "missing <Raw> element".to_string())?;
    let dict = raw_to_dict(raw)?;

    // BSIColumnData (custom estimating columns): NAMED deferral - see module doc. It is a
    // KEY INSIDE the parsed `<Raw>` dict (`/BSIColumnData[...]`), not a separate XML
    // element (the module doc previously claimed otherwise - confirmed wrong against the
    // real corpus). Detected here so a future feature can act on it; still not mapped onto
    // anything - Tool Chest has no estimating-columns UI concept yet, and a
    // present-but-unmapped block is not a reason to skip the item.
    let _ = dict.get(b"BSIColumnData");

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

    // Naming fidelity (module doc, "naming differs from the original" defect): Bluebeam's
    // own `<Name>` is an opaque internal id, not what Bluebeam shows the user - that's the
    // annotation's own `/Subj`. Prefer it; when `/Subj` is absent or blank, every Stamp
    // item observed in the 2026-08-08 UID-name follow-up (7/7 real corpus cases) is a
    // custom stamp Bluebeam built from a user-selected file, recorded as a private
    // `/TmpBRXFile` literal-string path (e.g. `D:\...\Stamps\MR Init.pdf`) - its basename
    // minus extension ("MR Init") is a real, meaningful name, unlike the opaque `<Name>`
    // UID. Falls back to `<Name>` only when NEITHER `/Subj` NOR `/TmpBRXFile` is present
    // (2/7 real cases - genuinely nothing better to show) so every tool still gets a
    // non-empty name.
    let display_name = markup
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| stamp_source_file_basename(&dict))
        .unwrap_or_else(|| name.to_string());

    let geometry = match placement_mode {
        PlacementMode::Drawing => Some(markup.geometry.clone()),
        PlacementMode::Properties => None,
    };

    // Stamp artwork fidelity (module doc "Stamp artwork" section) - fixed 2026-08-06 now
    // that real Bluebeam stamp samples exist to verify the wire format against. A genuine
    // Stamp/StampDynamic Raw dict names its appearance indirectly (`/AP<</N/BBObjPtr_id>>`)
    // rather than embedding it; resolve that against this item's own sibling <Resources>
    // blocks into a StampAsset the placement pipeline can splice into a real appearance
    // stream (see document::annots::write_markups). `None` for anything that isn't
    // Stamp-shaped this way (not a Stamp, or a Stamp using some other/no artwork
    // mechanism - none observed in the sample corpus, but not assumed impossible).
    let stamp = bb_form_xobject_root_id(&dict).map(|root_id| StampDef::Static {
        asset: StampAsset::BluebeamFormXObject { root_id, objects: collect_bb_resources(item) },
    });

    // Grouped/compound tools (module doc "<Child>", design doc §4): zero or more
    // `<Child>` siblings, each a SECOND paired annotation Bluebeam groups with this
    // one as one visual unit. Real corpus: 33/77 items carry at least one, up to 20 on
    // a single "compound stamp" tool. Reuses the exact same per-Raw conversion as the
    // parent above (subtype guard + `Markup::from_annotation_dict`) - no new dict-
    // parsing logic, just applied once per child instead of once per item. A child
    // whose own `<Raw>` fails to decode or names an unsupported subtype is dropped
    // (never fatal to the whole tool - same never-fatal posture as everything else in
    // this module) rather than failing the entire compound tool over one bad member.
    let children: Vec<ToolChild> = item
        .children()
        .filter(|n| n.has_tag_name("Child"))
        .filter_map(|child_node| import_child(child_node, item))
        .collect();

    Ok(Tool {
        id: uuid::Uuid::new_v4(),
        name: display_name,
        markup_type: markup.markup_type,
        appearance: markup.appearance,
        subject: markup.subject,
        placement_mode,
        geometry,
        stamp,
        children,
    })
}

/// Parse one `<Child>` element into a [`ToolChild`]. `item` is the owning
/// `<ToolChestItem>` - needed because `<Resources>` blocks (stamp artwork) are always
/// siblings of the item itself, not nested inside `<Child>` (verified against the real
/// corpus's `(Stamp, Stamp)` and `(Stamp, Square)` grouped pairs). Returns `None`
/// (never an `Err`/panic) on any decode failure - the caller drops it silently, same
/// never-fatal posture as a top-level item.
fn import_child(child_node: roxmltree::Node, item: roxmltree::Node) -> Option<ToolChild> {
    let raw = child_text(child_node, "Raw")?;
    let dict = raw_to_dict(raw).ok()?;

    match crate::document::annots::subtype(&dict) {
        Some(st) if crate::document::annots::MARKUP_SUBTYPES.contains(&st.as_str()) => {}
        _ => return None,
    }

    let markup = Markup::from_annotation_dict(&dict);
    let stamp = bb_form_xobject_root_id(&dict).map(|root_id| StampDef::Static {
        asset: StampAsset::BluebeamFormXObject { root_id, objects: collect_bb_resources(item) },
    });

    Some(ToolChild {
        markup_type: markup.markup_type,
        appearance: markup.appearance,
        subject: markup.subject,
        // A child's geometry is always captured, regardless of the PARENT tool's own
        // placement_mode field - a childless Properties-mode tool carries no geometry
        // by design (the user draws it fresh), but a grouped tool's members must keep
        // their exact relative layout to remain a recognisable compound shape, which
        // only a fixed geometry snapshot can preserve. Every real corpus group is a
        // Drawing-mode tool (Mode drawing), so this has not been observed to diverge
        // from the parent's mode in practice, but is not made conditional on it -
        // groups without fixed geometry would be meaningless (nothing to offset by).
        geometry: Some(markup.geometry),
        stamp,
    })
}

/// If `dict`'s `/AP` names its Normal appearance via a `/BBObjPtr_<id>` placeholder name
/// (Bluebeam's own indirect-reference stand-in - see module doc "Stamp artwork"), return
/// `<id>`. `None` for anything else (no `/AP`, an `/AP/N` that's already a real stream/
/// reference, or a name not matching the `BBObjPtr_` convention).
fn bb_form_xobject_root_id(dict: &Dictionary) -> Option<String> {
    let ap = dict.get(b"AP").ok()?.as_dict().ok()?;
    let n = ap.get(b"N").ok()?;
    let name = n.as_name().ok()?;
    std::str::from_utf8(name).ok()?.strip_prefix("BBObjPtr_").map(str::to_string)
}

/// Decode every `<Resources>` child of `item` into `(id, raw_bytes)` pairs - Bluebeam's
/// own sidecar storage for stamp/appearance-stream content (module doc "Stamp artwork").
/// Both `<ID>` and `<Data>` are hex+zlib exactly like `<Raw>`, but `<Data>` may decode to
/// BINARY content (an already-Flate-compressed PDF stream body), so this decodes to raw
/// bytes rather than reusing [`inflate_hex_zlib`]'s string-returning path. A block that
/// fails to decode is skipped rather than aborting the whole item (same never-fatal
/// posture as everything else in this module) - it simply leaves any `/BBObjPtr_<id>`
/// reference to it unresolved at splice time.
fn collect_bb_resources(item: roxmltree::Node) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for res in item.children().filter(|n| n.has_tag_name("Resources")) {
        let (Some(id_hex), Some(data_hex)) = (child_text(res, "ID"), child_text(res, "Data")) else {
            continue;
        };
        let (Some(id_bytes), Some(data_bytes)) = (decode_hex_zlib_bytes(id_hex), decode_hex_zlib_bytes(data_hex))
        else {
            continue;
        };
        let Ok(id) = String::from_utf8(id_bytes) else { continue };
        out.insert(id, data_bytes);
    }
    out
}

/// Second-tier stamp naming fallback (2026-08-08 corpus finding, see `import_item`'s
/// naming comment): a custom Stamp built from a user-selected file carries that file's
/// path as a private `/TmpBRXFile` literal string (backslash-separated Windows paths in
/// every real sample seen - Bluebeam itself is Windows-only). Returns the basename with
/// its extension stripped (`D:\...\Stamps\MR Init.pdf` -> `"MR Init"`), or `None` if the
/// key is absent, not a string, empty, or decodes to an empty/whitespace-only basename.
fn stamp_source_file_basename(dict: &Dictionary) -> Option<String> {
    let raw = dict.get(b"TmpBRXFile").ok()?.as_str().ok()?;
    let path = String::from_utf8_lossy(raw);
    let basename = path.rsplit(['\\', '/']).next().unwrap_or(&path).trim();
    if basename.is_empty() {
        return None;
    }
    let without_ext = match basename.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem,
        Some(_) => return None, // leading-dot-only basename (".pdf") - not a real name
        None => basename,       // no extension at all
    }
    .trim();
    if without_ext.is_empty() {
        None
    } else {
        Some(without_ext.to_string())
    }
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
    let bytes = decode_hex_zlib_bytes(hex).ok_or_else(|| "hex+zlib decode failed".to_string())?;
    String::from_utf8(bytes).map_err(|e| format!("inflated bytes are not valid UTF-8: {e}"))
}

/// Hex-decode + zlib-inflate a `<Raw>`/`<ID>`/`<Data>` blob to raw bytes (the byte-oriented
/// primitive both [`inflate_hex_zlib`] and `collect_bb_resources` build on - `<Data>` in
/// particular may decode to BINARY content, e.g. an already-Flate-compressed PDF stream
/// body, so it cannot go through a `String`-returning path). `None` on any failure (not
/// hex+zlib, bad hex, inflate error) - callers degrade gracefully rather than panicking.
fn decode_hex_zlib_bytes(hex: &str) -> Option<Vec<u8>> {
    let trimmed = hex.trim();
    if !is_hex_zlib(trimmed) {
        return None;
    }
    let bytes = hex_decode(trimmed).ok()?;
    let mut decoder = flate2::read::ZlibDecoder::new(&bytes[..]);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).ok()?;
    Some(out)
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

// ---------------------------------------------------------------------------
// Stamp artwork: Bluebeam `/BBObjPtr_<id>` graph resolution (module doc "Stamp artwork").
// ---------------------------------------------------------------------------

/// Replace every `/BBObjPtr_<id>` name placeholder found in the DICTIONARY portion of
/// `raw` (i.e. before a literal `stream\n`/`stream\r\n` keyword, if any) with the real
/// indirect reference `id_to_objid` assigns that id - e.g. `/BBObjPtr_ABC123` becomes
/// ` 5 0 R`. The replacement carries a LEADING SPACE deliberately: PDF name tokens are
/// self-delimiting (no separator needed between a preceding key name and this one, e.g.
/// `/ExtGState/BBObjPtr_X`), but `5 0 R` has no leading `/` to delimit it from whatever
/// precedes it - textually substituting without the space produces `/ExtGState5 0 R`,
/// which a PDF tokenizer reads as the single name `ExtGState5` followed by a floating,
/// meaningless `0 R` (confirmed by a failing round-trip before this fix was added - the
/// extra space is always harmless, PDF collapses whitespace runs). Never touches bytes
/// from `stream` onward: those are an opaque, often still-compressed binary payload, and
/// a `/BBObjPtr_` byte sequence occurring there by pure coincidence must not be
/// corrupted. An id with no entry in `id_to_objid` (a reference to a block that failed to
/// decode, or wasn't captured) is left as a literal, inert name - PDF viewers ignore a
/// `/Resources` entry that resolves to nothing rather than failing the whole page, so
/// this degrades that one visual detail, not the stamp.
pub(crate) fn resolve_bb_objptr_refs(raw: &[u8], id_to_objid: &std::collections::BTreeMap<String, (u32, u16)>) -> Vec<u8> {
    let stream_at = find_subslice(raw, b"stream\n")
        .or_else(|| find_subslice(raw, b"stream\r\n"))
        .unwrap_or(raw.len());
    let (head, tail) = raw.split_at(stream_at);
    let mut head_text = String::from_utf8_lossy(head).into_owned();
    for (id, (num, gen)) in id_to_objid {
        head_text = head_text.replace(&format!("/BBObjPtr_{id}"), &format!(" {num} {gen} R"));
    }
    let mut out = head_text.into_bytes();
    out.extend_from_slice(tail);
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse raw PDF object bytes (a `<< dict >>`, optionally followed by
/// `stream ... endstream`) into a [`lopdf::Object`] - the byte-oriented sibling of
/// [`parse_pdf_dict`], needed because a Bluebeam `<Resources>` graph node may be a Stream
/// (binary body, e.g. a Form XObject's compressed content), not just a bare dictionary
/// like `<Raw>` always is.
pub(crate) fn parse_pdf_object_bytes(bytes: &[u8]) -> Result<Object, String> {
    let bytes = fix_stream_length(bytes);
    let pdf_bytes = wrap_object_bytes_as_pdf(&bytes);
    let doc = lopdf::Document::load_mem(&pdf_bytes)
        .map_err(|e| format!("PDF object parse failed: {e}"))?;
    doc.get_object((1, 0))
        .cloned()
        .map_err(|e| format!("missing wrapped object: {e}"))
}

/// Correct a declared `/Length N` to the object's ACTUAL `stream...endstream` body size.
///
/// 2026-08-08 finding (Form XObject stamp rendering follow-up): real Bluebeam `.btx`
/// exports' Form XObject nodes carry a `/Length` that does NOT match their real content -
/// measured on `bench/corpus/btx/emittiv-markups.btx`'s own "emittiv stamp crop" stamp,
/// `/Length 20` against a genuinely longer body (`/R0 gs /MWFOForm Do \n\n`, ~22 bytes).
/// lopdf trusts a declared `/Length` when present rather than scanning for `endstream`
/// (the module doc's own prior assumption, "/Length must be present and exact", was
/// WRONG for real data - it held for every hand-authored test fixture, never checked
/// against a real sample) - a mismatch makes it read a truncated/garbled body, which can
/// silently produce a plain `Dictionary` instead of a `Stream` object entirely (observed:
/// a REAL corpus root Form XObject failed `splice_bb_form_xobject`'s "is this a Stream"
/// check with exactly this shape). Recomputing `/Length` from the real `stream`/
/// `endstream` byte offsets before handing the object to lopdf fixes both `write_markups`
/// (save-time splicing) and `build_isolated_form_xobject_pdf` (rasterization) - this was
/// a LATENT bug in the shipped splicing path, never caught because no prior test spliced
/// real corpus data through an actual PDF parse/render round-trip.
///
/// A no-op (returns the input unchanged) for a bare dictionary with no `stream` keyword,
/// or if the `stream`/`endstream`/`/Length` markers can't all be found (malformed input -
/// leaves it for `parse_pdf_object_bytes`'s own error path to report, rather than
/// guessing).
fn fix_stream_length(bytes: &[u8]) -> Vec<u8> {
    // Find the OPENING "stream" keyword - distinct from "endstream" (which also
    // contains the substring "stream") by requiring it not be preceded by "end".
    let Some(stream_kw) = (0..bytes.len().saturating_sub(6))
        .find(|&i| &bytes[i..i + 6] == b"stream" && !(i >= 3 && &bytes[i - 3..i] == b"end"))
    else {
        return bytes.to_vec(); // no stream body - plain dict, nothing to fix
    };
    // Per spec the "stream" keyword is followed by exactly one EOL (CRLF or LF) before
    // the body starts - skip it.
    let after_kw = stream_kw + 6;
    let body_start = if bytes.get(after_kw..after_kw + 2) == Some(b"\r\n") {
        after_kw + 2
    } else if bytes.get(after_kw) == Some(&b'\n') {
        after_kw + 1
    } else {
        return bytes.to_vec(); // no EOL after "stream" - malformed, leave for the error path
    };
    let Some(endstream_rel) = bytes[body_start..]
        .windows(9)
        .position(|w| w == b"endstream")
    else {
        return bytes.to_vec(); // "stream" with no matching "endstream" - malformed
    };
    let body_end = body_start + endstream_rel;
    let actual_len = body_end - body_start;

    // Rewrite the "/Length N" token (BEFORE "stream", inside the dict) to the real
    // length. Assumes an inline integer, never an indirect `/Length N 0 R` reference -
    // true of every real sample this shape has been observed in (self-contained
    // serialized nodes with nothing else to reference).
    let Some(length_kw) = bytes[..stream_kw].windows(7).rposition(|w| w == b"/Length") else {
        return bytes.to_vec(); // no /Length key at all - leave as-is
    };
    let digits_start = bytes[length_kw + 7..stream_kw]
        .iter()
        .position(|b| b.is_ascii_digit())
        .map(|p| length_kw + 7 + p);
    let Some(digits_start) = digits_start else {
        return bytes.to_vec(); // no digits found after /Length - malformed
    };
    let digits_end = digits_start
        + bytes[digits_start..stream_kw]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..digits_start]);
    out.extend_from_slice(actual_len.to_string().as_bytes());
    out.extend_from_slice(&bytes[digits_end..]);
    out
}

/// Byte-oriented sibling of [`wrap_dict_as_pdf`] - `object_bytes` is inserted verbatim
/// (never routed through a `&str`) so a binary stream body inside it survives intact.
fn wrap_object_bytes_as_pdf(object_bytes: &[u8]) -> Vec<u8> {
    let header = b"%PDF-1.4\n".to_vec();
    let obj_offset = header.len();

    let mut buf = header;
    buf.extend_from_slice(b"1 0 obj\n");
    buf.extend_from_slice(object_bytes);
    buf.extend_from_slice(b"\nendobj\n");

    let xref_offset = buf.len();
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

    // --- 2026-08-08 Form XObject rendering follow-up: /Length mismatch in real data ---

    #[test]
    fn parse_pdf_object_bytes_corrects_a_wrong_declared_length() {
        // Reproduces the EXACT shape found in bench/corpus/btx/emittiv-markups.btx's
        // "emittiv stamp crop" stamp's root Form XObject: /Length 20, but the real
        // stream body ("/R0 gs /MWFOForm Do \n\n") is longer. Trusting the wrong
        // declared length made lopdf misparse this into a plain Dictionary instead of
        // a Stream - this test pins the fix (recompute /Length from the real
        // stream/endstream boundary before handing bytes to lopdf).
        let raw = b"<</Length 20/Type/XObject/Subtype/Form/FormType 1/BBox[0 0 2021.747 741.9073]>>\nstream\n/R0 gs /MWFOForm Do \n\nendstream";

        let obj = parse_pdf_object_bytes(raw).expect("must parse");
        let stream = match obj {
            Object::Stream(s) => s,
            other => panic!("expected a Stream (the real bug: this came back as {other:?})"),
        };
        assert_eq!(
            stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form"
        );
        // The content lopdf actually extracted must be the REAL body, not a 20-byte
        // truncation of it.
        assert_eq!(stream.content, b"/R0 gs /MWFOForm Do \n\n");
    }

    #[test]
    fn parse_pdf_object_bytes_still_works_when_length_is_already_correct() {
        // Regression guard: fix_stream_length must be a no-op (not corrupt anything)
        // when the declared /Length is already right - covers every hand-authored test
        // fixture elsewhere in this module and in document::annots.
        let body = b"q 1 0 0 1 0 0 cm /Fx0 Do Q";
        let raw = format!(
            "<</Length {}/Type/XObject/Subtype/Form/FormType 1/BBox[0 0 10 10]>>\nstream\n{}endstream",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let obj = parse_pdf_object_bytes(raw.as_bytes()).expect("must parse");
        let stream = match obj {
            Object::Stream(s) => s,
            other => panic!("expected a Stream, got {other:?}"),
        };
        assert_eq!(stream.content, body);
    }

    #[test]
    fn fix_stream_length_is_a_no_op_for_a_bare_dictionary() {
        let dict = b"<</Type/ExtGState/OPM 1>>";
        assert_eq!(fix_stream_length(dict), dict.to_vec());
    }

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
        // PLAIN_ITEM's <Name> ("Fire Rated Door") is the OLD, wrong mental model of what
        // Bluebeam's <Name> holds - real exports carry an opaque id there. The tool's
        // display name now correctly prefers /Subj ("Door") - see module doc "naming".
        assert_eq!(tool.name, "Door");
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
        //
        // Updated for the naming fix (module doc "naming"): the tool's DISPLAY name now
        // prefers /Subj, so the Cyrillic /Subj value is what must survive as `tool.name`
        // here. The emoji <Name> is still exercised (proves XML text-content decoding
        // doesn't choke on it) but is expected to be DISCARDED in favour of /Subj, not to
        // become the tool's name - that's precisely the naming-precedence behaviour this
        // fix introduced.
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
        // /Subj is a PDF literal string outside Latin-1 (Cyrillic) - lopdf reads literal
        // strings as raw bytes; the annotation reader's `get_string` behaviour on non-
        // Latin-1 bytes is what's actually under test here (real-world PDFs may use
        // PDFDocEncoding or UTF-16BE with a BOM for such content, not raw UTF-8) - and it
        // is now BOTH the tool's name and its subject.
        assert_eq!(report.tools[0].name, "Порядок");
        assert_eq!(report.tools[0].subject.as_deref(), Some("Порядок"));
    }

    #[test]
    fn falls_back_to_xml_name_when_subj_is_absent_non_ascii_included() {
        // The FALLBACK half of the naming fix: an item with no /Subj at all must still
        // get a usable, correctly-decoded name from <Name> - including non-ASCII content,
        // the case the previous test no longer exercises now that /Subj wins when present.
        let xml = r#"<ToolChestData>
          <ToolChestItem>
            <Name>Détecteur de fumée 🔥</Name>
            <Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Square /Rect [10 20 110 70] /C [1 0 0] >>]]></Raw>
          </ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "Détecteur de fumée 🔥");
        assert_eq!(report.tools[0].subject, None);
    }

    // --- 2026-08-08 corpus finding: custom Stamp items still displaying UID names -------

    #[test]
    fn stamp_source_file_basename_strips_windows_path_and_extension() {
        let mut d = Dictionary::new();
        d.set(
            "TmpBRXFile",
            Object::string_literal(r"D:\00 SetUp\Configs\BlueBeam\Stamps\MR Init.pdf"),
        );
        assert_eq!(stamp_source_file_basename(&d).as_deref(), Some("MR Init"));
    }

    #[test]
    fn stamp_source_file_basename_handles_forward_slashes_and_no_extension() {
        let mut d = Dictionary::new();
        d.set("TmpBRXFile", Object::string_literal("/mnt/stamps/Approved"));
        assert_eq!(stamp_source_file_basename(&d).as_deref(), Some("Approved"));
    }

    #[test]
    fn stamp_source_file_basename_absent_key_returns_none() {
        let d = Dictionary::new();
        assert_eq!(stamp_source_file_basename(&d), None);
    }

    #[test]
    fn stamp_source_file_basename_blank_path_returns_none() {
        let mut d = Dictionary::new();
        d.set("TmpBRXFile", Object::string_literal("   "));
        assert_eq!(stamp_source_file_basename(&d), None);
    }

    #[test]
    fn stamp_source_file_basename_dotfile_with_no_stem_returns_none() {
        // A basename that IS just an extension (".pdf", no stem before the dot) must not
        // produce an empty name - falls through to the raw <Name> UID like any other
        // genuinely-nameless case.
        let mut d = Dictionary::new();
        d.set("TmpBRXFile", Object::string_literal(r"C:\Stamps\.pdf"));
        assert_eq!(stamp_source_file_basename(&d), None);
    }

    #[test]
    fn stamp_with_no_subj_but_tmp_brx_file_uses_the_source_filename_not_the_uid() {
        // Reproduces the real corpus shape exactly: a Stamp Raw dict with /TmpBRXFile
        // present and /Subj absent (measured 5/7 real UID-fallback cases) must now name
        // the tool from the source file, not the opaque XML <Name>.
        // PDF literal-string syntax requires a backslash to be escaped as `\\` - real
        // Bluebeam-exported data does this correctly (confirmed against the actual
        // corpus: decoded /TmpBRXFile bytes carry single 0x5C separators), so the test
        // fixture must too.
        let xml = r#"<ToolChestData>
          <ToolChestItem>
            <Name>XXEOVOCUQESTKIRL</Name>
            <Mode>drawing</Mode>
            <Raw><![CDATA[<< /Subtype /Stamp /Rect [0 0 202.4566 117.0346]
              /TmpBRXFile (D:\\00 SetUp\\Configs\\BlueBeam\\Stamps\\emittiv stamp crop.pdf) >>]]></Raw>
          </ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "emittiv stamp crop");
        assert_eq!(
            report.tools[0].subject, None,
            "subject itself must stay None - only the DISPLAY name falls back, /Subj was genuinely absent"
        );
    }

    #[test]
    fn stamp_with_neither_subj_nor_tmp_brx_file_still_falls_back_to_xml_name() {
        // The genuinely-nameless case (2/7 real UID-fallback cases: no /Subj, no
        // /TmpBRXFile either) must still degrade to the opaque <Name> rather than
        // producing an empty/panicking result - the original fallback floor is preserved.
        let xml = r#"<ToolChestData>
          <ToolChestItem>
            <Name>OYXLGGCUBDYSRJYS</Name>
            <Mode>drawing</Mode>
            <Raw><![CDATA[<< /Subtype /Stamp /Rect [0 0 50 50] >>]]></Raw>
          </ToolChestItem>
        </ToolChestData>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].name, "OYXLGGCUBDYSRJYS");
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
        assert_eq!(report.tools[0].name, "Door"); // PLAIN_ITEM's /Subj - see naming fix
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
    fn stamp_without_an_ap_reference_still_has_no_artwork_residual_case() {
        // RESIDUAL, CORRECTLY NAMED (not the general gap - see `bb_form_xobject_root_id_
        // and_sibling_resources_resolve_into_a_bluebeam_form_x_object` below and the module
        // doc "Stamp artwork" for the mechanism that IS now fixed, 2026-08-06). A Stamp
        // Raw payload with no `/AP` at all - not something observed in the real sample
        // corpus (every one of the 11 Stamp-subtype items across it uses the
        // `/AP<</N/BBObjPtr_id>>` mechanism), but not guaranteed impossible - still
        // correctly gets `stamp: None`, since there is nothing to resolve. It imports with
        // no error and no skip; placement falls back to `appearance::draw_stamp_box_and_
        // label` (a plain bordered box + the tool's name as text).
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
        assert!(tool.stamp.is_none(), "no /AP reference means nothing to resolve - box+label fallback is correct here");
    }

    // --- Stamp artwork fixed (module doc "Stamp artwork") - real mechanism, verified
    // against `bench/corpus/btx/` (every Stamp-subtype item there uses it) ---

    /// Hex+zlib-encode `text`, the inverse of `inflate_hex_zlib`/`decode_hex_zlib_bytes` -
    /// lets tests build real Bluebeam-shaped `<Raw>`/`<ID>`/`<Data>` blobs instead of
    /// relying on the plain-text `<Raw>` fallback (UNVERIFIED against any real sample -
    /// see module doc).
    fn hex_zlib_encode(bytes: &[u8]) -> String {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        compressed.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn bb_form_xobject_root_id_and_sibling_resources_resolve_into_a_bluebeam_form_x_object() {
        // Reproduces the exact shape found in bench/corpus/btx/my-tools.btx's
        // AnnotationBRXStamp item: /AP<</N/BBObjPtr_<id>>> plus a sibling <Resources>
        // block whose <ID> decodes to that same <id> and whose <Data> is the Form
        // XObject's own raw PDF-syntax bytes.
        let form_bytes = b"<</Type/XObject/Subtype/Form/FormType 1/BBox[0 0 100 100]>>\nstream\nfake-content\nendstream";
        let id_hex = hex_zlib_encode(b"LBLQFTNPNJGWOKCB");
        let data_hex = hex_zlib_encode(form_bytes);
        let raw_hex = hex_zlib_encode(
            b"<</Subtype/Stamp/Rect[0 0 117.3555 82.95998]/AP<</N/BBObjPtr_LBLQFTNPNJGWOKCB>>/Subj(Stamp)>>",
        );
        let xml = format!(
            r#"<BluebeamRevuToolSet Version="1"><ToolChestItem>
                <Name>OYXLGGCUBDYSRJYS</Name>
                <Mode>drawing</Mode>
                <Raw>{raw_hex}</Raw>
                <Resources><ID>{id_hex}</ID><Data>{data_hex}</Data></Resources>
            </ToolChestItem></BluebeamRevuToolSet>"#
        );

        let report = parse_btx_xml(&xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
        let tool = &report.tools[0];
        assert_eq!(tool.markup_type, crate::markup::MarkupType::Stamp);
        match &tool.stamp {
            Some(StampDef::Static { asset: StampAsset::BluebeamFormXObject { root_id, objects } }) => {
                assert_eq!(root_id, "LBLQFTNPNJGWOKCB");
                assert_eq!(objects.len(), 1);
                assert_eq!(objects.get("LBLQFTNPNJGWOKCB").map(Vec::as_slice), Some(&form_bytes[..]));
            }
            other => panic!("expected a resolved BluebeamFormXObject asset, got {other:?}"),
        }
    }

    // NOTE: the end-to-end "does this actually splice into a real indirect object on
    // save" proof lives in `document::annots::tests::
    // write_markups_splices_a_bluebeam_form_x_object_graph_into_real_indirect_objects` -
    // that module already has `one_page_doc`/`resolve_ap_n_stream`/`Markup::new` in scope,
    // so the round-trip test belongs there rather than duplicating that scaffolding here.

    // --- Ordering fidelity (module doc "<Index>") ---

    #[test]
    fn tools_are_reordered_by_index_not_left_in_xml_document_order() {
        // Real exports store items in an order that does NOT match Bluebeam's authored
        // display order (module doc) - this XML deliberately lists "Third"/"First"/
        // "Second" in that document order, with <Index> values proving the real order is
        // First(0) < Second(1) < Third(2).
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem><Name>Third</Name><Mode>properties</Mode><Index>2</Index>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] /Subj (Third) >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>First</Name><Mode>properties</Mode><Index>0</Index>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] /Subj (First) >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Second</Name><Mode>properties</Mode><Index>1</Index>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] /Subj (Second) >>]]></Raw></ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);
        let names: Vec<&str> = report.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn items_with_no_index_keep_their_relative_document_order() {
        // No <Index> anywhere - every item falls back to its document position as the
        // sort key, so a stable sort leaves them exactly as authored (the common case for
        // the existing, pre-fix test fixtures in this file, none of which set <Index>).
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem><Name>Alpha</Name><Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] /Subj (Alpha) >>]]></Raw></ToolChestItem>
          <ToolChestItem><Name>Beta</Name><Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 1 1] /Subj (Beta) >>]]></Raw></ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        let names: Vec<&str> = report.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
    }

    // --- BOM handling (module doc: every real export carries one) ---

    #[test]
    fn leading_utf8_bom_does_not_break_xml_parsing() {
        let xml_with_bom = format!("\u{FEFF}{PLAIN_ITEM}");
        let report = parse_btx_xml(&xml_with_bom);
        assert!(report.skipped.is_empty(), "a leading BOM must not fail XML parsing: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1);
    }

    // --- Grouped/paired <Child> annotations (module doc "<Child>") - NAMED, NOT FIXED ---

    #[test]
    fn grouped_child_annotation_is_captured_on_tool_children() {
        // FIXED 2026-08-11 (design doc `docs/design/2026-08-11-grouped-markups.md`) -
        // supersedes the former "named, not fixed" version of this test. A <Child>
        // represents a SECOND, paired annotation Bluebeam groups with the parent
        // (measured: 33/77 = 43% of real items across the sample corpus carry one -
        // not a rare edge case). `Tool` now carries a `children: Vec<ToolChild>` for
        // exactly this.
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem>
            <Name>Parent</Name>
            <Mode>drawing</Mode>
            <Raw><![CDATA[<< /Subtype /Polygon /Rect [0 0 50 50] /Subj (Cloud Shape) >>]]></Raw>
            <Child>
              <Type>Bluebeam.PDF.Annotations.AnnotationFreeText</Type>
              <Raw><![CDATA[<< /Subtype /FreeText /Rect [0 0 20 10] /Subj (Attached Label) >>]]></Raw>
            </Child>
          </ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "the parent still imports cleanly: {:?}", report.skipped);
        assert_eq!(report.tools.len(), 1, "exactly the PARENT becomes a top-level tool - the <Child> is not a second tool");
        let tool = &report.tools[0];
        assert_eq!(tool.subject.as_deref(), Some("Cloud Shape"));
        assert_eq!(tool.markup_type, crate::markup::MarkupType::Polygon);

        assert_eq!(tool.children.len(), 1, "the <Child> must now be captured, not discarded");
        let child = &tool.children[0];
        assert_eq!(child.subject.as_deref(), Some("Attached Label"), "child's own /Subj must be preserved");
        assert_eq!(child.markup_type, crate::markup::MarkupType::Text, "FreeText with no /CL maps to Text");
        assert!(child.geometry.is_some(), "a Drawing-mode compound tool's child must carry a fixed geometry template");
    }

    #[test]
    fn multiple_children_are_all_captured_not_just_the_first() {
        // The real corpus has items with up to 20 <Child> elements (a "compound stamp"
        // tool) - the importer must not silently drop all but one.
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem>
            <Name>Parent</Name>
            <Mode>drawing</Mode>
            <Raw><![CDATA[<< /Subtype /Square /Rect [0 0 100 100] /Subj (Backing) >>]]></Raw>
            <Child><Raw><![CDATA[<< /Subtype /FreeText /Rect [0 0 20 10] /Subj (Label A) >>]]></Raw></Child>
            <Child><Raw><![CDATA[<< /Subtype /FreeText /Rect [0 0 20 10] /Subj (Label B) >>]]></Raw></Child>
            <Child><Raw><![CDATA[<< /Subtype /Line /L [0 0 10 10] /Subj (Divider) >>]]></Raw></Child>
          </ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        assert_eq!(report.tools.len(), 1);
        let subjects: Vec<Option<&str>> = report.tools[0].children.iter().map(|c| c.subject.as_deref()).collect();
        assert_eq!(subjects, vec![Some("Label A"), Some("Label B"), Some("Divider")]);
    }

    #[test]
    fn a_child_with_an_unparseable_raw_is_dropped_not_fatal_to_the_tool() {
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem>
            <Name>Parent</Name>
            <Mode>drawing</Mode>
            <Raw><![CDATA[<< /Subtype /Polygon /Rect [0 0 50 50] /Subj (Cloud Shape) >>]]></Raw>
            <Child><Raw>not-valid-pdf-syntax-at-all</Raw></Child>
          </ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        assert!(report.skipped.is_empty(), "a bad child must not fail the whole item");
        assert_eq!(report.tools.len(), 1);
        assert!(report.tools[0].children.is_empty(), "the unparseable child is dropped, not fabricated");
    }

    #[test]
    fn a_tool_with_no_child_elements_has_an_empty_children_vec() {
        let xml = r#"<BluebeamRevuToolSet Version="1">
          <ToolChestItem>
            <Name>Solo</Name>
            <Mode>properties</Mode>
            <Raw><![CDATA[<< /Subtype /Circle /Rect [0 0 10 10] /Subj (Just a circle) >>]]></Raw>
          </ToolChestItem>
        </BluebeamRevuToolSet>"#;

        let report = parse_btx_xml(xml);
        assert_eq!(report.tools.len(), 1);
        assert!(report.tools[0].children.is_empty());
    }

    // --- Real-sample characterization (bench/corpus/btx/ - gitignored is NOT the case
    // here, these 4 files are checked in as real fixtures per the dispatch) ---

    fn real_corpus_files() -> Vec<std::path::PathBuf> {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("bench").join("corpus").join("btx");
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("btx")))
            .collect()
    }

    #[test]
    fn real_bluebeam_samples_import_without_crashing_and_every_tool_gets_a_nonempty_name() {
        // Runs against the actual 4-file, 77-item real-sample corpus this rewrite was
        // built against (see module doc). Gated on the files existing (same pattern as
        // `sample_tool_set_fixture_path`) so this degrades to a skip rather than a
        // failure in a build context that doesn't carry bench/corpus/btx/ (e.g. the CI
        // Docker build context, which historically has NOT COPY'd bench/ - see the
        // sample-tool-set fixture's own test for that exact prior incident).
        let files = real_corpus_files();
        if files.is_empty() {
            eprintln!("skip: bench/corpus/btx/ not present in this build context");
            return;
        }
        let mut total_tools = 0usize;
        let mut total_bluebeam_form_stamps = 0usize;
        for path in &files {
            let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
            let report = import_btx_bytes(&bytes);
            assert!(
                !report.tools.is_empty() || !report.skipped.is_empty(),
                "{path:?} produced neither tools nor skips - the parse silently found nothing"
            );
            for tool in &report.tools {
                assert!(!tool.name.trim().is_empty(), "{path:?}: every imported tool must have a non-empty name");
                if let Some(StampDef::Static { asset: StampAsset::BluebeamFormXObject { .. } }) = &tool.stamp {
                    total_bluebeam_form_stamps += 1;
                }
            }
            total_tools += report.tools.len();
        }
        assert!(total_tools > 0, "the real corpus must produce at least one tool across all 4 files");
        assert!(
            total_bluebeam_form_stamps > 0,
            "at least one real Stamp item must resolve to a BluebeamFormXObject asset - \
             every Stamp-subtype item in this corpus uses that mechanism (module doc)"
        );
    }

    #[test]
    #[ignore] // diagnostic only - prints the fidelity table numbers quoted in the PR body, not an assertion
    fn print_real_corpus_fidelity_numbers() {
        let files = real_corpus_files();
        if files.is_empty() {
            eprintln!("skip: no corpus");
            return;
        }
        for path in &files {
            let bytes = std::fs::read(path).unwrap();
            let report = import_btx_bytes(&bytes);
            let mut stamps_total = 0usize;
            let mut stamps_with_artwork = 0usize;
            let mut name_differs_from_subj = 0usize;
            let mut tools_with_children = 0usize;
            let mut total_children = 0usize;
            for t in &report.tools {
                if matches!(t.markup_type, crate::markup::MarkupType::Stamp | crate::markup::MarkupType::StampDynamic) {
                    stamps_total += 1;
                    if matches!(&t.stamp, Some(StampDef::Static { asset: StampAsset::BluebeamFormXObject { .. } })) {
                        stamps_with_artwork += 1;
                    }
                }
                if t.subject.as_deref() != Some(t.name.as_str()) {
                    name_differs_from_subj += 1;
                }
                if !t.children.is_empty() {
                    tools_with_children += 1;
                    total_children += t.children.len();
                }
            }
            eprintln!(
                "{:?}: tools={} skipped={} stamps={} stamps_with_artwork={} names_from_fallback_not_subj={} \
                 grouped_tools={} total_children_captured={}",
                path.file_name().unwrap(),
                report.tools.len(),
                report.skipped.len(),
                stamps_total,
                stamps_with_artwork,
                name_differs_from_subj,
                tools_with_children,
                total_children
            );
        }
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
