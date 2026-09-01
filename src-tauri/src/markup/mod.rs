//! Markup module — annotation model + PDF serialisation (spec §4, §6).
//!
//! M2 scope: annotation types, the common markup envelope (id/type/page/geometry/
//! appearance/audit), serialize → standard PDF annotations, Tool Chest / Tool Sets,
//! stamps (static + dynamic), .btx import.
//!
//! This slice: the **common markup envelope** — the data model the spec requires to
//! exist "from day one" (stable immutable id, full audit/attribution, and the reserved
//! review-workflow fields), so the future field-tool app + async sync layer reuse it
//! rather than forcing a rework (spec §6, decisions a/f, §12). PDF (de)serialisation,
//! tools/tool-sets, stamps, and `.btx` import build on this and land later in M2.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::geometry::{PdfPoint, Quad};

mod annotation;
pub(crate) mod appearance;

/// v1 markup types (spec §12 decision a — locked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkupType {
    Text,
    Callout,
    Cloud,
    Rectangle,
    Ellipse,
    Polygon,
    Line,
    Polyline,
    Arrow,
    Highlight,
    Ink,
    Stamp,
    StampDynamic,
    // Measurement types (spec §7)
    MeasurementLength,
    MeasurementPerimeter,
    MeasurementArea,
    MeasurementVolume,
    MeasurementCount,
    MeasurementAngle,
    MeasurementRadius,
}

impl MarkupType {
    /// Whether this type carries a [`Measurement`] payload (spec §7).
    pub fn is_measurement(self) -> bool {
        matches!(
            self,
            MarkupType::MeasurementLength
                | MarkupType::MeasurementPerimeter
                | MarkupType::MeasurementArea
                | MarkupType::MeasurementVolume
                | MarkupType::MeasurementCount
                | MarkupType::MeasurementAngle
                | MarkupType::MeasurementRadius
        )
    }
}

/// Stable user identity (spec §6): a `user_id` UUID plus an editable display name —
/// never a bare name string, so renames don't orphan attribution and the shape stays
/// compatible with real accounts / SSO when the post-v1 sync layer lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    pub user_id: Uuid,
    pub display_name: String,
}

/// Sync provenance — where a markup originated (spec §6 `origin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Origin {
    #[default]
    Desktop,
    FieldApp,
}

/// Reserved review-workflow state (spec §6 decision f, §13). The status values are
/// the reviewer verdicts; `None` is the v1 default (no UI surfaces the others yet, but
/// the field is embedded so a field-tool "issue" is just a markup with workflow state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MarkupStatus {
    #[default]
    None,
    Accepted,
    Rejected,
    Completed,
}

/// Distinct count-marker shapes (takeoff Count sets). A small, fixed palette so a user
/// can tell apart count categories at a glance (e.g. Type-A vs Type-B fixtures). Rendered
/// in the set's colour by the frontend overlay (spec §7 count measurement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CountSymbol {
    #[default]
    Circle,
    Square,
    Triangle,
    Diamond,
    Cross,
    Star,
    Hexagon,
}

/// Where a foreign `/OC` (optional content) value came from, preserved verbatim rather
/// than interpreted (spec: BB-interop fix wave 2026-08-11, obs:je08u4y8rukjzbpm2y5f).
/// Two shapes cover everything seen in practice: a real, already-opened PDF's own object
/// graph carries `/OC` as an indirect reference to an OCG/OCMD dictionary (ISO 32000-1
/// §8.11.2); Bluebeam's `.btx` Tool Set exports instead carry a plain PDF string naming
/// the source layer (e.g. `/OC (emittiv markups)`) - a portable stand-in, since a Tool
/// Set item has no resolvable cross-reference table of its own to point a real reference
/// at. redline does not resolve or build the OCG tree itself; round-tripping the exact
/// value it read (rather than reinterpreting or dropping it) means opening-then-resaving
/// a layered foreign PDF doesn't silently strip an annotation from its layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionalContent {
    Reference(u32, u16),
    Text(String),
}

