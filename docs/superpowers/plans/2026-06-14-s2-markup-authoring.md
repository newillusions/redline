# S2: Markup Authoring + Undo/Redo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Author, edit, and delete the full v1 markup type set in the GUI, with command-pattern undo/redo, persisting through the S1 save pipeline.

**Architecture:** A reactive Svelte store (`markup-store.svelte.ts`) is the in-session source of truth and owns a command-pattern history; the Rust `MarkupStore` is a mirror + save buffer kept in lock-step by per-op IPC (`add`/`update`/`delete`) drained through an ordered async queue and force-flushed before save. Interaction state machines in `markup-tools.ts` convert pointer events to PDF-space geometry via the existing `screenToPdfUserSpace`.

**Tech Stack:** Tauri 2 (Rust), Svelte 5 runes, TypeScript, vitest, Rust `#[test]`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-14-s2-markup-authoring-design.md`. **Decision:** `decision:vic6slsasg6njkf7haka`.

---

## File structure

| File | Responsibility | Status |
|---|---|---|
| `src-tauri/src/document/store.rs` | In-memory markup store; add `update`/`delete` | modify |
| `src-tauri/src/identity.rs` | Persisted `user_id`+display-name (first-run generate) | create |
| `src-tauri/src/commands/document.rs` | `update_markup`/`delete_markup`/`get_user_identity` commands | modify |
| `src-tauri/src/lib.rs` | Register `mod identity;` + 3 new commands in the handler | modify |
| `src/lib/ipc.ts` | `updateMarkup`/`deleteMarkup`/`getUserIdentity` wrappers | modify |
| `src/lib/markup-commands.ts` | Command pattern + `History` (pure, undoable) | create |
| `src/lib/markup-store.svelte.ts` | Reactive in-session SoT + ordered async mirror queue | create |
| `src/lib/markup-tools.ts` | Per-tool interaction → geometry builders | create (G3+) |
| `src/components/PropertiesPanel.svelte` | Appearance/contents/subject/layer editor | create (G7) |
| `src/lib/markup-render.ts` | Add `text` shape kind + cloud scallop helper | modify (G4/G5) |
| `src/components/Viewport.svelte` | Interactive overlay: capture, selection chrome, text editor | modify (G3+) |
| `src/App.svelte` | Own the store instance; flush-before-save wiring | modify (G2+) |

---

# GROUP 1 — Backend rails

Store mutation + commands + ipc wrappers + minimal identity. Pure Rust/TS; no UI. Each task is TDD.

### Task 1.1: `MarkupStore::update` and `::delete`

**Files:**
- Modify: `src-tauri/src/document/store.rs`

- [ ] **Step 1: Add the `uuid::Uuid` import** at the top of `store.rs` (currently only imported in the test module). After the existing `use` lines (around line 8 `use crate::markup::Markup;`):

```rust
use uuid::Uuid;
```

- [ ] **Step 2: Write the failing tests** — add to the `mod tests` block in `store.rs` (after `end_save_unknown_doc_is_noop`):

```rust
    #[test]
    fn update_replaces_markup_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        let id = m.id();
        s.add("d1", m.clone()).unwrap();

        let mut edited = m;
        edited.contents = Some("edited".into());
        s.update("d1", edited).unwrap();

        let got = s.list("d1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id(), id, "id preserved");
        assert_eq!(got[0].contents.as_deref(), Some("edited"));
    }

    #[test]
    fn update_unknown_id_errors() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        // markup() not added -> its id is absent
        assert!(s.update("d1", markup()).is_err());
        // unknown doc also errors
        assert!(s.update("nope", markup()).is_err());
    }

    #[test]
    fn delete_removes_by_id() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        let m = markup();
        let id = m.id();
        s.add("d1", m).unwrap();
        s.delete("d1", id).unwrap();
        assert_eq!(s.list("d1").unwrap().len(), 0);
    }

    #[test]
    fn delete_unknown_id_or_doc_errors() {
        let s = MarkupStore::default();
        s.register("d1", PathBuf::from("/tmp/a.pdf"));
        assert!(s.delete("d1", uuid::Uuid::new_v4()).is_err());
        assert!(s.delete("nope", uuid::Uuid::new_v4()).is_err());
    }
```

- [ ] **Step 3: Run, verify fail**

Run: `cd src-tauri && cargo test --lib store:: 2>&1 | tail -15`
Expected: FAIL — `no method named update`/`delete`.

- [ ] **Step 4: Implement** — add to `impl MarkupStore` (after the `add` method, before `seed_loaded`):

```rust
    /// Replace a markup by id. Errors on unknown doc or absent id.
    pub fn update(&self, doc_id: &str, m: Markup) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let slot = e
            .markups
            .iter_mut()
            .find(|x| x.id() == m.id())
            .ok_or_else(|| format!("unknown markup id {}", m.id()))?;
        *slot = m;
        Ok(())
    }

    /// Remove a markup by id. Errors on unknown doc or absent id.
    pub fn delete(&self, doc_id: &str, id: Uuid) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        let e = g
            .get_mut(doc_id)
            .ok_or_else(|| format!("unknown doc_id {doc_id}"))?;
        let before = e.markups.len();
        e.markups.retain(|x| x.id() != id);
        if e.markups.len() == before {
            return Err(format!("unknown markup id {id}"));
        }
        Ok(())
    }
```

- [ ] **Step 5: Run, verify pass**

Run: `cd src-tauri && cargo test --lib store:: 2>&1 | tail -15`
Expected: PASS (all store tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/document/store.rs
git commit -m "feat(markup): MarkupStore update + delete by id

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 1.2: minimal user identity service

**Files:**
- Create: `src-tauri/src/identity.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod identity;`)

- [ ] **Step 1: Write the failing test + module** — create `src-tauri/src/identity.rs`:

```rust
//! Minimal app-configured user identity (spec §6 / §12 g): a stable `user_id` (UUID)
//! plus an editable display name, generated on first run and persisted atomically.
//! S4 promotes this to the full user_id <-> display-name registry; the shape here is
//! forward-compatible (matches markup::UserRef).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: Uuid,
    pub display_name: String,
}

fn default_display_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "User".to_string())
}

