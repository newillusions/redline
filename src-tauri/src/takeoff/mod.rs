//! Takeoff module — scale calibration, measurement, quantity calc (spec §4, §7).
//!
//! M3 scope: per-page scale records, two-point / preset calibration,
//! measurement types with raw_measure (scale-independent, PDF user space) + recompute
//! invariant on recalibration, quantity rollups, Markup List export (XLSX/CSV).
//!
//! Measurement viewports (UX benchmark must-fix 7): named page regions with their own
//! scale, stored as `ScaleTarget::Viewport` records and written to the PDF as standard
//! `/VP` viewport dictionaries (ISO 32000-1 §12.9) - see `scale` (selection rule) and
//! `measure` (PDF read/write). docs/measurement-viewports.md.
//!
//! M4 S1 additions: preset-scale picker helpers + PDF /Measure viewport dict write
//! (spec §12.7) live in `measure`. Additional geometry tools (perimeter/volume/
//! angle/radius/area-with-cutouts) live in `crate::geometry`.
//!
//! Key design note (spec §7): `raw_measure` is stored scale-independent (PDF points)
//! and references a `scale_id`. Recalibrating a scale deterministically recomputes all
//! dependent measurements — no stale quantities.

pub mod count;
pub mod math;
pub mod measure;
pub mod scale;

pub use math::{
    compute_area, compute_length, convert_length, measurement_anchor, recompute_all,
    recompute_markup, recompute_measurement, rise_drop_total,
};
pub use measure::{
    applicable_scales, find_scale, plan_viewport_import, read_viewports, sync_viewports,
    unsynced_viewport_ids, write_measure_dict, PdfViewport, ViewportKey,
};
pub use scale::{
    page_scale, scale_at, viewport_at, BBox, ScaleMethod, ScaleRecord, ScaleStore, ScaleTarget,
};
