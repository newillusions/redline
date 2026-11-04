#!/usr/bin/env node
// Real-browser regression check for the sidebar/panel layout (owner defect 2026-11-04:
// "the tool chests in the side bars cannot be resized or repositioned, and when open they
// often cannot show their overflow").
//
// Root cause of the clipping it guards: Accordion's scoped `.accordion-body { overflow:
// hidden }` out-ranked the global `.panel-body-flush { overflow-y: auto }`, so no sidebar
// panel body could scroll, and every panel was flex:1 with no minimum height.
//
// Checks, at 1280x800, 1920x1080 and 1024x640@dpr 2.5 with an overfull mock (12 tool sets
// x 8 tools, 25 recent docs, 40 bookmarks, 40 pages, 15 versions):
//   1. every expanded panel's last item can be scrolled into view inside the panel;
//   2. the sidebar width handle resizes (mouse and keyboard) within its clamps;
//   3. the splitter between two panels moves height between just those two;
//   4. a panel moves to the other sidebar via its menu, and by dragging its grip;
//   5. collapse, width and placement survive a reload (= app restart: settings persist).
//
// Same mocked-IPC Vite pattern as tools/gui-harness-search-panel-layout.mjs.
// Run:  npm run dev                       (serves the frontend at :1421)
//       node tools/gui-harness-sidebar-layout.mjs
//       HARNESS_URL=http://localhost:1431/ node tools/gui-harness-sidebar-layout.mjs
// Exits 0 and prints PASS on success; exits 1 with FAIL lines otherwise.
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";

function installMockTauri() {
  const PAGES = 40;
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
    list_markups: () => [],
    add_markup: () => null,
    update_markup: () => null,
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

async function openApp(page) {
  await page.addInitScript(installMockTauri);
  await page.goto(URL, { waitUntil: "load" });
  await page.waitForSelector(".viewport-root", { timeout: 15000 });
  await page.waitForSelector('[data-panel-id="toolchest"] .tc-set', { timeout: 10000 });
  await page.waitForTimeout(400);
}

/** For every expanded panel: scroll its scroll container to the end and report whether the
 *  last content element is then fully inside the panel body. */
function lastItemsReachable() {
  const out = [];
  for (const frame of document.querySelectorAll("[data-panel-id]")) {
    const body = frame.querySelector(".panel-frame-body");
    if (!body) continue;
    const els = [body, ...body.querySelectorAll("*")];
    for (const e of els) {
      const s = getComputedStyle(e).overflowY;
      if ((s === "auto" || s === "scroll") && e.scrollHeight > e.clientHeight) e.scrollTop = e.scrollHeight;
    }
    const leaves = Array.from(body.querySelectorAll("*")).filter((e) => e.children.length === 0 && e.getBoundingClientRect().height > 0);
    const last = leaves.reduce((a, e) => (!a || e.getBoundingClientRect().bottom > a.getBoundingClientRect().bottom ? e : a), null);
    const bb = body.getBoundingClientRect();
    const lb = last ? last.getBoundingClientRect() : bb;
    // Leaves inside a scroller are measured after scrolling; anything still below the body
    // bottom is clipped with no way to reach it.
    let deepest = 0;
    for (const e of leaves) deepest = Math.max(deepest, e.getBoundingClientRect().bottom);
    out.push({ id: frame.dataset.panelId, bodyH: Math.round(bb.height), clippedPx: Math.max(0, Math.round(deepest - bb.bottom - 1)), lastText: (last?.textContent || "").trim().slice(0, 30), lastBottom: Math.round(lb.bottom) });
  }
  return out;
}

const browser = await chromium.launch();

// --- 1. overflow reachable at three window sizes ---------------------------------------
for (const [w, h, dpr] of [[1280, 800, 1], [1920, 1080, 1], [1024, 640, 2.5]]) {
  const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: dpr });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => fail(`page error: ${e.message}`));
  await openApp(page);
  const rows = await page.evaluate(lastItemsReachable);
  const bad = rows.filter((r) => r.clippedPx > 0);
  check(bad.length === 0 && rows.length >= 6,
    `${w}x${h}@${dpr}: all ${rows.length} expanded panels scroll to their last item`,
    `${w}x${h}@${dpr}: clipped panels ${JSON.stringify(bad)}`);
  const tiny = rows.filter((r) => r.bodyH < 40);
  check(tiny.length === 0, `${w}x${h}@${dpr}: no panel body squashed below 40px`, `${w}x${h}@${dpr}: squashed ${JSON.stringify(tiny)}`);
  await ctx.close();
}

