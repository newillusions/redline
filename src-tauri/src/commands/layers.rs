//! Layer (optional content) commands. Behaviour spec: `docs/layers.md`.
//!
//! The layer VIEW (which groups are shown) is session state in the frontend; this module
//! turns it into a render ([`set_layer_view`]) and keeps the parsed layer model of each open
//! document. Only [`add_layer`] and [`save_layers_default`] write the file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;
use tauri::State;

use crate::commands::document::apply_page_edit;
use crate::layers::overlay::OverlayBase;
use crate::layers::{read_model, LayerModel};
use crate::AppState;

/// Files above this size are not read for layers until the user asks: a full parse costs
/// seconds on a plan set, and a second parse at open would compete with the mark-up load.
const AUTO_READ_MAX_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Clone)]
struct Session {
    stamp: (u64, Option<SystemTime>),
    base: Result<OverlayBase, String>,
}

/// A view the user asked for, and the render generation it was last applied at.
#[derive(Clone)]
struct Wanted {
    off: BTreeSet<u32>,
    applied_gen: u64,
}

/// Per-document layer state: what the view overlay needs (tied to the file it was read from),
/// and the view the user wants. The view lives in the renderer's open document, which a save,
/// page operation or restore replaces, so [`ensure_view`] re-applies it when the renderer's
/// generation has moved on.
#[derive(Default)]
pub struct LayerSessions {
    sessions: Mutex<HashMap<String, Session>>,
    wanted: Mutex<HashMap<String, Wanted>>,
    apply: tokio::sync::Mutex<()>,
}

impl LayerSessions {
    /// Forget everything about a document (it was closed).
    pub fn remove(&self, doc_id: &str) {
        self.sessions.lock().unwrap().remove(doc_id);
        self.wanted.lock().unwrap().remove(doc_id);
    }

    fn forget_session(&self, doc_id: &str) {
        self.sessions.lock().unwrap().remove(doc_id);
    }

    /// True when the user has a non-default view of this document.
    pub fn has_view(&self, doc_id: &str) -> bool {
        self.wanted.lock().unwrap().contains_key(doc_id)
    }
}

fn stamp_of(path: &std::path::Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// What a read of the layers returned.
#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub enum LayersRead {
    /// The file is large; nothing was read. Call again with `force`.
    Deferred {
        size: u64,
    },
    Loaded {
        model: LayerModel,
    },
}

fn load_session(
    path: &std::path::Path,
    password: Option<&str>,
) -> Result<(LayerModel, Session), String> {
    let stamp = stamp_of(path).ok_or_else(|| format!("cannot stat {}", path.display()))?;
    let doc = lopdf::Document::load_with_password(path, password.unwrap_or(""))
        .map_err(|e| format!("could not read layers (wrong password?): {e}"))?;
    let mut model = read_model(&doc);
    let base = OverlayBase::new(&doc, &model, path);
    model.view_supported = base.is_ok();
    model.view_unsupported_reason = base.as_ref().err().cloned();
    Ok((model, Session { stamp, base }))
}

/// Read the layer model of an open document.
#[tauri::command]
pub async fn read_layers(
    state: State<'_, AppState>,
    doc_id: String,
    force: bool,
) -> Result<LayersRead, String> {
    let path = state
        .markups
        .path(&doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size > AUTO_READ_MAX_BYTES && !force {
        return Ok(LayersRead::Deferred { size });
    }
    let password = state.markups.password(&doc_id);
    let (model, session) =
        tokio::task::spawn_blocking(move || load_session(&path, password.as_deref()))
            .await
            .map_err(|e| e.to_string())??;
    state
        .layers
        .sessions
        .lock()
        .unwrap()
        .insert(doc_id, session);
    Ok(LayersRead::Loaded { model })
}

/// The three pieces a view needs, so the apply/re-apply logic runs without a Tauri `State`.
struct Ctx<'a> {
    render: &'a crate::render::RenderHandle,
    markups: &'a crate::document::store::MarkupStore,
    layers: &'a LayerSessions,
}

impl<'a> Ctx<'a> {
    fn of(state: &'a AppState) -> Self {
        Ctx {
            render: &state.render,
            markups: &state.markups,
            layers: &state.layers,
        }
    }
}

