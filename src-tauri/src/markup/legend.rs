//! Legends: a table of the document's mark-up quantities placed on a page, recomputed from the
//! live mark-ups. Behaviour spec: `docs/legends.md` (read it first).
//!
//! A legend is a [`MarkupType::Legend`] mark-up (geometry `Rect`) carrying a [`LegendProps`]
//! definition. The definition holds only what the user chose (title, scope, columns, one
//! [`LegendEntry`] per row, per-row text overrides); the numbers are never stored as truth:
//! [`compute_rows`] derives them from the other mark-ups every time, and [`refresh_for_save`]
//! does so at save to draw the appearance a foreign viewer shows.
//!
//! The same rules are implemented in `src/lib/legend.ts` for the live view; both are pinned to
//! one fixture (`tests/fixtures/legend-agreement.json`).
//!
//! Every value is untrusted (PDF on read, JSON from the GUI / MCP on write): [`LegendProps::
//! sanitized`] clamps or drops anything out of range, and all work is bounded by the caps below.

use std::collections::{BTreeSet, HashMap};

use lopdf::{Dictionary, Object};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CountSymbol, Markup, MarkupGeometry, MarkupType};
use crate::geometry::PdfPoint;

/// Entries (explicit rows) in one legend.
pub const MAX_ENTRIES: usize = 256;
/// Rows drawn, explicit plus implicit (auto count sets).
pub const MAX_ROWS: usize = 512;
/// Page ranges in a custom scope.
pub const MAX_PAGE_RANGES: usize = 50;
/// Characters in a row label / description.
pub const MAX_TEXT_CHARS: usize = 500;
/// Characters in a title.
pub const MAX_TITLE_CHARS: usize = 200;
/// Largest `/RLLegend` JSON the reader will parse.
pub const MAX_DEF_BYTES: usize = 512 * 1024;
/// Characters of the Pages cell.
pub const MAX_PAGES_CELL_CHARS: usize = 80;

pub const MIN_FONT_PT: f64 = 4.0;
pub const MAX_FONT_PT: f64 = 72.0;
pub const DEFAULT_FONT_PT: f64 = 9.0;
pub const MIN_WIDTH: f64 = 80.0;
pub const MAX_WIDTH: f64 = 20_000.0;
/// Default width of a new legend, PDF points.
pub const DEFAULT_WIDTH: f64 = 300.0;

/// Row height as a multiple of the font size.
pub const ROW_H_EM: f64 = 1.8;
/// Title bar height as a multiple of the font size.
pub const TITLE_H_EM: f64 = 2.2;
/// Average glyph width as a multiple of the font size (text clipping).
pub const GLYPH_EM: f64 = 0.52;

/// What a legend counts, as `from..=to` 1-based pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LegendScope {
    /// The page the legend sits on.
    CurrentPage,
    AllPages,
    Pages(Vec<[u32; 2]>),
}

/// A legend column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegendColumn {
    Symbol,
    Subject,
    Description,
    Quantity,
    Unit,
    Pages,
}

impl LegendColumn {
    pub const ALL: [LegendColumn; 6] = [
        LegendColumn::Symbol,
        LegendColumn::Subject,
        LegendColumn::Description,
        LegendColumn::Quantity,
        LegendColumn::Unit,
        LegendColumn::Pages,
    ];

    /// Header text.
    pub fn header(self) -> &'static str {
        match self {
            LegendColumn::Symbol => "",
            LegendColumn::Subject => "Subject",
            LegendColumn::Description => "Description",
            LegendColumn::Quantity => "Quantity",
            LegendColumn::Unit => "Unit",
            LegendColumn::Pages => "Pages",
        }
    }

    /// Share of the free width (the Symbol column is one row height wide instead).
    fn weight(self) -> f64 {
        match self {
            LegendColumn::Symbol => 0.0,
            LegendColumn::Subject => 3.0,
            LegendColumn::Description => 4.0,
            LegendColumn::Quantity => 1.5,
            LegendColumn::Unit => 1.0,
            LegendColumn::Pages => 2.0,
        }
    }
}

/// What a row matches: a mark-up type plus either a count set (counts of that set, any
/// colour) or colour + fill (the redline reading of "type and appearance").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegendKey {
    pub markup_type: MarkupType,
    #[serde(default)]
    pub count_set: Option<Uuid>,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub fill: Option<String>,
}

/// One row of a legend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegendEntry {
    pub key: LegendKey,
    /// Subject override; `None` follows the matching mark-ups.
    #[serde(default)]
    pub label: Option<String>,
    /// Description override; `None` follows the matching mark-ups' comments.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    /// Symbol shown while no mark-up matches (count rows).
    #[serde(default)]
    pub symbol: Option<CountSymbol>,
    /// Name of the count set the row was made for; shown while no mark-up matches.
    #[serde(default)]
    pub set_name: Option<String>,
}

/// The definition of a legend (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LegendProps {
    pub title: String,
    pub show_title: bool,
    pub show_headers: bool,
    pub scope: LegendScope,
    /// Count only mark-ups on this layer (the group's object number); `None` = every layer.
    /// Layer VISIBILITY never changes a legend: this filter is the only way a layer does.
    #[serde(default)]
    pub layer: Option<u32>,
    pub columns: Vec<LegendColumn>,
    pub entries: Vec<LegendEntry>,
    /// Count sets nobody claimed become rows after the explicit entries.
    pub auto_count_sets: bool,
    pub hide_empty: bool,
    pub font_size_pt: f64,
    /// Rows computed at save, for the appearance. Never part of the definition.
    #[serde(skip)]
    pub rows: Vec<LegendRow>,
}

impl Default for LegendProps {
    fn default() -> Self {
        LegendProps {
            title: "Legend".to_string(),
            show_title: true,
            show_headers: true,
            scope: LegendScope::CurrentPage,
            layer: None,
            columns: vec![
                LegendColumn::Symbol,
                LegendColumn::Subject,
                LegendColumn::Quantity,
                LegendColumn::Unit,
            ],
            entries: Vec::new(),
            auto_count_sets: false,
            hide_empty: false,
            font_size_pt: DEFAULT_FONT_PT,
            rows: Vec::new(),
        }
    }
}

/// Printable text: control characters become spaces, ends trimmed, at most `max` chars.
pub fn clean_text(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = cleaned.trim();
    t.chars().take(max).collect()
}

fn clean_color(c: &str) -> String {
    let b = c.as_bytes();
    if b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit) {
        c.to_ascii_lowercase()
    } else {
        String::new()
    }
}

