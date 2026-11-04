#!/usr/bin/env node
// Real-browser check of measurement viewports (UX benchmark must-fix 7, per-area scales,
// docs/measurement-viewports.md): the real Svelte app in headless Chromium over a mocked
// Tauri IPC; the document is calibrated 1:100 (metres).
//
// Checks:
//   1. the Viewport tool is in the palette; dragging a rectangle opens the New Viewport
//      dialog; Save (default 1:20) calls add_viewport with the page and the drawn rectangle;
//   2. the viewport is outlined on the page with its name and scale while a measuring tool
//      is active, and is listed in the Measurements panel;
//   3. a length measured from INSIDE the viewport is taken under its scale (1:20), one from
//      OUTSIDE under the page's 1:100 - the scale is picked by the first point, so a line that
//      starts inside and ends outside still uses the viewport;
//   4. the earlier outside measurement keeps its value (no silent recalculation).
//
// Run:  npx vite --port 1431 --strictPort      (serves the frontend, separate terminal)
//       HARNESS_URL=http://localhost:1431/ node tools/gui-harness-viewports.mjs
//   5. (review of PR #166) a Redraw that was cancelled by picking another tool does not hijack
//      the next New viewport drag; a Redraw finished on ANOTHER page moves the viewport to
//      that page (update_viewport carries the page);
//   6. a failed create is shown in the error banner.
//
// Exits 0 and prints PASS on success; exits 1 with FAIL lines otherwise.
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";