/// Render `doc_id` with exactly `off` hidden. Caller holds `layers.apply`.
async fn apply_view(ctx: &Ctx<'_>, doc_id: &str, off: BTreeSet<u32>) -> Result<(), String> {
    let path = ctx
        .markups
        .path(doc_id)
        .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
    // Read before applying: an open that races this apply makes the next check re-apply.
    let gen = ctx.render.generation();
    // A save or page operation rewrote the file since the session was read: read it again.
    let current = stamp_of(&path);
    let cached = ctx
        .layers
        .sessions
        .lock()
        .unwrap()
        .get(doc_id)
        .filter(|s| Some(s.stamp) == current)
        .cloned();
    let session = match cached {
        Some(s) => s,
        None => {
            let password = ctx.markups.password(doc_id);
            let p = path.clone();
            let (_, s) = tokio::task::spawn_blocking(move || load_session(&p, password.as_deref()))
                .await
                .map_err(|e| e.to_string())??;
            ctx.layers
                .sessions
                .lock()
                .unwrap()
                .insert(doc_id.to_string(), s.clone());
            s
        }
    };
    let base = session
        .base
        .map_err(|why| format!("layer views are not available: {why}"))?;
    let off2 = off.clone();
    let reader = tokio::task::spawn_blocking(move || base.reader(&off2))
        .await
        .map_err(|e| e.to_string())??;
    ctx.render
        .set_layer_view(doc_id.to_string(), Some(reader))
        .await
        .map_err(|e| format!("{e:#}"))?;
    ctx.layers.wanted.lock().unwrap().insert(
        doc_id.to_string(),
        Wanted {
            off,
            applied_gen: gen,
        },
    );
    Ok(())
}

/// Re-apply the user's view if the renderer reopened the document since it was applied.
/// Called before every tile render; a no-op for a document with the file's own view.
pub async fn ensure_view(state: &AppState, doc_id: &str) {
    ensure_view_with(&Ctx::of(state), doc_id).await;
}

async fn ensure_view_with(ctx: &Ctx<'_>, doc_id: &str) {
    let stale = || {
        ctx.layers
            .wanted
            .lock()
            .unwrap()
            .get(doc_id)
            .filter(|w| w.applied_gen != ctx.render.generation())
            .cloned()
    };
    if stale().is_none() {
        return;
    }
    let _guard = ctx.layers.apply.lock().await;
    let Some(w) = stale() else { return };
    if let Err(e) = apply_view(ctx, doc_id, w.off.clone()).await {
        log::warn!("layers: could not re-apply the view of {doc_id}: {e}");
        // Do not retry on every tile: try again after the next open/close.
        if let Some(w) = ctx.layers.wanted.lock().unwrap().get_mut(doc_id) {
            w.applied_gen = ctx.render.generation();
        }
    }
}

/// Show the document with exactly the groups in `off` hidden, or as the file is when `off`
/// is `None`. The file is not written. Fails (leaving the current view) when the file cannot
/// be overlaid; the reason is in the error.
#[tauri::command]
pub async fn set_layer_view(
    state: State<'_, AppState>,
    doc_id: String,
    off: Option<Vec<u32>>,
) -> Result<(), String> {
    let ctx = Ctx::of(&state);
    let _guard = ctx.layers.apply.lock().await;
    let Some(off) = off else {
        if !ctx.layers.has_view(&doc_id) {
            return Ok(());
        }
        ctx.render
            .set_layer_view(doc_id.clone(), None)
            .await
            .map_err(|e| format!("{e:#}"))?;
        ctx.layers.wanted.lock().unwrap().remove(&doc_id);
        return Ok(());
    };
    apply_view(&ctx, &doc_id, off.into_iter().collect()).await
}

