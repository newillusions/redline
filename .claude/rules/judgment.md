# Redline Judgment Rules

Distilled 2026-07-02 from incidents, corrections, and decisions in this project's history.
"When X, do Y, because Z" — the how-to-think layer above CLAUDE.md's facts and HANDOVER.md's
carry-forward gotchas. Don't duplicate those; this file is for judgment calls, not API facts.

## Verification

- **When declaring a perf/scale claim "passed" or a milestone "gated through"**, label it
  indicative (Apple Silicon dev Mac, headless bench) vs definitive (16 GB floor machine,
  Windows + macOS, GUI interactive) and never conflate them. The only §20 verdict that exists
  today is indicative; the definitive floor run is still owed and is the actual M1 Go/No-Go
  (obs:c81hlst1z1piolr46fxs).
- **When touching the render path, markup serde, or geometry, or before calling a milestone
  shipped**, run the GUI harness smoke (`tools/gui-harness.mjs`, see `Redline Visual
  Verification Procedure`) — not just `cargo test`/`vitest`. Six M1 render-loop bugs (IPC
  casing, flipped matrix, DPR scaling, zoom runaway, sub-pixel seams) were all invisible to
  headless tests and only surfaced on a real `cargo tauri dev` session
  (obs:uzqmi1zvl1e2hibsint5, obs:nx5nqon8k8xrty2vljsz). The human Acrobat/Bluebeam save check
  is a separate, still-owed step — don't treat the automated G9 round-trip test as a
  substitute for it.
- **When a user directive marks an edge case as a required v1 use case** (e.g. dense A0 sheets,
  >2 GB scanned sets, 2026-06-07), don't unilaterally downgrade it to a "documented limitation"
  if it turns out hard — it stays a blocking requirement until fixed or the user revises it
  (obs:jlgh7mlgkqeyeepculg7).

## PDF / Rust internals

- **When reading any PDF numeric field via lopdf**, use `.as_float()`, never `.as_f32()`.
  Integer-valued reals (e.g. `3.0`) serialize without a decimal point and reload as
  `Object::Integer`; `.as_f32()` only matches `Object::Real` and silently drops the value on
  save→reopen. This bug hid in dict-level round-trip tests because they never hit lopdf's text
  serializer — only a real file-save test catches it (obs:etkspf90cxwy6dpqk03r).
- **When annotation/markup reads are slow on large files**, don't reach for pdfium-render's
  private annotation accessors to bypass lopdf — they're `pub(crate)` and unreachable without
  `unsafe`. lopdf has no lazy/partial load (0.36), so a full parse of a 150 MB/691pp file costs
  ~96s; the shipped mitigation is an mtime-keyed cache in MarkupStore, not a different read
  path (obs:l5km49kpebmkfbk3nb1w).
- **When declaring struct fields that own FFI/dylib-backed resources** (PDFium docs, mmaps,
  the library handle itself), order fields so owned resources drop before the handle that
  owns their lifetime — Rust drops fields in declaration order, and the library unloading
  before its documents close is a SIGSEGV at teardown, not a compile error
  (obs:n1x6kvpkpp02e5d2x863).

## Frontend / IPC

- **When adding a new Tauri v2 `invoke` call**, camelCase every multi-word arg key in
  `ipc.ts` — Tauri maps camelCase JS keys to snake_case Rust params, and this only breaks on
  multi-word commands (single-word args look fine because camel==snake). The vitest mock
  never catches this; it only surfaces in a live GUI session. Add a casing-assertion test
  alongside any new IPC wrapper (obs:me9oo7nq06hvpbne926f).
- **When adding a markup geometry/shape kind**, check whether it needs an explicit special
  case in `markupToSvg`/`SvgShape` before it falls through to the generic Rect/Polyline
  branch — Ellipse silently rendered as a rectangle for exactly this reason. Mirror the
  existing Arrow-before-Polyline special-case pattern (obs:qrju44oolawd8f2ovans).
- **When two feature branches touch overlapping UI surface concurrently** (e.g. a new panel
  vs. an in-flight markup-model PR), route the new feature's IPC/state into its own file
  (`src/lib/recent-docs.ts`, not `ipc.ts`) rather than editing the shared hot file — this is
  how PR #29 avoided conflicting with PR #28's `ipc.ts`/`Viewport.svelte`/`MeasurementPanel.svelte`
  changes. Note the conflict zone in the branch's own commit/PR description so the next
  session doesn't have to reconstruct it.

## Shipping

- **When shipping via `/sendit` as a background pipeline agent**, don't rely on
  `mode: "bypassPermissions"` to clear a session-level Bash denial on a sub-agent — it
  doesn't override it, and the pipeline stalls silently at the first Bash call. Confirmed
  twice (2026-06-14 S1 ship, 2026-06-16). Run `/sendit --dry-run` first if unsure, and expect
  to run the pipeline in the foreground if a background stall recurs
  (obs:6s1iwnoeppezf4ll5qsr).