/// A Count "set" / category: a named bucket with its own colour + symbol so distinct item
/// types are counted and tallied separately (spec §7). Document-scoped for v1 (definitions
/// live in the markup store); each [`MarkupType::MeasurementCount`] markup references the set
/// it belongs to via [`Markup::count_set`], and the full set is embedded on the PDF annotation
/// (private `/RLCountSet*` keys + the standard `/C` colour) so it round-trips losslessly with
/// the document — no sidecar. Modelled cleanly so it can later be promoted to a reusable
/// `.btx`-style Tool Set (spec §6, follow-up).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountSet {
    /// Stable id (UUID), shared by every count markup in the set.
    pub id: Uuid,
    /// User-facing label (e.g. "Type-A fixture").
    pub name: String,
    /// Hex colour (`#rrggbb`) — also written to the annotation `/C` so external viewers
    /// render the marker in the set colour.
    pub color: String,
    /// The marker shape drawn at each count point.
    pub symbol: CountSymbol,
}

/// Stroke / fill line style (spec §6 appearance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// Font for text-bearing markups (spec §6 appearance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    pub family: String,
    pub size_pt: f64,
}

/// Visual appearance (spec §6): colour / weight / opacity / fill / line-style / font.
/// Colours are hex strings (`#rrggbb`).
///
/// Opacity model (three independent controls, corrected 2026-07-07 - see the
/// markup-controls-callout PR): `opacity` is STROKE/LINE alpha only; `fill_opacity` is
/// fill alpha, fully independent of `opacity`; text glyphs are never dimmed by either and
/// always render at full alpha. All three are `0.0..=1.0`. The PDF serialisation
/// ([`appearance::build_ap_stream`]) applies `opacity` and `fill_opacity` as separate
/// `/CA`/`/ca` ExtGState scopes around just the stroke/fill paint operators respectively,
/// and leaves text drawing unscoped - see that module's doc comment for why the annotation
/// dict's own top-level `/CA` can no longer carry this value (a blanket group alpha there
/// would double-dim/re-couple fill and text, which is exactly the bug this model fixes).
/// The frontend SVG overlay (`markup-render.ts`) mirrors this with native
/// `stroke-opacity`/`fill-opacity` attributes instead of a single group `opacity`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    pub color: String,
    pub line_weight: f64,
    /// STROKE/LINE opacity only (the "Opacity" UI control). Never applied to fill or text.
    pub opacity: f64,
    pub fill: Option<String>,
    pub line_style: LineStyle,
    pub font: Option<FontSpec>,
    /// Box-border colour for text-bearing markups (Text / Callout), distinct from the
    /// glyph `color`. `None` ⇒ the border falls back to `color` (and matches the standard
    /// FreeText `/C` semantics for foreign annotations). Persists via private `/RLOutlineColor`.
    /// `#[serde(default)]` keeps pre-outline JSON (no field) deserialising to `None`.
    #[serde(default)]
    pub outline_color: Option<String>,
    /// Fill alpha (`0.0..=1.0`), fully INDEPENDENT of `opacity` (the "Fill opacity" UI
    /// control) - setting one never moves the other. `None` => fully opaque fill (`1.0`),
    /// regardless of the stroke `opacity` value. Persists via the private `/RLFillOpacity`
    /// key. `#[serde(default)]` keeps pre-field JSON deserialising.
    #[serde(default)]
    pub fill_opacity: Option<f64>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            color: "#000000".to_string(),
            line_weight: 1.0,
            opacity: 1.0,
            fill: None,
            line_style: LineStyle::Solid,
            font: None,
            outline_color: None,
            fill_opacity: None,
        }
    }
}

