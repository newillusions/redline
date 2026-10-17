//! Sidecar companion folder beside each PDF (spec §15/§18).
//! M3 scope: meta.json (schema_version + scales).
//! M4 S2 scope: meta.json gains `versions` array; VersionRecord defined here.
pub mod meta;
pub use meta::{load_meta, save_meta, save_scales, sidecar_dir, stage_scales, ScalesUndo, SidecarMeta, VersionRecord};