impl LegendKey {
    fn sanitized(&self) -> LegendKey {
        let by_set = self.count_set.is_some();
        LegendKey {
            markup_type: self.markup_type,
            count_set: self.count_set,
            // A set key matches by set id alone; the colour is display only.
            color: clean_color(&self.color),
            fill: if by_set {
                None
            } else {
                self.fill
                    .as_deref()
                    .map(clean_color)
                    .filter(|c| !c.is_empty())
            },
        }
    }
}

impl LegendProps {
    /// The definition as safe to draw and write: text cleaned and capped, font clamped, at
    /// most [`MAX_ENTRIES`] entries (a markup type that cannot be listed, the legend itself,
    /// is dropped), distinct non-empty columns, at most [`MAX_PAGE_RANGES`] valid ranges.
    pub fn sanitized(&self) -> LegendProps {
        let mut columns: Vec<LegendColumn> = Vec::new();
        for c in &self.columns {
            if !columns.contains(c) {
                columns.push(*c);
            }
        }
        if columns.is_empty() {
            columns = LegendProps::default().columns;
        }
        let scope = match &self.scope {
            LegendScope::Pages(r) => {
                let ranges: Vec<[u32; 2]> = r
                    .iter()
                    .filter(|[a, b]| *a >= 1 && a <= b)
                    .take(MAX_PAGE_RANGES)
                    .copied()
                    .collect();
                if ranges.is_empty() {
                    LegendScope::CurrentPage
                } else {
                    LegendScope::Pages(ranges)
                }
            }
            other => other.clone(),
        };
        let font = if self.font_size_pt.is_finite() {
            self.font_size_pt.clamp(MIN_FONT_PT, MAX_FONT_PT)
        } else {
            DEFAULT_FONT_PT
        };
        let mut seen: Vec<Norm> = Vec::new();
        let mut entries: Vec<LegendEntry> = Vec::new();
        for e in &self.entries {
            if e.key.markup_type == MarkupType::Legend {
                continue;
            }
            let key = e.key.sanitized();
            let n = Norm::of(&key);
            if seen.contains(&n) {
                continue;
            }
            seen.push(n);
            entries.push(LegendEntry {
                key,
                label: e.label.as_deref().map(|t| clean_text(t, MAX_TEXT_CHARS)),
                description: e
                    .description
                    .as_deref()
                    .map(|t| clean_text(t, MAX_TEXT_CHARS)),
                hidden: e.hidden,
                symbol: e.symbol,
                set_name: e
                    .set_name
                    .as_deref()
                    .map(|t| clean_text(t, MAX_TEXT_CHARS))
                    .filter(|t| !t.is_empty()),
            });
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
        LegendProps {
            title: clean_text(&self.title, MAX_TITLE_CHARS),
            show_title: self.show_title,
            show_headers: self.show_headers,
            scope,
            layer: self.layer.filter(|n| *n > 0),
            columns,
            entries,
            auto_count_sets: self.auto_count_sets,
            hide_empty: self.hide_empty,
            font_size_pt: font,
            rows: Vec::new(),
        }
    }
}

// --- matching -------------------------------------------------------------------------

/// A key reduced to what matching compares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Norm {
    ty: MarkupType,
    set: Option<Uuid>,
    color: String,
    fill: Option<String>,
}

impl Norm {
    fn of(k: &LegendKey) -> Norm {
        if k.count_set.is_some() {
            Norm {
                ty: k.markup_type,
                set: k.count_set,
                color: String::new(),
                fill: None,
            }
        } else {
            Norm {
                ty: k.markup_type,
                set: None,
                color: k.color.to_ascii_lowercase(),
                fill: k.fill.as_ref().map(|f| f.to_ascii_lowercase()),
            }
        }
    }

    fn matches(&self, m: &Markup) -> bool {
        if m.markup_type != self.ty {
            return false;
        }
        let set = m.count_set.as_ref().map(|c| c.id);
        if set != self.set {
            return false;
        }
        if self.set.is_some() {
            return true;
        }
        m.appearance.color.eq_ignore_ascii_case(&self.color)
            && match (&m.appearance.fill, &self.fill) {
                (None, None) => true,
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => false,
            }
    }
}

/// The key a mark-up would be listed under.
pub fn key_of(m: &Markup) -> LegendKey {
    LegendKey {
        markup_type: m.markup_type,
        count_set: m.count_set.as_ref().map(|c| c.id),
        color: m
            .count_set
            .as_ref()
            .map_or_else(|| m.appearance.color.clone(), |c| c.color.clone()),
        fill: if m.count_set.is_some() {
            None
        } else {
            m.appearance.fill.clone()
        },
    }
    .sanitized()
}

fn in_scope(scope: &LegendScope, legend_page: u32, page: u32) -> bool {
    match scope {
        LegendScope::CurrentPage => page == legend_page,
        LegendScope::AllPages => true,
        LegendScope::Pages(ranges) => ranges.iter().any(|[a, b]| (*a..=*b).contains(&(page + 1))),
    }
}

// --- rows -----------------------------------------------------------------------------

/// One computed row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegendRow {
    /// Index of the explicit entry; `None` for an implicit (auto count set) row.
    pub entry: Option<usize>,
    pub key: LegendKey,
    pub subject: String,
    pub description: String,
    pub quantity: f64,
    pub quantity_text: String,
    pub unit: String,
    pub pages: String,
    /// Mark-ups claimed by the row.
    pub matched: usize,
    pub color: String,
    pub fill: Option<String>,
    pub symbol: Option<CountSymbol>,
}

/// `{:.2}` rounding half up on the magnitude, like JavaScript's `toFixed(2)`.
pub fn fixed2(q: f64) -> String {
    let q = if q == 0.0 { 0.0 } else { q };
    let mag = q.abs();
    // Only an exactly representable tie (a multiple of 1/8 ending in 5 at the third decimal)
    // differs between Rust's round-half-even and JavaScript's round-half-up.
    let is_tie = (mag * 8.0).fract() == 0.0 && ((mag * 1000.0).round() as u64) % 10 == 5;
    let s = if is_tie {
        format!("{:.2}", mag + 0.0005)
    } else {
        format!("{mag:.2}")
    };
    if q < 0.0 {
        format!("-{s}")
    } else {
        s
    }
}

fn pages_text(pages: &BTreeSet<u32>) -> String {
    pages_text_labelled(pages, &[])
}

