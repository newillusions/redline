//! Tool Chest - reusable markup templates ("Tools"), grouped into named Tool Sets
//! (spec section 102-126 "Tools & Tool Sets", "Stamps", "Importing Bluebeam Tool Sets").
//!
//! A Tool is a serialized markup template: markup type + saved Appearance, plus an
//! optional fixed geometry for Drawing-mode tools (symbols/stamps). The existing
//! Markup/Appearance model already carries everything a tool needs, so tool sets fall
//! out of it with little new machinery (per spec).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::markup::{Appearance, Markup, MarkupGeometry, MarkupType};

pub mod btx;
pub mod sequence;
pub mod stamp;
pub mod store;

pub use sequence::SequenceCounters;
pub use stamp::{CounterScope, DynamicField, StampAsset, StampDef};
pub use store::ToolChestStore;

/// How placing a tool affects newly-created geometry (spec "Tools & Tool Sets" - matches
/// Bluebeam's two placement modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlacementMode {
    /// Apply the tool's saved appearance to newly drawn geometry (the default).
    Properties,
    /// Drop an exact copy of the tool's fixed geometry (used for symbols/stamps).
    Drawing,
}

/// A reusable markup template (spec "Tools & Tool Sets").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub id: Uuid,
    pub name: String,
    pub markup_type: MarkupType,
    pub appearance: Appearance,
    pub subject: Option<String>,
    pub placement_mode: PlacementMode,
    /// Fixed geometry template for Drawing-mode tools (symbols/stamps): an exact snapshot
    /// of the source markup's PDF-space geometry at the moment "save as tool" ran. `None`
    /// for Properties-mode tools. At placement time the frontend computes an anchor from
    /// this template (its own bounding-box min corner, or its point for `Point` geometry)
    /// and translates every coordinate by `(click_point - anchor)`, so the shape keeps its
    /// original size/orientation and its anchor corner lands under the click - see
    /// `translateToolGeometry` in `src/lib/markup-tools.ts`.
    #[serde(default)]
    pub geometry: Option<MarkupGeometry>,
    /// Present iff this tool is a stamp (spec "Stamps").
    #[serde(default)]
    pub stamp: Option<StampDef>,
}

impl Tool {
    /// Build a Tool from a markup the user has selected ("save current markup as tool",
    /// spec "Tools & Tool Sets"). Serializes the markup's type + appearance [+ geometry,
    /// for Drawing mode] into a fresh, independent template - later edits to the source
    /// markup never affect the tool.
    pub fn from_markup(markup: &Markup, name: impl Into<String>, placement_mode: PlacementMode) -> Self {
        let geometry = match placement_mode {
            PlacementMode::Drawing => Some(markup.geometry.clone()),
            PlacementMode::Properties => None,
        };
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            markup_type: markup.markup_type,
            appearance: markup.appearance.clone(),
            subject: markup.subject.clone(),
            placement_mode,
            geometry,
            stamp: None,
        }
    }
}

/// A named, ordered collection of Tools (spec "Tools & Tool Sets"), serialized to a
/// versioned JSON file (own format, sync-friendly per spec section 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSet {
    pub id: Uuid,
    pub name: String,
    pub tools: Vec<Tool>,
}

impl ToolSet {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), name: name.into(), tools: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PdfPoint;
    use crate::markup::{MarkupGeometry as Geom, UserRef};
    use uuid::Uuid as U;

    fn user() -> UserRef {
        UserRef { user_id: U::new_v4(), display_name: "Alice".to_string() }
    }

    fn rect_markup() -> Markup {
        Markup::new(
            MarkupType::Rectangle,
            2,
            Geom::Rect { min: PdfPoint { x: 10.0, y: 20.0 }, max: PdfPoint { x: 110.0, y: 70.0 } },
            Appearance { color: "#ff0000".to_string(), line_weight: 3.0, ..Appearance::default() },
            user(),
        )
    }

    // --- (a) Tool round-trips: serialize a markup -> Tool -> JSON -> back ---

    #[test]
    fn properties_mode_tool_carries_type_and_appearance_but_no_geometry() {
        let m = rect_markup();
        let tool = Tool::from_markup(&m, "My Rectangle", PlacementMode::Properties);

        assert_eq!(tool.markup_type, MarkupType::Rectangle);
        assert_eq!(tool.appearance, m.appearance);
        assert!(tool.geometry.is_none(), "properties-mode tools carry no fixed geometry");
        assert_eq!(tool.name, "My Rectangle");
    }

    #[test]
    fn drawing_mode_tool_carries_fixed_geometry() {
        let m = rect_markup();
        let tool = Tool::from_markup(&m, "Stamp Symbol", PlacementMode::Drawing);

        assert_eq!(tool.geometry, Some(m.geometry.clone()));
    }

    #[test]
    fn tool_json_round_trips() {
        let m = rect_markup();
        let tool = Tool::from_markup(&m, "Round Trip Tool", PlacementMode::Drawing);

        let json = serde_json::to_string(&tool).expect("serialize");
        let back: Tool = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tool, back);
    }

    #[test]
    fn tool_from_markup_gets_a_fresh_independent_id() {
        let m = rect_markup();
        let t1 = Tool::from_markup(&m, "A", PlacementMode::Properties);
        let t2 = Tool::from_markup(&m, "B", PlacementMode::Properties);
        assert_ne!(t1.id, t2.id, "each derived tool gets its own stable id");
    }

    // --- ToolSet basics ---

    #[test]
    fn new_tool_set_is_empty_with_a_stable_id() {
        let set = ToolSet::new("My Set");
        assert_eq!(set.name, "My Set");
        assert!(set.tools.is_empty());
    }

    #[test]
    fn tool_set_json_round_trips_with_ordered_tools() {
        let m = rect_markup();
        let mut set = ToolSet::new("Ordered Set");
        set.tools.push(Tool::from_markup(&m, "First", PlacementMode::Properties));
        set.tools.push(Tool::from_markup(&m, "Second", PlacementMode::Properties));

        let json = serde_json::to_string(&set).expect("serialize");
        let back: ToolSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.tools.len(), 2);
        assert_eq!(back.tools[0].name, "First");
        assert_eq!(back.tools[1].name, "Second");
        assert_eq!(set, back);
    }

    #[test]
    fn pre_stamp_json_without_geometry_or_stamp_keys_still_deserializes() {
        // Guards forward-compat: a Tool JSON blob missing the newer `geometry`/`stamp`
        // keys (e.g. hand-authored fixtures, or a hypothetical pre-M2 file) must still
        // deserialize, defaulting both to None.
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "name": "Legacy Tool",
            "markup_type": "Rectangle",
            "appearance": Appearance::default(),
            "subject": null,
            "placement_mode": "Properties",
        })
        .to_string();
        let tool: Tool = serde_json::from_str(&json).expect("deserialize legacy tool json");
        assert!(tool.geometry.is_none());
        assert!(tool.stamp.is_none());
    }
}
