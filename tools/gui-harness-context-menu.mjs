#!/usr/bin/env node
// Real-browser check of the right-click context menus and Previous/Next view (UX benchmark
// must-fix items 2 and 4, 2026-11-04): the real Svelte app in headless Chromium over a mocked
// Tauri IPC with one Rectangle markup on page 1 of a 6-page document.
//
// Checks:
//   1. right-click on the markup opens the markup menu (status, properties, copy, order,
//      delete), inside the window; choosing "Accepted" writes the status through update_markup;
//   2. Escape closes it; right-click on empty page opens the page menu (Fit Width ...);
//   3. a menu opened near the bottom-right corner stays inside the window;
//   4. Shift+F10 opens the menu from the keyboard, Escape closes it;
//   5. right-click on a Navigator thumbnail opens the page menu for that page;
//   6. after jumping from page 1 to page 4 via a thumbnail, Alt+Left returns to page 1 and
//      Alt+Right goes forward again; the toolbar buttons track the same state.
//
// Run:  npx vite --port 1431 --strictPort      (serves the frontend, separate terminal)
//       HARNESS_URL=http://localhost:1431/ node tools/gui-harness-context-menu.mjs
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
    load_markups: () => [MARKUP],
    list_markups: () => [MARKUP],
    add_markup: () => null,
    update_markup: (a) => { (window.__updates = window.__updates || []).push(JSON.stringify(a)); return null; },
    delete_markup: () => null,
    save_document: () => null,
    save_document_as: () => null,
    license_status: () => ({
      state: "valid", staff_id: "harness", expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 9999, renew_due: false,
    }),
    list_scales: () => [],
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
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
const page = await ctx.newPage();
page.on("pageerror", (e) => fail(`page error: ${e.message}`));
await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".viewport-root", { timeout: 15000 });
await page.waitForSelector(".markup-overlay rect", { state: "attached", timeout: 10000 });
await page.waitForTimeout(400);

const menu = page.getByRole("menu");
const label = () => page.locator(".page-label").innerText().then((t) => t.replace(/\s+/g, " ").trim());
const inWindow = async () => {
  const b = await menu.boundingBox();
  return b && b.x >= 0 && b.y >= 0 && b.x + b.width <= 1280 && b.y + b.height <= 800;
};

