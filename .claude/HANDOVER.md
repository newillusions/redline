# Redline - Handover Notes

## Current Status

**M1-M6 + Phase 1.1 (compare) + Windows-distribution infra + Tool Chest v0.3.1 polish
(PR #48) + S2b client entitlement (PR #49) + docops/highlight bugfix batch (PR #50) all
merged to `main`. 0 open PRs.**

## Last Session

**Date**: 2026-07-08 (PR #50, dispatched by the orchestrator - 4-bug live-use batch)
**Summary**: Root-caused and fixed 3 of 4 reported live-use defects, all traced to one
ordering bug in `apply_page_edit` (`commands/document.rs`): it called
`write_markups(doc, markups)` AFTER `op(doc)`, not before. This defeated
`flatten_document` (baked+removed annotations, then write_markups immediately re-added
fresh live copies - "Flatten does nothing"), `optimize_document` (compressed all streams,
then write_markups added brand-new uncompressed appearance streams on top - "Optimize does
nothing"), and caused the reported "background artifact on move" bug (`store.flush()`
only drains to the in-memory Rust mirror, never writes the file; flatten's `Document::load`
still read the stale on-disk position, baking a permanent ghost at the old spot while
write_markups added a correctly-positioned live annotation). **Fix**: reorder to
write_markups first, then op - also the correct order for the existing page-restructuring
ops (rotate/delete/reorder/insert always needed op to see the CURRENT markup state, not a
stale one). Extracted `apply_edit_and_save` as a pure function (no Tauri `State`/render
engine) for direct file round-trip testing; confirmed each new test fails against the old
ordering and passes against the fix (temporarily reverted, re-verified, restored).
4th defect (highlight "not using text selection") is NOT a regression - `selectText` (I-beam)
tool -> Enter -> `commitTextSelectionHighlight` is fully wired with real PDFium text-range
selection, already tested. It's a discoverability gap: a separate freeform-rectangle
"Highlight" tool sat 5 slots away from "Select Text" in the toolbar, and Acrobat/Bluebeam
users expect "Highlight" itself to snap to text. Fixed via toolbar reorder (Select Text now
immediately after Highlight) + tooltip clarification only - zero behavior change to either
tool. Verified: `cargo test` 380 passed/1 pre-existing ignored (5 new), `cargo clippy
--all-targets` 0 new warnings, `npm run check` 0 errors, `npm test` 634 passed/34 files
(1 new). PR #50 squash-merged as `02a4e5d8decacc003815a9232ccd1616dffb8cd4`, CI green
(run #129).
**Not touched**: release/auto-updater/minisign manifest (out of scope per dispatch).
**Owed**: live re-test of Flatten/Optimize/Redact buttons + move-then-flatten in the real
app - the GUI harness (`tools/gui-harness.mjs`) mocks Tauri IPC with no
flatten/optimize/redact handlers, so it can't exercise this backend PDF-persistence bug;
file round-trip tests are the correct automated verification but a human GUI pass is still
the final word.

### Previous session (2026-07-08, PR #49)
**Summary**: Implemented S2b - redline gates on a valid, device-bound Ed25519 token from
the emittiv-staff license service (S2a). New `src-tauri/src/license/` module: `token.rs`
offline-verifies the compact `<payload>.<signature>` token, mirroring emittiv-staff's
`verifyToken` field-for-field (Ed25519 signs the raw base64url payload segment, not the
re-serialized JSON); `gate.rs` layers device-binding + a 3-day renew-due window on top,
pure/IO-free; `device.rs`/`store.rs` persist the per-install device fingerprint and
last-issued token (atomic temp+rename writes, same pattern as `storage::settings.rs`);
`client.rs`/`service.rs` split the network issue/renew calls from orchestration (mirrors
emittiv-staff's `license-service.ts` DbLike injection) so activate/renew are unit-testable
against a fake `LicenseClient`. Three new Tauri commands (`license_status`,
`activate_license`, `renew_license`). Frontend: `ActivationGate.svelte` blocks the whole
app shell until `license_status` reports valid; `App.svelte`'s onMount side effects
(recent docs, auto-open, drag-drop) now run only after the gate passes. An offboarded
staff record refuses renewal, but the already-issued token keeps gating on its own expiry
- that window is the intended grace period. Verified: `cargo test` 375 passed/1
pre-existing ignored (23 new license tests), `cargo clippy --all-targets` 0 new warnings,
`npm run check` 0 errors, `npm test` 633 passed/34 files (8 new). PR #49 merged as
`de1f8c20040eb06a297e60f5c647b92c8af28e02`, CI green (run #126).
**Deferred**: `REDLINE_LICENSE_API_URL` is unconfigured - emittiv-staff's license service
(S2a) has no deployment yet (no Dockerfile/URL). Live e2e (real activation code -> real
Tauri build) is owed once it deploys.

### Previous session (2026-07-08, PR #48)
**Summary**: Tool Chest v0.3.1 polish - true PNG-backed stamp appearance (real `/AP /N`
Image XObject + SMask, not box+label), dynamic stamp local-timezone dates, a
`StampPromptDialog` for `PromptedText` fields, drag-to-reorder in the Tool Chest panel.
`appearance::build_ap_stream` split into a pure `build_ap_stream` + `finish_ap_stream` so
the `Document`-owning caller resolves auxiliary Image XObjects into real indirect refs.
Verified: `cargo test -p redline` 352 passed/1 ignored, `cargo test -p pdf-diff` 7 passed,
`cargo clippy` 0 new warnings, `npm run check` 0 errors, `npm test` 625 passed (33 files).
Live GUI confirmation (stamp rendering in Acrobat/Bluebeam, prompt-dialog flow, drag feel)
still owed to a human session. Detail: `obs:e1tujicl7p4uck906rxa`.

## Next Steps

1. **Live-verify PR #50's docops/highlight fixes**: click Flatten, Optimize, and Apply
   Redactions in the real app on a document with markups (including one moved but not
   explicitly saved before flattening) and confirm the visible fix; confirm the toolbar
   now shows Select Text immediately after Highlight.
2. **Live-verify PR #48's Tool Chest polish**: a placed PNG stamp actually renders its
   graphic in Acrobat/Bluebeam (not a box+label), the local-tz date/time on a dynamic
   stamp, the `PromptedText` dialog end-to-end, and drag-reorder feel in the Tool Chest
   panel.
3. **S2b live e2e**: once emittiv-staff's license service is deployed, set
   `REDLINE_LICENSE_API_URL` and run the activation flow (code -> issue -> gate -> renew)
   through a real Tauri build.

Before the first tagged Windows/macOS release:

4. **Orchestrator: generate the redline minisign keypair** (`tauri signer generate`) and
   replace the placeholder `pubkey` in `src-tauri/tauri.conf.json` (currently decodes to an
   "untrusted comment: PLACEHOLDER..." block - clearly non-functional by design).
5. **Orchestrator: create GitHub mirror repo** `newillusions/redline` and add secrets
   `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `GITEA_TOKEN` (same
   names/convention as e-fees).
6. **Push tag `v0.2.0`** (current `Cargo.toml`/`package.json` version) once 4-5 are done,
   to trigger and verify the first release build end-to-end (especially the Windows leg,
   which this session could not run locally).
7. **Open decision, not yet made**: CLAUDE.md says "macOS (universal)" but PR #33 ships
   e-fees-style separate aarch64/x86_64 artifacts, not a combined universal binary - needs
   an explicit call from Martin/orchestrator.
8. **§20 definitive floor-machine run** (16 GB RAM, Windows + macOS) - the formal M1
   Go/No-Go, still owed, blocked on hardware access. Procedure: `bench/RUNBOOK-S20.md`.
9. **G9 human visual check** - regenerate the sample via `cd src-tauri && cargo test
   g9_emit_sample -- --ignored --nocapture`, open in Acrobat AND Bluebeam. Owed since M2.
10. **Project direction beyond polish** (pause / registration fast-follow / next milestone)
    is an owner-gated decision on Martin's business backlog - not yet made, don't infer one.

## Housekeeping flagged for the orchestrator

- `.claude/worktrees/` still has untracked, uncommitted agent-worktree directories (flagged
  2026-07-02, still present 2026-07-08). Not part of this task; flagging again for cleanup
  since it's untracked disk clutter in the repo root.

## Key Context

| Item | Value |
|------|-------|
| Remote | `git@ssh.forge.mms.name:emittiv/redline.git` |
| Main branch | `main` @ `02a4e5d` (M1-M6 + Phase 1.1 + Windows-dist infra + Tool Chest polish + S2b + docops/highlight bugfix batch merged) |
| KB mission record | `project:q8gm8dv3k7smld12rm25` (stage: stabilizing, health: on_track) |
| Ship pipeline | `.claude/skills/sendit/SKILL.md` |
| Judgment rules | `.claude/rules/judgment.md` (2026-07-02 - incident/decision distillation) |
| PR #48 | `https://forge.mms.name/emittiv/redline/pulls/48` (Tool Chest v0.3.1 polish - merged `7f4a36b`) |
| PR #49 | `https://forge.mms.name/emittiv/redline/pulls/49` (S2b client entitlement - merged `de1f8c2`) |
| PR #50 | `https://forge.mms.name/emittiv/redline/pulls/50` (docops write-markups-ordering + highlight discoverability fix - merged `02a4e5d`) |
| S2b license contract | `emittiv-staff/src/lib/server/license.ts` (authoritative token shape - do not change without a hub message) |

## Key Gotchas (carry forward)

- **`FolderIndex::alive()`** = `Arc::strong_count(&self.inner) > 1` - background watcher thread exits within ~1s of AppState replacing the index
- **Background indexer uses `std::thread::spawn`** (not tokio) - watcher loop is indefinitely blocking, must NOT consume tokio's blocking thread pool
- **Tantivy `Document` trait must be imported** for `to_json()` to be in scope: `use tantivy::{Document, ...};`
- **Svelte store is in-session SoT**; Rust store is a mirror + save buffer. `flush()` throws on undrained mirror queue.
- **lopdf reals: read with `as_float()`, NEVER `as_f32()`** - see `.claude/rules/judgment.md` for the full incident (integer-valued reals silently corrupt on save→reopen)
- **lopdf borrow checker pattern**: immutable read phase (collect owned structs) then mutable write phase - avoids aliasing on `&mut Document`
- **lopdf `Stream::compress()` threshold**: only applies Deflate when `compressed.len() + 19 < original.len()` - streams shorter than ~50 bytes typically don't compress
- **M5 flatten v1 limitation**: only handles indirect /AP /N appearance streams; inline /AP /N streams (rare) are preserved as-is
- **M5 optimize v1 limitation**: no deep image downsampling (spec §8 - deferred to pluggable engine)
- **Recent docs IPC**: lives in `src/lib/recent-docs.ts` (NOT `ipc.ts`) - intentional conflict-avoidance pattern, see judgment.md
- **License IPC**: lives in `src/lib/license.ts` (NOT `ipc.ts`) - same conflict-avoidance pattern
- Overlay `pointer-events` toggles via `isCreateTool()`; Hand tool pans, creation tools capture on SVG overlay
- §5 precision invariant: overlay maps PDF user space → screen every render (never reads raster)
- **`next_version_seq`** in `SidecarMeta` is monotonic - increment BEFORE deriving seq; don't revert to `versions.len()+1` (breaks after prune)
- PDFium 2 GiB limit, global C state, `RenderEngine` drop order - unchanged from M1
- **`appearance::build_ap_stream` is Document-free/pure** - it returns an `ApBuild` (bbox +
  content + resources + any auxiliary `StampImageXObject`s, unresolved). Only
  `annots::write_markups` calls `finish_ap_stream` after resolving those into real indirect
  objects (PDF streams must be indirect - spec 7.3.8). Don't add a Document param back onto
  `build_ap_stream` itself - that's what makes it test-friendly.
- **License public key parsing**: the baked `LICENSE_PUBLIC_KEY_PEM` is Ed25519 SPKI-DER;
  `token::parse_public_key_pem` strips a fixed 12-byte RFC 8410 prefix rather than pulling
  in an ASN.1 crate - do not "simplify" this into a generic X.509 parser, the fixed-prefix
  trick only works because it's specifically an Ed25519 SPKI key.
- **`apply_page_edit`/`apply_edit_and_save`** (`commands/document.rs`): writes markups
  into the loaded doc BEFORE running `op` (flatten/optimize/redact/rotate/delete/reorder/
  insert), never after - see the `apply_edit_and_save` doc comment. `op` always needs the
  CURRENT markup state, and nothing may run after it that could re-add/regenerate what it
  just baked or compressed. This was reversed until PR #50 (2026-07-08) - the bug and its
  full root-cause chain are documented there and in `obs:mwen68znlue4jfrzewxb`.
- Tests: `npm run test` (vitest, mixed node+jsdom). Rust: `cargo test` from `src-tauri/` (not project root)

---
*Updated: 2026-07-08 (docops/highlight bugfix batch, PR #50)*
