//! Markup module — annotation model + PDF serialisation (spec §4, §6).
//!
//! M2 scope: annotation types, the common markup envelope (id/type/page/geometry/
//! appearance/audit), serialize → standard PDF annotations, Tool Chest / Tool Sets,
//! stamps (static + dynamic), .btx import.
//!
//! M1: stub only — types scaffolded, no logic.

/// v1 markup types (spec §12 decision a — locked).
#[derive(Debug, Clone, PartialEq, Eq)]
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
