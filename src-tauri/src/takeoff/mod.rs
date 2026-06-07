//! Takeoff module — scale calibration, measurement, quantity calc (spec §4, §7).
//!
//! M3 scope: per-page scale records, two-point / page-declared / preset calibration,
//! measurement types with raw_measure (scale-independent, PDF user space) + recompute
//! invariant on recalibration, quantity rollups, Markup List export (XLSX/CSV).
//!
//! M1: stub only.
//!
//! Key design note (spec §7): `raw_measure` is stored scale-independent (PDF points)
//! and references a `scale_id`. Recalibrating a scale deterministically recomputes all
//! dependent measurements — no stale quantities.
