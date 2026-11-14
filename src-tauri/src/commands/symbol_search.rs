//! Tauri command for Symbol Search: find every copy of a boxed luminaire symbol.
//!
//! A thin passthrough, like `commands/geometry.rs`: extraction runs on the render thread and
//! matching on blocking threads inside `crate::symbol_search::search`, so this command never
//! holds the UI or the render thread while matching.

use tauri::State;

use crate::symbol_search::{self, SymbolSearchRequest, SymbolSearchResponse};
use crate::AppState;

#[tauri::command]
pub async fn symbol_search(
    state: State<'_, AppState>,
    request: SymbolSearchRequest,
) -> Result<SymbolSearchResponse, String> {
    symbol_search::search(&state.render, request)
        .await
        .map_err(|e| format!("{e:#}"))
}
