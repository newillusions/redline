//! Tauri IPC commands — the bridge between the Svelte webview and the Rust core.
//!
//! Pattern mirrors e-fees: one sub-module per domain, all commands registered in lib.rs
//! via `tauri::generate_handler![]`.

pub mod compare;
pub mod diag;
pub mod docops;
pub mod document;
pub mod recent_docs;
pub mod render;
pub mod search;
pub mod settings;
pub mod takeoff;
pub mod text;
pub mod text_select;
pub mod versioning;