/// The Pages cell. A document with page labels lists them (`E-101, E-102`, an unlabelled page
/// by its number); one without lists page-number ranges (`1-3, 5`). Clipped either way.
fn pages_text_labelled(pages: &BTreeSet<u32>, labels: &[String]) -> String {
    let label_of = |p: u32| {
        labels
            .get(p as usize)
            .map(String::as_str)
            .filter(|l| !l.is_empty())
    };
    if pages.iter().any(|p| label_of(*p).is_some()) {
        let mut out = String::new();
        for p in pages {
            if !out.is_empty() {
                out.push_str(", ");
            }
            match label_of(*p) {
                Some(l) => out.push_str(l),
                None => out.push_str(&(p + 1).to_string()),
            }
            if out.chars().count() > MAX_PAGES_CELL_CHARS {
                break;
            }
        }
        if out.chars().count() > MAX_PAGES_CELL_CHARS {
            let cut: String = out.chars().take(MAX_PAGES_CELL_CHARS - 3).collect();
            return format!("{cut}...");
        }
        return out;
    }
    let mut out = String::new();
    let mut it = pages.iter().map(|p| p + 1).peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while it.peek() == Some(&(end + 1)) {
            end = it.next().unwrap_or(end);
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        if end == start {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}-{end}"));
        }
        if out.chars().count() > MAX_PAGES_CELL_CHARS {
            break;
        }
    }
    if out.chars().count() > MAX_PAGES_CELL_CHARS {
        let cut: String = out.chars().take(MAX_PAGES_CELL_CHARS - 3).collect();
        return format!("{cut}...");
    }
    out
}

/// The first candidate that is a valid `#rrggbb`, else black. Colours come from PDF data and
/// are drawn into the appearance, so an unusable one must never reach the drawing code.
fn safe_color(candidates: &[Option<&str>]) -> String {
    candidates
        .iter()
        .flatten()
        .map(|c| clean_color(c))
        .find(|c| !c.is_empty())
        .unwrap_or_else(|| "#000000".to_string())
}

/// Types whose symbol cell shows a line sample rather than a box.
pub fn is_line_type(t: MarkupType) -> bool {
    matches!(
        t,
        MarkupType::Line
            | MarkupType::Arrow
            | MarkupType::Polyline
            | MarkupType::Ink
            | MarkupType::Underline
            | MarkupType::StrikeOut
            | MarkupType::Squiggly
            | MarkupType::MeasurementLength
            | MarkupType::MeasurementAngle
            | MarkupType::MeasurementRadius
    )
}

fn is_summed(t: MarkupType) -> bool {
    matches!(
        t,
        MarkupType::MeasurementLength
            | MarkupType::MeasurementPerimeter
            | MarkupType::MeasurementArea
            | MarkupType::MeasurementVolume
    )
}

/// (quantity, text, unit) for the mark-ups one row claimed. The rule is the one the
/// Quantities panel, the Markups List and the exports use (docs/legends.md R9).
fn quantity_of(ty: MarkupType, items: &[&Markup]) -> (f64, String, String) {
    let first_unit = || {
        items
            .iter()
            .filter_map(|m| m.measurement.as_ref())
            .map(|m| m.unit.trim())
            .find(|u| !u.is_empty())
            .map(|u| clean_text(u, 16))
    };
    if ty == MarkupType::MeasurementCount {
        let n: u64 = items
            .iter()
            .map(|m| {
                u64::from(
                    m.measurement
                        .as_ref()
                        .and_then(|x| x.count_value)
                        .unwrap_or(1),
                )
            })
            .sum();
        return (
            n as f64,
            n.to_string(),
            first_unit().unwrap_or_else(|| "ea".to_string()),
        );
    }
    if is_summed(ty) {
        let q: f64 = items
            .iter()
            .filter_map(|m| m.measurement.as_ref())
            .map(|m| m.computed_quantity)
            .filter(|v| v.is_finite())
            .sum();
        return (q, fixed2(q), first_unit().unwrap_or_default());
    }
    (
        items.len() as f64,
        items.len().to_string(),
        "ea".to_string(),
    )
}

fn first_text(items: &[&Markup], f: impl Fn(&Markup) -> Option<&str>) -> Option<String> {
    items
        .iter()
        .filter_map(|m| f(m))
        .map(|t| clean_text(t, MAX_TEXT_CHARS))
        .find(|t| !t.is_empty())
}

fn type_label(t: MarkupType) -> String {
    format!("{t:?}")
}

fn build_row(
    entry_idx: Option<usize>,
    entry: Option<&LegendEntry>,
    key: &LegendKey,
    items: &[&Markup],
    labels: &[String],
) -> LegendRow {
    let (quantity, quantity_text, unit) = quantity_of(key.markup_type, items);
    let set = items.iter().find_map(|m| m.count_set.as_ref());
    let label = entry
        .and_then(|e| e.label.as_deref())
        .map(|t| clean_text(t, MAX_TEXT_CHARS))
        .filter(|t| !t.is_empty());
    let subject = label
        .or_else(|| first_text(items, |m| m.subject.as_deref()))
        .or_else(|| {
            set.map(|s| clean_text(&s.name, MAX_TEXT_CHARS))
                .filter(|t| !t.is_empty())
        })
        .or_else(|| {
            entry
                .and_then(|e| e.set_name.clone())
                .filter(|t| !t.is_empty())
        })
        .unwrap_or_else(|| type_label(key.markup_type));
    let description = entry
        .and_then(|e| e.description.as_deref())
        .map(|t| clean_text(t, MAX_TEXT_CHARS))
        .filter(|t| !t.is_empty())
        .or_else(|| first_text(items, |m| m.contents.as_deref()))
        .unwrap_or_default();
    let pages: BTreeSet<u32> = items.iter().map(|m| m.page).collect();
    let first = items.first();
    let symbol = if key.markup_type == MarkupType::MeasurementCount {
        Some(
            set.map(|s| s.symbol)
                .or_else(|| entry.and_then(|e| e.symbol))
                .unwrap_or_default(),
        )
    } else {
        None
    };
    LegendRow {
        entry: entry_idx,
        key: key.clone(),
        subject,
        description,
        quantity,
        quantity_text,
        unit,
        pages: pages_text_labelled(&pages, labels),
        matched: items.len(),
        color: safe_color(&[
            set.map(|s| s.color.as_str()),
            first.map(|m| m.appearance.color.as_str()),
            Some(key.color.as_str()),
        ]),
        fill: first
            .and_then(|m| {
                if set.is_some() {
                    None
                } else {
                    m.appearance.fill.as_deref()
                }
            })
            .map(clean_color)
            .filter(|c| !c.is_empty())
            .or_else(|| {
                if first.is_none() {
                    key.fill.clone()
                } else {
                    None
                }
            }),
        symbol,
    }
}