/// Markup geometry in PDF user space at f64 (spec §5/§6) — never raster coordinates.
/// One variant per shape family; all coordinates are PDF points (origin bottom-left).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarkupGeometry {
    /// Single anchor (text note, count point, stamp origin).
    Point(PdfPoint),
    /// Axis-aligned rectangle / ellipse bounding box.
    Rect { min: PdfPoint, max: PdfPoint },
    /// Ordered vertices: line / polyline / arrow / polygon / cloud.
    Polyline(Vec<PdfPoint>),
    /// Freehand ink - one or more independent strokes.
    Ink(Vec<Vec<PdfPoint>>),
    /// One quadrilateral per visual text line (PDF `/QuadPoints`), used by
    /// text-anchored [`MarkupType::Highlight`] annotations built from a text
    /// selection (redline text-selection feature). Never merged across lines -
    /// each quad hugs exactly one line segment of the underlying text, so a
    /// multi-line selection renders as N separate translucent bands, matching
    /// how Acrobat/Bluebeam render real text-markup Highlights.
    Quads(Vec<Quad>),
}

/// Audit + attribution carried by every markup (spec §6). The annotation embeds
/// creator + last-modified; the sidecar (§15) keeps the full append-only history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Audit {
    pub created_by: UserRef,
    pub created_at: DateTime<Utc>,
    pub modified_by: UserRef,
    pub modified_at: DateTime<Utc>,
    /// Monotonic, bumped once per edit.
    pub revision: u64,
    pub origin: Origin,
}

/// A reply in a markup's comment thread (spec §6 reserved workflow — empty in v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: Uuid,
    pub author: UserRef,
    pub at: DateTime<Utc>,
    pub contents: String,
}

/// Reserved review-workflow fields (spec §6 decision f). Present from day one but
/// unused by the v1 UI; the field-tools app + async sync reuse this directly.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Workflow {
    pub status: MarkupStatus,
    pub assignee: Option<UserRef>,
    pub thread: Vec<Reply>,
}

/// Measurement payload for measurement markups (spec §6/§7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub scale_ref: Option<String>,
    pub raw_measure: f64,
    pub unit: String,
    pub computed_quantity: f64,
    /// Depth for volume measurements.
    pub depth: Option<f64>,
    /// For MeasurementCount: the integer count value (raw_measure = count_value as f64).
    #[serde(default)]
    pub count_value: Option<u32>,
    /// Estimating custom columns (spec §7).
    pub custom_columns: BTreeMap<String, String>,
}

