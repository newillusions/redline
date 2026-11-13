# Snaps (as of 2026-11-13)

Point capture for the snap-eligible tools (calibrate, Line, Arrow, Polyline, Polygon, Cloud,
measurement tools) snaps to vector geometry within 8 screen pixels.

| Kind | Source | Where |
|---|---|---|
| Endpoint | every path vertex (on-curve points only) | Rust `geometry::build_snap_index` |
| Midpoint | each segment; a cubic Bezier at t = 0.5 | Rust |
| Intersection | proper crossings of straight segments (not shared ends) | Rust, rstar candidate pairs, capped |
| ArcCenter | circle fitted to >= 2 consecutive cubic segments | Rust |
| Quadrant | compass points of a fitted circle that the arc reaches | Rust |
| Perpendicular | foot of the perpendicular from the line's start point | TS `findBestSnap` |
| Nearest | closest point on a straight segment | TS `findBestSnap` |

Ranking: kind priority dominates, distance breaks ties within a priority:
Endpoint / ArcCenter (0), Intersection (1), Midpoint / Quadrant / Perpendicular (2), Nearest (4).

Limits:
- PDFium reports a cubic Bezier as three consecutive `BezierTo` points (two control points
  and the end). The extractor folds these into curve segments, so control points are no longer
  snap vertices (previously they leaked in as Endpoints).
- A single cubic (rounded corner) is never fitted as an arc; two or more consecutive cubics
  whose end points and t = 0.5 points lie on one circle (tolerance max(0.25 pt, 2% of r)) are.
- Ellipses are not fitted (no centre snap).
- Intersection search stops at 4,000,000 candidate pairs or 200,000 targets per page; a
  pathological page then has fewer intersection snaps, never a slower interaction.
- Perpendicular and Nearest use straight segments only (curve pieces excluded).
- Polar/extension tracking is not implemented.

Attribution: approach adapted from redcolumn (Apache-2.0); see `NOTICE`.