const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
const page = await ctx.newPage();
page.on("pageerror", (e) => fail(`page error: ${e.message}`));
await openApp(page);
const left = page.locator('[data-testid="panel-left"]');
const right = page.locator('[data-testid="panel-right"]');

// --- 2. sidebar width: drag + keyboard + clamp -----------------------------------------
const w0 = (await left.boundingBox()).width;
const hb = await page.locator('[data-testid="resize-sidebar-left"]').boundingBox();
await page.mouse.move(hb.x + hb.width / 2, hb.y + 200);
await page.mouse.down();
await page.mouse.move(hb.x + 80, hb.y + 200, { steps: 8 });
await page.mouse.up();
const w1 = (await left.boundingBox()).width;
check(Math.abs(w1 - w0 - 80) <= 2, `left sidebar dragged wider ${w0} -> ${w1}`, `drag resize wrong: ${w0} -> ${w1} (expected +80)`);
await page.locator('[data-testid="resize-sidebar-left"]').focus();
await page.keyboard.press("ArrowLeft");
const w2 = (await left.boundingBox()).width;
check(Math.abs(w1 - w2 - 16) <= 1, `ArrowLeft narrows by 16px (${w1} -> ${w2})`, `keyboard resize wrong: ${w1} -> ${w2}`);
for (let i = 0; i < 40; i++) await page.keyboard.press("Shift+ArrowLeft");
const wMin = (await left.boundingBox()).width;
check(Math.abs(wMin - 180) <= 1, `width clamps at the 180px minimum`, `min clamp wrong: ${wMin}`);
for (let i = 0; i < 40; i++) await page.keyboard.press("Shift+ArrowRight");
const wMax = (await left.boundingBox()).width;
check(wMax <= 1280 * 0.4 + 1, `width clamps at 40% of the window (${wMax}px)`, `max clamp wrong: ${wMax}`);

// --- 3. splitter between Recent Documents and Tool Chest -------------------------------
// Collapse two panels first so the pair has room above their 96px minimum to trade.
await page.click('[data-testid="accordion-navigator"]');
await page.click('[data-testid="accordion-history"]');
const hBefore = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll("[data-panel-id]")].map((f) => [f.dataset.panelId, Math.round(f.getBoundingClientRect().height)])));
const sp = await page.locator('[data-testid="resize-split-recent"]').boundingBox();
await page.mouse.move(sp.x + 50, sp.y + sp.height / 2);
await page.mouse.down();
await page.mouse.move(sp.x + 50, sp.y + 60, { steps: 6 });
await page.mouse.up();
const hAfter = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll("[data-panel-id]")].map((f) => [f.dataset.panelId, Math.round(f.getBoundingClientRect().height)])));
const others = Object.keys(hBefore).filter((k) => k !== "recent" && k !== "toolchest" && Math.abs(hBefore[k] - hAfter[k]) > 1);
const pairKept = Math.abs(hAfter.recent + hAfter.toolchest - hBefore.recent - hBefore.toolchest) <= 2;
check(hAfter.recent >= hBefore.recent + 45 && hAfter.toolchest <= hBefore.toolchest - 45 && pairKept && others.length === 0,
  `splitter moved height Recent ${hBefore.recent}->${hAfter.recent}, Tool Chest ${hBefore.toolchest}->${hAfter.toolchest}, others unchanged`,
  `splitter wrong: before ${JSON.stringify(hBefore)} after ${JSON.stringify(hAfter)}`);

// The split can never push a panel below its minimum.
await page.mouse.move(sp.x + 50, sp.y + 60 + sp.height / 2);
await page.mouse.down();
await page.mouse.move(sp.x + 50, sp.y + 900, { steps: 6 });
await page.mouse.up();
const tcMin = Math.round((await page.locator('[data-panel-id="toolchest"]').boundingBox()).height);
check(tcMin >= 95 && tcMin <= 97, `split stops at the 96px panel minimum (${tcMin}px)`, `split went below minimum: ${tcMin}px`);
await page.click('[data-testid="accordion-navigator"]');
await page.click('[data-testid="accordion-history"]');

// --- 4a. move via the panel menu --------------------------------------------------------
await page.click('[data-testid="panel-menu-layers"]');
await page.getByRole("menuitem", { name: "Move to right sidebar" }).click();
check(await right.locator('[data-panel-id="layers"]').count() === 1 && await left.locator('[data-panel-id="layers"]').count() === 0,
  "Layers moved to the right sidebar from its menu", "menu move to right sidebar failed");