/// The mark-ups one entry claimed, split by measurement unit (first-seen order) for the
/// summed measurement types, so 5.0 m and 7.5 ft are two rows, not "12.50 m". Everything else,
/// and an entry that claimed nothing, is one group.
fn split_by_unit<'a>(ty: MarkupType, items: &[&'a Markup]) -> Vec<Vec<&'a Markup>> {
    if !is_summed(ty) || items.is_empty() {
        return vec![items.to_vec()];
    }
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<&'a Markup>> = Vec::new();
    for m in items {
        let u = m
            .measurement
            .as_ref()
            .map(|x| x.unit.trim().to_string())
            .unwrap_or_default();
        match order.iter().position(|x| *x == u) {
            Some(i) => groups[i].push(m),
            None => {
                order.push(u);
                groups.push(vec![m]);
            }
        }
    }
    groups
}

/// The visible rows of a legend sitting on `legend_page`, computed from `markups`.
/// `props` must be sanitised. The legend mark-ups themselves are never counted.
pub fn compute_rows(legend_page: u32, props: &LegendProps, markups: &[Markup]) -> Vec<LegendRow> {
    compute_rows_labelled(legend_page, props, markups, &[])
}

/// [`compute_rows`] with the document's page labels (text per page, `""` = none) for the
/// Pages column.
pub fn compute_rows_labelled(
    legend_page: u32,
    props: &LegendProps,
    markups: &[Markup],
    labels: &[String],
) -> Vec<LegendRow> {
    let norms: Vec<Norm> = props.entries.iter().map(|e| Norm::of(&e.key)).collect();
    let mut claimed: Vec<Vec<&Markup>> = vec![Vec::new(); props.entries.len()];
    // Implicit rows: count sets no explicit entry claimed, in order of first appearance.
    let mut implicit_order: Vec<Uuid> = Vec::new();
    let mut implicit: HashMap<Uuid, Vec<&Markup>> = HashMap::new();
    for m in markups {
        if m.markup_type == MarkupType::Legend || !in_scope(&props.scope, legend_page, m.page) {
            continue;
        }
        if let Some(layer) = props.layer {
            if !matches!(m.optional_content, Some(crate::markup::OptionalContent::Reference(n, _)) if n == layer)
            {
                continue;
            }
        }
        if let Some(i) = norms.iter().position(|n| n.matches(m)) {
            claimed[i].push(m);
        } else if props.auto_count_sets && m.markup_type == MarkupType::MeasurementCount {
            if let Some(cs) = &m.count_set {
                if !implicit.contains_key(&cs.id) {
                    if implicit_order.len() >= MAX_ENTRIES {
                        continue;
                    }
                    implicit_order.push(cs.id);
                }
                implicit.entry(cs.id).or_default().push(m);
            }
        }
    }
    let mut rows: Vec<LegendRow> = Vec::new();
    for (i, e) in props.entries.iter().enumerate() {
        if e.hidden || (props.hide_empty && claimed[i].is_empty()) {
            continue;
        }
        // Measured quantities in different units are never added together: one row per unit.
        for group in split_by_unit(e.key.markup_type, &claimed[i]) {
            rows.push(build_row(Some(i), Some(e), &e.key, &group, labels));
            if rows.len() >= MAX_ROWS {
                return rows;
            }
        }
    }
    for id in implicit_order {
        let items = implicit.remove(&id).unwrap_or_default();
        let key = LegendKey {
            markup_type: MarkupType::MeasurementCount,
            count_set: Some(id),
            color: String::new(),
            fill: None,
        };
        rows.push(build_row(None, None, &key, &items, labels));
        if rows.len() >= MAX_ROWS {
            break;
        }
    }
    rows
}

// --- layout ---------------------------------------------------------------------------

/// One column's horizontal extent, relative to the legend's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnBox {
    pub column: LegendColumn,
    pub x: f64,
    pub w: f64,
}

/// Sizes of a drawn legend (PDF points).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub font: f64,
    pub row_h: f64,
    pub title_h: f64,
    pub header_h: f64,
    pub columns: Vec<ColumnBox>,
}

/// Layout for a legend `width` wide showing `rows` rows. `props` must be sanitised.
pub fn layout(props: &LegendProps, width: f64, rows: usize) -> Layout {
    let font = props.font_size_pt;
    let row_h = font * ROW_H_EM;
    let title_h = if props.show_title {
        font * TITLE_H_EM
    } else {
        0.0
    };
    let header_h = if props.show_headers { row_h } else { 0.0 };
    let width = if width.is_finite() {
        width.clamp(MIN_WIDTH, MAX_WIDTH)
    } else {
        DEFAULT_WIDTH
    };
    let symbol_w = row_h;
    let flexible: f64 = props.columns.iter().map(|c| c.weight()).sum();
    let free = (width
        - if props.columns.contains(&LegendColumn::Symbol) {
            symbol_w
        } else {
            0.0
        })
    .max(0.0);
    let mut x = 0.0;
    let mut columns = Vec::with_capacity(props.columns.len());
    for c in &props.columns {
        let w = if *c == LegendColumn::Symbol {
            if flexible == 0.0 {
                width
            } else {
                symbol_w
            }
        } else {
            free * c.weight() / flexible
        };
        columns.push(ColumnBox { column: *c, x, w });
        x += w;
    }
    Layout {
        width,
        height: title_h + header_h + row_h * rows.max(1) as f64,
        font,
        row_h,
        title_h,
        header_h,
        columns,
    }
}

/// Text clipped to a column: `...` replaces the tail that does not fit.
pub fn clip_cell(s: &str, col_w: f64, font: f64) -> String {
    let max = ((col_w - font) / (GLYPH_EM * font)).floor();
    // NaN (a non-finite width) also lands here, like in the TypeScript twin.
    if max.is_nan() || max < 1.0 {
        return String::new();
    }
    let max = max as usize;
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let head: String = s.chars().take(max - 3).collect();
    format!("{head}...")
}

/// The cell text of `row` under `column` (not clipped).
pub fn cell_text(row: &LegendRow, column: LegendColumn) -> String {
    match column {
        LegendColumn::Symbol => String::new(),
        LegendColumn::Subject => row.subject.clone(),
        LegendColumn::Description => row.description.clone(),
        LegendColumn::Quantity => row.quantity_text.clone(),
        LegendColumn::Unit => row.unit.clone(),
        LegendColumn::Pages => row.pages.clone(),
    }
}

