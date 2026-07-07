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

    /// Record an edit: bump the monotonic `revision` and update `modified_by` /
    /// `modified_at`. The id and `created_*` fields are left untouched.
    pub fn touch(&mut self, modified_by: UserRef) {
        self.audit.revision += 1;
        self.audit.modified_by = modified_by;
        self.audit.modified_at = Utc::now();
    }
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
}
