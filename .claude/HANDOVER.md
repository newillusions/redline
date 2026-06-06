# redline — Handover Notes

## Current Status
New project — bootstrapped, not yet started building.

## Setup
Bootstrapped by the workspace orchestrator on 2026-06-06.
- v1 spec copied to `docs/bluebeam-alternative-v1-spec.md` (authoritative — read first).
- Git initialized; remote `emittiv/redline` (SSH).
- Requirements are still being actively developed with the user (in the orchestrator conversation) — the spec will continue to evolve.

## Next Steps
1. `/lamp-on`
2. `/onboard` (self-audit and fill gaps — incl. building the `/sendit` skill)
3. Read `docs/bluebeam-alternative-v1-spec.md` end to end.
4. Begin **M1**: Tauri 2 + Svelte 5 shell with the 3-column dockable layout (spec §17); PDFium tiled render; open/pan/zoom a large PDF smoothly. Validate against 300 MB+ plan sets early — that is the make-or-break performance gate (spec §5/§11).

## Key Context
- Precedent Tauri+Rust+Svelte projects to mine: `e-fees`, `cad-export`.
- Precision invariant: display raster vs vector geometry are independent layers; measurement math in PDF user space at f64 (spec §5).
- `.btx` import is a v1 requirement and reuses the annotation parser (spec §6).
