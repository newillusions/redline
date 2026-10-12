//! Page labels (/PageLabels, ISO 32000-1 12.4.2): reading, formatting, editing and writing.
//!
//! The file stores *ranges* (a number tree keyed by the first page index of each range).
//! Editing is easier per page, so edits go through [`expand`] (one [`PageSpec`] per page),
//! change the specs, and [`compress`] back into the fewest ranges. Both directions are
//! lossless for what the format can express, which keeps load / save idempotent.

use std::collections::HashSet;

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::{clean_input_text, deref, text_of};

/// Most label ranges read from one file.
pub const MAX_LABEL_RANGES: usize = 10_000;
/// Deepest /Kids nesting followed in the number tree.
pub const MAX_TREE_DEPTH: usize = 16;
/// Most tree nodes visited (a hostile /Kids fan-out cannot stall the reader).
const MAX_TREE_STEPS: usize = 200_000;
/// Longest label prefix, in characters.
pub const MAX_LABEL_CHARS: usize = 256;
/// Largest start value honoured (keeps arithmetic and the output text bounded).
const MAX_START: u32 = 1_000_000_000;
/// Longest run of repeated letters in an A / a label before falling back to decimal.
const MAX_ALPHA_REPEAT: u32 = 8;

/// Numbering style of the numeric part (ISO 32000-1 Table 159, key /S).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelStyle {
    #[serde(rename = "D")]
    Decimal,
    #[serde(rename = "R")]
    UpperRoman,
    #[serde(rename = "r")]
    LowerRoman,
    #[serde(rename = "A")]
    UpperAlpha,
    #[serde(rename = "a")]
    LowerAlpha,
}

impl LabelStyle {
    fn from_name(n: &[u8]) -> Option<Self> {
        match n {
            b"D" => Some(Self::Decimal),
            b"R" => Some(Self::UpperRoman),
            b"r" => Some(Self::LowerRoman),
            b"A" => Some(Self::UpperAlpha),
            b"a" => Some(Self::LowerAlpha),
            _ => None,
        }
    }

    fn name(self) -> &'static [u8] {
        match self {
            Self::Decimal => b"D",
            Self::UpperRoman => b"R",
            Self::LowerRoman => b"r",
            Self::UpperAlpha => b"A",
            Self::LowerAlpha => b"a",
        }
    }
}

/// One labelling range: pages `start..` (0-based) up to the next range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LabelRange {
    pub start: u32,
    pub style: Option<LabelStyle>,
    pub prefix: String,
    /// Value of the numeric part on the first page of the range (>= 1).
    pub first: u32,
}

/// The label of one page in editable form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSpec {
    pub style: Option<LabelStyle>,
    pub prefix: String,
    pub number: u32,
}

impl PageSpec {
    fn empty() -> Self {
        PageSpec {
            style: None,
            prefix: String::new(),
            number: 1,
        }
    }

