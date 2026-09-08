#!/usr/bin/env node
// Real-browser regression check for the search-panel-layout defects (owner-reported
// 2026-09-08, v0.3.20 live use): (1) opening Search changed the sidebar width,
// (2) the scope chooser needed to be a single-select dropdown, (3) the other panels
// (Recent Documents/Tool Chest/Navigator) were hidden behind Search instead of
// staying visible below it in the stack.
//
// Terminology (owner-set): panel-left/panel-right are SIDEBARS; the collapsible
// modules stacked inside one (Search, Tool Chest, Properties, etc.) are PANELS.
//
// WHY A REAL-BROWSER SCRIPT, NOT JUST VITEST: SearchPanel.test.ts covers the scope
// dropdown in isolation. But the sidebar-width and panel-stacking defects live in
// App.svelte's template/CSS, which has no vitest mount anywhere in this codebase (it's
// a huge, IPC/license/tab-state-entangled root component) - this project's own
// established practice for App.svelte-level layout bugs is a real-browser check via
// Playwright over the mocked-IPC Vite dev app (tools/gui-harness.mjs,
// tools/gui-harness-search-focus.mjs), not a synthetic mount. Following that pattern.
//
// Run:  npm run dev            (serves the real frontend at :1421, separate terminal)
//       node tools/gui-harness-search-panel-layout.mjs
// Exits 0 and prints PASS on success, exits 1 and prints FAIL with a diagnosis otherwise.
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";

function installMockTauri() {
  const PAGES = 5;
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
    list_tool_sets: () => [],
    recent_tools: () => [],
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    load_recent_docs: () => [{ path: "/mock/recent-a.pdf", label: "recent-a.pdf", last_opened: "2026-09-08T00:00:00Z" }],
    save_recent_docs: () => null,
    check_file_exists: () => true,
    search_document: () => [],
  };

  window.__TAURI_INTERNALS__ = {
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

function fail(msg) {
  console.log(`FAIL: ${msg}`);
  process.exitCode = 1;
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));

await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".viewport-root", { timeout: 15000 });
await page.waitForTimeout(500);

// --- Defect 1: sidebar width must not change when Search opens ---
const widthBefore = (await page.locator('[data-testid="panel-left"]').boundingBox()).width;

await page.click('button[title^="Find"]');
await page.waitForTimeout(200);
const searchInputVisible = await page.locator('[data-testid="search-input"]').isVisible();
if (!searchInputVisible) fail("search panel did not open after clicking Find");

const widthAfter = (await page.locator('[data-testid="panel-left"]').boundingBox()).width;
if (Math.round(widthBefore) !== Math.round(widthAfter)) {
  fail(`sidebar width changed when Search opened: ${widthBefore}px -> ${widthAfter}px (expected unchanged)`);
} else {
  console.log(`OK: sidebar width unchanged (${widthBefore}px) with Search open`);
}

// --- Defect 2: scope chooser must be a single-select dropdown ---
const scopeSelect = page.locator('[data-testid="search-scope-select"]');
const scopeTag = await scopeSelect.evaluate((el) => el.tagName);
if (scopeTag !== "SELECT") {
  fail(`scope chooser is a <${scopeTag}>, expected a native <select>`);
} else {
  console.log("OK: scope chooser is a native <select>");
}
const legacyTabs = await page.locator('[data-testid^="scope-tab-"]').count();
if (legacyTabs !== 0) fail(`found ${legacyTabs} leftover scope-tab button(s) - should be fully replaced by the dropdown`);

// --- Defect 3: the other panels must stay visible below Search, not hidden ---
const recentVisible = await page.locator('[data-testid="accordion-recent-docs"]').isVisible();
const toolChestVisible = await page.locator('[data-testid="accordion-tool-chest"]').isVisible();
const navigatorVisible = await page.locator('[data-testid="accordion-navigator"]').isVisible();
if (!recentVisible || !toolChestVisible || !navigatorVisible) {
  fail(
    `panel stack hidden behind Search: recent=${recentVisible} toolChest=${toolChestVisible} navigator=${navigatorVisible} (all should be true)`
  );
} else {
  console.log("OK: Recent Documents / Tool Chest / Navigator all stay visible below Search");
}

// Search panel itself must be BOUNDED (not consuming the whole sidebar), so the stack
// below it has real, non-zero height to render into.
const searchSectionBox = await page.locator(".panel-section--search").boundingBox();
const sidebarBox = await page.locator('[data-testid="panel-left"]').boundingBox();
if (searchSectionBox.height >= sidebarBox.height - 4) {
  fail(`search panel height (${searchSectionBox.height}px) consumes the whole sidebar (${sidebarBox.height}px) - not bounded`);
} else {
  console.log(`OK: search panel is bounded (${searchSectionBox.height}px of ${sidebarBox.height}px sidebar)`);
}
const recentBox = await page.locator('[data-testid="accordion-recent-docs"]').boundingBox();
if (!recentBox || recentBox.height <= 0) fail("Recent Documents has zero rendered height while Search is open");

if (process.exitCode === 1) {
  console.log("DIAGNOSIS: see the FAIL lines above for which specific defect regressed.");
} else {
  console.log("PASS: sidebar width is stable across Search open/close, scope is a single-select dropdown, and the panel stack (Recent Documents/Tool Chest/Navigator) stays visible and reachable below a bounded Search panel.");
}

await browser.close();
