#!/usr/bin/env node
// Real-browser check of mark-up in continuous scroll (reading mode) - UX benchmark must-fix,
// docs/continuous-markup.md. The real Svelte app in headless Chromium over a mocked Tauri IPC,
// with a 150-sheet set of mixed page sizes (A1 landscape, A3 portrait, one rotated sheet) and
// 30 mark-ups on every sheet.
//
// Checks:
//   1. reading mode shows no "Switch to page view to mark up" hint and the palette is enabled;
//   2. a Rectangle drawn on page 1 lands on page 0 (0-based) at the drawn place;
//   3. scrolled to page 3, a Rectangle drawn there lands on page 2 at the drawn place, and a
//      drag that starts on page 3 and ends on page 4 stays on page 3, clipped to its edge;
//   4. the Select tool selects a mark-up on page 3; Delete removes it; Cmd/Ctrl+Z restores it;
//   5. right-click on page 3 shows the page menu and Rotate Page Clockwise rotates page 2,
//      not the page at the top of the view;
//   6. a length measured on page 3 is a MeasurementLength on page 2 under the document scale;
//   7. tool keys work while scrolled (R picks Rectangle, V picks Select);
//   8. pages stay virtualised (a handful of page boxes mounted after scrolling to the end);
//   9-11 (review of PR #168): keys still act after a jump far from the page the pointer was
//      over; leaving Calibrate after one point does not freeze the other pages; Space-pan gives
//      back the tool once, after a key-repeat on another page and after the arming page unmounts.
//
// PERF: scrolls the whole 150-sheet set at 240 px per frame, then back up, and prints the
// frame-time numbers (mean FPS, p95 and worst frame, frames over 50 ms) plus the time from a
// jump to the last page until its page box shows and until its mark-ups show. Run it on the old and the new build to compare.
// HARNESS_CPU_THROTTLE=4 runs the scroll with the CPU slowed 4x; HARNESS_PERF_TOOL=r presses that
// tool key before the scroll (default: the Hand tool).
// HARNESS_PERF_ONLY=1 skips checks 1-8 (for measuring a build that cannot mark up yet).
//
// Run:  npx vite --port 1431 --strictPort      (serves the frontend, separate terminal)
//       HARNESS_URL=http://localhost:1431/ node tools/gui-harness-continuous-markup.mjs
//
// Exits 0 and prints PASS on success; exits 1 with FAIL lines otherwise.
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";
const PERF_ONLY = process.env.HARNESS_PERF_ONLY === "1";