/// `[x0, y0, x1, y1]` a legend occupies when it shows `rows` rows: the stored left, right and
/// top edges are the user's; the bottom edge follows the rows.
pub fn effective_rect(m: &Markup, rows: usize) -> Option<[f64; 4]> {
    rect_for(&m.legend.as_ref()?.sanitized(), &m.geometry, rows)
}

/// [`effective_rect`] for an already sanitised definition and a geometry.
pub fn rect_for(props: &LegendProps, geometry: &MarkupGeometry, rows: usize) -> Option<[f64; 4]> {
    let MarkupGeometry::Rect { min, max } = geometry else {
        return None;
    };
    if ![min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite()) {
        return None;
    }
    let x0 = min.x.min(max.x);
    let top = min.y.max(max.y);
    let l = layout(props, (max.x - min.x).abs(), rows);
    Some([x0, top - l.height, x0 + l.width, top])
}

/// [`refresh_for_save`] over quantities recomputed under the document's current `scales`, on a
/// copy, exactly as the exports do first (so a measurement whose stored quantity predates the
/// latest scale still prints the number the export prints). `scales` empty = stored values.
pub fn refresh_for_save_scaled(
    markups: &[Markup],
    scales: &[crate::takeoff::ScaleRecord],
) -> HashMap<Uuid, (LegendProps, SaveUpdate)> {
    refresh_for_save_scaled_labelled(markups, scales, &[])
}

/// [`refresh_for_save_scaled`] with the document's page labels for the Pages column.
pub fn refresh_for_save_scaled_labelled(
    markups: &[Markup],
    scales: &[crate::takeoff::ScaleRecord],
    labels: &[String],
) -> HashMap<Uuid, (LegendProps, SaveUpdate)> {
    if scales.is_empty() || !markups.iter().any(|m| m.markup_type == MarkupType::Legend) {
        return refresh_for_save_labelled(markups, labels);
    }
    let mut copy = markups.to_vec();
    crate::takeoff::recompute_all(&mut copy, scales);
    refresh_for_save_labelled(&copy, labels)
}

/// Rows and geometry to apply to a legend at save.
pub struct SaveUpdate {
    pub rows: Vec<LegendRow>,
    pub geometry: MarkupGeometry,
}

/// For every legend among `markups`: its sanitised definition with the freshly computed rows
/// and the geometry that matches them, keyed by mark-up id. Empty when there is no legend.
pub fn refresh_for_save(markups: &[Markup]) -> HashMap<Uuid, (LegendProps, SaveUpdate)> {
    refresh_for_save_labelled(markups, &[])
}

/// [`refresh_for_save`] with the document's page labels for the Pages column.
pub fn refresh_for_save_labelled(
    markups: &[Markup],
    labels: &[String],
) -> HashMap<Uuid, (LegendProps, SaveUpdate)> {
    let mut out = HashMap::new();
    for m in markups {
        if m.markup_type != MarkupType::Legend {
            continue;
        }
        let Some(raw) = &m.legend else { continue };
        let props = raw.sanitized();
        let rows = compute_rows_labelled(m.page, &props, markups, labels);
        let Some(r) = rect_for(&props, &m.geometry, rows.len()) else {
            continue;
        };
        let geometry = MarkupGeometry::Rect {
            min: PdfPoint { x: r[0], y: r[1] },
            max: PdfPoint { x: r[2], y: r[3] },
        };
        out.insert(m.id(), (props, SaveUpdate { rows, geometry }));
    }
    out
}

// --- PDF keys -------------------------------------------------------------------------

/// Write `/RLLegend` (the sanitised definition as JSON) for a legend mark-up.
pub fn write_keys(d: &mut Dictionary, m: &Markup) {
    if m.markup_type != MarkupType::Legend {
        return;
    }
    let Some(p) = &m.legend else { return };
    if let Ok(json) = serde_json::to_string(&p.sanitized()) {
        d.set("RLLegend", Object::string_literal(json));
    }
}