/// Add a blank layer (optionally under `parent`) to the file. Like a page operation, this
/// writes the file now (with the document's current mark-ups) and reloads the renderer; the
/// caller re-applies its view afterwards. Returns the new group's object number.
#[tauri::command]
pub async fn add_layer(
    state: State<'_, AppState>,
    doc_id: String,
    name: String,
    parent: Option<u32>,
) -> Result<u32, String> {
    let made = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let made2 = made.clone();
    apply_page_edit(&state, &doc_id, move |doc| {
        let id = crate::layers::edit::add_layer(doc, &name, parent)?;
        made2.store(id, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .await?;
    // The file was rewritten: the session is stale (re-read on the next apply). The user's
    // view is kept and re-applied by `ensure_view` on the next tile.
    state.layers.forget_session(&doc_id);
    Ok(made.load(std::sync::atomic::Ordering::SeqCst))
}

/// Save a view (`off` = the hidden groups) as the file's default configuration. Writes the
/// file now, like [`add_layer`].
#[tauri::command]
pub async fn save_layers_default(
    state: State<'_, AppState>,
    doc_id: String,
    off: Vec<u32>,
) -> Result<(), String> {
    let off: BTreeSet<u32> = off.into_iter().collect();
    apply_page_edit(&state, &doc_id, move |doc| {
        crate::layers::edit::set_default_state(doc, &off)
    })
    .await?;
    // The view IS the file's default now.
    state.layers.remove(&doc_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The view surviving a save: the renderer's document is replaced (generation moves on) and
    //! `ensure_view_with` puts the user's view back before the next tile. Needs PDFium
    //! (`PDFIUM_DYNAMIC_LIB_PATH`), like the other PDFium tests.
    use super::*;
    use crate::layers::tests::fixture;
    use crate::render::{RenderHandle, TileRequest};
    use lopdf::dictionary;

    /// (left square, right square) at the middle of each, drawn through the real handle.
    async fn shown(render: &RenderHandle) -> ([u8; 3], [u8; 3]) {
        let t = render
            .render_tile(TileRequest {
                doc_id: "d".into(),
                page_index: 0,
                tile_size_css: 512,
                tile_x: 0,
                tile_y: 0,
                zoom: 1.0,
                dpr: 1.0,
            })
            .await
            .unwrap();
        let png = crate::markup::appearance::base64_decode(&t.png_base64).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        (img.get_pixel(50, 50).0, img.get_pixel(150, 50).0)
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    const WHITE: [u8; 3] = [255, 255, 255];

    #[tokio::test(flavor = "multi_thread")]
    async fn the_view_survives_a_reopen_and_a_rewrite_of_the_file() {
        if std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
            eprintln!("skip: no PDFIUM_DYNAMIC_LIB_PATH");
            return;
        }
        let mut fx = fixture();
        let (red, blue) = (fx.red, fx.blue);
        fx.doc.catalog_mut().unwrap().set(
            "OCProperties",
            dictionary! {
                "OCGs" => vec![red.into(), blue.into()],
                "D" => dictionary! {},
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        fx.doc.save(&path).unwrap();

        let render = RenderHandle::spawn().unwrap();
        let markups = crate::document::store::MarkupStore::default();
        markups.register("d", path.clone(), None);
        let layers = LayerSessions::default();
        let ctx = Ctx {
            render: &render,
            markups: &markups,
            layers: &layers,
        };
        render
            .open_document(path.clone(), "d".into(), None)
            .await
            .unwrap();
        assert_eq!(shown(&render).await, (RED, BLUE));

        // The user hides "Blue".
        apply_view(&ctx, "d", [blue.0].into()).await.unwrap();
        assert_eq!(shown(&render).await, (RED, WHITE));
        assert!(layers.has_view("d"));

        // Nothing to do while the document has not been reopened.
        ensure_view_with(&ctx, "d").await;
        assert_eq!(shown(&render).await, (RED, WHITE));

        // A save closes and reopens the renderer's document: the view is gone ...
        render.close_document("d".into()).await.unwrap();
        render
            .open_document(path.clone(), "d".into(), None)
            .await
            .unwrap();
        assert_eq!(shown(&render).await, (RED, BLUE));
        // ... and comes back before the next tile.
        ensure_view_with(&ctx, "d").await;
        assert_eq!(shown(&render).await, (RED, WHITE));

        // The file is rewritten (a layer is added, as the add-layer command does), then reopened:
        // the stale parse is replaced and the view still holds.
        let mut doc = lopdf::Document::load(&path).unwrap();
        let new_id = crate::layers::edit::add_layer(&mut doc, "Added", None).unwrap();
        doc.save(&path).unwrap();
        render.close_document("d".into()).await.unwrap();
        render
            .open_document(path.clone(), "d".into(), None)
            .await
            .unwrap();
        assert_eq!(shown(&render).await, (RED, BLUE));
        ensure_view_with(&ctx, "d").await;
        assert_eq!(shown(&render).await, (RED, WHITE));
        let (model, _) = load_session(&path, None).unwrap();
        assert!(model.layers.iter().any(|l| l.id == new_id));

        // Back to the file's own view releases the overlay and forgets the wish.
        render.set_layer_view("d".into(), None).await.unwrap();
        layers.wanted.lock().unwrap().remove("d");
        assert_eq!(shown(&render).await, (RED, BLUE));
        render.close_document("d".into()).await.unwrap();
        render.open_document(path, "d".into(), None).await.unwrap();
        ensure_view_with(&ctx, "d").await;
        assert_eq!(shown(&render).await, (RED, BLUE), "no wish, no overlay");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_view_that_cannot_be_applied_is_reported_and_leaves_the_page_alone() {
        if std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
            eprintln!("skip: no PDFIUM_DYNAMIC_LIB_PATH");
            return;
        }
        let mut fx = fixture(); // no optional content at all
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.pdf");
        fx.doc.save(&path).unwrap();
        let render = RenderHandle::spawn().unwrap();
        let markups = crate::document::store::MarkupStore::default();
        markups.register("d", path.clone(), None);
        let layers = LayerSessions::default();
        let ctx = Ctx {
            render: &render,
            markups: &markups,
            layers: &layers,
        };
        render.open_document(path, "d".into(), None).await.unwrap();
        let err = apply_view(&ctx, "d", [5].into()).await.unwrap_err();
        assert!(err.contains("not available"), "{err}");
        assert!(!layers.has_view("d"));
        assert_eq!(shown(&render).await, (RED, BLUE));
    }
}