/// The common markup envelope (spec §6). Every markup — annotation or measurement — is
/// one of these. `id` is the stable sync/merge anchor and maps to the PDF `/NM`
/// annotation name on save; it is assigned at creation and never changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Markup {
    /// Stable UUID, assigned at creation, immutable — no setter is provided.
    id: Uuid,
    pub markup_type: MarkupType,
    /// Zero-based page index.
    pub page: u32,
    pub geometry: MarkupGeometry,
    pub appearance: Appearance,
    /// Summary-grouping subject (→ `/Subj`).
    pub subject: Option<String>,
    /// Optional OCG / logical layer.
    pub layer: Option<String>,
    /// Note text (→ `/Contents`).
    pub contents: Option<String>,
    /// Flat group membership (G8). All markups sharing the same non-None `group_id`
    /// move together as one unit. Serialised to `/RLGroup` in the annotation dict.
    /// `#[serde(default)]` ensures pre-G8 JSON (no field) deserialises to `None`.
    #[serde(default)]
    pub group_id: Option<Uuid>,
    pub audit: Audit,
    pub workflow: Workflow,
    /// Present iff `markup_type.is_measurement()`.
    pub measurement: Option<Measurement>,
    /// The Count set this markup belongs to (only meaningful for
    /// [`MarkupType::MeasurementCount`]). The whole definition is embedded so the marker
    /// renders in its set colour + symbol and the assignment round-trips through the PDF
    /// annotation. `#[serde(default)]` keeps pre-count-set JSON deserialising to `None`.
    #[serde(default)]
    pub count_set: Option<CountSet>,
    /// Snapshot of the placing Tool's backing visual asset, for `MarkupType::Stamp` /
    /// `StampDynamic` markups only (spec "Stamps"). Set once at placement time
    /// (`Markup::with_stamp_asset`) so `appearance::build_ap_stream` can render the real
    /// stamp graphic instead of the bordered-box fallback. Deliberately NOT reconstructed
    /// by `from_annotation_dict` on reopen - the appearance is already baked into the
    /// saved `/AP /N` stream by then, so there is nothing to recover this field FROM (the
    /// asset itself is never persisted as a separate private key - that would duplicate
    /// the image bytes already sitting in the appearance stream). `#[serde(default)]`
    /// keeps pre-this-field JSON deserialising to `None`.
    #[serde(default)]
    pub stamp_asset: Option<crate::toolchest::StampAsset>,
    /// Standard `/F` annotation flags (ISO 32000-1 §12.5.3 table 165 - Print/Hidden/
    /// NoZoom/NoRotate/Locked/...), the raw bit field. Every real annotation in the BB
    /// corpus carries `/F` (almost always exactly `4`, the Print bit) and redline's
    /// write path dropped it entirely pre-fix (BB-interop fix wave 2026-08-11,
    /// obs:je08u4y8rukjzbpm2y5f) - plausibly interop-relevant since a strict viewer can
    /// legitimately honour Locked/NoZoom/Print per this value. Defaults to `4` (Print)
    /// for markups redline creates itself, matching the real corpus rather than the PDF
    /// spec's own bare default of `0` (not printed) - see [`default_annot_flags`].
    /// `#[serde(default = "default_annot_flags")]` keeps pre-this-field JSON
    /// deserialising to the same default rather than `0`.
    #[serde(default = "default_annot_flags")]
    pub annot_flags: i32,
    /// Standard `/RC` rich-text string (ISO 32000-1 §12.5.6.18, an XFA-flavoured XML
    /// body) - Bluebeam/Acrobat's richer representation of a text-bearing markup's
    /// content, alongside the plain `/Contents`. redline does not parse, edit, or
    /// render this - `contents`/`subject` remain the single source of truth for the
    /// plain-text note - it is preserved verbatim on round-trip only, so re-saving a
    /// Bluebeam-authored FreeText/Circle markup doesn't silently strip formatting a
    /// stricter viewer may still read back. `#[serde(default)]` keeps pre-this-field
    /// JSON deserialising to `None`.
    #[serde(default)]
    pub rich_text: Option<String>,
    /// Standard `/OC` optional-content value, preserved verbatim - see
    /// [`OptionalContent`]. `#[serde(default)]` keeps pre-this-field JSON
    /// deserialising to `None`.
    #[serde(default)]
    pub optional_content: Option<OptionalContent>,
    /// A foreign `/DA` default-appearance string that carried no parseable font/size
    /// operator (e.g. a colour-only Bluebeam `/DA` like `"0.5 0 1 rg"`, no `Tf` at all -
    /// a real corpus shape, not hypothetical). Only ever populated on read when
    /// `appearance.font` could NOT be recovered from it (see `font_from_da`); write-side
    /// fallback only - `to_annotation_dict` always derives `/DA` from `appearance.font`
    /// when one is present, and only falls back to re-emitting this raw string when it
    /// is not, so redline's own font model always wins the moment the markup actually
    /// has a font. `#[serde(default)]` keeps pre-this-field JSON deserialising to `None`.
    #[serde(default)]
    pub raw_da: Option<String>,
}

/// Default `/F` annotation flags for a markup redline creates itself: `4` (Print, bit 3
/// per ISO 32000-1 table 165) - matches what every real Bluebeam-authored annotation in
/// the BB corpus carries, so redline-authored markups print by default instead of
/// relying on the PDF spec's own bare default (`0`, not printed).
fn default_annot_flags() -> i32 {
    4
}