// --- 4b. move by dragging the grip into the other sidebar ------------------------------
const grip = await page.locator('[data-testid="panel-grip-toolchest"]').boundingBox();
const props = await right.locator('[data-panel-id="properties"]').boundingBox();
await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
await page.mouse.down();
await page.mouse.move(props.x + props.width / 2, props.y + 5, { steps: 10 });
check(await right.locator(".drop-marker").count() === 1, "a drop marker shows in the target sidebar while dragging", "no drop marker while dragging");
await page.mouse.up();
const rightOrder = await right.locator("[data-panel-id]").evaluateAll((els) => els.map((e) => e.dataset.panelId));
check(rightOrder[0] === "toolchest", `Tool Chest dragged to the top of the right sidebar (${rightOrder.join(",")})`, `drag move failed: right = ${rightOrder.join(",")}`);

// --- 4c. keyboard on the grip ------------------------------------------------------------
await page.locator('[data-testid="panel-grip-toolchest"]').focus();
await page.keyboard.press("ArrowDown");
const rightOrder2 = await right.locator("[data-panel-id]").evaluateAll((els) => els.map((e) => e.dataset.panelId));
check(rightOrder2[1] === "toolchest", `ArrowDown on the grip moves the panel down (${rightOrder2.join(",")})`, `grip keyboard move failed: ${rightOrder2.join(",")}`);

// --- 5. collapse + persistence across reload ---------------------------------------------
await page.click('[data-testid="accordion-bookmarks"]');
check(await page.locator('[data-panel-id="bookmarks"] .panel-frame-body').count() === 0, "Bookmarks collapsed", "collapse failed");
const widthBeforeReload = (await left.boundingBox()).width;
await page.waitForTimeout(800); // debounce
const saves = await page.evaluate(() => window.__saves || 0);
check(saves >= 1 && saves <= 6, `layout saved ${saves} time(s) (debounced)`, `unexpected save count ${saves}`);
await page.reload({ waitUntil: "load" });
await page.waitForSelector('[data-panel-id="toolchest"]', { timeout: 15000 });
await page.waitForTimeout(400);
const persisted = {
  width: (await left.boundingBox()).width,
  right: await right.locator("[data-panel-id]").evaluateAll((els) => els.map((e) => e.dataset.panelId)),
  bookmarksCollapsed: await page.locator('[data-panel-id="bookmarks"] .panel-frame-body').count() === 0,
};
check(Math.abs(persisted.width - widthBeforeReload) <= 1 && persisted.right.join(",") === rightOrder2.join(",") && persisted.bookmarksCollapsed,
  `layout survived a restart (${JSON.stringify(persisted)})`, `layout not restored: ${JSON.stringify(persisted)} expected width ${widthBeforeReload}, right ${rightOrder2.join(",")}`);

// --- bottom panel height ------------------------------------------------------------------
await page.click('button[aria-label="Toggle markups list / quantities panel"]');
const bottom = page.locator('[data-testid="bottom-panel"]');
const b0 = (await bottom.boundingBox()).height;
const bh = await page.locator('[data-testid="resize-bottom-panel"]').boundingBox();
await page.mouse.move(bh.x + 300, bh.y + bh.height / 2);
await page.mouse.down();
await page.mouse.move(bh.x + 300, bh.y - 100, { steps: 6 });
await page.mouse.up();
const b1 = (await bottom.boundingBox()).height;
check(Math.abs(b1 - b0 - 100) <= 2, `bottom panel dragged taller ${b0} -> ${b1}`, `bottom resize wrong: ${b0} -> ${b1}`);

// --- reset -------------------------------------------------------------------------------
await page.click('[data-testid="panel-menu-toolchest"]');
await page.getByRole("menuitem", { name: "Reset panel layout" }).click();
const afterReset = await right.locator("[data-panel-id]").evaluateAll((els) => els.map((e) => e.dataset.panelId));
check(afterReset.join(",") === "properties", "Reset panel layout restores the default", `reset failed: ${afterReset.join(",")}`);

await browser.close();
if (failed) {
  console.log("FAIL: see lines above.");
  process.exit(1);
}
console.log("PASS: every sidebar panel scrolls to its last item at 1280x800, 1920x1080 and 1024x640@2.5; sidebars and panel splits resize (mouse + keyboard, clamped); panels move by menu, grip drag and grip keys; the layout persists across a restart and resets.");