function installMockTauri() {
  const PAGES = 6;
  const PAGE_W = 612;
  const PAGE_H = 792;

  function tilePng() {
    const c = document.createElement("canvas");
    c.width = 64;
    c.height = 64;
    const g = c.getContext("2d");
    g.fillStyle = "#eef3ff";
    g.fillRect(0, 0, 64, 64);
    return { w: 64, h: 64, b64: c.toDataURL("image/png").split(",")[1] };
  }

  const MARKUP = {
    id: "m1", markup_type: "Rectangle", page: 0,
    geometry: { Rect: { min: { x: 100, y: 300 }, max: { x: 250, y: 400 } } },
    appearance: { color: "#e02424", line_weight: 3, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null },
    subject: null, layer: null, contents: null, group_id: null,
    audit: { created_by: { user_id: "u", display_name: "H" }, created_at: "2026-01-01T00:00:00Z", modified_by: { user_id: "u", display_name: "H" }, modified_at: "2026-01-01T00:00:00Z", revision: 1, origin: "Desktop" },
    workflow: { status: "None", assignee: null, thread: [], status_history: [] },
    measurement: null,
  };

  const H = {
    auto_open_path: () => "/mock/contract.pdf",
    open_document: (a) => ({ doc_id: "mock-doc", path: a.path || "/mock/contract.pdf", page_count: PAGES }),
    close_document: () => null,
    get_page_count: () => PAGES,
    get_page_size: (a) => ({ doc_id: a.docId, page_index: a.pageIndex, width_pts: PAGE_W, height_pts: PAGE_H }),
    render_tile: (a) => {
      const t = tilePng();
      const r = a.req;
      return {
        doc_id: r.doc_id, page_index: r.page_index, tile_x: r.tile_x, tile_y: r.tile_y,
        width_px: t.w, height_px: t.h, zoom: r.zoom, dpr: r.dpr, png_base64: t.b64, render_ms: 1,
      };
    },
    process_rss_mb: () => 100,
    get_user_identity: () => ({ user_id: "00000000-0000-0000-0000-000000000001", display_name: "Harness" }),
    load_markups: () => [],
    list_markups: () => (window.__added || []),
    add_markup: (a) => { (window.__added = window.__added || []).push(a.markup); return null; },
    add_viewport: (a) => {
      if (a.name === "fail") throw new Error("sidecar is read-only");
      (window.__viewports = window.__viewports || []).push(a);
      return {
        id: "vp-scale-1", applies_to: { kind: "Viewport", page: a.page, viewport_id: "vp-1", name: a.name, bbox: a.bbox },
        method: a.twoPoint ? "TwoPoint" : "Preset", ratio: a.ratio, unit: a.unit, label: a.label, precision: a.precision,
      };
    },
    update_viewport: (a) => {
      (window.__updatesVp = window.__updatesVp || []).push(a);
      return {
        id: a.scaleId, applies_to: { kind: "Viewport", page: a.page ?? 0, viewport_id: "vp-1", name: "Viewport 1", bbox: a.bbox ?? [0, 0, 10, 10] },
        method: "Preset", ratio: (20 * 0.0254) / 72, unit: "m", label: "1:20", precision: 2,
      };
    },
    put_scale: (a) => { (window.__puts = window.__puts || []).push(a); return null; },
    take_scale_notices: () => [],
    update_markup: (a) => { (window.__updates = window.__updates || []).push(JSON.stringify(a)); return null; },
    delete_markup: () => null,
    save_document: () => null,
    save_document_as: () => null,
    license_status: () => ({
      state: "valid", staff_id: "harness", expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 9999, renew_due: false,
    }),
    // The document is calibrated 1:100 (metres) as a whole.
    list_scales: () => [{ id: "default-1-100", applies_to: { kind: "DocumentDefault" }, method: "Preset", ratio: (100 * 0.0254) / 72, unit: "m", label: "1:100", precision: 2 }],
    get_page_snap_targets: () => [],
    list_tool_sets: () => Array.from({ length: 12 }, (_, s) => ({ id: "set" + s, name: "Tool Set " + (s + 1), tools: Array.from({ length: 8 }, (_, t) => ({ id: `t${s}-${t}`, name: `Tool ${s + 1}.${t + 1}`, markup_type: "Rectangle", appearance: { color: "#ff0000", line_weight: 1, opacity: 1, fill: null, line_style: "Solid", font: null }, subject: null, placement_mode: "Properties", geometry: null, stamp: null, children: [] })) })),
    recent_tools: () => [],
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    load_recent_docs: () => Array.from({ length: 25 }, (_, i) => ({ path: `/mock/recent-${i}.pdf`, label: `recent-${i}.pdf`, last_opened: "2026-09-08T00:00:00Z" })),
    save_recent_docs: () => null,
    check_file_exists: () => true,
    search_document: () => [],
    // Settings persist in localStorage so a page reload stands in for an app restart.
    load_settings: () => JSON.parse(localStorage.getItem("__mock_settings") || "null") || {
      theme: "dark", default_tool: null, measurement_unit: "m", author_name: "", last_window: null,
      recent_colors: [], auto_ocr_on_open: false, viewer_mode: "single", panel_layout: null,
    },
    save_settings: (a) => { localStorage.setItem("__mock_settings", JSON.stringify(a.settings)); window.__saves = (window.__saves || 0) + 1; return null; },
    list_document_versions: () => Array.from({ length: 15 }, (_, i) => ({ id: "v" + i, label: "Snapshot " + i, created_at: "2026-09-08T00:00:00Z", size_bytes: 1000, reason: "manual" })),
    get_navigation: () => ({ page_count: PAGES, outline: { nodes: Array.from({ length: 40 }, (_, i) => ({ id: String(i), title: "Bookmark " + i, open: false, target: { kind: "page", page: i % PAGES, view: null }, color: null, bold: false, italic: false, children: [] })), total: 40, broken: 0, read_only: null }, labels: [], links: Array.from({ length: 30 }, (_, i) => ({ id: `obj:${i}:0`, page: i % PAGES, rect: [0, 0, 10, 10], target: { kind: "page", page: 1, view: null } })), links_truncated: false, read_only: null, undo_depth: 0, redo_depth: 0 }),
  };

  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
    transformCallback(cb) {
      const id = (window.__cbid = (window.__cbid || 0) + 1);
      window[`__cb_${id}`] = cb;
      return id;
    },
    invoke(cmd, args) {
      const h = H[cmd];
      if (!h) {
        console.warn("MOCK_INVOKE_UNHANDLED", cmd, JSON.stringify(args || {}));
        return Promise.resolve(null);
      }
      return Promise.resolve(h(args || {}));
    },
  };
}



let failed = false;
function fail(msg) { console.log(`FAIL: ${msg}`); failed = true; }
function ok(msg) { console.log(`OK: ${msg}`); }
function check(cond, good, bad) { cond ? ok(good) : fail(bad); }

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
const page = await ctx.newPage();
page.on("pageerror", (e) => fail(`page error: ${e.message}`));
page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") console.log(`console.${m.type()}: ${m.text()}`); });
await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".viewport-root", { timeout: 15000 });
await page.waitForTimeout(600);

const PT = 0.0254 / 72; // metres per point at 1:1
const overlay = await page.locator(".markup-overlay").boundingBox();
const cx = overlay.x + overlay.width / 2;
const cy = overlay.y + overlay.height / 2;

async function drag(x0, y0, x1, y1) {
  await page.mouse.move(x0, y0);
  await page.mouse.down();
  await page.mouse.move((x0 + x1) / 2, (y0 + y1) / 2, { steps: 4 });
  await page.mouse.move(x1, y1, { steps: 4 });
  await page.mouse.up();
  await page.waitForTimeout(250);
}