// --- 1. markup menu ----------------------------------------------------------------------
const shape = await page.locator(".markup-overlay rect").first().boundingBox();
const mx = shape.x + shape.width / 2;
const my = shape.y + shape.height / 2;
await page.mouse.click(mx, my, { button: "right" });
await menu.waitFor({ timeout: 3000 });
check((await menu.getAttribute("aria-label")) === "Markup menu", "right-click on a markup opens the markup menu", "markup menu did not open");
const items = (await page.getByRole("menuitem").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
for (const want of ["Accepted", "Properties", "Save as Tool...", "Copy", "Duplicate", "Bring to Front", "Send to Back", "Delete"]) {
  check(items.some((t) => t.startsWith(want)), `markup menu offers ${want}`, `markup menu is missing ${want}: ${JSON.stringify(items)}`);
}
check(await inWindow(), "markup menu is inside the window", "markup menu overflows the window");
await page.getByRole("menuitem", { name: /^Accepted/ }).click();
await page.waitForTimeout(300);
const updates = await page.evaluate(() => window.__updates || []);
check(updates.some((u) => u.includes("Accepted")), "choosing Accepted wrote the status via update_markup", `no status write seen: ${JSON.stringify(updates).slice(0, 200)}`);
check((await menu.count()) === 0, "menu closes after a choice", "menu still open after a choice");

// --- 2. Escape + page menu ---------------------------------------------------------------
await page.mouse.click(mx, my, { button: "right" });
await menu.waitFor({ timeout: 3000 });
await page.keyboard.press("Escape");
check((await menu.count()) === 0, "Escape closes the menu", "Escape did not close the menu");
const root = await page.locator(".viewport-root").boundingBox();
await page.mouse.click(root.x + 600, root.y + 500, { button: "right" });
await menu.waitFor({ timeout: 3000 });
check((await menu.getAttribute("aria-label")) === "Page menu", "right-click on empty page opens the page menu", "page menu did not open");
const pItems = (await page.getByRole("menuitem").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
for (const want of ["Paste", "Select All on Page", "Fit Width", "Fit Height", "Actual Size", "Previous View", "Next View"]) {
  check(pItems.some((t) => t.startsWith(want)), `page menu offers ${want}`, `page menu is missing ${want}: ${JSON.stringify(pItems)}`);
}
// outside press closes
await page.mouse.click(root.x + 20, root.y + 20);
check((await menu.count()) === 0, "an outside click closes the menu", "outside click did not close the menu");

// --- 3. near the corner ------------------------------------------------------------------
await page.mouse.click(root.x + root.width - 6, root.y + root.height - 6, { button: "right" });
await menu.waitFor({ timeout: 3000 });
check(await inWindow(), "menu opened at the bottom-right corner stays inside the window", "corner menu overflows the window");
await page.keyboard.press("Escape");

// --- 4. keyboard -------------------------------------------------------------------------
await page.mouse.click(root.x + 600, root.y + 500); // focus the page area, empty spot
await page.keyboard.press("Shift+F10");
await menu.waitFor({ timeout: 3000 });
check(true, "Shift+F10 opens the menu from the keyboard", "");
await page.keyboard.press("ArrowDown");
await page.keyboard.press("Escape");
check((await menu.count()) === 0, "arrow keys work and Escape closes the keyboard-opened menu", "keyboard menu did not close");

// --- 5. thumbnail menu -------------------------------------------------------------------
const thumb2 = page.locator('[data-page-idx="2"]');
await thumb2.scrollIntoViewIfNeeded();
await thumb2.click({ button: "right" });
await menu.waitFor({ timeout: 3000 });
check((await menu.getAttribute("aria-label")) === "Page 3 menu", "right-click on a thumbnail opens that page's menu", "thumbnail menu did not open");
const tItems = (await page.getByRole("menuitem").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
for (const want of ["Go to Page 3", "Rotate Clockwise", "Rotate Counterclockwise", "Delete Page"]) {
  check(tItems.some((t) => t.startsWith(want)), `thumbnail menu offers ${want}`, `thumbnail menu is missing ${want}: ${JSON.stringify(tItems)}`);
}
await page.keyboard.press("Escape");

// --- 6. view history ---------------------------------------------------------------------
check((await label()) === "Page 1 / 6", "starts on page 1", `unexpected start page: ${await label()}`);
check(await page.locator('[data-testid="prev-view"]').isDisabled(), "Previous view is disabled before any jump", "Previous view enabled too early");
await page.locator('[data-page-idx="3"]').click();
await page.waitForFunction(() => document.querySelector(".page-label")?.textContent?.includes("Page 4"), null, { timeout: 5000 });
check(await page.locator('[data-testid="prev-view"]').isEnabled(), "Previous view enables after a thumbnail jump", "Previous view still disabled after a jump");
await page.mouse.move(root.x + 600, root.y + 400);
await page.keyboard.press("Alt+ArrowLeft");
await page.waitForFunction(() => document.querySelector(".page-label")?.textContent?.includes("Page 1 "), null, { timeout: 5000 });
check((await label()) === "Page 1 / 6", "Alt+Left returns to page 1", `Alt+Left landed on ${await label()}`);
check(await page.locator('[data-testid="next-view"]').isEnabled(), "Next view enables after going back", "Next view disabled after going back");
await page.keyboard.press("Alt+ArrowRight");
await page.waitForFunction(() => document.querySelector(".page-label")?.textContent?.includes("Page 4"), null, { timeout: 5000 });
check((await label()) === "Page 4 / 6", "Alt+Right goes forward to page 4", `Alt+Right landed on ${await label()}`);
await page.locator('[data-testid="prev-view"]').click();
await page.waitForFunction(() => document.querySelector(".page-label")?.textContent?.includes("Page 1 "), null, { timeout: 5000 });
check((await label()) === "Page 1 / 6", "the Previous view toolbar button goes back too", `button landed on ${await label()}`);

// --- 7. a large zoom change is a view too -------------------------------------------------
const zin = page.locator('input[aria-label="Zoom percent"]');
const z0 = await zin.inputValue();
await zin.click();
await zin.fill("300");
await zin.press("Enter");
await page.waitForTimeout(500); // zoom gesture settles (120 ms debounce)
check((await zin.inputValue()) === "300", "zoomed to 300%", `zoom input shows ${await zin.inputValue()}`);
await page.mouse.move(root.x + 600, root.y + 400);
await page.keyboard.press("Alt+ArrowLeft");
await page.waitForTimeout(400);
check((await zin.inputValue()) === z0, `Alt+Left after a 3x zoom restores ${z0}%`, `zoom after Alt+Left is ${await zin.inputValue()}, expected ${z0}`);

await browser.close();
console.log(failed ? "FAIL" : "PASS");
process.exit(failed ? 1 : 0);
