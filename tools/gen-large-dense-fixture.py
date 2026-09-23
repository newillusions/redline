#!/usr/bin/env python3
"""Generate e2e/fixtures/large-dense.pdf - a 40-page fixture with dense vector line/rect
content per page, used by e2e/specs/perf-tool-lag.spec.js (wdio.perf.conf.js) to stress the
per-page vector snap-target index (src-tauri/src/geometry/mod.rs build_snap_index) at a
scale closer to a real dense architectural sheet than the 1-page e2e-sample.pdf.

Requires pymupdf (`pip install pymupdf`). Deterministic (seeded) - re-run to regenerate the
committed fixture if the shape needs to change; do not hand-edit the PDF bytes.
"""
import pymupdf
import random

random.seed(42)
doc = pymupdf.open()
PAGES = 40
W, H = 842, 595  # A4 landscape-ish, in points

for pi in range(PAGES):
    page = doc.new_page(width=W, height=H)
    # 60 lines + 20 rects/page = ~260 Endpoint/Midpoint snap targets/page (60*3 + 20*8).
    for _ in range(60):
        x0, y0 = random.uniform(20, W - 20), random.uniform(20, H - 20)
        x1, y1 = random.uniform(20, W - 20), random.uniform(20, H - 20)
        page.draw_line((x0, y0), (x1, y1), color=(0, 0, 0), width=0.5)
    for _ in range(20):
        x, y = random.uniform(20, W - 100), random.uniform(20, H - 40)
        w, h = random.uniform(20, 80), random.uniform(10, 40)
        page.draw_rect(pymupdf.Rect(x, y, x + w, y + h), color=(0, 0, 0), width=0.5)
    page.insert_text((20, 20), f"Sheet A-{pi:03d} - dense line-work test page", fontsize=8)

doc.save("e2e/fixtures/large-dense.pdf")
print("pages:", doc.page_count)