// --- 1. draw a viewport ----------------------------------------------------------------
const tool = page.locator('.tool-strip .tool-btn[title^="Viewport"]');
check((await tool.count()) === 1, "the Viewport tool is in the palette", "no Viewport tool button");
await tool.click();
const box = { x0: cx - 120, y0: cy - 100, x1: cx + 40, y1: cy + 20 };
await drag(box.x0, box.y0, box.x1, box.y1);
const dialog = page.getByRole("dialog", { name: "New Viewport" });
await dialog.waitFor({ timeout: 3000 }).catch(() => {});
check(await dialog.isVisible(), "dragging a rectangle opens the New Viewport dialog", "no New Viewport dialog");
await dialog.getByRole("button", { name: "Save" }).click();
await page.waitForTimeout(300);
const vps = await page.evaluate(() => window.__viewports || []);
const v = vps[0];
check(vps.length === 1 && v.page === 0 && v.label === "1:20" && v.unit === "m" && Math.abs(v.ratio - 20 * 0.0254 / 72) < 1e-12,
  `add_viewport called once: page ${v && v.page}, ${v && v.name}, ${v && v.label}, bbox ${v && JSON.stringify(v.bbox.map((n) => Math.round(n)))}`,
  `add_viewport calls: ${JSON.stringify(vps)}`);
const w = v ? v.bbox[2] - v.bbox[0] : 0;
const h = v ? v.bbox[3] - v.bbox[1] : 0;
check(w > 50 && h > 50, `the rectangle is ${Math.round(w)} x ${Math.round(h)} pt in page space`, `bad bbox ${JSON.stringify(v && v.bbox)}`);

// --- 2. outline + panel --------------------------------------------------------------------
await page.locator('.tool-strip .tool-btn[title^="Measure Length"]').click();
await page.waitForTimeout(200);
const tag = (await page.locator(".viewport-outline text").textContent().catch(() => "")) || "";
check(tag.includes("Viewport 1") && tag.includes("1:20"), `outline drawn with its tag "${tag.trim()}"`, `no outline tag: "${tag}"`);
const ob = await page.locator(".viewport-outline rect").boundingBox();
check(ob && Math.abs(ob.x - box.x0) < 3 && Math.abs(ob.y - box.y0) < 3 && Math.abs(ob.width - (box.x1 - box.x0)) < 3,
  "the outline sits where the rectangle was drawn", `outline ${JSON.stringify(ob)} vs drawn ${JSON.stringify(box)}`);
const panelRow = page.locator('table[aria-label="Measurement viewports"] tr[data-viewport="vp-1"]');
// Open the bottom panel if it is hidden, then switch it to Quantities.
if ((await page.getByRole("tab", { name: "Quantities" }).count()) === 0) {
  await page.getByRole("button", { name: "Toggle markups list / quantities panel" }).click();
  await page.waitForTimeout(300);
}
await page.getByRole("tab", { name: "Quantities" }).click();
await page.waitForTimeout(300);
if ((await panelRow.count()) === 0) {
  // Its accordion may be collapsed: open it.
  await page.locator('[data-testid="accordion-bottom-panel"] button').first().click().catch(() => {});
  await page.waitForTimeout(300);
}
check((await panelRow.count()) === 1 && /Viewport 1/.test(await panelRow.textContent()), "the Measurements panel lists the viewport",
  "viewport not listed in the Measurements panel");

// --- 3. measure inside / outside ---------------------------------------------------------------
await page.locator('.tool-strip .tool-btn[title^="Measure Length"]').click();
await page.waitForTimeout(300);
// The bottom panel resized the canvas: measure against where the outline is now.
const r = await page.locator(".viewport-outline rect").boundingBox();
await drag(r.x + 10, r.y + r.height + 40, r.x + 110, r.y + r.height + 40); // below it: 1:100
await drag(r.x + 20, r.y + 30, r.x + r.width + 80, r.y + 30); // starts inside, ends outside: 1:20
await page.waitForTimeout(300);
const added = (await page.evaluate(() => window.__added || [])).filter((m) => m.markup_type === "MeasurementLength");
const [outside, inside] = added;
const expect = (m, factor) => m.measurement.raw_measure * factor * PT;
check(added.length === 2, "two length measurements created", `created ${added.length}`);
if (added.length === 2) {
  check(outside.measurement.scale_ref === "default-1-100" && Math.abs(outside.measurement.computed_quantity - expect(outside, 100)) < 1e-9,
    `outside the viewport: 1:100 (${outside.measurement.computed_quantity.toFixed(3)} m for ${outside.measurement.raw_measure.toFixed(1)} pt)`,
    `outside measurement: ${JSON.stringify(outside.measurement)}`);
  check(inside.measurement.scale_ref === "vp-scale-1" && Math.abs(inside.measurement.computed_quantity - expect(inside, 20)) < 1e-9,
    `first point inside the viewport: 1:20 (${inside.measurement.computed_quantity.toFixed(3)} m for ${inside.measurement.raw_measure.toFixed(1)} pt)`,
    `inside measurement: ${JSON.stringify(inside.measurement)}`);
  const cell = await page.locator('table[aria-label="Measurement quantities"]').textContent().catch(() => "");
  check(/Viewport 1 \(1:20\)/.test(cell) && /1:100/.test(cell), "the quantities table names each measurement's scale",
    `quantities table: ${cell}`);
  const shown = outside.measurement.computed_quantity.toFixed(2);
  check(cell.includes(shown), `the earlier 1:100 measurement still shows ${shown} m after the viewport measurement`,
    `quantities table lost ${shown}: ${cell}`);
}