impl Markup {
    /// Create a new markup with a fresh stable id. `created_at` and `modified_at` are
    /// stamped from a single `now`, `revision` starts at 0, workflow is empty, and the
    /// measurement payload is absent (set it via [`Markup::with_measurement`]).
    pub fn new(
        markup_type: MarkupType,
        page: u32,
        geometry: MarkupGeometry,
        appearance: Appearance,
        created_by: UserRef,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            markup_type,
            page,
            geometry,
            appearance,
            subject: None,
            layer: None,
            contents: None,
            group_id: None,
            audit: Audit {
                created_by: created_by.clone(),
                created_at: now,
                modified_by: created_by,
                modified_at: now,
                revision: 0,
                origin: Origin::Desktop,
            },
            workflow: Workflow::default(),
            measurement: None,
            count_set: None,
            stamp_asset: None,
            annot_flags: default_annot_flags(),
            rich_text: None,
            optional_content: None,
            raw_da: None,
        }
    }

    /// The stable, immutable id (read-only — there is deliberately no setter).
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Attach a measurement payload (builder-style). Only meaningful for measurement
    /// markup types, but not enforced here — callers set type + payload together.
    pub fn with_measurement(mut self, m: Measurement) -> Self {
        self.measurement = Some(m);
        self
    }

    /// Attach a stamp's backing visual asset (builder-style, spec "Stamps"). Only
    /// meaningful for `MarkupType::Stamp`/`StampDynamic`, but not enforced here - callers
    /// set type + asset together at placement time.
    pub fn with_stamp_asset(mut self, asset: crate::toolchest::StampAsset) -> Self {
        self.stamp_asset = Some(asset);
        self
    }

    /// Record an edit: bump the monotonic `revision` and update `modified_by` /
    /// `modified_at`. The id and `created_*` fields are left untouched.
    pub fn touch(&mut self, modified_by: UserRef) {
        self.audit.revision += 1;
        self.audit.modified_by = modified_by;
        self.audit.modified_at = Utc::now();
    }

    /// ISO 32000-1 §12.5.3 Table 165 bit 8, `Locked` (decimal `128` / `0x80`): the
    /// annotation may not be deleted or have any of its properties modified by the
    /// user, other than `/Contents` (MCP server design, 2026-09-01, §4).
    pub fn is_locked(&self) -> bool {
        self.annot_flags & 0x80 != 0
    }

    /// ISO 32000-1 §12.5.3 Table 165 bit 10, `LockedContents` (decimal `512` /
    /// `0x200`): `/Contents` (and the appearance it drives) may not be modified, but
    /// other properties - including deletion - remain editable (MCP server design,
    /// 2026-09-01, §4). Independent of [`Markup::is_locked`] - either, both, or
    /// neither bit may be set.
    pub fn is_contents_locked(&self) -> bool {
        self.annot_flags & 0x200 != 0
    }
}

/// Structured refusal for a locked-markup mutation attempt (MCP server design §4 item
/// 4). Serializes to `{"error":"markup_locked","markup_id":"...","flag":"Locked"}` (or
/// `"LockedContents"`) - a calling agent gets something concrete to relay back to the
/// user, never a silent no-op and never a partial write.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkupLockError {
    pub error: &'static str,
    pub markup_id: String,
    pub flag: &'static str,
}

impl std::fmt::Display for MarkupLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap_or_else(|_| "markup_locked".to_string())
        )
    }
}

impl std::error::Error for MarkupLockError {}

