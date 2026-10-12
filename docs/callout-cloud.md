# Callout, Cloud and Cloud+ - behaviour spec

Status as of 2026-10-12. Built to this document; the same text is in the PR body.

Labels on every row:

- **V** verified: Revu behaviour seen in a Bluebeam tutorial video; the source is the scout report
  `scout-revu-behaviour.md` section 2.5 (video id `@seconds` shown). No Bluebeam software was
  decompiled or instrumented.
- **I** inferred: reasoned from verified facts, a standard, or a forum source; the basis is named.
- **U** unknown: nobody has seen it; redline picks a behaviour and says so.

"redline" column: **yes** = built and tested here, **no** = not built (listed under Follow-ups).

## 1. Callout (PDF FreeText `/IT /FreeTextCallout`)

A callout is a text box plus one or more leaders (arrows) from the box to things on the page.

### Gestures

| Behaviour | Label | Source / basis | redline |
|---|---|---|---|
| Two clicks: first click is the arrow tip, second click places the text box, then type | V | `BbBylKOedOw@93-103` | yes (existing; the box is now sized to the text) |
| Press at the tip, drag, release at the box position, then type | V | `564PIJdWHjg@22-47` ("click and drag ... Revu will prompt you to add text") | yes: the release arms the tip and the click after it opens the editor |
| The two descriptions above are both accepted by Revu | I | they differ; both are plausible for one tool | yes (both work) |
| New callout has a knee (the leader bends once) | V | `m12ozrH62zI@65-72` names the knee | yes |
| Where the knee starts: on a short run straight out of the box side, 18 pt | I | Revu draws a bend by default; the exact default is not stated | yes |
| Leader attaches to the middle of the box side that faces the tip (left/right, else top/bottom) | I | not stated | yes |
| Drag the knee to bend the leader | V | `m12ozrH62zI@65-72` | yes |
| Drag the arrow tip | V (arrows are draggable) / I (handle look) | `m12ozrH62zI@129-146` | yes |
| Shift+click on the knee adds another leader (Revu: hover the knee, hold Shift, cursor shows "+", each click adds one) | V | `m12ozrH62zI@92-120` | yes (the new leader starts at the knee and the same drag places its tip). The "+" cursor is not drawn. |
| Right-click the knee > Leader adds a leader | V | `m12ozrH62zI@72-82` | no (no context menu on markups yet) |
| Shift+click an arrow tip removes that leader (Revu: Shift over an arrow, cursor shows "-", click removes) | V | `m12ozrH62zI@129-146` | yes; the last leader cannot be removed. The "-" cursor is not drawn. |
| Right-click an arrow > delete removes it | V | `m12ozrH62zI@129-146` | no (no context menu) |
| Removing the first leader promotes the next one | U | not stated | yes (redline's choice) |
| When the box moves, its leaders follow | V | `m12ozrH62zI@84-90` | yes: dragging the box moves the box and every leader's attach point; tips and knees stay. Moving the whole markup moves everything. |
| Drag the box body to move the box | I | Text Box behaviour `KgPuPBH_rbI@97-113` ("drag body to move") | yes |
| Eight resize handles on the box | I | Text Box behaviour `KgPuPBH_rbI@97-113` ("drag handle to resize") | yes; minimum side 12 pt |
| Double-click edits the text | I | redline convention | yes (existing) |
| Delete key removes the selected callout | V (Delete for markups) | `FRreLAUjUb4@82-84` (Cloud) | yes (existing) |
| Leader count cap | n/a | redline safety limit | 17 leaders in all (1 + 16 extra), 3 points each |

### Text box size

| Behaviour | Label | Source / basis | redline |
|---|---|---|---|
| Revu has an "Autosize Text Box and Callout Markups" preference and an auto-size command | V (exists) | MCP `markup_function` "Auto-size text box markups"; ATG Revu Toolkit snippet (preference path) | partly |
| Alt+Z autosizes one markup | U (low confidence, web snippet only) | not verified | no |
| What exactly auto-size does (wrap, padding, grow direction) | U | not stated | redline's choice: box fits the text (no wrapping, one line per newline, average glyph width 0.55 em, 4 pt / 2 pt padding), keeping the top-left corner |
| New callouts start auto-sized | I | the preference name suggests it is a default behaviour | yes |
| Editing the text (inline editor, Properties "Contents") or the font size refits an auto-size box | I | follows from auto-size | yes |
| A manual box resize turns auto-size off; Properties has an "Auto-size box" checkbox to turn it on again | I | redline's choice so that a manual size is kept | yes |

### Properties and defaults

| Behaviour | Label | Source / basis | redline |
|---|---|---|---|
| Properties: line colour, thickness, fill, font, contents, subject | V (Properties exists) | right-click menu `564PIJdWHjg@81-108` | yes (existing panel) |
| Default font Helvetica 12, border = line colour, fill none, arrow style, box size | U | not in any caption | redline's existing defaults |
| Right-click menu: Properties, Add to Tool Chest, Reply, Status | V | `564PIJdWHjg@81-108`, `mJZz8TMFyIw@146-152` | no |
| Keyboard shortcut for the tool | U | letter lost in captions | none |

### What is written to the PDF

| Item | Key | Notes |
|---|---|---|
| Annotation | `/Subtype /FreeText`, `/IT /FreeTextCallout` | standard (ISO 32000-1 12.5.6.6) |
| Primary leader | `/CL` with 4 numbers (tip, attach) or 6 numbers (tip, knee, attach) | tip first, as the standard defines it |
| Line ending | `/LE /ClosedArrow` | matches the arrowhead drawn |
| Text box | `/Rect` minus `/RD` (left, top, right, bottom insets) | standard; other viewers read the box from it |
| Text box, exact | `/RLCalloutBox [x0 y0 x1 y1]` | private; the f32 `/Rect` minus `/RD` is not exact, so redline reads this first |
| Extra leaders | `/RLCalloutLeaders [[x y x y (x y)] ...]` | private; **also drawn into `/AP`**, so every viewer shows them |
| Auto-size | `/RLCalloutAuto true` | private |
| Appearance | `/AP /N` draws every leader with its arrowhead, then the box and text | `/Rect` equals the `/AP /BBox` (union of all leaders and the box) |

How Revu stores several leaders is **U**: no source was found (not in the interop scout, not in the Raw
`/IT /FreeTextCallout` forum sample, which shows one callout). redline therefore keeps extra leaders in
the private key above and in the appearance stream. Consequences, stated plainly:

- Opening a Revu callout reads its `/CL` as the primary leader and its box from `/Rect` + `/RD`. If Revu
  stores extra leaders in some key redline does not know, that data is dropped on redline's re-save
  (the existing rule: only allow-listed foreign keys are carried).
- Revu or Acrobat editing a redline callout may drop `/RLCalloutLeaders` (they ignore unknown keys but
  regenerate appearances); the extra leaders then disappear from the file.
- `/CL` is capped at 3 points. A foreign `/CL` with more is reduced to first, middle and last.

A callout with no `callout` data (files from before this change, or a foreign callout without `/RD`)
keeps the old behaviour: a 144 x 18 pt box at the last leader point. The first edit that moves or
resizes its box (or adds a leader) gives it explicit box data; the box is then where the editor shows it
(top-left at the old anchor). The old saved-PDF appearance drew that legacy box above the anchor; edited
callouts no longer have that mismatch. Dragging only a tip or knee does not add box data.

## 2. Cloud

| Behaviour | Label | Source / basis | redline |
|---|---|---|---|
| Key Y selects the tool | V | `FRreLAUjUb4@27-30` | yes |
| Press-drag a rectangle; the cloud is created on release | V | `FRreLAUjUb4@35-44` | yes: 4-vertex polygon, any drag direction; a drag under 5 screen px is a click; a rectangle side under 4 pt is rejected |
| Click anchor points; finish by placing the last anchor on the first | V | `FRreLAUjUb4@46-61` | yes: a click within 8 px of the first vertex (after 3 vertices) closes the shape. Double-click and Enter still finish, as before. |
| The click after a drag-created cloud is ignored | I | needed so the release does not also add a vertex | yes (300 ms guard) |
| Snap-to-content affects placement | V | `FRreLAUjUb4@12-17` | yes (Cloud and Cloud+ use redline's existing vector snap) |
| Delete key removes it | V | `FRreLAUjUb4@82-84` | yes (existing) |
| Appearance: line colour, fill transparency, line thickness | V | `FRreLAUjUb4@61-80` | yes (existing panel) |
| Esc cancels an unfinished shape | I | redline convention | yes (existing) |

PDF: `/Subtype /Polygon`, `/Vertices`, `/BE << /S /C /I 2 >>` (cloudy border, intensity 2), `/IT /PolygonCloud`
(existing). Intensity is fixed at 2; a foreign `/BE /I` is not kept (**follow-up**). The saved appearance is a
plain polygon; redline's own viewer draws the scallops.

Bounded work: the screen scallop count is capped at 4000 (a larger cloud gets proportionally larger
scallops), at most 2000 vertices are walked, and non-finite vertices or radius give no path.

## 3. Cloud+

| Behaviour | Label | Source / basis | redline |
|---|---|---|---|
| Key K selects the tool | V | `FRreLAUjUb4@93-100` | yes |
| Same cloud gestures as Cloud (drag rectangle, or click the anchors) | V | `FRreLAUjUb4@93-100` | yes |
| After the cloud, click again to place the callout text box, then type | V | `BbBylKOedOw@176-196`, `FRreLAUjUb4@112-116` | yes |
| The leader runs from the cloud to the text box | I | "cloud-like polygon and a callout box" | yes: the tip is the closest point on the cloud outline to the box click |
| Reposition the text box with Shift+drag | U (ambiguous caption, `FRreLAUjUb4@123-125`) | | no (see below) |
| Font controls only on the text part | V | `FRreLAUjUb4@144-150` | yes (the callout carries the font) |

Representation (**I**; how Revu stores Cloud+ is U): a Cloud markup and a Callout markup in one group
(`/RLGroup`, plus the existing real `/IRT` + `/RT /Group` group link). Both are standard annotations
that every viewer shows. They are created together as one undo step. Selecting either selects both, so
the text box is moved by ungrouping first (Ctrl/Cmd+Shift+G); direct Shift+drag of the box inside a
Cloud+ is a follow-up.

## 4. Interop and safety rules kept

- Foreign annotations keep identity (`source_ref`, `source_nm`); reader-owned fields never come from IPC.
- Only allow-listed foreign keys are carried; `/IT`, `/LE`, `/RD` are written from the model.
- Load/save is idempotent over 3 cycles with Revu-style 16-char `/NM` (test below).
- Untrusted geometry is bounded: non-finite or oversized box -> legacy box; at most 16 extra leaders,
  3 points per leader; a 200-point `/CL` is reduced to 3; 5000 leaders in a PDF are read as 16;
  non-finite leader points are dropped.

## 5. Tests

Rust (`src-tauri/src/markup/callout.rs`, `appearance.rs`):
`writes_the_standard_callout_keys_with_a_knee`,
`redline_callouts_are_unchanged_by_three_save_load_cycles`,
`a_revu_callout_keeps_its_text_box_name_and_leader_over_three_cycles`,
`hostile_props_and_leaders_are_bounded_and_never_panic`,
`callout_draws_one_arrowhead_per_leader_through_the_knee`,
`callout_bbox_covers_every_leader_and_the_explicit_box`,
`callout_with_hostile_props_falls_back_to_the_legacy_box_without_panicking`.

Frontend: `src/lib/callout.test.ts` (geometry, knee, leaders, box, auto-size, cloud helpers, bounded
`cloudPath`) and the `Viewport callout + cloud` block in `Viewport.interaction.test.ts` (C1-C6, K1-K3, H1).

## 6. Follow-ups (not in this change)

- Right-click menu on markups (Leader, delete arrow, Properties, Reply, Status).
- "+" / "-" cursors while Shift is held over a knee / tip.
- Alt+Z autosize command and an "Autosize" application preference.
- Direct Shift+drag of the Cloud+ text box without ungrouping.
- Keep a foreign `/BE /I` cloud intensity, and a per-cloud intensity control.
- Find out how Revu stores several leaders and Cloud+ (needs a Revu-made sample file) and align.
- Keyboard shortcut for Callout (letter unknown).
- Redline-drawn cloud scallops in the saved appearance (today a plain polygon).