/// Read `/RLLegend`: size-capped, parsed, sanitised. `None` when absent or unusable.
pub fn read_props(d: &Dictionary) -> Option<LegendProps> {
    let s = d
        .get(b"RLLegend")
        .ok()?
        .as_str()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())?;
    if s.len() > MAX_DEF_BYTES {
        return None;
    }
    serde_json::from_str::<LegendProps>(&s)
        .ok()
        .map(|p| p.sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Appearance, CountSet, Measurement, UserRef};

    fn user() -> UserRef {
        UserRef {
            user_id: Uuid::nil(),
            display_name: "T".into(),
        }
    }

    fn rect_geom() -> MarkupGeometry {
        MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 310.0, y: 200.0 },
        }
    }

    fn mk(t: MarkupType, page: u32, color: &str) -> Markup {
        let a = Appearance {
            color: color.to_string(),
            ..Appearance::default()
        };
        let g = match t {
            MarkupType::MeasurementCount => MarkupGeometry::Point(PdfPoint { x: 5.0, y: 5.0 }),
            _ => rect_geom(),
        };
        Markup::new(t, page, g, a, user())
    }

    fn count(page: u32, set: &CountSet, n: u32) -> Markup {
        let mut m = mk(MarkupType::MeasurementCount, page, &set.color);
        m.count_set = Some(set.clone());
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

    fn length(page: u32, q: f64, unit: &str) -> Markup {
        let mut m = mk(MarkupType::MeasurementLength, page, "#112233");
        m.measurement = Some(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: unit.into(),
            computed_quantity: q,
            depth: None,
            count_value: None,
            custom_columns: Default::default(),
            rise_drop: Vec::new(),
        });
        m
    }

    fn set(name: &str, color: &str) -> CountSet {
        CountSet {
            id: Uuid::new_v4(),
            name: name.into(),
            color: color.into(),
            symbol: CountSymbol::Square,
        }
    }

    fn entry_for(m: &Markup) -> LegendEntry {
        LegendEntry {
            key: key_of(m),
            label: None,
            description: None,
            hidden: false,
            symbol: None,
            set_name: None,
        }
    }

    fn props_all(entries: Vec<LegendEntry>) -> LegendProps {
        LegendProps {
            scope: LegendScope::AllPages,
            entries,
            ..LegendProps::default()
        }
        .sanitized()
    }

    #[test]
    fn count_row_sums_count_values_across_pages_and_scope() {
        let s = set("Downlight", "#ff0000");
        let ms = vec![count(0, &s, 1), count(0, &s, 2), count(2, &s, 4)];
        let p = props_all(vec![entry_for(&ms[0])]);
        let rows = compute_rows(0, &p, &ms);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].quantity_text, "7");
        assert_eq!(rows[0].unit, "ea");
        assert_eq!(rows[0].pages, "1, 3");
        assert_eq!(rows[0].subject, "Downlight");
        // Current page only: pages 1 only.
        let cur = LegendProps {
            scope: LegendScope::CurrentPage,
            ..p.clone()
        };
        assert_eq!(compute_rows(0, &cur, &ms)[0].quantity_text, "3");
        assert_eq!(compute_rows(2, &cur, &ms)[0].quantity_text, "4");
        let custom = LegendProps {
            scope: LegendScope::Pages(vec![[2, 3]]),
            ..p
        };
        assert_eq!(compute_rows(0, &custom, &ms)[0].quantity_text, "4");
    }

    #[test]
    fn rows_update_on_add_edit_and_delete() {
        let s = set("A", "#ff0000");
        let mut ms = vec![count(0, &s, 1)];
        let p = props_all(vec![entry_for(&ms[0])]);
        assert_eq!(compute_rows(0, &p, &ms)[0].quantity_text, "1");
        ms.push(count(0, &s, 1)); // add
        assert_eq!(compute_rows(0, &p, &ms)[0].quantity_text, "2");
        ms[1].measurement.as_mut().unwrap().count_value = Some(5); // edit
        assert_eq!(compute_rows(0, &p, &ms)[0].quantity_text, "6");
        ms.remove(0); // delete
        assert_eq!(compute_rows(0, &p, &ms)[0].quantity_text, "5");
        ms.clear();
        let r = compute_rows(0, &p, &ms);
        assert_eq!(
            r[0].quantity_text, "0",
            "an entry with nothing left stays at zero"
        );
        assert_eq!(r[0].matched, 0);
    }

    #[test]
    fn hide_empty_and_hidden_drop_rows() {
        let s = set("A", "#ff0000");
        let m = count(0, &s, 1);
        let mut p = props_all(vec![entry_for(&m)]);
        p.hide_empty = true;
        assert!(compute_rows(0, &p, &[]).is_empty());
        assert_eq!(compute_rows(0, &p, std::slice::from_ref(&m)).len(), 1);
        p.entries[0].hidden = true;
        assert!(compute_rows(0, &p, &[m]).is_empty());
    }

    #[test]
    fn lengths_sum_computed_quantity_with_two_decimals() {
        let ms = vec![
            length(0, 10.004, "m"),
            length(0, 5.0, "m"),
            length(0, f64::NAN, "m"),
        ];
        let p = props_all(vec![entry_for(&ms[0])]);
        let r = &compute_rows(0, &p, &ms)[0];
        assert_eq!(r.quantity_text, "15.00");
        assert_eq!(r.unit, "m");
        assert_eq!(r.matched, 3);
    }

    #[test]
    fn fixed2_rounds_half_up_like_to_fixed() {
        assert_eq!(fixed2(0.125), "0.13");
        assert_eq!(fixed2(2.5), "2.50");
        assert_eq!(fixed2(1.005), "1.00");
        assert_eq!(fixed2(-0.125), "-0.13");
        assert_eq!(fixed2(0.0), "0.00");
        assert_eq!(fixed2(-0.0), "0.00");
        assert_eq!(fixed2(1234.5678), "1234.57");
    }

    #[test]
    fn other_types_count_markups() {
        let ms = vec![
            mk(MarkupType::Cloud, 0, "#ff0000"),
            mk(MarkupType::Cloud, 0, "#FF0000"),
        ];
        let p = props_all(vec![entry_for(&ms[0])]);
        let r = &compute_rows(0, &p, &ms)[0];
        assert_eq!(r.quantity_text, "2", "colour compare ignores case");
        assert_eq!(r.unit, "ea");
    }

    #[test]
    fn first_entry_claims_a_markup_once() {
        let s = set("A", "#ff0000");
        let m = count(0, &s, 1);
        let e = entry_for(&m);
        // Two entries with the same key are collapsed by sanitising.
        let p = props_all(vec![e.clone(), e]);
        assert_eq!(p.entries.len(), 1);
        // A set-less entry does not claim a set count.
        let setless = LegendEntry {
            key: LegendKey {
                markup_type: MarkupType::MeasurementCount,
                count_set: None,
                color: "#ff0000".into(),
                fill: None,
            },
            label: None,
            description: None,
            hidden: false,
            symbol: None,
            set_name: None,
        };
        let p2 = props_all(vec![setless]);
        assert_eq!(compute_rows(0, &p2, &[m])[0].matched, 0);
    }

    #[test]
    fn auto_count_sets_append_unclaimed_sets_in_order() {
        let a = set("A", "#ff0000");
        let b = set("B", "#00ff00");
        let ms = vec![count(0, &b, 2), count(0, &a, 1), count(0, &b, 1)];
        let mut p = props_all(vec![entry_for(&ms[1])]);
        p.auto_count_sets = true;
        let rows = compute_rows(0, &p, &ms);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject, "A");
        assert_eq!(rows[1].subject, "B");
        assert_eq!(rows[1].quantity_text, "3");
        assert_eq!(rows[1].entry, None);
    }

    #[test]
    fn overrides_and_follow_the_markups_text() {
        let s = set("Set name", "#ff0000");
        let mut m = count(0, &s, 1);
        m.subject = Some("From markup".into());
        m.contents = Some("  fixture note ".into());
        let mut p = props_all(vec![entry_for(&m)]);
        let r = &compute_rows(0, &p, &[m.clone()])[0];
        assert_eq!(r.subject, "From markup");
        assert_eq!(r.description, "fixture note");
        p.entries[0].label = Some("Override".into());
        p.entries[0].description = Some("Own text".into());
        let r = &compute_rows(0, &p, &[m])[0];
        assert_eq!(r.subject, "Override");
        assert_eq!(r.description, "Own text");
    }

    #[test]
    fn a_legend_is_never_counted() {
        let mut l = mk(MarkupType::Legend, 0, "#000000");
        l.legend = Some(LegendProps::default());
        let p = props_all(vec![entry_for(&mk(MarkupType::Legend, 0, "#000000"))]);
        assert!(p.entries.is_empty(), "a legend cannot be listed");
        assert!(compute_rows(0, &p, &[l]).is_empty());
    }

    #[test]
    fn sanitise_clamps_everything_hostile() {
        let hostile = LegendProps {
            title: "x".repeat(10_000) + "\u{0}\n",
            font_size_pt: f64::NAN,
            columns: vec![],
            scope: LegendScope::Pages(vec![[0, 3], [5, 2]]),
            entries: (0..2000)
                .map(|i| LegendEntry {
                    key: LegendKey {
                        markup_type: MarkupType::Cloud,
                        count_set: None,
                        color: format!("#{:06x}", i),
                        fill: Some("javascript".into()),
                    },
                    label: Some("l".repeat(9999)),
                    description: Some("\u{7}bell".into()),
                    hidden: false,
                    symbol: None,
                    set_name: None,
                })
                .collect(),
            ..LegendProps::default()
        };
        let s = hostile.sanitized();
        assert_eq!(s.title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(s.font_size_pt, DEFAULT_FONT_PT);
        assert_eq!(s.columns, LegendProps::default().columns);
        assert_eq!(s.scope, LegendScope::CurrentPage);
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        assert!(s.entries.iter().all(|e| e.key.fill.is_none()));
        assert!(s.entries[0].label.as_ref().unwrap().chars().count() <= MAX_TEXT_CHARS);
        assert_eq!(s.entries[0].description.as_deref(), Some("bell"));
        let big = LegendProps {
            font_size_pt: 1e9,
            ..LegendProps::default()
        }
        .sanitized();
        assert_eq!(big.font_size_pt, MAX_FONT_PT);
    }

    #[test]
    fn thousands_of_markups_and_entries_stay_fast() {
        let sets: Vec<CountSet> = (0..256).map(|i| set(&format!("S{i}"), "#ff0000")).collect();
        let ms: Vec<Markup> = (0..6000)
            .map(|i| count(i % 7, &sets[(i as usize) % 256], 1))
            .collect();
        let entries: Vec<LegendEntry> = sets.iter().map(|s| entry_for(&count(0, s, 1))).collect();
        let p = props_all(entries);
        assert_eq!(p.entries.len(), 256);
        let t = std::time::Instant::now();
        let rows = compute_rows(0, &p, &ms);
        assert!(t.elapsed().as_millis() < 2000, "took {:?}", t.elapsed());
        assert_eq!(rows.len(), 256);
        let total: u64 = rows.iter().map(|r| r.quantity as u64).sum();
        assert_eq!(total, 6000);
    }

    #[test]
    fn huge_count_values_cannot_overflow() {
        let s = set("A", "#ff0000");
        let ms = vec![count(0, &s, u32::MAX), count(0, &s, u32::MAX)];
        let p = props_all(vec![entry_for(&ms[0])]);
        assert_eq!(
            compute_rows(0, &p, &ms)[0].quantity_text,
            (u64::from(u32::MAX) * 2).to_string()
        );
    }

    #[test]
    fn pages_cell_compresses_and_clips() {
        let set: BTreeSet<u32> = [0, 1, 2, 4, 9].into_iter().collect();
        assert_eq!(pages_text(&set), "1-3, 5, 10");
        // With page labels the cell lists them instead (an unlabelled page by its number).
        let labels: Vec<String> = ["E-101", "E-102", "E-103", "", "E-105"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let some: BTreeSet<u32> = [0, 1, 3, 4].into_iter().collect();
        assert_eq!(
            pages_text_labelled(&some, &labels),
            "E-101, E-102, 4, E-105"
        );
        // No labelled page in the set: ranges, as before.
        let unlabelled: BTreeSet<u32> = [3].into_iter().collect();
        assert_eq!(pages_text_labelled(&unlabelled, &labels), "4");
        assert_eq!(pages_text_labelled(&set, &[]), "1-3, 5, 10");
        let many: BTreeSet<u32> = (0..200).map(|i| i * 2).collect();
        let t = pages_text(&many);
        assert!(t.chars().count() <= MAX_PAGES_CELL_CHARS);
        assert!(t.ends_with("..."));
    }

    #[test]
    fn layout_is_derived_from_rows_and_font() {
        let p = LegendProps::default().sanitized();
        let l = layout(&p, 300.0, 3);
        assert!((l.row_h - 16.2).abs() < 1e-9);
        assert!((l.height - (9.0 * 2.2 + 16.2 + 16.2 * 3.0)).abs() < 1e-9);
        let total: f64 = l.columns.iter().map(|c| c.w).sum();
        assert!((total - 300.0).abs() < 1e-9);
        assert_eq!(l.columns[0].column, LegendColumn::Symbol);
        assert!((l.columns[0].w - l.row_h).abs() < 1e-9);
        // Width clamps; zero rows still has a row of height.
        assert_eq!(layout(&p, 1.0, 0).width, MIN_WIDTH);
        assert!((layout(&p, 300.0, 0).height - (9.0 * 2.2 + 16.2 + 16.2)).abs() < 1e-9);
    }

    #[test]
    fn clip_cell_marks_the_cut() {
        assert_eq!(clip_cell("abc", 100.0, 9.0), "abc");
        let c = clip_cell(&"x".repeat(100), 100.0, 9.0);
        assert!(c.ends_with("..."));
        assert!(c.chars().count() <= 19);
        assert_eq!(clip_cell("abc", 5.0, 9.0), "");
    }

    #[test]
    fn effective_rect_keeps_top_left_and_follows_rows() {
        let mut m = mk(MarkupType::Legend, 0, "#000000");
        m.legend = Some(LegendProps::default());
        let r1 = effective_rect(&m, 1).unwrap();
        let r5 = effective_rect(&m, 5).unwrap();
        assert_eq!(r1[0], 10.0);
        assert_eq!(r1[3], 200.0);
        assert_eq!(r5[3], 200.0);
        assert!(r5[1] < r1[1]);
        assert!((r5[2] - 310.0).abs() < 1e-9);
    }

    #[test]
    fn keys_round_trip_and_forged_or_huge_defs_are_refused() {
        let s = set("A", "#ff0000");
        let c = count(0, &s, 1);
        let mut l = mk(MarkupType::Legend, 0, "#000000");
        l.legend = Some(props_all(vec![entry_for(&c)]));
        let mut d = Dictionary::new();
        write_keys(&mut d, &l);
        let back = read_props(&d).expect("round trip");
        assert_eq!(back, l.legend.as_ref().unwrap().sanitized());
        // Not written for another type.
        let mut other = Dictionary::new();
        let mut c2 = c.clone();
        c2.legend = l.legend.clone();
        write_keys(&mut other, &c2);
        assert!(!other.has(b"RLLegend"));
        // Oversized and malformed definitions are refused.
        let mut huge = Dictionary::new();
        huge.set(
            "RLLegend",
            Object::string_literal("x".repeat(MAX_DEF_BYTES + 1)),
        );
        assert!(read_props(&huge).is_none());
        let mut bad = Dictionary::new();
        bad.set("RLLegend", Object::string_literal("{not json"));
        assert!(read_props(&bad).is_none());
    }

    #[test]
    fn refresh_computes_rows_and_matching_geometry() {
        let s = set("A", "#ff0000");
        let c = count(0, &s, 3);
        let mut l = mk(MarkupType::Legend, 0, "#000000");
        l.legend = Some(props_all(vec![entry_for(&c)]));
        let ms = vec![c, l.clone()];
        let upd = refresh_for_save(&ms);
        let (props, u) = &upd[&l.id()];
        assert_eq!(u.rows.len(), 1);
        assert_eq!(u.rows[0].quantity_text, "3");
        let MarkupGeometry::Rect { min, max } = &u.geometry else {
            panic!("rect")
        };
        let lay = layout(props, 300.0, 1);
        assert!((max.y - min.y - lay.height).abs() < 1e-9);
        assert_eq!(max.y, 200.0);
    }

    // --- the shared fixture (also run by src/lib/legend.test.ts) -----------------------

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../../tests/fixtures/legend-agreement.json")).unwrap()
    }

    fn fixture_sets(f: &serde_json::Value) -> HashMap<String, CountSet> {
        f["sets"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    CountSet {
                        id: Uuid::parse_str(v["id"].as_str().unwrap()).unwrap(),
                        name: v["name"].as_str().unwrap().into(),
                        color: v["color"].as_str().unwrap().into(),
                        symbol: serde_json::from_value(v["symbol"].clone()).unwrap(),
                    },
                )
            })
            .collect()
    }

    fn fixture_markups(f: &serde_json::Value) -> Vec<Markup> {
        let sets = fixture_sets(f);
        f["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|it| {
                let t: MarkupType = serde_json::from_value(it["type"].clone()).unwrap();
                let mut m = mk(
                    t,
                    it["page"].as_u64().unwrap() as u32,
                    it["color"].as_str().unwrap(),
                );
                m.appearance.fill = it["fill"].as_str().map(String::from);
                m.subject = it["subject"].as_str().map(String::from);
                m.contents = it["contents"].as_str().map(String::from);
                m.optional_content = it["oc"]
                    .as_u64()
                    .map(|n| crate::markup::OptionalContent::Reference(n as u32, 0));
                m.count_set = it["set"].as_str().map(|k| sets[k].clone());
                if t == MarkupType::MeasurementCount || it["computed"].is_number() {
                    m.measurement = Some(Measurement {
                        scale_ref: None,
                        raw_measure: 0.0,
                        unit: it["unit"].as_str().unwrap_or("ea").into(),
                        computed_quantity: it["computed"].as_f64().unwrap_or(0.0),
                        depth: None,
                        count_value: it["count_value"].as_u64().map(|v| v as u32),
                        custom_columns: Default::default(),
                        rise_drop: Vec::new(),
                    });
                }
                m
            })
            .collect()
    }

    fn fixture_props(f: &serde_json::Value, mut v: serde_json::Value) -> LegendProps {
        let sets = fixture_sets(f);
        if let Some(es) = v.get_mut("entries").and_then(|e| e.as_array_mut()) {
            for e in es {
                if let Some(k) = e["key"]["count_set"].as_str().map(String::from) {
                    e["key"]["count_set"] = serde_json::json!(sets[&k].id.to_string());
                }
            }
        }
        serde_json::from_value::<LegendProps>(v)
            .unwrap()
            .sanitized()
    }

    #[test]
    fn fixture_rows_match_the_expected_rows() {
        let f = fixture();
        let ms = fixture_markups(&f);
        for case in f["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let p = fixture_props(&f, case["props"].clone());
            let rows = compute_rows(case["legend_page"].as_u64().unwrap() as u32, &p, &ms);
            let want = case["rows"].as_array().unwrap();
            assert_eq!(rows.len(), want.len(), "{name}: row count {rows:#?}");
            for (r, w) in rows.iter().zip(want) {
                assert_eq!(r.entry.map(|e| e as u64), w["entry"].as_u64(), "{name}");
                assert_eq!(r.subject, w["subject"].as_str().unwrap(), "{name}");
                assert_eq!(r.description, w["description"].as_str().unwrap(), "{name}");
                assert_eq!(
                    r.quantity_text,
                    w["quantity_text"].as_str().unwrap(),
                    "{name}"
                );
                assert_eq!(r.unit, w["unit"].as_str().unwrap(), "{name}");
                assert_eq!(r.pages, w["pages"].as_str().unwrap(), "{name}");
                assert_eq!(r.matched as u64, w["matched"].as_u64().unwrap(), "{name}");
                if let Some(c) = w["color"].as_str() {
                    assert_eq!(r.color, c, "{name}: colour");
                }
            }
        }
    }

    #[test]
    fn fixture_layouts_clips_and_rounding_match() {
        let f = fixture();
        for l in f["layouts"].as_array().unwrap() {
            let p = fixture_props(&f, l["props"].clone());
            let got = layout(
                &p,
                l["width"].as_f64().unwrap(),
                l["rows"].as_u64().unwrap() as usize,
            );
            assert!((got.width - l["width_out"].as_f64().unwrap()).abs() < 1e-9);
            assert!(
                (got.height - l["height"].as_f64().unwrap()).abs() < 1e-6,
                "{got:?}"
            );
            let want = l["columns"].as_array().unwrap();
            assert_eq!(got.columns.len(), want.len());
            for (c, w) in got.columns.iter().zip(want) {
                let col: LegendColumn = serde_json::from_value(w[0].clone()).unwrap();
                assert_eq!(c.column, col);
                assert!((c.x - w[1].as_f64().unwrap()).abs() < 1e-6, "{c:?} vs {w}");
                assert!((c.w - w[2].as_f64().unwrap()).abs() < 1e-6, "{c:?} vs {w}");
            }
        }
        for c in f["clips"].as_array().unwrap() {
            let got = clip_cell(
                c["text"].as_str().unwrap(),
                c["width"].as_f64().unwrap(),
                c["font"].as_f64().unwrap(),
            );
            assert_eq!(got, c["out"].as_str().unwrap(), "{c}");
        }
        for pair in f["fixed2"].as_array().unwrap() {
            assert_eq!(
                fixed2(pair[0].as_f64().unwrap()),
                pair[1].as_str().unwrap(),
                "{pair}"
            );
        }
    }
}