// --- 5. Redraw: cancelled by a tool change; finished on another page -----------------------
const row = page.locator('table[aria-label="Measurement viewports"] tr[data-viewport="vp-1"]');
await row.getByRole("button", { name: "Redraw" }).click();
await page.locator('.tool-strip .tool-btn[title^="Measure Length"]').click(); // changes tool: cancels
await page.locator('.tool-strip .tool-btn[title^="Viewport"]').click();
const r2 = await page.locator(".markup-overlay").boundingBox();
await drag(r2.x + 40, r2.y + 40, r2.x + 140, r2.y + 120);
const newDialog = page.getByRole("dialog", { name: "New Viewport" });
await newDialog.waitFor({ timeout: 3000 }).catch(() => {});
const upd0 = await page.evaluate(() => (window.__updatesVp || []).length);
check(await newDialog.isVisible() && upd0 === 0, "after a cancelled Redraw, a Viewport drag makes a NEW viewport (no update_viewport)",
  `dialog visible=${await newDialog.isVisible()}, update_viewport calls=${upd0}`);
await newDialog.getByRole("button", { name: "Cancel" }).click();

await row.getByRole("button", { name: "Redraw" }).click();
const thumb = page.locator('[data-page-idx="1"]');
if ((await thumb.count()) > 0) {
  await thumb.first().click();
  await page.waitForFunction(() => document.querySelector(".page-label")?.textContent?.includes("Page 2"), null, { timeout: 5000 }).catch(() => {});
} else {
  await page.keyboard.press("PageDown");
  await page.waitForTimeout(500);
}
const onPage2 = ((await page.locator(".page-label").innerText().catch(() => "")) || "").includes("Page 2");
check(onPage2, "moved to page 2 with the Redraw armed", "could not reach page 2");
const r3 = await page.locator(".markup-overlay").boundingBox();
await drag(r3.x + 60, r3.y + 60, r3.x + 200, r3.y + 160);
await page.waitForTimeout(300);
const upd = await page.evaluate(() => window.__updatesVp || []);
check(upd.length === 1 && upd[0].page === 1 && Array.isArray(upd[0].bbox),
  `the Redraw on page 2 sends page 1 (0-based) with the new rectangle: ${JSON.stringify(upd[0] && { page: upd[0].page })}`,
  `update_viewport calls: ${JSON.stringify(upd)}`);
const puts = await page.evaluate(() => window.__puts || []);
check(puts.some((p) => p.scaleId === "vp-scale-1" && p.record && p.record.applies_to.page === 1),
  "the redraw is recorded as an undoable scale change (put_scale)", `put_scale calls: ${JSON.stringify(puts)}`);

// --- 6. a failed create reaches the error banner ---------------------------------------------
await page.locator('.tool-strip .tool-btn[title^="Viewport"]').click();
const r4 = await page.locator(".markup-overlay").boundingBox();
await drag(r4.x + 300, r4.y + 60, r4.x + 420, r4.y + 160);
const d4 = page.getByRole("dialog", { name: "New Viewport" });
await d4.waitFor({ timeout: 3000 });
await d4.getByLabel("Viewport name").fill("fail");
await d4.getByRole("button", { name: "Save" }).click();
await page.waitForTimeout(400);
const alertText = (await page.getByRole("alert").allInnerTexts().catch(() => [])).join(" ");
check(/Creating the viewport failed: .*sidecar is read-only/.test(alertText), "a failed create is shown in the error banner",
  `no banner; alerts: ${JSON.stringify(alertText)}`);

await browser.close();
if (failed) {
  console.log("FAIL: see lines above.");
  process.exit(1);
}
console.log("PASS: a viewport drawn with the Viewport tool gets its own scale, is outlined and listed, and measurements take the scale under their first point (1:20 inside, 1:100 outside).");