/// Load `<dir>/identity.json`, generating + persisting one on first run. A corrupt or
/// unreadable file is replaced with a fresh identity (never hard-fails the app).
pub fn load_or_create(dir: &Path) -> Result<Identity, String> {
    let path = dir.join("identity.json");
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(id) = serde_json::from_slice::<Identity>(&bytes) {
            return Ok(id);
        }
    }
    let id = Identity {
        user_id: Uuid::new_v4(),
        display_name: default_display_name(),
    };
    fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(&id).map_err(|e| e.to_string())?;
    fs::write(&tmp, json).map_err(|e| format!("write identity: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("rename identity: {e}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_then_reuses_identity() {
        let dir = std::env::temp_dir().join(format!("redline-id-{}", Uuid::new_v4()));
        let first = load_or_create(&dir).expect("first run generates");
        let second = load_or_create(&dir).expect("second run reuses");
        assert_eq!(first, second, "identity is stable across runs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_replaced() {
        let dir = std::env::temp_dir().join(format!("redline-id-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("identity.json"), b"not json").unwrap();
        let id = load_or_create(&dir).expect("replaces corrupt file");
        assert!(!id.display_name.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Register the module** — in `src-tauri/src/lib.rs`, add alongside the other top-level `mod` declarations (search for `mod commands;` / `mod document;` near the top of the file):

```rust
mod identity;
```

- [ ] **Step 3: Run, verify pass**

Run: `cd src-tauri && cargo test --lib identity:: 2>&1 | tail -15`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/identity.rs src-tauri/src/lib.rs
git commit -m "feat(identity): persisted first-run user identity (user_id + display name)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 1.3: `update_markup` / `delete_markup` / `get_user_identity` commands

**Files:**
- Modify: `src-tauri/src/commands/document.rs`
- Modify: `src-tauri/src/lib.rs` (handler registration)

- [ ] **Step 1: Add the commands** — in `src-tauri/src/commands/document.rs`, after `add_markup` (line ~63). Also add `use tauri::Manager;` to the imports at the top (needed for `app.path()`):

```rust
/// Replace an existing markup (move/resize/edit). Errors if the id is absent.
#[tauri::command]
pub async fn update_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup: Markup,
) -> Result<(), String> {
    state.markups.update(&doc_id, markup)
}

/// Delete a markup by id (string UUID from the frontend).
#[tauri::command]
pub async fn delete_markup(
    state: State<'_, AppState>,
    doc_id: String,
    markup_id: String,
) -> Result<(), String> {
    let id = uuid::Uuid::parse_str(&markup_id).map_err(|e| format!("bad markup id: {e}"))?;
    state.markups.delete(&doc_id, id)
}

/// Return the persisted app user identity, generating it on first run.
#[tauri::command]
pub fn get_user_identity(app: tauri::AppHandle) -> Result<crate::identity::Identity, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("config dir: {e}"))?;
    crate::identity::load_or_create(&dir)
}
```

- [ ] **Step 2: Register the three commands** — in `src-tauri/src/lib.rs`, inside `tauri::generate_handler![ ... ]`, after `commands::document::save_document_as,`:

```rust
            commands::document::update_markup,
            commands::document::delete_markup,
            commands::document::get_user_identity,
```

- [ ] **Step 3: Verify it compiles**

Run: `cd src-tauri && cargo build 2>&1 | tail -8`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/document.rs src-tauri/src/lib.rs
git commit -m "feat(commands): update_markup, delete_markup, get_user_identity

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 1.4: ipc.ts wrappers

**Files:**
- Modify: `src/lib/ipc.ts`

- [ ] **Step 1: Add the wrappers** — in `src/lib/ipc.ts`, after `addMarkup` (line ~176). `UserRef` is already exported in this file:

```typescript
export async function updateMarkup(doc_id: string, markup: Markup): Promise<void> {
  return invoke<void>("update_markup", { doc_id, markup });
}

export async function deleteMarkup(doc_id: string, markup_id: string): Promise<void> {
  return invoke<void>("delete_markup", { doc_id, markup_id });
}

/** Persisted app user identity (generated on first run). */
export async function getUserIdentity(): Promise<UserRef> {
  return invoke<UserRef>("get_user_identity");
}
```

- [ ] **Step 2: Verify check passes**

Run: `npm run check 2>&1 | tail -3`
Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add src/lib/ipc.ts
git commit -m "feat(ipc): updateMarkup, deleteMarkup, getUserIdentity wrappers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**G1 gate:** `cd src-tauri && cargo test --lib 2>&1 | tail -5` all pass; `cargo clippy --all-targets 2>&1 | tail -3` 0 warnings; `npm run check` 0 errors.

---

# GROUP 2 — Undo/sync core

The command stack (pure, vitest) and the reactive store with the ordered async mirror queue.

### Task 2.1: command pattern + history (`markup-commands.ts`)

**Files:**
- Create: `src/lib/markup-commands.ts`
- Create: `src/lib/markup-commands.test.ts`

**Design:** A command knows how to `apply` and `invert` against a *mutable markup list abstraction* (`MarkupSink`) and declares the backend op it implies. The store (Task 2.2) provides the `MarkupSink` and consumes the ops. `History` holds undo/redo stacks; `push` applies + records; `undo`/`redo` move between stacks and return the op(s) to mirror.

- [ ] **Step 1: Write the failing tests** — `src/lib/markup-commands.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { History, CreateCmd, UpdateCmd, DeleteCmd, type MarkupSink, type MirrorOp } from "./markup-commands";
import type { Markup } from "./ipc";

// Minimal in-memory sink standing in for the reactive store.
class ArraySink implements MarkupSink {
  list: Markup[] = [];
  insert(m: Markup) { this.list.push(m); }
  replace(m: Markup) { const i = this.list.findIndex((x) => x.id === m.id); if (i >= 0) this.list[i] = m; }
  removeById(id: string) { this.list = this.list.filter((x) => x.id !== id); }
  getById(id: string) { return this.list.find((x) => x.id === id); }
}

function mk(id: string, contents: string | null = null): Markup {
  return {
    id, markup_type: "Rectangle", page: 0,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
    appearance: { color: "#f00", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents,
    audit: { created_by: { user_id: "u", display_name: "U" }, created_at: "", modified_by: { user_id: "u", display_name: "U" }, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] }, measurement: null,
  };
}

describe("History undo/redo", () => {
  it("CreateCmd applies, undo removes, redo re-adds — and emits add/delete ops", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    const m = mk("a");

    const ops: MirrorOp[] = [];
    ops.push(h.push(new CreateCmd(m)));
    expect(sink.list.length).toBe(1);
    expect(ops[0]).toEqual({ kind: "add", markup: m });

    const undoOp = h.undo()!;
    expect(sink.list.length).toBe(0);
    expect(undoOp).toEqual({ kind: "delete", id: "a" });

    const redoOp = h.redo()!;
    expect(sink.list.length).toBe(1);
    expect(redoOp).toEqual({ kind: "add", markup: m });
  });

  it("UpdateCmd swaps before<->after on undo/redo with update ops", () => {
    const sink = new ArraySink();
    const before = mk("a", "old");
    sink.insert(before);
    const h = new History(sink);
    const after = mk("a", "new");

    const op = h.push(new UpdateCmd(before, after));
    expect(sink.getById("a")!.contents).toBe("new");
    expect(op).toEqual({ kind: "update", markup: after });

    const undoOp = h.undo()!;
    expect(sink.getById("a")!.contents).toBe("old");
    expect(undoOp).toEqual({ kind: "update", markup: before });
  });

  it("DeleteCmd removes, undo restores — delete/add ops", () => {
    const sink = new ArraySink();
    const m = mk("a");
    sink.insert(m);
    const h = new History(sink);

    const op = h.push(new DeleteCmd(m));
    expect(sink.list.length).toBe(0);
    expect(op).toEqual({ kind: "delete", id: "a" });

    const undoOp = h.undo()!;
    expect(sink.list.length).toBe(1);
    expect(undoOp).toEqual({ kind: "add", markup: m });
  });

  it("a fresh push clears the redo stack", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    h.push(new CreateCmd(mk("a")));
    h.undo();
    expect(h.canRedo).toBe(true);
    h.push(new CreateCmd(mk("b")));
    expect(h.canRedo).toBe(false);
  });

  it("undo/redo at the ends are no-ops returning null", () => {
    const sink = new ArraySink();
    const h = new History(sink);
    expect(h.undo()).toBeNull();
    expect(h.redo()).toBeNull();
  });
});
```

- [ ] **Step 2: Run, verify fail**

Run: `npm run test 2>&1 | tail -15`
Expected: FAIL — `./markup-commands` not found.

- [ ] **Step 3: Implement** — `src/lib/markup-commands.ts`:

```typescript
/**
 * Command-pattern undo/redo for markup edits (spec §15 — in-session editing, distinct
 * from the durable audit trail). Pure: operates on a MarkupSink abstraction and returns
 * the backend MirrorOp each command implies, so the store can mirror it asynchronously.
 */
import type { Markup } from "./ipc";

/** The mutable markup collection a command acts on (the reactive store implements this). */
export interface MarkupSink {
  insert(m: Markup): void;
  replace(m: Markup): void;
  removeById(id: string): void;
  getById(id: string): Markup | undefined;
}

/** A backend mirror operation implied by a command (1:1 with the granular IPC ops). */
export type MirrorOp =
  | { kind: "add"; markup: Markup }
  | { kind: "update"; markup: Markup }
  | { kind: "delete"; id: string };

export interface Command {
  apply(sink: MarkupSink): MirrorOp;
  invert(sink: MarkupSink): MirrorOp;
}

export class CreateCmd implements Command {
  constructor(private readonly markup: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.insert(this.markup); return { kind: "add", markup: this.markup }; }
  invert(sink: MarkupSink): MirrorOp { sink.removeById(this.markup.id); return { kind: "delete", id: this.markup.id }; }
}

export class UpdateCmd implements Command {
  constructor(private readonly before: Markup, private readonly after: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.replace(this.after); return { kind: "update", markup: this.after }; }
  invert(sink: MarkupSink): MirrorOp { sink.replace(this.before); return { kind: "update", markup: this.before }; }
}

export class DeleteCmd implements Command {
  constructor(private readonly markup: Markup) {}
  apply(sink: MarkupSink): MirrorOp { sink.removeById(this.markup.id); return { kind: "delete", id: this.markup.id }; }
  invert(sink: MarkupSink): MirrorOp { sink.insert(this.markup); return { kind: "add", markup: this.markup }; }
}

export class History {
  private undoStack: Command[] = [];
  private redoStack: Command[] = [];
  constructor(private readonly sink: MarkupSink) {}

  get canUndo(): boolean { return this.undoStack.length > 0; }
  get canRedo(): boolean { return this.redoStack.length > 0; }

  /** Apply a command, record it for undo, clear the redo stack. Returns the mirror op. */
  push(cmd: Command): MirrorOp {
    const op = cmd.apply(this.sink);
    this.undoStack.push(cmd);
    this.redoStack = [];
    return op;
  }

  undo(): MirrorOp | null {
    const cmd = this.undoStack.pop();
    if (!cmd) return null;
    const op = cmd.invert(this.sink);
    this.redoStack.push(cmd);
    return op;
  }

  redo(): MirrorOp | null {
    const cmd = this.redoStack.pop();
    if (!cmd) return null;
    const op = cmd.apply(this.sink);
    this.undoStack.push(cmd);
    return op;
  }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `npm run test 2>&1 | tail -10`
Expected: PASS (markup-commands + existing markup-render suites).

- [ ] **Step 5: Commit**

```bash
git add src/lib/markup-commands.ts src/lib/markup-commands.test.ts
git commit -m "feat(markup): command-pattern undo/redo history (pure, vitest)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 2.2: reactive store + ordered async mirror queue (`markup-store.svelte.ts`)

**Files:**
- Create: `src/lib/markup-store.svelte.ts`
- Create: `src/lib/markup-store.test.ts`

**Design:** `MarkupStore` wraps a `$state` markups array (also implementing `MarkupSink` for the History), `selectedIds`, `activeTool`, `draftAppearance`. Mutators (`create`/`update`/`delete`/`undo`/`redo`) push through History and enqueue the resulting `MirrorOp` on a FIFO drained by a single async loop that calls the injected IPC functions in order. `flush()` awaits a full drain (called before save). A failing op sets `mirrorError` and halts the drain (retried on next enqueue / flush). IPC is injected (constructor deps) so the store is testable without Tauri.

- [ ] **Step 1: Write the failing tests** — `src/lib/markup-store.test.ts`:

```typescript
import { describe, it, expect, vi } from "vitest";
import { MarkupStore } from "./markup-store.svelte";
import type { Markup } from "./ipc";

function mk(id: string): Markup {
  return {
    id, markup_type: "Rectangle", page: 0,
    geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } },
    appearance: { color: "#f00", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null },
    subject: null, layer: null, contents: null,
    audit: { created_by: { user_id: "u", display_name: "U" }, created_at: "", modified_by: { user_id: "u", display_name: "U" }, modified_at: "", revision: 0, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [] }, measurement: null,
  };
}

function fakeIpc() {
  return {
    add: vi.fn(async (_d: string, _m: Markup) => {}),
    update: vi.fn(async (_d: string, _m: Markup) => {}),
    remove: vi.fn(async (_d: string, _id: string) => {}),
  };
}

describe("MarkupStore", () => {
  it("create adds to markups and mirrors an add op", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    expect(s.markups.length).toBe(1);
    await s.flush();
    expect(ipc.add).toHaveBeenCalledTimes(1);
    expect(ipc.add).toHaveBeenCalledWith("doc1", expect.objectContaining({ id: "a" }));
  });

  it("update then delete mirror in order", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    const a = mk("a");
    s.create(a);
    s.update(a, { ...a, contents: "x" });
    s.delete("a");
    expect(s.markups.length).toBe(0);
    await s.flush();
    expect(ipc.add.mock.invocationCallOrder[0]).toBeLessThan(ipc.update.mock.invocationCallOrder[0]);
    expect(ipc.update.mock.invocationCallOrder[0]).toBeLessThan(ipc.remove.mock.invocationCallOrder[0]);
  });

  it("undo of a create mirrors a delete", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    s.undo();
    expect(s.markups.length).toBe(0);
    await s.flush();
    expect(ipc.remove).toHaveBeenCalledWith("doc1", "a");
  });

  it("seed loads markups without enqueuing mirror ops", async () => {
    const ipc = fakeIpc();
    const s = new MarkupStore("doc1", ipc);
    s.seed([mk("a"), mk("b")]);
    expect(s.markups.length).toBe(2);
    await s.flush();
    expect(ipc.add).not.toHaveBeenCalled();
  });

  it("a failed op records mirrorError and halts the drain", async () => {
    const ipc = fakeIpc();
    ipc.add.mockRejectedValueOnce(new Error("boom"));
    const s = new MarkupStore("doc1", ipc);
    s.create(mk("a"));
    await s.flush();
    expect(s.mirrorError).toContain("boom");
  });
});
```

- [ ] **Step 2: Run, verify fail**

Run: `npm run test 2>&1 | tail -12`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement** — `src/lib/markup-store.svelte.ts`:

```typescript
/**
 * In-session source of truth for markups (spec §6/§15). Owns the reactive markup array,
 * selection + active tool, and a command-pattern History. Each committed command's
 * MirrorOp is drained through an ordered FIFO to the Rust store (the save buffer) via the
 * injected IPC. flush() awaits a full drain — App.svelte calls it before save_document.
 */
import type { Markup, Appearance } from "./ipc";
import { History, CreateCmd, UpdateCmd, DeleteCmd, type MarkupSink, type MirrorOp } from "./markup-commands";

/** The IPC surface the store mirrors to (injected for testability). */
export interface MarkupIpc {
  add(doc_id: string, m: Markup): Promise<void>;
  update(doc_id: string, m: Markup): Promise<void>;
  remove(doc_id: string, id: string): Promise<void>;
}

export type ToolKind =
  | "hand" | "select" | "Rectangle" | "Ellipse" | "Line" | "Arrow" | "Highlight"
  | "Polyline" | "Polygon" | "Cloud" | "Ink" | "Text" | "Callout";

const DEFAULT_APPEARANCE: Appearance = {
  color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null,
};

export class MarkupStore implements MarkupSink {
  markups = $state<Markup[]>([]);
  selectedIds = $state<Set<string>>(new Set());
  activeTool = $state<ToolKind>("hand");
  draftAppearance = $state<Appearance>({ ...DEFAULT_APPEARANCE });
  mirrorError = $state<string | null>(null);

  private history = new History(this);
  private queue: MirrorOp[] = [];
  private draining = false;

  constructor(private readonly docId: string, private readonly ipc: MarkupIpc) {}

  // --- MarkupSink (used by History; never enqueues — the History caller does) ---
  insert(m: Markup) { this.markups.push(m); }
  replace(m: Markup) { const i = this.markups.findIndex((x) => x.id === m.id); if (i >= 0) this.markups[i] = m; }
  removeById(id: string) { this.markups = this.markups.filter((x) => x.id !== id); this.selectedIds.delete(id); }
  getById(id: string) { return this.markups.find((x) => x.id === id); }

  // --- Loading (no undo entry, no mirror — the PDF already has these) ---
  seed(markups: Markup[]) { this.markups = markups; this.history = new History(this); this.queue = []; }

  // --- Mutations (undoable + mirrored) ---
  create(m: Markup) { this.enqueue(this.history.push(new CreateCmd(m))); }
  update(before: Markup, after: Markup) { this.enqueue(this.history.push(new UpdateCmd(before, after))); }
  delete(id: string) { const m = this.getById(id); if (m) this.enqueue(this.history.push(new DeleteCmd(m))); }
  undo() { const op = this.history.undo(); if (op) this.enqueue(op); }
  redo() { const op = this.history.redo(); if (op) this.enqueue(op); }

  get canUndo() { return this.history.canUndo; }
  get canRedo() { return this.history.canRedo; }

  // --- Ordered async mirror ---
  private enqueue(op: MirrorOp) { this.queue.push(op); void this.drain(); }

  private async drain(): Promise<void> {
    if (this.draining) return;
    this.draining = true;
    try {
      while (this.queue.length > 0) {
        const op = this.queue[0];
        try {
          if (op.kind === "add") await this.ipc.add(this.docId, op.markup);
          else if (op.kind === "update") await this.ipc.update(this.docId, op.markup);
          else await this.ipc.remove(this.docId, op.id);
        } catch (e) {
          this.mirrorError = `Sync failed: ${e instanceof Error ? e.message : String(e)}`;
          return; // halt; queue head stays for retry on next enqueue/flush
        }
        this.queue.shift();
        this.mirrorError = null;
      }
    } finally {
      this.draining = false;
    }
  }

  /** Await a full drain of pending mirror ops (call before save). */
  async flush(): Promise<void> {
    await this.drain();
    if (this.queue.length > 0) throw new Error(this.mirrorError ?? "mirror queue not drained");
  }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `npm run test 2>&1 | tail -10`
Expected: PASS. Then `npm run check 2>&1 | tail -3` → 0 errors.

> Note: `.svelte.ts` runes (`$state`) compile under the Svelte vite plugin; vitest already transforms them in this project's config. If a test runner error mentions `$state` is undefined, confirm `vitest.config` uses the svelte plugin (it does for the existing `.test.ts`). The store's reactive fields are exercised as plain values in tests.

- [ ] **Step 5: Commit**

```bash
git add src/lib/markup-store.svelte.ts src/lib/markup-store.test.ts
git commit -m "feat(markup): reactive store + ordered async mirror queue

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task 2.3: wire the store into App.svelte (seed on load, flush before save)

**Files:**
- Modify: `src/App.svelte`

- [ ] **Step 1: Construct the store** — in `App.svelte` script, replace the bare `let markups = $state<Markup[]>([])` with a store instance built from the real IPC, and derive `markups` from it. Import the store + ipc fns:

```typescript
import { MarkupStore } from "$lib/markup-store.svelte";
import { addMarkup, updateMarkup, deleteMarkup } from "$lib/ipc";

let store = $state<MarkupStore | null>(null);
const markups = $derived(store?.markups ?? []);
```

- [ ] **Step 2: On open**, where `currentDoc` is assigned and `loadMarkups` resolves, build the store and seed it:

```typescript
store = new MarkupStore(doc.doc_id, {
  add: addMarkup, update: updateMarkup, remove: deleteMarkup,
});
loadMarkups(doc.doc_id)
  .then((m) => { store?.seed(m); })
  .catch((e) => { openError = `Load markups failed: ${e}`; });
```

- [ ] **Step 3: Before save**, in the Save / Save-As handlers, flush the mirror first:

```typescript
await store?.flush();
await saveDocument(currentDoc.doc_id);
```

- [ ] **Step 4: Pass the store to Viewport** (overlay reads `store.markups`; G3 adds tool/selection use). Update the tag: `<Viewport docInfo={currentDoc} {markups} />` stays valid (markups is the derived array). Keep `store` available for G3 wiring.

- [ ] **Step 5: Verify + commit**

Run: `npm run check 2>&1 | tail -3` → 0 errors. `npm run test 2>&1 | tail -5` → green.

```bash
git add src/App.svelte
git commit -m "feat(ui): own the markup store in App, flush mirror before save

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**G2 gate:** vitest green (commands + store + render); `npm run check` 0 errors; `cargo build` clean. At this point there is no authoring UI yet, but the store + undo + mirror are fully exercised by tests.

---

# GROUPS 3–9 — task map (detailed JIT before each group executes)

Each group below will be expanded into bite-sized TDD tasks (full code, no placeholders) immediately before it is executed, grounded in the concrete interfaces built in G1–G2. They are listed here so the slice shape and sequencing are fixed.

**Testing strategy (set 2026-06-14 — supersedes per-group manual GUI smoke).** An automated interaction harness is in place (`@testing-library/svelte` + vitest jsdom, e-fees' pattern; setup `src/tests/setup.ts`, component tests carry `// @vitest-environment jsdom`). **Every tool/gesture group ships interaction tests** that mount the real component, script the pointer/keyboard gesture, and assert on store state + the rendered SVG, including a **glued-on-zoom assertion** (the §5 no-drift invariant). This replaces per-operation manual checking. Manual GUI verification collapses to a **single full-app smoke at G9** (real PDFium tiles + real save round-trip in Acrobat/Bluebeam) — the only things the headless harness can't cover. Each later group's "Exit" implicitly includes: interaction tests green + the standing gates (vitest, `npm run check` 0 errors, `cargo test --test-threads=1`, clippy 0, cargo fmt).

### G3 — Drag-draw tools + toolbar + overlay capture  *(DETAILED — ready to execute)*

First GUI increment: a tool strip selects a tool; with a drag-draw tool active, press-drag-release on the overlay draws a Rectangle/Ellipse/Line/Arrow/Highlight that commits through `store.create` (→ mirror → save round-trip), and undo removes it. Hand tool keeps today's pan; Select is a placeholder (G6).

**Files:** create `src/lib/markup-tools.ts` + `src/lib/markup-tools.test.ts` + `src/components/ToolPalette.svelte`; modify `src/App.svelte` + `src/components/Viewport.svelte`.

#### Task 3.1: pure geometry + markup builders (`markup-tools.ts`) — TDD

- [ ] **Step 1: failing tests** — `src/lib/markup-tools.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { dragDrawGeometry, buildMarkup, RECT_TOOLS } from "./markup-tools";
import type { Appearance, UserRef } from "./ipc";

const AP: Appearance = { color: "#e02424", line_weight: 2, opacity: 1, fill: null, line_style: "Solid", font: null };
const USER: UserRef = { user_id: "11111111-1111-1111-1111-111111111111", display_name: "Tester" };

describe("dragDrawGeometry", () => {
  it("normalizes a Rect tool to min/max regardless of drag direction", () => {
    const g = dragDrawGeometry("Rectangle", { x: 60, y: 70 }, { x: 10, y: 20 });
    expect(g).toEqual({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 70 } } });
  });
  it("uses Rect geometry for Ellipse and Highlight too", () => {
    expect("Rect" in dragDrawGeometry("Ellipse", { x: 0, y: 0 }, { x: 5, y: 5 })).toBe(true);
    expect("Rect" in dragDrawGeometry("Highlight", { x: 0, y: 0 }, { x: 5, y: 5 })).toBe(true);
    expect(RECT_TOOLS.has("Rectangle")).toBe(true);
  });
  it("uses a 2-point Polyline (in drag order) for Line and Arrow", () => {
    const g = dragDrawGeometry("Line", { x: 1, y: 2 }, { x: 3, y: 4 });
    expect(g).toEqual({ Polyline: [{ x: 1, y: 2 }, { x: 3, y: 4 }] });
    expect("Polyline" in dragDrawGeometry("Arrow", { x: 0, y: 0 }, { x: 1, y: 1 })).toBe(true);
  });
});

describe("buildMarkup", () => {
  it("builds an envelope with audit from identity, revision 0, created==modified", () => {
    const m = buildMarkup({
      markupType: "Rectangle", page: 2,
      geometry: { Rect: { min: { x: 0, y: 0 }, max: { x: 1, y: 1 } } },
      appearance: AP, identity: USER, now: "2026-06-14T00:00:00Z", id: "abc",
    });
    expect(m.id).toBe("abc");
    expect(m.markup_type).toBe("Rectangle");
    expect(m.page).toBe(2);
    expect(m.appearance).toEqual(AP);
    expect(m.audit.created_by).toEqual(USER);
    expect(m.audit.modified_by).toEqual(USER);
    expect(m.audit.created_at).toBe("2026-06-14T00:00:00Z");
    expect(m.audit.modified_at).toBe("2026-06-14T00:00:00Z");
    expect(m.audit.revision).toBe(0);
    expect(m.audit.origin).toBe("Desktop");
    expect(m.workflow).toEqual({ status: "None", assignee: null, thread: [] });
    expect(m.subject).toBeNull();
    expect(m.contents).toBeNull();
    expect(m.measurement).toBeNull();
  });
});
```

- [ ] **Step 2: run, fail** — `npm run test 2>&1 | tail -12` → module missing.

- [ ] **Step 3: implement** — `src/lib/markup-tools.ts`:

```typescript
/**
 * Pure interaction helpers: build markup geometry from pointer gestures (PDF user space)
 * and assemble a Markup envelope. No DOM, no Svelte, no clocks/UUIDs inside — the caller
 * passes `id` + `now` so this stays deterministic and unit-testable. Viewport.svelte does
 * the screen→PDF conversion (via the tested `screenToPdfUserSpace`) before calling these.
 */
import type { Markup, MarkupType, MarkupGeometry, Appearance, UserRef, PdfPoint } from "./ipc";
import type { ToolKind } from "./markup-store.svelte";

/** Drag-draw tools whose geometry is an axis-aligned bounding Rect. */
export const RECT_TOOLS: ReadonlySet<ToolKind> = new Set<ToolKind>(["Rectangle", "Ellipse", "Highlight"]);

/** Build geometry for a drag-draw tool from two PDF-space points (press + release). */
export function dragDrawGeometry(tool: ToolKind, a: PdfPoint, b: PdfPoint): MarkupGeometry {
  if (RECT_TOOLS.has(tool)) {
    return {
      Rect: {
        min: { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y) },
        max: { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y) },
      },
    };
  }
  return { Polyline: [a, b] }; // Line / Arrow
}

/** Assemble a fresh markup envelope. `id` (UUID) and `now` (ISO-8601) are injected. */
export function buildMarkup(opts: {
  markupType: MarkupType;
  page: number;
  geometry: MarkupGeometry;
  appearance: Appearance;
  identity: UserRef;
  now: string;
  id: string;
}): Markup {
  return {
    id: opts.id,
    markup_type: opts.markupType,
    page: opts.page,
    geometry: opts.geometry,
    appearance: opts.appearance,
    subject: null,
    layer: null,
    contents: null,
    audit: {
      created_by: opts.identity,
      created_at: opts.now,
      modified_by: opts.identity,
      modified_at: opts.now,
      revision: 0,
      origin: "Desktop",
    },
    workflow: { status: "None", assignee: null, thread: [] },
    measurement: null,
  };
}
```

- [ ] **Step 4: run, pass** — `npm run test 2>&1 | tail -8`. Then `npm run check 2>&1 | tail -3` → 0 errors.
- [ ] **Step 5: commit** — `feat(markup): drag-draw geometry + markup envelope builders` (+ Co-Authored-By trailer).

#### Task 3.2: tool palette + identity wiring (`ToolPalette.svelte`, `App.svelte`)

- [ ] **Step 1: create `src/components/ToolPalette.svelte`** — a horizontal tool strip; buttons set `store.activeTool`; the active tool is highlighted. ToolKinds for G3: `hand`, `select`, `Rectangle`, `Ellipse`, `Line`, `Arrow`, `Highlight` (the rest exist in `ToolKind` but their tools land in G4/G5 — do not surface buttons for them yet).

```svelte
<script lang="ts">
  import type { MarkupStore, ToolKind } from "$lib/markup-store.svelte";
  const { store }: { store: MarkupStore } = $props();
  const TOOLS: { kind: ToolKind; label: string; title: string }[] = [
    { kind: "hand", label: "✋", title: "Pan (Hand)" },
    { kind: "select", label: "▭", title: "Select" },
    { kind: "Rectangle", label: "▢", title: "Rectangle" },
    { kind: "Ellipse", label: "◯", title: "Ellipse" },
    { kind: "Line", label: "╱", title: "Line" },
    { kind: "Arrow", label: "↗", title: "Arrow" },
    { kind: "Highlight", label: "▬", title: "Highlight" },
  ];
</script>
<div class="tool-strip" role="toolbar" aria-label="Markup tools">
  {#each TOOLS as t (t.kind)}
    <button
      class="tool-btn"
      class:active={store.activeTool === t.kind}
      title={t.title}
      aria-pressed={store.activeTool === t.kind}
      onclick={() => (store.activeTool = t.kind)}
    >{t.label}</button>
  {/each}
</div>
<style>
  .tool-strip {
    display: flex; gap: var(--space-1);
    padding: var(--space-1) var(--space-3);
    background: var(--color-bg-toolbar);
    border-bottom: 1px solid var(--color-border);
    flex-shrink: 0;
  }
  .tool-btn {
    background: var(--color-bg-active); border: 1px solid var(--color-border);
    border-radius: var(--radius-sm); color: var(--color-text);
    cursor: pointer; font-size: var(--font-size-base);
    width: 32px; height: 28px; line-height: 1; transition: background 120ms;
  }
  .tool-btn:hover { background: var(--color-bg-hover); }
  .tool-btn.active { background: var(--color-primary); color: var(--color-text-inverse); border-color: var(--color-primary); }
</style>
```

- [ ] **Step 2: `App.svelte`** — import `ToolPalette`; render it (only when `store`) directly under the `</header>` toolbar and above `{#if openError}`. Pass `store` to BOTH the palette and the viewport.

```svelte
  import ToolPalette from "./components/ToolPalette.svelte";
```
```svelte
  </header>

  {#if store}
    <ToolPalette {store} />
  {/if}

  {#if openError}
```
And change the viewport tag (centre column) to pass the store:
```svelte
        <Viewport docInfo={currentDoc} {store} />
```
(Viewport will read `store.markups` directly — see Task 3.3 — so the separate `{markups}` prop is removed. The `markups`/`addMarkup` etc. imports in App stay; `markups` derived may become unused — if `npm run check` flags it, delete the `const markups = $derived(...)` line.)

- [ ] **Step 3:** `npm run check 2>&1 | tail -3` → 0 errors. Commit — `feat(ui): markup tool palette + wire store into App`.

#### Task 3.3: overlay pointer capture + draw (`Viewport.svelte`)

Viewport currently: takes `{ docInfo, markups }`, pans via `viewport-root` mouse handlers, renders `pageShapes` (from `markups`) in the `.markup-overlay` SVG (`pointer-events: none`). Change it to take the `store`, derive markups from it, and capture draw gestures on the overlay when a drag-draw tool is active.

- [ ] **Step 1: props + identity** — replace the props rune and derive markups from the store; fetch identity once for created markups:

```typescript
import { MarkupStore } from "$lib/markup-store.svelte";
import { getUserIdentity, type UserRef } from "$lib/ipc";
import { dragDrawGeometry, buildMarkup, RECT_TOOLS } from "$lib/markup-tools";
import { screenToPdfUserSpace } from "$lib/viewport"; // add to the existing viewport import if not present

const { docInfo, store }: { docInfo: DocumentInfo; store: MarkupStore } = $props();

let identity = $state<UserRef | null>(null);
onMount(() => { getUserIdentity().then((u) => (identity = u)).catch(() => {}); });
```
Replace the existing `pageShapes` derivation source `markups` with `store.markups`:
```typescript
const pageShapes = $derived<SvgShape[]>(
  store.markups.filter((m) => m.page === pageIndex).map((m) => markupToSvg(m, viewState)),
);
```

- [ ] **Step 2: draw gesture state + handlers** — add draw state and pointer handlers that run when `store.activeTool` is a drag-draw tool. Use container-local CSS px → PDF via `screenToPdfUserSpace`. A live preview markup is rendered but NOT committed until pointerup; on commit, `store.create(buildMarkup(...))`.

```typescript
const DRAG_TOOLS = new Set(["Rectangle", "Ellipse", "Line", "Arrow", "Highlight"]);
let drawing = $state(false);
let drawStartPdf: { x: number; y: number } | null = null;
let previewMarkup = $state<Markup | null>(null);

function localPdf(e: PointerEvent): { x: number; y: number } | null {
  if (!containerEl) return null;
  const r = containerEl.getBoundingClientRect();
  return screenToPdfUserSpace(e.clientX - r.left, e.clientY - r.top, viewState);
}

function onOverlayPointerDown(e: PointerEvent) {
  if (!DRAG_TOOLS.has(store.activeTool) || !identity) return;
  const p = localPdf(e); if (!p) return;
  (e.target as Element).setPointerCapture(e.pointerId);
  drawing = true; drawStartPdf = p;
  e.stopPropagation(); e.preventDefault();
}
function onOverlayPointerMove(e: PointerEvent) {
  if (!drawing || !drawStartPdf || !identity) return;
  const p = localPdf(e); if (!p) return;
  const tool = store.activeTool;
  previewMarkup = buildMarkup({
    markupType: tool as Markup["markup_type"], page: pageIndex,
    geometry: dragDrawGeometry(tool, drawStartPdf, p),
    appearance: store.draftAppearance, identity, now: new Date().toISOString(), id: "preview",
  });
}
function onOverlayPointerUp(e: PointerEvent) {
  if (!drawing || !drawStartPdf || !identity) { drawing = false; return; }
  const p = localPdf(e);
  drawing = false;
  const start = drawStartPdf; drawStartPdf = null; previewMarkup = null;
  if (!p || (p.x === start.x && p.y === start.y)) return; // zero-size = no-op
  const tool = store.activeTool;
  store.create(buildMarkup({
    markupType: tool as Markup["markup_type"], page: pageIndex,
    geometry: dragDrawGeometry(tool, start, p),
    appearance: store.draftAppearance, identity, now: new Date().toISOString(),
    id: crypto.randomUUID(),
  }));
}
```

- [ ] **Step 3: overlay element** — make the overlay capture events only when a draw tool is active (so Hand-tool pan still works through it). Add `class:drawing` + pointer handlers to the existing `<svg class="markup-overlay">`, and render the preview shape on top of `pageShapes`:

```svelte
  <svg
    class="markup-overlay"
    class:capture={DRAG_TOOLS.has(store.activeTool)}
    aria-hidden="true"
    onpointerdown={onOverlayPointerDown}
    onpointermove={onOverlayPointerMove}
    onpointerup={onOverlayPointerUp}
  >
    {#each pageShapes as shape (shape.id)}
      ... (existing shape rendering unchanged) ...
    {/each}
    {#if previewMarkup}
      {@const pv = markupToSvg(previewMarkup, viewState)}
      ... render pv the same way as a shape (a small {#if pv.kind===...} block, or factor a snippet) ...
    {/if}
  </svg>
```
CSS: the base `.markup-overlay` keeps `pointer-events: none`; add `.markup-overlay.capture { pointer-events: auto; cursor: crosshair; }`.

> Implementer note: factor the per-shape SVG rendering into a Svelte `{#snippet shape(s)}` and call it for both `pageShapes` items and the preview, to avoid duplicating the 5-branch `{#if shape.kind}` block. Keep behavior identical to S2a.

- [ ] **Step 4:** `npm run check 2>&1 | tail -3` → 0 errors; `npm run test 2>&1 | tail -5` → green; `cd src-tauri && cargo build` → clean.
- [ ] **Step 5: commit** — `feat(ui): draw drag-tool markups on the overlay (create + undo + mirror)`.

**G3 exit / manual smoke (controller verifies after review):** `cargo tauri dev` with an annotated or blank PDF → pick Rectangle → drag → a red rectangle appears glued to the page; pan/zoom keeps it glued; Cmd+S then reopen round-trips it; (once an undo key is bound, or via a quick test) undo removes it. Hand tool still pans.

### G4 — Multi-click + freehand tools  *(DETAILED — ready to execute)*

Adds Polyline/Polygon/Cloud (multi-click: click per vertex, Enter/dblclick finish, Esc cancel) and Pen/Ink (freehand drag). Cloud renders with a scalloped path. Pure builders + cloud-path are unit-tested; the gestures get interaction tests (mount + scripted clicks/drag + glued-on-zoom), per the testing strategy above.

**Files:** modify `src/lib/markup-tools.ts` (+test), `src/lib/markup-render.ts` (+test), `src/components/ToolPalette.svelte`, `src/components/Viewport.svelte` (+interaction tests).

#### Task 4.1: multi-click + ink pure helpers (`markup-tools.ts`) — TDD

- [ ] **Step 1: failing tests** — append to `src/lib/markup-tools.test.ts`:

```typescript
import { MULTI_CLICK_TOOLS, isMultiClickTool, isInkTool, polylineGeometry, inkGeometry, minVertices, isMultiClickComplete } from "./markup-tools";

describe("multi-click + ink helpers", () => {
  it("classifies tools", () => {
    expect(isMultiClickTool("Polyline")).toBe(true);
    expect(isMultiClickTool("Polygon")).toBe(true);
    expect(isMultiClickTool("Cloud")).toBe(true);
    expect(isMultiClickTool("Rectangle")).toBe(false);
    expect(isMultiClickTool("hand")).toBe(false);
    expect(isInkTool("Ink")).toBe(true);
    expect(isInkTool("Polyline")).toBe(false);
    expect(MULTI_CLICK_TOOLS.has("Cloud")).toBe(true);
  });
  it("minVertices: polyline 2, polygon/cloud 3", () => {
    expect(minVertices("Polyline")).toBe(2);
    expect(minVertices("Polygon")).toBe(3);
    expect(minVertices("Cloud")).toBe(3);
  });
  it("isMultiClickComplete gates on minVertices", () => {
    expect(isMultiClickComplete("Polyline", [{x:0,y:0}])).toBe(false);
    expect(isMultiClickComplete("Polyline", [{x:0,y:0},{x:1,y:1}])).toBe(true);
    expect(isMultiClickComplete("Polygon", [{x:0,y:0},{x:1,y:1}])).toBe(false);
    expect(isMultiClickComplete("Polygon", [{x:0,y:0},{x:1,y:1},{x:2,y:0}])).toBe(true);
  });
  it("polylineGeometry copies the vertices into a Polyline", () => {
    const verts = [{x:0,y:0},{x:10,y:0},{x:10,y:10}];
    const g = polylineGeometry(verts) as { Polyline: typeof verts };
    expect(g.Polyline).toEqual(verts);
    expect(g.Polyline).not.toBe(verts); // defensive copy
  });
  it("inkGeometry wraps strokes into an Ink", () => {
    const strokes = [[{x:0,y:0},{x:1,y:1}]];
    const g = inkGeometry(strokes) as { Ink: typeof strokes };
    expect(g.Ink).toEqual(strokes);
  });
});
```

- [ ] **Step 2: run, fail** — `npm run test 2>&1 | tail -8`.

- [ ] **Step 3: implement** — append to `src/lib/markup-tools.ts` (it already imports `MarkupType`, `MarkupGeometry`, `PdfPoint`, `ToolKind`):

```typescript
/** Multi-click polyline-family tools (click per vertex; closed for Polygon/Cloud). */
export type MultiClickTool = Extract<MarkupType, "Polyline" | "Polygon" | "Cloud">;
export const MULTI_CLICK_TOOLS: ReadonlySet<MultiClickTool> =
  new Set<MultiClickTool>(["Polyline", "Polygon", "Cloud"]);
export function isMultiClickTool(t: ToolKind): t is MultiClickTool {
  return (MULTI_CLICK_TOOLS as ReadonlySet<string>).has(t);
}
export function isInkTool(t: ToolKind): t is Extract<MarkupType, "Ink"> {
  return t === "Ink";
}

/** Minimum vertices before a multi-click shape can be committed. */
export function minVertices(tool: MultiClickTool): number {
  return tool === "Polyline" ? 2 : 3; // Polygon / Cloud are closed → need 3
}
export function isMultiClickComplete(tool: MultiClickTool, verts: PdfPoint[]): boolean {
  return verts.length >= minVertices(tool);
}

/** Geometry builders (defensive copies — callers mutate their working arrays). */
export function polylineGeometry(verts: PdfPoint[]): MarkupGeometry {
  return { Polyline: verts.map((p) => ({ x: p.x, y: p.y })) };
}
export function inkGeometry(strokes: PdfPoint[][]): MarkupGeometry {
  return { Ink: strokes.map((s) => s.map((p) => ({ x: p.x, y: p.y }))) };
}
```

- [ ] **Step 4: pass** — `npm run test 2>&1 | tail -6`; `npm run check 2>&1 | tail -3` → 0 errors.
- [ ] **Step 5: commit** — `feat(markup): multi-click + ink geometry helpers`.

#### Task 4.2: cloud scallop render (`markup-render.ts`) — TDD

- [ ] **Step 1: failing tests** — append to `src/lib/markup-render.test.ts`:

```typescript
import { cloudPath } from "./markup-render";

describe("cloud rendering", () => {
  it("cloudPath returns a closed arc path through the points", () => {
    const d = cloudPath([{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 40 }], 5);
    expect(d.startsWith("M")).toBe(true);
    expect(d).toContain("A");      // arc bumps
    expect(d.trimEnd().endsWith("Z")).toBe(true); // closed
  });
  it("longer edges get more bumps than shorter ones", () => {
    const shortP = cloudPath([{ x: 0, y: 0 }, { x: 10, y: 0 }], 5);
    const longP = cloudPath([{ x: 0, y: 0 }, { x: 100, y: 0 }], 5);
    const count = (s: string) => (s.match(/A/g) ?? []).length;
    expect(count(longP)).toBeGreaterThan(count(shortP));
  });
  it("maps a Cloud markup to a cloud shape (path), not a plain polygon", () => {
    const m = mk({ Polyline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 50 }] }, "Cloud");
    const s = markupToSvg(m, VS);
    expect(s.kind).toBe("cloud");
    if (s.kind !== "cloud") throw new Error("kind");
    expect(typeof s.path).toBe("string");
    expect(s.path.length).toBeGreaterThan(0);
  });
});
```
(`mk` and `VS` are defined at the top of the existing `markup-render.test.ts`.)

- [ ] **Step 2: run, fail.**

- [ ] **Step 3: implement** in `src/lib/markup-render.ts`:
  - Add a cloud variant to the union: `| (SvgStyle & { kind: "cloud"; path: string })`.
  - Add the exported helper:
```typescript
/**
 * Revision-cloud path: walk each closed edge placing outward semicircular arc "bumps"
 * (~2r apart). Screen-space points in, SVG path `d` out. Aesthetic only (not measured).
 */
export function cloudPath(pts: { x: number; y: number }[], r: number): string {
  if (pts.length < 2) return "";
  const loop = [...pts, pts[0]];
  let d = `M ${pts[0].x.toFixed(2)} ${pts[0].y.toFixed(2)}`;
  for (let i = 0; i < loop.length - 1; i++) {
    const a = loop[i], b = loop[i + 1];
    const len = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    const bumps = Math.max(1, Math.round(len / (r * 2)));
    const ux = (b.x - a.x) / len, uy = (b.y - a.y) / len;
    const step = len / bumps;
    let cx = a.x, cy = a.y;
    for (let j = 0; j < bumps; j++) {
      const nx = cx + ux * step, ny = cy + uy * step;
      const rad = (step / 2).toFixed(2);
      d += ` A ${rad} ${rad} 0 0 1 ${nx.toFixed(2)} ${ny.toFixed(2)}`;
      cx = nx; cy = ny;
    }
  }
  return d + " Z";
}
```
  - In `markupToSvg`, BEFORE the generic `"Polyline" in g` branch, special-case Cloud:
```typescript
  if ("Polyline" in g && m.markup_type === "Cloud") {
    const screen = g.Polyline.map((p) => pdfUserSpaceToScreen(p.x, p.y, v));
    return { ...style, kind: "cloud", path: cloudPath(screen, Math.max(4, 6 * v.zoom)) };
  }
```

- [ ] **Step 4: pass** + `npm run check`.
- [ ] **Step 5: commit** — `feat(ui): revision-cloud scallop rendering`.

#### Task 4.3: multi-click + freehand wiring (`Viewport.svelte`, `ToolPalette.svelte`)

- [ ] **Step 1: ToolPalette** — add buttons for `Polyline` (`⋁`), `Polygon` (`⬠`), `Cloud` (`☁`), `Ink` (`✎`) to the `TOOLS` array (after Highlight). Same `store.activeTool` wiring.

- [ ] **Step 2: Viewport overlay capture** — the overlay must capture when ANY creation tool is active. Replace the draw-tool gate used for `.capture` and the `onMouseDown` pan-guard with a combined predicate:
```typescript
import { isDrawTool, isMultiClickTool, isInkTool, dragDrawGeometry, buildMarkup,
         polylineGeometry, inkGeometry, isMultiClickComplete, type MultiClickTool } from "$lib/markup-tools";
const isCreateTool = (t = store.activeTool) => isDrawTool(t) || isMultiClickTool(t) || isInkTool(t);
```
Use `isCreateTool()` for `class:capture` and the `onMouseDown` early-return. Keep the existing drag-draw path gated on `isDrawTool`.

- [ ] **Step 3: multi-click state machine** — add:
```typescript
let mcVerts = $state<{ x: number; y: number }[]>([]);
let mcCursor = $state<{ x: number; y: number } | null>(null);

function onOverlayClick(e: MouseEvent) {
  if (!isMultiClickTool(store.activeTool) || !identity) return;
  const p = localPdfFromMouse(e); if (!p) return;
  mcVerts = [...mcVerts, p];
}
function onOverlayDblClick() { finishMultiClick(); }
function finishMultiClick() {
  const tool = store.activeTool;
  if (!isMultiClickTool(tool) || !identity) return;
  if (!isMultiClickComplete(tool, mcVerts)) { resetMultiClick(); return; }
  store.create(buildMarkup({
    markupType: tool, page: pageIndex, geometry: polylineGeometry(mcVerts),
    appearance: store.draftAppearance, identity, now: new Date().toISOString(), id: crypto.randomUUID(),
  }));
  resetMultiClick();
}
function resetMultiClick() { mcVerts = []; mcCursor = null; }
```
`localPdfFromMouse` mirrors the existing `localPdf` but for `MouseEvent` (factor a shared `clientToPdf(clientX, clientY)` helper used by both pointer + mouse/click paths). Track `mcCursor` on overlay pointermove while `mcVerts.length > 0` for the rubber-band preview. Wire `onclick`/`ondblclick` on the overlay `<svg>`.

- [ ] **Step 4: keyboard** — extend the existing `window` keydown (or add a viewport-scoped handler): `Enter` → `finishMultiClick()`; `Escape` → `resetMultiClick()` AND `cancelDraw()` (ink/drag). Only when a doc is open.

- [ ] **Step 5: freehand ink** — when `isInkTool(store.activeTool)`, reuse the pointer handlers: pointerdown starts `let inkStroke = $state<{x,y}[]>([])` with the first point; pointermove (while drawing) pushes points (throttle: skip if within ~1px of the last to avoid huge arrays); pointerup commits `store.create(buildMarkup({ markupType: "Ink", geometry: inkGeometry([inkStroke]), ... }))` if the stroke has ≥2 points, else no-op; reset. Route the existing `onOverlayPointerDown/Move/Up` to branch by tool family (drag-draw vs ink). `cancelDraw()` also clears `inkStroke`.

- [ ] **Step 6: previews** — render in-progress shapes via the existing `{#snippet shape}`:
  - multi-click: a `previewMarkup` built from `mcVerts` + (if set) `mcCursor` appended, using the active tool's markupType — so it shows the polygon/polyline/cloud forming.
  - ink: a `previewMarkup` Ink built from the live `inkStroke`.
  Add a `cloud` branch to the `{#snippet shape}` block: `{:else if shape.kind === "cloud"}<path d={shape.path} stroke={shape.stroke} stroke-width={shape.strokeWidth} fill={shape.fill} opacity={shape.opacity} stroke-dasharray={shape.dashArray ?? undefined} />`.

- [ ] **Step 7:** `npm run check` 0 errors; `npm run test` green; `cd src-tauri && cargo build` clean. Commit — `feat(ui): multi-click polyline/polygon/cloud + freehand ink authoring`.

#### Task 4.4: interaction tests

- [ ] Append to `src/components/Viewport.interaction.test.ts` (jsdom): 
  - **Polygon via clicks:** activeTool "Polygon"; click 3 distinct overlay points; dispatch `dblclick` (or Enter keydown); assert one markup, type Polygon, geometry Polyline with 3 verts (exact PDF coords per the documented transform).
  - **Polyline (2 verts):** activeTool "Polyline"; 2 clicks + Enter; assert created.
  - **Esc cancels:** 2 clicks then Escape keydown; assert `store.markups.length === 0`.
  - **Cloud renders a path:** create a Cloud (3 clicks + finish); assert an `svg.markup-overlay path` element exists.
  - **Ink freehand:** activeTool "Ink"; pointerdown→several pointermoves→pointerup; assert one Ink markup with ≥2 sampled points; assert ink renders (≥1 polyline).
  - **Glued-on-zoom (polyline):** draw a polyline; read a vertex's screen position from the rendered points; wheel-zoom; assert the points string scaled with zoom (no drift).
- [ ] `npm run test 2>&1 | tail -8` all green; commit — `test(g4): interaction tests for multi-click + ink + cloud`.

**G4 exit:** all interaction + unit tests green + standing gates. (No separate manual GUI step — folds into the G9 full-app smoke.)

### G5 — Text + Callout  *(DETAILED — ready to execute)*

Place and edit on-page text and callouts: a Text tool drops a text box (one click → inline
`<textarea>` → commit); a Callout drops a leader line + text box (click target, click anchor →
`<textarea>`). Both commit `contents` + `font` through `store.create` (→ mirror → save) and are
undoable. **Full fix (decided 2026-06-15):** Text/Callout persist as PDF **FreeText** with the
font in `/DA` so they render as on-page text in Acrobat/Bluebeam and survive save→reopen — this
is the one group whose backend (annotation serde) is in scope.

**Decisions (locked):**
- **Text geometry = `Rect`** (a real text box; `/Rect` is meaningful for external viewers).
- **Callout geometry = `Polyline`** (the leader; the *last* vertex is the text anchor). Persisted
  as `/CL` (spec §19.2), not `/Vertices`.
- **Font**: written to standard `/DA` (interop) **and** private `/RLFontFamily`+`/RLFontSize`
  (lossless), read back from the `/RL*` keys — mirrors the file's existing dual-key pattern
  (standard for interop, `/RL*` for exact redline round-trip).
- **Subtype**: `Text`+`Callout` → `FreeText`. `MeasurementCount` keeps `/Subtype Text` (M3 concern).
- Foreign `FreeText` import: has `/CL` → `Callout`, else → `Text`.

**Files:** modify `src/lib/markup-tools.ts` (+test), `src/lib/markup-render.ts` (+test),
`src-tauri/src/markup/annotation.rs` (serde +tests), `src/components/ToolPalette.svelte`,
`src/components/Viewport.svelte` (+interaction tests).

#### Task 5.1: text/callout pure helpers (`markup-tools.ts`) — TDD

- [ ] **Step 1: failing tests** — append to `src/lib/markup-tools.test.ts`:

```typescript
import { TEXT_TOOLS, isTextTool, textBoxGeometry, calloutGeometry, DEFAULT_TEXT_FONT } from "./markup-tools";

describe("text/callout helpers", () => {
  it("classifies text-entry tools", () => {
    expect(isTextTool("Text")).toBe(true);
    expect(isTextTool("Callout")).toBe(true);
    expect(isTextTool("Rectangle")).toBe(false);
    expect(isTextTool("hand")).toBe(false);
    expect(TEXT_TOOLS.has("Callout")).toBe(true);
  });
  it("textBoxGeometry: Rect with top-left at the anchor (PDF y-up: box extends right + down)", () => {
    const g = textBoxGeometry({ x: 10, y: 100 }, { width: 144, height: 18 }) as { Rect: { min: PdfPoint; max: PdfPoint } };
    expect(g.Rect.min).toEqual({ x: 10, y: 82 });   // y - height
    expect(g.Rect.max).toEqual({ x: 154, y: 100 }); // x + width, y
  });
  it("calloutGeometry: 2-point Polyline target→anchor (anchor is last)", () => {
    const g = calloutGeometry({ x: 0, y: 0 }, { x: 50, y: 60 }) as { Polyline: PdfPoint[] };
    expect(g.Polyline).toEqual([{ x: 0, y: 0 }, { x: 50, y: 60 }]);
  });
  it("DEFAULT_TEXT_FONT is Helvetica 12pt", () => {
    expect(DEFAULT_TEXT_FONT).toEqual({ family: "Helvetica", size_pt: 12 });
  });
  it("buildMarkup carries contents when provided (still null by default)", () => {
    const base = { markupType: "Text" as const, page: 0,
      geometry: textBoxGeometry({ x: 0, y: 0 }), appearance: AP, identity: USER, now: "t", id: "x" };
    expect(buildMarkup(base).contents).toBeNull();
    expect(buildMarkup({ ...base, contents: "hi" }).contents).toBe("hi");
  });
});
```
(`AP`/`USER`/`buildMarkup`/`PdfPoint` are already imported at the top of the test file.)

- [ ] **Step 2: run, fail** — `npm run test 2>&1 | tail -10`.

- [ ] **Step 3: implement** — append to `src/lib/markup-tools.ts`:

```typescript
/** Text-entry tools (inline textarea commits contents + font). */
export type TextTool = Extract<MarkupType, "Text" | "Callout">;
export const TEXT_TOOLS: ReadonlySet<TextTool> = new Set<TextTool>(["Text", "Callout"]);
export function isTextTool(t: ToolKind): t is TextTool {
  return (TEXT_TOOLS as ReadonlySet<string>).has(t);
}

/** Default font for new text/callout markups (G7 adds the picker). */
export const DEFAULT_TEXT_FONT = { family: "Helvetica", size_pt: 12 } as const;

/** Default text-box size in PDF points (≈2in × ~1 line @12pt). */
export const DEFAULT_TEXT_BOX = { width: 144, height: 18 } as const;

/** Build a Text-box Rect from a top-left anchor (PDF user space, y-up). */
export function textBoxGeometry(anchor: PdfPoint, box: { width: number; height: number } = DEFAULT_TEXT_BOX): MarkupGeometry {
  return {
    Rect: {
      min: { x: anchor.x, y: anchor.y - box.height },
      max: { x: anchor.x + box.width, y: anchor.y },
    },
  };
}

/** Build a Callout leader Polyline from the target point to the text anchor (anchor last). */
export function calloutGeometry(target: PdfPoint, anchor: PdfPoint): MarkupGeometry {
  return { Polyline: [{ x: target.x, y: target.y }, { x: anchor.x, y: anchor.y }] };
}
```
Then extend `buildMarkup`'s `opts` with `contents?: string | null;` and change the body line to
`contents: opts.contents ?? null,` (backward-compatible — existing callers omit it).

- [ ] **Step 4: pass** + `npm run check 2>&1 | tail -3` → 0 errors.
- [ ] **Step 5: commit** — `feat(markup): text-box + callout-leader geometry helpers`.

#### Task 5.2: text + callout render kinds (`markup-render.ts`) — TDD

- [ ] **Step 1: failing tests** — in `src/lib/markup-render.test.ts`: (a) **change** the existing
  "maps a Point to a screen-space marker position" test's type from `"Text"` to `"MeasurementCount"`
  (Text is no longer a Point); (b) append:

```typescript
describe("text + callout rendering", () => {
  it("maps a Text (Rect box) to a text shape at the box top-left, font-scaled by zoom", () => {
    const m = mk({ Rect: { min: { x: 10, y: 20 }, max: { x: 60, y: 40 } } }, "Text",
      { font: { family: "Helvetica", size_pt: 12 } });
    m.contents = "hello";
    const s = markupToSvg(m, VS);
    if (s.kind !== "text") throw new Error("kind");
    expect(s.text).toBe("hello");
    expect(s.fontPx).toBe(12 * VS.zoom);   // scaled by zoom
    // top-left = screen of (min.x, max.y) — verify it matches the transform
    const tl = pdfUserSpaceToScreen(10, 40, VS);
    expect(s.x).toBeCloseTo(tl.x); expect(s.y).toBeCloseTo(tl.y);
  });
  it("Text with null contents renders empty text, default 12pt", () => {
    const s = markupToSvg(mk({ Rect: { min: { x: 0, y: 0 }, max: { x: 10, y: 10 } } }, "Text"), VS);
    if (s.kind !== "text") throw new Error("kind");
    expect(s.text).toBe(""); expect(s.fontPx).toBe(12 * VS.zoom);
  });
  it("maps a Callout (Polyline leader) to a callout shape: leader points + text at the last vertex", () => {
    const m = mk({ Polyline: [{ x: 0, y: 0 }, { x: 50, y: 60 }] }, "Callout");
    m.contents = "see note";
    const s = markupToSvg(m, VS);
    if (s.kind !== "callout") throw new Error("kind");
    expect(s.points.split(" ").length).toBe(2);
    expect(s.text).toBe("see note");
    const anchor = pdfUserSpaceToScreen(50, 60, VS);
    expect(s.x).toBeCloseTo(anchor.x); expect(s.y).toBeCloseTo(anchor.y);
  });
});
```
(`pdfUserSpaceToScreen` import + the `ap`/font param of `mk` already exist; `mk` accepts a 3rd
`Partial<Appearance>` arg per the helper.)

- [ ] **Step 2: run, fail.**

- [ ] **Step 3: implement** — in `src/lib/markup-render.ts`:
  - Extend the `SvgShape` union:
```typescript
  | (SvgStyle & { kind: "text"; x: number; y: number; text: string; fontPx: number })
  | (SvgStyle & { kind: "callout"; points: string; x: number; y: number; text: string; fontPx: number })
```
  - Add `const DEFAULT_FONT_PT = 12;`
  - In `markupToSvg`, **before** the `if ("Rect" in g)` branch, special-case Text/Callout:
```typescript
  const fontPx = (m.appearance.font?.size_pt ?? DEFAULT_FONT_PT) * v.zoom;
  if (m.markup_type === "Text" && "Rect" in g) {
    const tl = pdfUserSpaceToScreen(g.Rect.min.x, g.Rect.max.y, v); // PDF top-left (y-up)
    return { ...style, kind: "text", x: tl.x, y: tl.y, text: m.contents ?? "", fontPx };
  }
  if (m.markup_type === "Callout" && "Polyline" in g) {
    const last = g.Polyline[g.Polyline.length - 1] ?? { x: 0, y: 0 };
    const anchor = pdfUserSpaceToScreen(last.x, last.y, v);
    return { ...style, kind: "callout", points: pointsStr(g.Polyline, v),
      x: anchor.x, y: anchor.y, text: m.contents ?? "", fontPx };
  }
```
  (Place these after `const g = m.geometry;` and the `style`/`fontPx` setup. The Cloud Polyline
  special-case stays as-is below.)

- [ ] **Step 4: pass** + `npm run check`.
- [ ] **Step 5: commit** — `feat(ui): SVG text + callout shape rendering`.

#### Task 5.3: FreeText subtype + `/CL` leader + font serde (`annotation.rs`) — TDD

- [ ] **Step 1: failing tests** — in `src-tauri/src/markup/annotation.rs` tests:
  (a) **change** `point_markup_round_trips` to use `MarkupType::MeasurementCount` (keeps Point
  coverage; Text is now a Rect box). (b) add `assert_eq!(back.appearance.font, m.appearance.font, "font");`
  to `assert_roundtrip`. (c) append:

```rust
    #[test]
    fn freetext_with_font_round_trips_and_emits_da() {
        let g = MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 20.0 },
            max: PdfPoint { x: 160.0, y: 38.0 },
        };
        let mut m = fixture(g, MarkupType::Text);
        m.appearance.font = Some(FontSpec { family: "Helvetica".into(), size_pt: 12.0 });
        let d = m.to_annotation_dict();
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("FreeText"));
        assert!(d.has(b"DA"), "FreeText with a font must emit /DA");
        assert_roundtrip(&m); // assert_roundtrip now also checks font
    }

    #[test]
    fn callout_emits_cl_leader_and_round_trips() {
        let g = MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 50.0, y: 60.0 },
        ]);
        let mut m = fixture(g, MarkupType::Callout);
        m.appearance.font = Some(FontSpec { family: "Helvetica".into(), size_pt: 14.0 });
        let d = m.to_annotation_dict();
        assert_eq!(get_name(&d, b"Subtype").as_deref(), Some("FreeText"));
        assert!(d.has(b"CL"), "Callout must emit /CL leader");
        assert!(!d.has(b"Vertices"), "Callout uses /CL, not /Vertices");
        assert_roundtrip(&m);
    }

    #[test]
    fn foreign_freetext_imports_as_text_without_cl_callout_with_cl() {
        let mut d = Dictionary::new();
        d.set("Subtype", name("FreeText"));
        d.set("Rect", Object::Array(vec![real(5.0), real(6.0), real(100.0), real(26.0)]));
        d.set("Contents", Object::string_literal("foreign text"));
        assert_eq!(Markup::from_annotation_dict(&d).markup_type, MarkupType::Text);
        d.set("CL", Object::Array(vec![real(0.0), real(0.0), real(5.0), real(6.0)]));
        assert_eq!(Markup::from_annotation_dict(&d).markup_type, MarkupType::Callout);
    }
```
  Add `FontSpec` to the `use super::{...}` import in the test module if not already in scope
  (it is imported at module top for the impl; the tests use `super::*`).

- [ ] **Step 2: run, fail** — `cd src-tauri && cargo test --lib annotation:: 2>&1 | tail -20`.

- [ ] **Step 3: implement** in `src-tauri/src/markup/annotation.rs`:
  - Add `FontSpec` to the top-level `use super::{...}` list.
  - `pdf_subtype`: split Text off MeasurementCount —
    `MarkupType::MeasurementCount => "Text",` and add `MarkupType::Text | MarkupType::Callout => "FreeText",`.
  - Geometry write (the `MarkupGeometry::Polyline(pts)` arm in `to_annotation_dict`): add a
    Callout branch before the `else`:
```rust
            MarkupGeometry::Polyline(pts) => {
                if matches!(pdf_subtype(t), "Line") && pts.len() >= 2 {
                    d.set("L", flatten(&pts[..2]));
                } else if matches!(t, MarkupType::Callout) {
                    d.set("CL", flatten(pts)); // callout leader line (spec §19.2)
                } else {
                    d.set("Vertices", flatten(pts));
                }
            }
```
  - Font write — after the `BS` block, before the `/RL*` keys:
```rust
        // Font: FreeText /DA (interop) + lossless /RLFont* round-trip (spec §6).
        if let Some(font) = &self.appearance.font {
            let rgb = hex_to_rgb(&self.appearance.color).unwrap_or([0.0, 0.0, 0.0]);
            d.set(
                "DA",
                Object::string_literal(format!(
                    "/Helv {:.0} Tf {:.3} {:.3} {:.3} rg",
                    font.size_pt, rgb[0], rgb[1], rgb[2]
                )),
            );
            d.set("RLFontFamily", Object::string_literal(font.family.clone()));
            d.set("RLFontSize", real(font.size_pt));
        }
```
  - `geometry_from_dict`: in the `Some("poly")` arm and the `_` default arm, add `/CL` to the
    reals chain: `get_reals(d, b"Vertices").or_else(|| get_reals(d, b"CL")).or_else(|| get_reals(d, b"L"))`.
  - `from_annotation_dict` subtype fallback: replace the `Some("FreeText") => Some(MarkupType::Callout)`
    line with `Some("FreeText") => Some(if d.has(b"CL") { MarkupType::Callout } else { MarkupType::Text }),`.
  - Font read: add a `get_real` helper —
    `fn get_real(d: &Dictionary, key: &[u8]) -> Option<f64> { d.get(key).ok()?.as_f32().ok().map(|f| f as f64) }`
    — and in the `Appearance { ... }` construction replace `font: None,` with:
```rust
                font: get_real(d, b"RLFontSize").map(|size_pt| FontSpec {
                    family: get_string(d, b"RLFontFamily").unwrap_or_else(|| "Helvetica".to_string()),
                    size_pt,
                }),
```
  - Update the module doc comment (lines 12–17): font is now mapped (`/DA` + `/RLFont*`); drop it
    from the "NOT yet mapped" list.

- [ ] **Step 4: pass** — `cd src-tauri && cargo test --lib 2>&1 | tail -8` all green;
  `cargo clippy --all-targets 2>&1 | tail -3` 0 warnings; `cargo fmt`.
- [ ] **Step 5: commit** — `feat(markup): persist Text/Callout as FreeText with /CL leader + font /DA`.

#### Task 5.4: ToolPalette buttons

- [ ] Add to the `TOOLS` array in `src/components/ToolPalette.svelte` (after `Ink`):
  `{ kind: "Text", label: "A", title: "Text" },` and `{ kind: "Callout", label: "💬", title: "Callout" }`.
  `npm run check` → 0 errors. Commit — `feat(ui): Text + Callout tool buttons`.

#### Task 5.5: inline text editor + gestures (`Viewport.svelte`)

Text/Callout are NOT pointer-capture draws — they place an anchor then open an inline
screen-positioned `<textarea>`. Add to `Viewport.svelte`:

- [ ] **Step 1:** import `isTextTool, textBoxGeometry, calloutGeometry, DEFAULT_TEXT_FONT` from
  `$lib/markup-tools`; include `isTextTool(t)` in the `isCreateTool` predicate.
- [ ] **Step 2: editor state** —
```typescript
  // Inline text editor (Text/Callout). screenX/Y position the textarea over the overlay.
  let editor = $state<{ screenX: number; screenY: number; anchorPdf: { x: number; y: number };
    leaderPdf: { x: number; y: number } | null; value: string } | null>(null);
  let calloutTarget: { x: number; y: number } | null = null; // first Callout click (leader start)
```
- [ ] **Step 3: placement** — in `onOverlayClick` (which already early-returns for non-multi-click),
  add a text-tool branch BEFORE the multi-click logic:
  - `Text`: open the editor at the click — `anchorPdf = localPdfFromMouse(e)`, `leaderPdf = null`,
    `screenX/Y` from the event (container-local), `value = ""`.
  - `Callout`: first click sets `calloutTarget = anchorPdf`; second click opens the editor with
    `anchorPdf = <2nd click>`, `leaderPdf = calloutTarget`, then clears `calloutTarget`.
  Use a container-local screen position (clientX/Y − container rect) for `screenX/Y`.
- [ ] **Step 4: commit/cancel** — `commitEditor()`:
  - trim `editor.value`; if empty → `cancelEditor()` (no-op, per exit).
  - build appearance `{ ...store.draftAppearance, font: store.draftAppearance.font ?? DEFAULT_TEXT_FONT }`.
  - Text: `geometry = textBoxGeometry(editor.anchorPdf)`, `markupType = "Text"`.
  - Callout: `geometry = calloutGeometry(editor.leaderPdf!, editor.anchorPdf)`, `markupType = "Callout"`.
  - `store.create(buildMarkup({ ..., contents: editor.value.trim(), id: crypto.randomUUID(), now: new Date().toISOString() }))`.
  - `cancelEditor()` clears `editor` + `calloutTarget`.
  Commit on textarea **blur** and on **Cmd/Ctrl+Enter**; **Escape** cancels (and must not also
  trigger the global multi-click Escape — guard the global `onKeyDown` Escape with `if (editor) return;`).
- [ ] **Step 5: tool-switch reset** — extend the existing `$effect` that resets gesture state to also
  `cancelEditor()` and clear `calloutTarget`.
- [ ] **Step 6: render** — add `text` + `callout` branches to the `{#snippet shape}` block:
```svelte
      {:else if s.kind === "text"}
        <text x={s.x} y={s.y} fill={s.stroke} font-size={s.fontPx}
          dominant-baseline="hanging" opacity={s.opacity}>{s.text}</text>
      {:else if s.kind === "callout"}
        <polyline points={s.points} stroke={s.stroke} stroke-width={s.strokeWidth}
          fill="none" opacity={s.opacity} stroke-dasharray={s.dashArray ?? undefined} />
        <text x={s.x} y={s.y} fill={s.stroke} font-size={s.fontPx}
          dominant-baseline="hanging" opacity={s.opacity}>{s.text}</text>
```
  Render the `<textarea>` (when `editor`) absolutely positioned at `editor.screenX/Y` inside
  `.viewport-root`; `bind:value={editor.value}`; `onblur={commitEditor}`,
  `onkeydown` for Cmd/Ctrl+Enter (commit) and Escape (cancel); autofocus. Style via design tokens.
- [ ] **Step 7:** `npm run check` 0 errors; `npm run test` green; `cd src-tauri && cargo build` clean.
  Commit — `feat(ui): inline text editor + Text/Callout placement`.

#### Task 5.6: interaction tests (`Viewport.interaction.test.ts`)

- [ ] Append jsdom interaction tests (mirror the existing `mountViewport`/`ptr` harness):
  - **Text place + commit:** activeTool "Text"; click overlay; a `textarea` appears; set its value
    + dispatch `input`; dispatch `blur` (or Cmd+Enter keydown); assert one markup, type "Text",
    geometry `Rect`, `contents` set; assert a `text.markup-overlay`/`svg text` element renders the text.
  - **Empty text = no-op:** click, leave textarea empty, blur; assert `store.markups.length === 0`.
  - **Callout place:** activeTool "Callout"; click target, click anchor; textarea appears; type +
    commit; assert one markup type "Callout", geometry `Polyline` of 2 verts (exact PDF coords),
    contents set; assert overlay has a `polyline` + `text`.
  - **Font defaulted:** the created Text markup has `appearance.font` = Helvetica 12 (DEFAULT_TEXT_FONT).
  - **Glued-on-zoom (text):** place a Text; read the rendered `<text>` x/y; wheel-zoom; assert x/y and
    `font-size` scaled with zoom (no drift).
- [ ] `npm run test 2>&1 | tail -8` all green; commit — `test(g5): interaction tests for text + callout`.

**G5 exit:** all unit + interaction + Rust serde tests green + standing gates (vitest, `npm run check`
0 errors, `cargo test --test-threads=1`, clippy 0, `cargo fmt`). Save round-trip of Text/Callout
(incl. font) is covered by the Rust round-trip tests; external-viewer fidelity (Acrobat/Bluebeam) folds
into the G9 full-app smoke. Precise callout text-box `/Rect` (currently the leader bbox) is a noted
refinement, not a blocker.

### G6 — Select / move / resize / delete
- `markup-tools.ts`: `hitTest(markups, pdfPoint)` (topmost; bbox/segment distance) and `boundsOf(markup)` + `translateGeometry`/`scaleGeometryToBounds`. vitest.
- `Viewport.svelte`: Select tool — click hit-tests (`store.selectedIds`), shift adds; selection chrome (bbox + 8 handles) in the overlay; drag-inside = live move preview → one `UpdateCmd` on up; drag-handle = resize; Delete key = `store.delete`. Coalesced gesture → single command.
- **Exit:** select/move/resize/delete every drawn type; one undo per gesture; save round-trips.

### G7 — Properties panel
- `PropertiesPanel.svelte` (right column): bound to the single selection's `Appearance`+`contents`+`subject`+`layer`; edits build an `UpdateCmd` (clone + bump audit via identity + patch); with no selection, edits `store.draftAppearance`.
- A small `bumpAudit(markup, identity)` helper (revision+1, modified_*); vitest.
- **Exit:** change color/weight/opacity/fill/line-style/font + note text on a selection; undoable; save round-trips.

### G8 — Grouping (cut-line)
- Rust: add `group_id: Option<Uuid>` to `Markup` (default `None`, `#[serde(default)]`); `/RLGroup` key in `markup/annotation.rs` (to_dict / from_dict) round-trip; Rust test.
- TS: `group_id: string | null` on `Markup` (ipc.ts); group/ungroup commands (set/clear `group_id` across selection as a batch of `UpdateCmd`s); group-aware select (selecting one selects the group) + move.
- **Exit:** group/ungroup; group moves together; `/RLGroup` survives save→reopen (external-viewer ignores unknown key gracefully).

### G9 — Ship
- Manual GUI verification matrix (every tool; edit/move/resize; properties; comments; group/ungroup; undo/redo across mixed ops; Save → Acrobat/Bluebeam check; overlay glued at extreme zoom + page-nav).
- Update `.claude/HANDOVER.md` (S2 done) + tick roadmap.
- `/code-review` (render + annotation serde touchpoints), then `/sendit`.

---

## Self-review

**Spec coverage:** envelope mutation + per-op sync (G1/G2 ✓), undo/redo command stack (G2 ✓), identity (G1 ✓), all 5 interaction archetypes incl. Text/Callout (G3–G5 ✓), select/transform (G6 ✓), properties + comments (G7 ✓), grouping incl. `/RLGroup` persistence (G8 ✓), flush-before-save (G2.3 ✓), stamp explicitly deferred to S5 (spec ✓). No gaps.

**Placeholder scan:** G1/G2 tasks carry full code + exact commands + expected output. G3–G9 are intentionally task-map level (detailed JIT, per the foundation-first decision stated up top) — not placeholders inside an executable task.

**Type consistency:** `MarkupSink`/`MirrorOp`/`Command` defined in Task 2.1 are consumed identically in Task 2.2; `MarkupStore` implements `MarkupSink` (insert/replace/removeById/getById) exactly as the History expects; `MarkupIpc` (add/update/remove) matches the injected `{add: addMarkup, update: updateMarkup, remove: deleteMarkup}` in Task 2.3; Rust `update`/`delete` signatures match the `update_markup`/`delete_markup` commands; `Identity` serde shape (`user_id`/`display_name`) matches TS `UserRef`.