    fn is_empty_label(&self) -> bool {
        self.style.is_none() && self.prefix.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

fn roman(mut n: u32, upper: bool) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (v, s) in TABLE {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    Some(if upper { out } else { out.to_lowercase() })
}

fn alpha(n: u32, upper: bool) -> Option<String> {
    if n == 0 {
        return None;
    }
    let repeat = (n - 1) / 26 + 1;
    if repeat > MAX_ALPHA_REPEAT {
        return None;
    }
    let base = if upper { b'A' } else { b'a' };
    let ch = (base + ((n - 1) % 26) as u8) as char;
    Some(std::iter::repeat(ch).take(repeat as usize).collect())
}

/// The numeric part of a label. Values the style cannot express (a Roman numeral above 3999,
/// a run of more than eight repeated letters) fall back to decimal rather than failing.
pub fn format_number(style: LabelStyle, n: u32) -> String {
    let n = n.max(1);
    let text = match style {
        LabelStyle::Decimal => None,
        LabelStyle::UpperRoman => roman(n, true),
        LabelStyle::LowerRoman => roman(n, false),
        LabelStyle::UpperAlpha => alpha(n, true),
        LabelStyle::LowerAlpha => alpha(n, false),
    };
    text.unwrap_or_else(|| n.to_string())
}

/// The label text of a spec: prefix plus the numeric part (none when the style is none).
pub fn spec_text(spec: &PageSpec) -> String {
    match spec.style {
        Some(s) => format!("{}{}", spec.prefix, format_number(s, spec.number)),
        None => spec.prefix.clone(),
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn parse_range(doc: &Document, start: u32, d: &Dictionary) -> LabelRange {
    let style = d
        .get(b"S")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_name().ok())
        .and_then(LabelStyle::from_name);
    let prefix = d
        .get(b"P")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_str().ok())
        .map(|b| text_of(b, MAX_LABEL_CHARS))
        .unwrap_or_default();
    let first = d
        .get(b"St")
        .ok()
        .and_then(|o| deref(doc, o))
        .and_then(|o| o.as_i64().ok())
        .filter(|v| *v >= 1)
        .map(|v| v.min(i64::from(MAX_START)) as u32)
        .unwrap_or(1);
    LabelRange {
        start,
        style,
        prefix,
        first,
    }
}

struct Walk {
    out: Vec<LabelRange>,
    visited: HashSet<ObjectId>,
    steps: usize,
    /// A cap, the depth limit or a loop cut the walk short: the ranges read are partial.
    cut: bool,
}

fn walk(doc: &Document, obj: &Object, depth: usize, w: &mut Walk) {
    if depth > MAX_TREE_DEPTH || w.out.len() >= MAX_LABEL_RANGES || w.steps >= MAX_TREE_STEPS {
        w.cut = true;
        return;
    }
    w.steps += 1;
    let Ok((id, node)) = doc.dereference(obj) else {
        return;
    };
    if let Some(id) = id {
        if !w.visited.insert(id) {
            w.cut = true;
            return;
        }
    }
    let Object::Dictionary(d) = node else {
        return;
    };
    if let Some(Object::Array(nums)) = d.get(b"Nums").ok().and_then(|o| deref(doc, o)) {
        for pair in nums.chunks_exact(2) {
            if w.out.len() >= MAX_LABEL_RANGES {
                w.cut = true;
                break;
            }
            let Some(key) = deref(doc, &pair[0]).and_then(|o| o.as_i64().ok()) else {
                continue;
            };
            if key < 0 || key > i64::from(u32::MAX) {
                continue;
            }
            if let Some(Object::Dictionary(ld)) = deref(doc, &pair[1]) {
                w.out.push(parse_range(doc, key as u32, ld));
            }
        }
    }
    if let Some(Object::Array(kids)) = d.get(b"Kids").ok().and_then(|o| deref(doc, o)) {
        for k in kids {
            if w.out.len() >= MAX_LABEL_RANGES || w.steps >= MAX_TREE_STEPS {
                w.cut = true;
                break;
            }
            walk(doc, k, depth + 1, w);
        }
    }
}

fn read_walk(doc: &Document) -> Walk {
    let mut w = Walk {
        out: Vec::new(),
        visited: HashSet::new(),
        steps: 0,
        cut: false,
    };
    if let Ok(root) = doc.catalog().and_then(|c| c.get(b"PageLabels")) {
        walk(doc, root, 0, &mut w);
    }
    w
}

/// The document's label ranges, sorted by start page. Never fails: a damaged tree yields the
/// ranges that could be read.
pub fn read_ranges(doc: &Document) -> Vec<LabelRange> {
    let mut out = read_walk(doc).out;
    out.sort_by_key(|r| r.start);
    out.dedup_by_key(|r| r.start);
    out
}

/// Why the label tree cannot be edited, or `None` when it was read completely. A tree cut short
/// by the depth, range or step cap (or a loop) is a partial read: rewriting /PageLabels from it
/// would silently drop the ranges that were not read.
pub fn read_only_reason(doc: &Document) -> Option<String> {
    read_walk(doc)
        .cut
        .then(|| "the page-label tree is too deep or too large to read completely".to_string())
}

/// Ranges for an edit: refuses a partially read tree.
pub fn read_ranges_for_edit(doc: &Document) -> anyhow::Result<Vec<LabelRange>> {
    if let Some(reason) = read_only_reason(doc) {
        anyhow::bail!("The page labels are read-only: {reason}.");
    }
    Ok(read_ranges(doc))
}

/// Label text for every page (empty string where a page has none).
pub fn page_labels(ranges: &[LabelRange], page_count: u32) -> Vec<String> {
    expand(ranges, page_count)
        .iter()
        .map(|s| s.as_ref().map(spec_text).unwrap_or_default())
        .collect()
}

// ---------------------------------------------------------------------------
// Per-page editing model
// ---------------------------------------------------------------------------

/// One spec per page; `None` before the first range (a file whose tree lacks page 0).
pub fn expand(ranges: &[LabelRange], page_count: u32) -> Vec<Option<PageSpec>> {
    let mut out: Vec<Option<PageSpec>> = Vec::with_capacity(page_count as usize);
    let mut ri = 0usize;
    for page in 0..page_count {
        while ri + 1 < ranges.len() && ranges[ri + 1].start <= page {
            ri += 1;
        }
        match ranges.get(ri) {
            Some(r) if r.start <= page => {
                let number = r.first.saturating_add(page - r.start).min(MAX_START);
                out.push(Some(PageSpec {
                    style: r.style,
                    prefix: r.prefix.clone(),
                    number,
                }));
            }
            _ => out.push(None),
        }
    }
    out
}

/// The fewest ranges that give every page its spec. Empty (no labels at all) when every page
/// has an empty label.
pub fn compress(specs: &[Option<PageSpec>]) -> Vec<LabelRange> {
    let all_empty = specs
        .iter()
        .all(|s| s.as_ref().map(PageSpec::is_empty_label).unwrap_or(true));
    if all_empty {
        return Vec::new();
    }
    let mut out: Vec<LabelRange> = Vec::new();
    let mut prev: Option<PageSpec> = None;
    for (i, s) in specs.iter().enumerate() {
        let cur = s.clone().unwrap_or_else(PageSpec::empty);
        let continues = match &prev {
            Some(p) => {
                p.style == cur.style
                    && p.prefix == cur.prefix
                    && (cur.style.is_none() || cur.number == p.number.saturating_add(1))
            }
            None => false,
        };
        if !continues {
            out.push(LabelRange {
                start: i as u32,
                style: cur.style,
                prefix: cur.prefix.clone(),
                first: if cur.style.is_some() {
                    cur.number.max(1)
                } else {
                    1
                },
            });
        }
        prev = Some(cur);
    }
    out
}

/// "Number Pages": give `pages` (0-based, ascending, unique) the style and prefix, numbered
/// in order from `start`. A non-contiguous selection continues the count across the gap.
/// Style none with a blank prefix removes the labels of those pages.
pub fn apply_numbering(
    ranges: &[LabelRange],
    page_count: u32,
    pages: &[u32],
    style: Option<LabelStyle>,
    prefix: &str,
    start: u32,
) -> Result<Vec<LabelRange>> {
    let mut specs = expand(ranges, page_count);
    let prefix = clean_input_text(prefix, MAX_LABEL_CHARS);
    let start = start.clamp(1, MAX_START);
    for (k, page) in pages.iter().enumerate() {
        let slot = specs.get_mut(*page as usize).with_context(|| {
            format!(
                "page {} is outside this {page_count}-page document",
                page + 1
            )
        })?;
        *slot = Some(PageSpec {
            style,
            prefix: prefix.clone(),
            number: start.saturating_add(k as u32).min(MAX_START),
        });
    }
    Ok(compress(&specs))
}

/// Rename one page's label to plain text (style none, the text as the prefix). Blank text
/// removes the label.
pub fn rename_label(
    ranges: &[LabelRange],
    page_count: u32,
    page: u32,
    text: &str,
) -> Result<Vec<LabelRange>> {
    apply_numbering(ranges, page_count, &[page], None, text, 1)
}

/// Carry labels through a page operation: `map[new_index]` is the old index of the page now
/// at `new_index`, or `None` for a new (blank) page, which gets an empty label.
pub fn remap(ranges: &[LabelRange], old_count: u32, map: &[Option<u32>]) -> Vec<LabelRange> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let old = expand(ranges, old_count);
    let new: Vec<Option<PageSpec>> = map
        .iter()
        .map(|m| m.and_then(|o| old.get(o as usize).cloned().flatten()))
        .collect();
    compress(&new)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn range_dict(r: &LabelRange) -> Dictionary {
    let mut d = Dictionary::new();
    if let Some(s) = r.style {
        d.set("S", Object::Name(s.name().to_vec()));
    }
    if !r.prefix.is_empty() {
        d.set("P", crate::markup::replies::encode_text(&r.prefix));
    }
    if r.style.is_some() && r.first != 1 {
        d.set("St", Object::Integer(i64::from(r.first)));
    }
    d
}

/// Replace the document's /PageLabels with `ranges` (a new, direct /Nums number tree set on
/// the catalog). Empty `ranges` removes the entry.
///
/// No existing object is deleted or overwritten: the old tree, if it was an indirect object,
/// is left where it is (orphaned, harmless, dropped by Optimise). The entry is untrusted: it
/// can point at a page, an annotation or an object other things share, and removing "the old
/// tree" would destroy them.
pub fn write_ranges(doc: &mut Document, ranges: &[LabelRange]) -> Result<()> {
    if ranges.is_empty() {
        doc.catalog_mut().context("catalog")?.remove(b"PageLabels");
    } else {
        let mut nums: Vec<Object> = Vec::with_capacity(ranges.len() * 2);
        for r in ranges {
            nums.push(Object::Integer(i64::from(r.start)));
            nums.push(Object::Dictionary(range_dict(r)));
        }
        let mut tree = Dictionary::new();
        tree.set("Nums", Object::Array(nums));
        doc.catalog_mut()
            .context("catalog")?
            .set("PageLabels", Object::Dictionary(tree));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: u32, style: Option<LabelStyle>, prefix: &str, first: u32) -> LabelRange {
        LabelRange {
            start,
            style,
            prefix: prefix.into(),
            first,
        }
    }

    #[test]
    fn formats_all_styles() {
        use LabelStyle::*;
        assert_eq!(format_number(Decimal, 12), "12");
        assert_eq!(format_number(UpperRoman, 4), "IV");
        assert_eq!(format_number(LowerRoman, 1994), "mcmxciv");
        assert_eq!(format_number(UpperAlpha, 1), "A");
        assert_eq!(format_number(UpperAlpha, 26), "Z");
        assert_eq!(format_number(UpperAlpha, 27), "AA");
        assert_eq!(format_number(UpperAlpha, 53), "AAA");
        assert_eq!(format_number(LowerAlpha, 28), "bb");
    }

    #[test]
    fn unrepresentable_values_fall_back_to_decimal() {
        use LabelStyle::*;
        assert_eq!(format_number(UpperRoman, 4000), "4000");
        assert_eq!(
            format_number(UpperAlpha, 26 * 8 + 1),
            (26 * 8 + 1).to_string()
        );
        assert_eq!(format_number(UpperAlpha, u32::MAX), u32::MAX.to_string());
        assert_eq!(
            format_number(Decimal, 0),
            "1",
            "numbers below 1 are clamped"
        );
    }

    #[test]
    fn iso_example_document() {
        // i ii iii iv, 1 2 3, A-8 A-9 (ISO 32000-1 12.4.2 example).
        let ranges = vec![
            r(0, Some(LabelStyle::LowerRoman), "", 1),
            r(4, Some(LabelStyle::Decimal), "", 1),
            r(7, Some(LabelStyle::Decimal), "A-", 8),
        ];
        assert_eq!(
            page_labels(&ranges, 9),
            vec!["i", "ii", "iii", "iv", "1", "2", "3", "A-8", "A-9"]
        );
    }

    #[test]
    fn pages_before_the_first_range_have_no_label() {
        let ranges = vec![r(2, Some(LabelStyle::Decimal), "E-", 101)];
        assert_eq!(page_labels(&ranges, 4), vec!["", "", "E-101", "E-102"]);
    }

    #[test]
    fn expand_then_compress_is_lossless() {
        let ranges = vec![
            r(0, Some(LabelStyle::LowerRoman), "", 1),
            r(4, Some(LabelStyle::Decimal), "", 1),
            r(7, Some(LabelStyle::Decimal), "A-", 8),
            r(9, None, "Index", 1),
        ];
        let again = compress(&expand(&ranges, 12));
        assert_eq!(again, ranges);
        assert_eq!(page_labels(&again, 12), page_labels(&ranges, 12));
    }

    #[test]
    fn compress_merges_consecutive_pages_and_splits_on_a_jump() {
        let specs: Vec<Option<PageSpec>> = [1u32, 2, 3, 7, 8]
            .iter()
            .map(|n| {
                Some(PageSpec {
                    style: Some(LabelStyle::Decimal),
                    prefix: "E-".into(),
                    number: *n,
                })
            })
            .collect();
        let got = compress(&specs);
        assert_eq!(
            got,
            vec![
                r(0, Some(LabelStyle::Decimal), "E-", 1),
                r(3, Some(LabelStyle::Decimal), "E-", 7),
            ]
        );
    }

    #[test]
    fn compress_of_no_labels_is_empty_and_page_zero_always_has_an_entry() {
        assert!(compress(&[None, None, None]).is_empty());
        let mut specs = vec![None; 3];
        specs[2] = Some(PageSpec {
            style: Some(LabelStyle::Decimal),
            prefix: String::new(),
            number: 3,
        });
        let got = compress(&specs);
        assert_eq!(got[0].start, 0, "ISO: the tree shall include page index 0");
        assert_eq!(page_labels(&got, 3), vec!["", "", "3"]);
    }

    #[test]
    fn number_pages_over_a_range() {
        let got =
            apply_numbering(&[], 6, &[1, 2, 3], Some(LabelStyle::Decimal), "E-", 101).unwrap();
        assert_eq!(
            page_labels(&got, 6),
            vec!["", "E-101", "E-102", "E-103", "", ""]
        );
        assert_eq!(got[0].start, 0);
    }

    #[test]
    fn a_non_contiguous_selection_continues_the_count() {
        // `1-3, 5` starting at 1 gives 1, 2, 3 on pages 1-3 and 4 on page 5 (spec RL2).
        let got = apply_numbering(&[], 6, &[0, 1, 2, 4], Some(LabelStyle::Decimal), "", 1).unwrap();
        assert_eq!(page_labels(&got, 6), vec!["1", "2", "3", "", "4", ""]);
    }

    #[test]
    fn style_none_with_a_prefix_is_a_text_only_label() {
        let got = apply_numbering(&[], 3, &[1], None, "Cover", 1).unwrap();
        assert_eq!(page_labels(&got, 3), vec!["", "Cover", ""]);
    }

    #[test]
    fn style_none_with_a_blank_prefix_removes_labels() {
        let ranges = vec![r(0, Some(LabelStyle::Decimal), "E-", 1)];
        let got = apply_numbering(&ranges, 4, &[1, 2], None, "", 1).unwrap();
        assert_eq!(page_labels(&got, 4), vec!["E-1", "", "", "E-4"]);
        let none = apply_numbering(&ranges, 4, &[0, 1, 2, 3], None, "", 1).unwrap();
        assert!(none.is_empty(), "all pages unlabelled removes /PageLabels");
    }

    #[test]
    fn rename_one_page_keeps_its_neighbours_numbering() {
        let ranges = vec![r(0, Some(LabelStyle::Decimal), "E-", 1)];
        let got = rename_label(&ranges, 4, 1, "Legend").unwrap();
        assert_eq!(page_labels(&got, 4), vec!["E-1", "Legend", "E-3", "E-4"]);
    }

    #[test]
    fn out_of_range_pages_are_refused() {
        assert!(apply_numbering(&[], 3, &[3], Some(LabelStyle::Decimal), "", 1).is_err());
        assert!(rename_label(&[], 3, 9, "x").is_err());
    }

    #[test]
    fn labels_follow_their_pages_through_page_operations() {
        let ranges = vec![r(0, Some(LabelStyle::Decimal), "E-", 1)];
        // Delete page index 1 of 4: E-1, E-3, E-4.
        let after_delete = remap(&ranges, 4, &[Some(0), Some(2), Some(3)]);
        assert_eq!(page_labels(&after_delete, 3), vec!["E-1", "E-3", "E-4"]);
        // Reorder: new page 0 is old page 2, and so on.
        let after_reorder = remap(&ranges, 4, &[Some(2), Some(0), Some(3), Some(1)]);
        assert_eq!(
            page_labels(&after_reorder, 4),
            vec!["E-3", "E-1", "E-4", "E-2"]
        );
        // Insert a blank page at index 2: it has no label and the rest keep theirs.
        let after_insert = remap(&ranges, 4, &[Some(0), Some(1), None, Some(2), Some(3)]);
        assert_eq!(
            page_labels(&after_insert, 5),
            vec!["E-1", "E-2", "", "E-3", "E-4"]
        );
    }

    #[test]
    fn remap_with_no_labels_stays_empty() {
        assert!(remap(&[], 3, &[Some(2), Some(1), Some(0)]).is_empty());
    }

    #[test]
    fn hostile_start_values_stay_bounded() {
        let ranges = vec![r(0, Some(LabelStyle::UpperAlpha), "", MAX_START)];
        let labels = page_labels(&ranges, 3);
        assert!(labels.iter().all(|l| l.len() < 64), "{labels:?}");
    }
}