function installMockTauri() {
  const PAGES = 150;
  // A1 landscape sheets, every 10th an A3 portrait detail sheet, page 7 rotated (dims swapped).
  const dims = (i) => (i === 7 ? [1684, 2384] : i % 10 === 9 ? [842, 1191] : [2384, 1684]);

  function tilePng() {
    const c = document.createElement("canvas");
    c.width = 64;
    c.height = 64;
    const g = c.getContext("2d");
    g.fillStyle = "#eef3ff";
    g.fillRect(0, 0, 64, 64);
    return { w: 64, h: 64, b64: c.toDataURL("image/png").split(",")[1] };
  }
  const TILE = tilePng();

  const audit = { created_by: { user_id: "u", display_name: "H" }, created_at: "2026-01-01T00:00:00Z", modified_by: { user_id: "u", display_name: "H" }, modified_at: "2026-01-01T00:00:00Z", revision: 1, origin: "Desktop" };
  const appearance = { color: "#e02424", line_weight: 3, opacity: 1, fill: null, line_style: "Solid", font: null, outline_color: null, fill_opacity: null };
  const seed = [];
  for (let p = 0; p < PAGES; p++) {
    for (let k = 0; k < 30; k++) {
      // Kept in the lower half of the sheet so the upper half is free for the checks.
      const x = 60 + (k % 10) * 70;
      const y = 60 + Math.floor(k / 10) * 70;
      seed.push({
        id: `seed-${p}-${k}`, markup_type: "Rectangle", page: p,
        geometry: { Rect: { min: { x, y }, max: { x: x + 40, y: y + 40 } } },
        appearance, subject: null, layer: null, contents: null, group_id: null, audit,
        workflow: { status: "None", assignee: null, thread: [], status_history: [] }, measurement: null,
      });
    }
  }

  const H = {
    auto_open_path: () => "/mock/sheet-set.pdf",
    open_document: (a) => ({ doc_id: "mock-doc", path: a.path || "/mock/sheet-set.pdf", page_count: PAGES }),
    close_document: () => null,
    get_page_count: () => PAGES,
    get_page_size: (a) => ({ doc_id: a.docId, page_index: a.pageIndex, width_pts: dims(a.pageIndex)[0], height_pts: dims(a.pageIndex)[1] }),
    render_tile: (a) => {
      const r = a.req;
      window.__renders = (window.__renders || 0) + 1;
      return {
        doc_id: r.doc_id, page_index: r.page_index, tile_x: r.tile_x, tile_y: r.tile_y,
        width_px: TILE.w, height_px: TILE.h, zoom: r.zoom, dpr: r.dpr, png_base64: TILE.b64, render_ms: 1,
      };
    },
    process_rss_mb: () => 100,
    get_user_identity: () => ({ user_id: "00000000-0000-0000-0000-000000000001", display_name: "Harness" }),
    load_markups: () => seed,
    list_markups: () => seed.concat(window.__added || []),
    add_markup: (a) => { (window.__added = window.__added || []).push(a.markup); return null; },
    update_markup: (a) => { (window.__updates = window.__updates || []).push(a); return null; },
    delete_markup: (a) => { (window.__deleted = window.__deleted || []).push(a); return null; },
    rotate_page: (a) => { (window.__rotated = window.__rotated || []).push(a); return null; },
    put_scale: () => null,
    take_scale_notices: () => [],
    save_document: () => null,
    save_document_as: () => null,
    license_status: () => ({
      state: "valid", staff_id: "harness", expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 9999, renew_due: false,
    }),
    list_scales: () => [{ id: "default-1-100", applies_to: { kind: "DocumentDefault" }, method: "Preset", ratio: (100 * 0.0254) / 72, unit: "m", label: "1:100", precision: 2 }],
    get_page_snap_targets: () => [],
    list_tool_sets: () => [],
    recent_tools: () => [],
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    load_recent_docs: () => [],
    save_recent_docs: () => null,
    check_file_exists: () => true,
    search_document: () => [],
    load_settings: () => ({
      theme: "dark", default_tool: null, measurement_unit: "m", author_name: "", last_window: null,
      recent_colors: [], auto_ocr_on_open: false, viewer_mode: "reading", panel_layout: null,
    }),
    save_settings: () => null,
    list_document_versions: () => [],
    get_navigation: () => ({ page_count: PAGES, outline: { nodes: [], total: 0, broken: 0, read_only: null }, labels: [], links: [], links_truncated: false, read_only: null, undo_depth: 0, redo_depth: 0 }),
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
      if (!h) return Promise.resolve(null);
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
page.on("console", (m) => { if (m.type() === "error") console.log(`console.error: ${m.text()}`); });
await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".reading-scroll", { timeout: 15000 });
await page.waitForSelector('.reading-page[data-page-index="0"]', { timeout: 15000 });
await page.waitForTimeout(800);

const scrollTo = async (top) => {
  await page.evaluate((t) => { document.querySelector(".reading-scroll").scrollTop = t; }, top);
  await page.waitForTimeout(400);
};
const pageBox = (i) => page.locator(`.reading-page[data-page-index="${i}"]`).boundingBox();
const pageTop = (i) => page.evaluate((k) => parseFloat(document.querySelector(`.reading-page[data-page-index="${k}"]`)?.style.top ?? "NaN"), i);

async function drag(x0, y0, x1, y1) {
  await page.mouse.move(x0, y0);
  await page.mouse.down();
  await page.mouse.move((x0 + x1) / 2, (y0 + y1) / 2, { steps: 4 });
  await page.mouse.move(x1, y1, { steps: 4 });
  await page.mouse.up();
  await page.waitForTimeout(250);
}
const added = () => page.evaluate(() => window.__added || []);
/** PDF point (bottom-left origin) of a screen point inside page box `b` of a page `wPts` wide. */
const toPdf = (b, wPts, hPts, x, y) => ({ x: ((x - b.x) / b.width) * wPts, y: hPts - ((y - b.y) / b.height) * hPts });

if (!PERF_ONLY) {
  // --- 1. not read-only --------------------------------------------------------------------
  check((await page.locator('[data-testid="palette-read-only-hint"]').count()) === 0,
    "no read-only hint in reading mode", "the 'Switch to page view' hint is still shown");
  const rectBtn = page.locator('.tool-strip .tool-btn[title^="Rectangle"]');
  check((await rectBtn.count()) === 1 && !(await rectBtn.isDisabled()), "the Rectangle tool is enabled", "Rectangle tool missing or disabled");

  // --- 2. draw on page 1 --------------------------------------------------------------------
  await rectBtn.click();
  let b0 = await pageBox(0);
  await drag(b0.x + b0.width * 0.5, b0.y + 40, b0.x + b0.width * 0.6, b0.y + 120);
  let a = await added();
  const r0 = a[0];
  const exp0 = toPdf(b0, 2384, 1684, b0.x + b0.width * 0.5, b0.y + 40);
  check(a.length === 1 && r0.page === 0 && Math.abs(r0.geometry.Rect.min.x - exp0.x) < 4 && Math.abs(r0.geometry.Rect.max.y - exp0.y) < 4,
    `rectangle on page 1 saved on page 0 at x=${r0 && Math.round(r0.geometry.Rect.min.x)} (expected ${Math.round(exp0.x)})`,
    `page-1 rectangle: ${JSON.stringify(a.map((m) => ({ page: m.page, g: m.geometry })))}`);

  // --- 3. scroll to page 3 and draw ------------------------------------------------------------
  await scrollTo((await pageTop(2)) - 40);
  const b2 = await pageBox(2);
  check(b2 && b2.y > 0 && b2.y < 200, `scrolled: page 3 is at the top of the view (y=${b2 && Math.round(b2.y)})`, `page 3 box ${JSON.stringify(b2)}`);
  await page.keyboard.press("r"); // tool key, while scrolled
  await drag(b2.x + b2.width * 0.3, b2.y + 60, b2.x + b2.width * 0.4, b2.y + 160);
  a = await added();
  const r2 = a[1];
  const exp2 = toPdf(b2, 2384, 1684, b2.x + b2.width * 0.4, b2.y + 160);
  check(a.length === 2 && r2.page === 2 && Math.abs(r2.geometry.Rect.max.x - exp2.x) < 4 && Math.abs(r2.geometry.Rect.min.y - exp2.y) < 4,
    `rectangle on page 3 saved on page 2 at the drawn place (max.x ${r2 && Math.round(r2.geometry.Rect.max.x)} vs ${Math.round(exp2.x)})`,
    `page-3 rectangle: ${JSON.stringify(r2)}`);

  // A drag from the bottom of page 3 onto page 4 stays on page 3, clipped to its bottom edge.
  await scrollTo((await pageTop(3)) - 450);
  const b2b = await pageBox(2);
  const b3 = await pageBox(3);
  await drag(b2b.x + 200, b2b.y + b2b.height - 80, b3.x + 300, b3.y + 120);
  a = await added();
  const rx = a[2];
  check(a.length === 3 && rx.page === 2 && rx.geometry.Rect.min.y >= 0 && rx.geometry.Rect.min.y < 1,
    `a drag across the page gap stays on page 3, clipped at its edge (min.y ${rx && rx.geometry.Rect.min.y.toFixed(2)})`,
    `cross-page drag: ${JSON.stringify(rx)}`);

  // --- 4. select, delete, undo on page 3 ------------------------------------------------------
  await scrollTo((await pageTop(2)) - 40);
  const b2c = await pageBox(2);
  await page.keyboard.press("v");
  // The page-3 rectangle drawn above: click its left edge.
  const edge = { x: b2c.x + b2c.width * 0.3, y: b2c.y + 110 };
  await page.mouse.click(edge.x, edge.y);
  await page.waitForTimeout(200);
  const chrome = await page.locator(`.reading-page[data-page-index="2"] .selection-box`).count();
  check(chrome > 0, "Select tool selects the mark-up on page 3", "no selection chrome on page 3");
  await page.keyboard.press("Delete");
  await page.waitForTimeout(200);
  const del = await page.evaluate(() => window.__deleted || []);
  check(del.length === 1 && JSON.stringify(del[0]).includes(r2.id), "Delete removes it", `delete_markup calls: ${JSON.stringify(del)}`);
  await page.keyboard.press(process.platform === "darwin" ? "Meta+z" : "Control+z");
  await page.waitForTimeout(300);
  const restored = (await added()).filter((m) => m.id === r2.id).length;
  check(restored === 2, "Cmd/Ctrl+Z restores it (one undo step)", `add_markup calls for ${r2.id}: ${restored}`);

  // --- 5. right-click rotate acts on the page under the pointer -------------------------------
  // Put page 2 at the top of the view and right-click page 3 lower down.
  await scrollTo((await pageTop(1)) + 200);
  const b2d = await pageBox(2);
  await page.mouse.click(b2d.x + b2d.width * 0.8, b2d.y + 80, { button: "right" });
  await page.waitForTimeout(200);
  const rot = page.getByRole("menuitem", { name: "Rotate Page Clockwise" });
  check((await rot.count()) === 1, "right-click on page 3 opens the page menu", "no Rotate Page Clockwise item");
  if ((await rot.count()) === 1) {
    await rot.click();
    await page.waitForTimeout(300);
  }
  const rotated = await page.evaluate(() => window.__rotated || []);
  check(rotated.length === 1 && rotated[0].pageIdx === 2,
    "Rotate Page Clockwise rotates page 3 (page_idx 2), the page under the pointer", `rotate_page calls: ${JSON.stringify(rotated)}`);
  await page.waitForSelector('.reading-page[data-page-index="2"]', { timeout: 5000 }).catch(() => {});
  await page.waitForTimeout(600);

  // --- 6. measure on page 3 -----------------------------------------------------------------------
  await scrollTo((await pageTop(2)) - 40);
  const b2e = await pageBox(2);
  await page.keyboard.press("m");
  await drag(b2e.x + b2e.width * 0.55, b2e.y + 100, b2e.x + b2e.width * 0.7, b2e.y + 100);
  const meas = (await added()).filter((m) => m.markup_type === "MeasurementLength");
  check(meas.length === 1 && meas[0].page === 2 && meas[0].measurement && meas[0].measurement.scale_ref === "default-1-100",
    `a length measured on page 3 is on page 2 under 1:100 (${meas[0] && meas[0].measurement && meas[0].measurement.computed_quantity.toFixed(2)} m)`,
    `measurements: ${JSON.stringify(meas)}`);

  // --- 7. tool keys -------------------------------------------------------------------------------
  await page.keyboard.press("r");
  const rActive = await page.locator('.tool-strip .tool-btn[title^="Rectangle"]').evaluate((el) => el.classList.contains("active") || el.getAttribute("aria-pressed") === "true");
  await page.keyboard.press("v");
  const vActive = await page.locator('.tool-strip .tool-btn[title^="Select"]').first().evaluate((el) => el.classList.contains("active") || el.getAttribute("aria-pressed") === "true");
  check(rActive && vActive, "tool keys work in reading mode (R, V)", `R active=${rActive}, V active=${vActive}`);

  const pressed = (title) => page.locator(`.tool-strip .tool-btn[title^="${title}"]`).first().getAttribute("aria-pressed");

  // --- 9. keys still work after a jump away from the page the pointer was over (review 1) -----
  await scrollTo((await pageTop(2)) - 40);
  const b2f = await pageBox(2);
  await page.mouse.move(b2f.x + 300, b2f.y + 300); // pointer over page 3
  await page.waitForTimeout(150);
  await page.locator(".page-indicator-display").click();
  await page.getByLabel("Jump to page").fill("80");
  await page.getByLabel("Jump to page").press("Enter");
  await page.waitForTimeout(500);
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  await page.keyboard.press("e");
  await page.waitForTimeout(100);
  const eOn = await pressed("Ellipse");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(100);
  const escOn = await pressed("Select");
  check(eOn === "true" && escOn === "true", "after a jump to page 80, tool keys and Escape still act (E, Escape)",
    `after jump: Ellipse pressed=${eOn}, Select after Escape=${escOn}`);

  // --- 10. leaving Calibrate after the first point frees the other pages (review 2) ------------
  await scrollTo(0); // back from page 80: pages 0-3 are the same size, so page 3's top is 3 boxes down
  const h0 = await page.evaluate(() => parseFloat(document.querySelector('.reading-page[data-page-index="0"]').style.height));
  await scrollTo(3 * (h0 + 12) - 450);
  const b2g = await pageBox(2);
  const b3g = await pageBox(3);
  await page.locator('.tool-strip .tool-btn[title^="Calibrate Scale"]').click();
  await page.mouse.click(b2g.x + 300, b2g.y + b2g.height - 200); // first point on page 3
  await page.waitForTimeout(150);
  await page.keyboard.press("r");
  const before10 = (await added()).length;
  await drag(b3g.x + 200, b3g.y + 60, b3g.x + 320, b3g.y + 140); // on page 4
  const a10 = await added();
  check(a10.length === before10 + 1 && a10[a10.length - 1].page === 3,
    "after leaving Calibrate with one point placed, a rectangle can still be drawn on page 4",
    `added ${a10.length - before10}, last page ${a10[a10.length - 1] && a10[a10.length - 1].page}`);

  // --- 11. Space-pan restores the tool once, across pages (review 3) ---------------------------
  await page.keyboard.press("r");
  await page.mouse.move(b2g.x + 300, b2g.y + b2g.height - 300); // over page 3
  await page.keyboard.down("Space");
  await page.waitForTimeout(80);
  const handOn = await pressed("Pan (Hand)");
  await page.mouse.move(b3g.x + 300, b3g.y + 100); // over page 4
  await page.keyboard.down("Space"); // auto-repeat, now seen with page 4 active
  await page.keyboard.down("Space");
  await page.keyboard.up("Space");
  await page.waitForTimeout(100);
  const rBack = await pressed("Rectangle");
  // Hold Space, scroll the arming page out of the render window, release.
  await page.mouse.move(b3g.x + 300, b3g.y + 100);
  await page.keyboard.down("Space");
  await page.evaluate(() => { document.querySelector(".reading-scroll").scrollTop += 40000; });
  await page.waitForTimeout(400);
  await page.keyboard.up("Space");
  await page.waitForTimeout(100);
  const rBack2 = await pressed("Rectangle");
  check(handOn === "true" && rBack === "true" && rBack2 === "true",
    "Space-pan restores Rectangle after a repeat on another page, and after the arming page scrolled away",
    `Space: hand=${handOn}, after repeat+release=${rBack}, after unmount+release=${rBack2}`);
  await page.keyboard.press("Escape");
}

// --- 8 / PERF: scroll the whole set -----------------------------------------------------------------
if (process.env.HARNESS_PERF_TOOL) await page.keyboard.press(process.env.HARNESS_PERF_TOOL);
await scrollTo(0);
// HARNESS_CPU_THROTTLE=4 slows the CPU 4x (Chrome DevTools emulation) to approximate the floor machine.
const throttle = Number(process.env.HARNESS_CPU_THROTTLE || "1");
if (throttle > 1) {
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
}
const perf = await page.evaluate(async () => {
  const el = document.querySelector(".reading-scroll");
  const frames = [];
  let maxMounted = 0;
  const step = 240;
  const run = (dir) => new Promise((resolve) => {
    let last = performance.now();
    const tick = () => {
      const now = performance.now();
      frames.push(now - last);
      last = now;
      maxMounted = Math.max(maxMounted, document.querySelectorAll(".reading-page").length);
      const before = el.scrollTop;
      el.scrollTop = before + dir * step;
      if (el.scrollTop === before) resolve();
      else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const t0 = performance.now();
  await run(1);
  await run(-1);
  const total = performance.now() - t0;
  frames.shift();
  const sorted = [...frames].sort((x, y) => x - y);
  const mean = frames.reduce((s, x) => s + x, 0) / frames.length;
  // Jump to the last page and time until its page box is mounted.
  const tj = performance.now();
  el.scrollTop = el.scrollHeight;
  await new Promise((resolve) => {
    const poll = () => (document.querySelector('.reading-page[data-page-index="149"]') ? resolve() : requestAnimationFrame(poll));
    poll();
  });
  const jumpMs = performance.now() - tj;
  // ...and until that page's mark-up layer is mounted (layers mount on idle after a scroll).
  const hasLayers = !!document.querySelector(".tool-strip");
  await new Promise((resolve) => {
    const deadline = performance.now() + 3000;
    const poll = () => (document.querySelector('.reading-page[data-page-index="149"] .viewport-root') || performance.now() > deadline ? resolve() : requestAnimationFrame(poll));
    poll();
  });
  const layerMs = performance.now() - tj;
  return {
    frames: frames.length, totalMs: Math.round(total), meanFps: +(1000 / mean).toFixed(1),
    p95Ms: +sorted[Math.floor(sorted.length * 0.95)].toFixed(1), worstMs: +sorted[sorted.length - 1].toFixed(1),
    over50: frames.filter((x) => x > 50).length, maxMountedPages: maxMounted, jumpToLastMs: Math.round(jumpMs),
    lastPageMarkupsMs: document.querySelector('.reading-page[data-page-index="149"] .viewport-root') ? Math.round(layerMs) : null,
    lastPageShapes: document.querySelectorAll('.reading-page[data-page-index="149"] rect').length, hasLayers,
  };
});
console.log(`PERF (cpu throttle ${throttle}x): ${JSON.stringify(perf)}`);
if (!PERF_ONLY) {
  check(perf.lastPageShapes >= 30, `the last page's 30 mark-ups show after the jump (${perf.lastPageMarkupsMs} ms)`,
    `last page shows ${perf.lastPageShapes} shapes`);
}
check(perf.maxMountedPages <= 8, `pages stay virtualised (at most ${perf.maxMountedPages} page boxes mounted)`, `${perf.maxMountedPages} page boxes mounted`);

await browser.close();
if (failed) {
  console.log("FAIL: see lines above.");
  process.exit(1);
}
console.log("PASS: mark-up, selection, measurement, the page menu and tool keys work in continuous scroll, on the page under the pointer.");
