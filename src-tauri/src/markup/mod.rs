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

use crate::geometry::PdfPoint;

mod annotation;

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
/// Colours are hex strings (`#rrggbb`); opacity is `0.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    pub color: String,
    pub line_weight: f64,
    pub opacity: f64,
    pub fill: Option<String>,
    pub line_style: LineStyle,
    pub font: Option<FontSpec>,
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
    /// Freehand ink — one or more independent strokes.
    Ink(Vec<Vec<PdfPoint>>),
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