/// Shared lock-respect guard (MCP server design §4, items 1-3) - the one choke point
/// both the GUI's mutation commands and the MCP bridge must call through, so the fix
/// closes the gap for both surfaces from a single change rather than an MCP-only check
/// a future GUI code path could still bypass. Wired into `MarkupStore::update`/
/// `MarkupStore::delete` (`document::store`), which both `commands::document::
/// update_markup`/`delete_markup` (GUI, via Tauri `invoke`) and the MCP bridge
/// (`rpc::tools`) call through - there is no other path to mutate a markup.
///
/// v1 treats `Locked` OR `LockedContents` as "refuse the whole mutation" rather than
/// honouring the ISO spec's finer per-property distinction (`LockedContents` still
/// permits deletion and non-content edits) - a deliberate simplification, not a
/// misreading: `update_markup` replaces the whole annotation state in one call with no
/// field-level patch API yet, so partial honouring risks a "the caller only meant to
/// move it but the whole call got through and also changed the locked note" gap. Refuse
/// more than the spec strictly requires, never less; revisit if a field-level patch
/// tool is ever proposed.
///
/// `create_markup` has no target to check - there is nothing to lock before it exists -
/// so this guard is deliberately only wired into update/delete, not creation.
pub fn check_not_locked(existing: &Markup) -> Result<(), MarkupLockError> {
    if existing.is_locked() {
        return Err(MarkupLockError {
            error: "markup_locked",
            markup_id: existing.id().to_string(),
            flag: "Locked",
        });
    }
    if existing.is_contents_locked() {
        return Err(MarkupLockError {
            error: "markup_locked",
            markup_id: existing.id().to_string(),
            flag: "LockedContents",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: &str) -> UserRef {
        UserRef {
            user_id: Uuid::new_v4(),
            display_name: name.to_string(),
        }
    }

    fn sample() -> Markup {
        Markup::new(
            MarkupType::Rectangle,
            3,
            MarkupGeometry::Rect {
                min: PdfPoint { x: 10.0, y: 20.0 },
                max: PdfPoint { x: 110.0, y: 70.0 },
            },
            Appearance::default(),
            user("Alice"),
        )
    }

    #[test]
    fn new_markup_has_stable_id_and_initial_audit() {
        let m = sample();
        assert_eq!(m.audit.revision, 0);
        // created == modified on a fresh markup (stamped from one `now`).
        assert_eq!(m.audit.created_at, m.audit.modified_at);
        assert_eq!(m.audit.created_by, m.audit.modified_by);
        assert_eq!(m.audit.origin, Origin::Desktop);
        assert!(m.measurement.is_none());
    }

    #[test]
    fn reserved_workflow_defaults_to_empty() {
        let m = sample();
        assert_eq!(m.workflow.status, MarkupStatus::None);
        assert!(m.workflow.assignee.is_none());
        assert!(m.workflow.thread.is_empty());
    }

    #[test]
    fn touch_bumps_revision_and_modified_but_not_id_or_creation() {
        let mut m = sample();
        let id_before = m.id();
        let created_by_before = m.audit.created_by.clone();
        let created_at_before = m.audit.created_at;

        m.touch(user("Bob"));

        assert_eq!(m.id(), id_before, "id must be immutable across edits");
        assert_eq!(m.audit.revision, 1);
        assert_eq!(m.audit.created_by, created_by_before, "creator unchanged");
        assert_eq!(
            m.audit.created_at, created_at_before,
            "creation time unchanged"
        );
        assert_eq!(m.audit.modified_by.display_name, "Bob");
        assert!(m.audit.modified_at >= created_at_before);
    }

    #[test]
    fn serde_round_trip_preserves_everything() {
        let mut m = sample();
        m.subject = Some("Door schedule".to_string());
        m.contents = Some("verify fire rating".to_string());
        m.workflow.status = MarkupStatus::Accepted;

        let json = serde_json::to_string(&m).expect("serialize");
        let back: Markup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    // --- G8: group_id field tests ---

    #[test]
    fn new_markup_has_no_group() {
        let m = sample();
        assert!(
            m.group_id.is_none(),
            "fresh markup must have group_id == None"
        );
    }

    #[test]
    fn serde_round_trip_preserves_group_id() {
        let mut m = sample();
        let gid = Uuid::new_v4();
        m.group_id = Some(gid);

        let json = serde_json::to_string(&m).expect("serialize");
        let back: Markup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.group_id,
            Some(gid),
            "group_id must survive JSON round-trip"
        );
    }

    #[test]
    fn serde_default_group_id_when_absent() {
        // Serialize a markup, remove the group_id key, then deserialize.
        // This confirms #[serde(default)] maps the absent key to None.
        let m = sample();
        let json = serde_json::to_string(&m).expect("serialize");
        // Strip the group_id key from the JSON object.
        let stripped = if json.contains("\"group_id\":null,") {
            json.replace("\"group_id\":null,", "")
        } else if json.contains(",\"group_id\":null") {
            json.replace(",\"group_id\":null", "")
        } else {
            json.replace("\"group_id\":null", "")
        };
        let back: Markup = serde_json::from_str(&stripped).expect("deserialize stripped");
        assert!(
            back.group_id.is_none(),
            "absent group_id field must deserialize to None"
        );
    }

    // --- end G8 tests ---

    // --- Count sets ---

    fn count_set() -> CountSet {
        CountSet {
            id: Uuid::new_v4(),
            name: "Type-A fixture".to_string(),
            color: "#0066ff".to_string(),
            symbol: CountSymbol::Triangle,
        }
    }

    #[test]
    fn new_markup_has_no_count_set() {
        assert!(
            sample().count_set.is_none(),
            "fresh markup must have count_set == None"
        );
    }

    #[test]
    fn count_markup_carries_set_and_round_trips() {
        let cs = count_set();
        let mut m = Markup::new(
            MarkupType::MeasurementCount,
            0,
            MarkupGeometry::Point(PdfPoint { x: 12.0, y: 34.0 }),
            Appearance {
                color: cs.color.clone(),
                ..Appearance::default()
            },
            user("Alice"),
        )
        .with_measurement(Measurement {
            scale_ref: None,
            raw_measure: 1.0,
            unit: "ea".to_string(),
            computed_quantity: 1.0,
            depth: None,
            count_value: Some(1),
            custom_columns: BTreeMap::new(),
        });
        m.count_set = Some(cs.clone());

        let json = serde_json::to_string(&m).expect("serialize");
        let back: Markup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.count_set, Some(cs));
        assert_eq!(m, back);
    }

    #[test]
    fn count_symbol_serializes_as_variant_name() {
        // The enum is a unit enum: serde emits the bare variant name (used by the /RL tag).
        assert_eq!(
            serde_json::to_string(&CountSymbol::Hexagon).unwrap(),
            "\"Hexagon\""
        );
        assert_eq!(CountSymbol::default(), CountSymbol::Circle);
    }

    #[test]
    fn serde_default_count_set_when_absent() {
        // Pre-count-set JSON (no count_set key) must deserialise to None.
        let m = sample();
        let json = serde_json::to_string(&m).expect("serialize");
        let stripped = if json.contains("\"count_set\":null,") {
            json.replace("\"count_set\":null,", "")
        } else if json.contains(",\"count_set\":null") {
            json.replace(",\"count_set\":null", "")
        } else {
            json.replace("\"count_set\":null", "")
        };
        let back: Markup = serde_json::from_str(&stripped).expect("deserialize stripped");
        assert!(
            back.count_set.is_none(),
            "absent count_set field must deserialize to None"
        );
    }

    // --- Quads geometry (text-anchored Highlight) ---

    #[test]
    fn quads_markup_serde_round_trips() {
        let quads = vec![
            [
                PdfPoint { x: 72.0, y: 712.0 },
                PdfPoint { x: 500.0, y: 712.0 },
                PdfPoint { x: 72.0, y: 700.0 },
                PdfPoint { x: 500.0, y: 700.0 },
            ],
            [
                PdfPoint { x: 72.0, y: 698.0 },
                PdfPoint { x: 220.0, y: 698.0 },
                PdfPoint { x: 72.0, y: 686.0 },
                PdfPoint { x: 220.0, y: 686.0 },
            ],
        ];
        let m = Markup::new(
            MarkupType::Highlight,
            2,
            MarkupGeometry::Quads(quads.clone()),
            Appearance::default(),
            user("Alice"),
        );
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Markup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
        match back.geometry {
            MarkupGeometry::Quads(q) => assert_eq!(q, quads, "quad count and points preserved"),
            other => panic!("expected Quads, got {other:?}"),
        }
    }

    #[test]
    fn measurement_markup_carries_payload() {
        let mut cols = BTreeMap::new();
        cols.insert("cost_code".to_string(), "03-30-00".to_string());
        let m = Markup::new(
            MarkupType::MeasurementArea,
            0,
            MarkupGeometry::Polyline(vec![
                PdfPoint { x: 0.0, y: 0.0 },
                PdfPoint { x: 100.0, y: 0.0 },
                PdfPoint { x: 100.0, y: 50.0 },
            ]),
            Appearance::default(),
            user("Alice"),
        )
        .with_measurement(Measurement {
            scale_ref: Some("1/8in=1ft".to_string()),
            raw_measure: 5000.0,
            unit: "sf".to_string(),
            computed_quantity: 5000.0,
            depth: None,
            count_value: None,
            custom_columns: cols,
        });

        assert!(m.markup_type.is_measurement());
        let meas = m.measurement.as_ref().expect("payload present");
        assert_eq!(meas.unit, "sf");
        assert_eq!(
            meas.custom_columns.get("cost_code").map(String::as_str),
            Some("03-30-00")
        );

        // Round-trips with the payload intact.
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Markup = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back);
    }

    // --- MCP server design §4: markup lock guard ---

    #[test]
    fn unlocked_control_case_is_not_locked_either_way() {
        // Default annot_flags is 4 (Print only) - the control case.
        let m = sample();
        assert!(!m.is_locked());
        assert!(!m.is_contents_locked());
        assert!(check_not_locked(&m).is_ok());
    }

    #[test]
    fn locked_bit_8_is_detected() {
        let mut m = sample();
        m.annot_flags = 0x80; // Locked only
        assert!(m.is_locked());
        assert!(!m.is_contents_locked());
    }

    #[test]
    fn locked_contents_bit_10_is_detected_independently() {
        let mut m = sample();
        m.annot_flags = 0x200; // LockedContents only
        assert!(!m.is_locked());
        assert!(m.is_contents_locked());
    }

    #[test]
    fn both_lock_bits_combine_with_other_flags_losslessly() {
        // Print (4) + Locked (0x80) + LockedContents (0x200), a realistic combination
        // since annot_flags round-trips the raw /F bit field alongside these two bits.
        let mut m = sample();
        m.annot_flags = 4 | 0x80 | 0x200;
        assert!(m.is_locked());
        assert!(m.is_contents_locked());
    }

    #[test]
    fn check_not_locked_refuses_locked_bluebeam_authored_markup() {
        // "Foreign (Bluebeam-authored)" is simulated by origin - the guard is a pure
        // bit test and must not special-case origin either way (see the next test).
        let mut m = sample();
        m.audit.origin = Origin::FieldApp; // stand-in for "not authored by this session"
        m.annot_flags = 0x80;
        let err = check_not_locked(&m).expect_err("locked markup must be refused");
        assert_eq!(err.error, "markup_locked");
        assert_eq!(err.flag, "Locked");
        assert_eq!(err.markup_id, m.id().to_string());
    }

    #[test]
    fn check_not_locked_refuses_locked_redline_authored_markup_too() {
        // The guard is origin-agnostic: a markup redline itself created and locked
        // must be refused exactly like a foreign one - proves there is no accidental
        // "only foreign annotations are locked" special-casing.
        let mut m = sample();
        assert_eq!(m.audit.origin, Origin::Desktop);
        m.annot_flags = 0x80;
        assert!(check_not_locked(&m).is_err());
    }

    #[test]
    fn check_not_locked_refuses_locked_contents_only() {
        let mut m = sample();
        m.annot_flags = 0x200;
        let err = check_not_locked(&m).expect_err("LockedContents-only must be refused");
        assert_eq!(err.flag, "LockedContents");
    }

    #[test]
    fn check_not_locked_allows_unlocked_control_case() {
        let m = sample(); // default annot_flags = 4 (Print), no lock bits
        assert!(check_not_locked(&m).is_ok());
    }

    #[test]
    fn markup_lock_error_serializes_to_the_documented_shape() {
        let mut m = sample();
        m.annot_flags = 0x80;
        let err = check_not_locked(&m).unwrap_err();
        let json: serde_json::Value = serde_json::from_str(&err.to_string()).unwrap();
        assert_eq!(json["error"], "markup_locked");
        assert_eq!(json["flag"], "Locked");
        assert_eq!(json["markup_id"], m.id().to_string());
    }
}
