#!/usr/bin/env node
// Real-browser check that the Settings dialog stays usable with the 54-row shortcut table:
// it fits a 1024x640 window, the body scrolls, and Save / Cancel stay visible and clickable.
// Run:  npm run dev  (or HARNESS_URL=...)  then  node tools/gui-harness-settings-dialog.mjs
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";

function installMockTauri() {
  const PAGES = 3;
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
    add_markup: (a) => { (window.__added = window.__added || []).push(a.markup); return null; },
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
    compose_stamp_text: (a) => a.baseText,
    recent_tools: () => JSON.parse(sessionStorage.getItem("__mock_recent") || "[]"),
    record_recent_tool: (a) => { const r = JSON.parse(sessionStorage.getItem("__mock_recent") || "[]").filter((t) => t.id !== a.tool.id); r.unshift(a.tool); sessionStorage.setItem("__mock_recent", JSON.stringify(r)); window.__recorded = (window.__recorded || []).concat(a.tool.name); return null; },
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    load_recent_docs: () => Array.from({ length: 25 }, (_, i) => ({ path: `/mock/recent-${i}.pdf`, label: `recent-${i}.pdf`, last_opened: "2026-09-08T00:00:00Z" })),
    save_recent_docs: () => null,
    check_file_exists: () => true,
    search_document: () => [],
    // Settings persist in localStorage so a page reload stands in for an app restart.
    load_settings: () => JSON.parse(localStorage.getItem("__mock_settings") || "null") || {
      theme: "dark", default_tool: null, measurement_unit: "m", author_name: "", last_window: null,
      recent_colors: [], auto_ocr_on_open: false, viewer_mode: localStorage.getItem("__mock_viewer") || "single", panel_layout: null,
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
function check(cond, good, bad) { console.log(cond ? `OK: ${good}` : `FAIL: ${bad}`); if (!cond) failed = true; }

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
const page = await ctx.newPage();
await page.addInitScript(installMockTauri);
page.on("pageerror", (e) => console.log("PAGEERROR", e.message));
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".tool-strip", { timeout: 15000 });
await page.locator('button[title*="Settings" i], button[aria-label*="Settings" i]').first().click();
await page.waitForSelector('[role="dialog"][aria-label="Settings"]');
await page.setViewportSize({ width: 1024, height: 640 });
await page.waitForTimeout(300);
await page.waitForSelector("#settings-reuse-tools");
const box = await page.locator('[role="dialog"][aria-label="Settings"]').boundingBox();
check(box.height <= 640 * 0.91, `dialog height ${Math.round(box.height)} fits the 640px window`, `dialog is ${Math.round(box.height)}px tall in a 640px window`);
check(box.y >= 0 && box.y + box.height <= 640, "dialog is fully inside the window", "dialog spills outside the window");
for (const name of ["Save", "Cancel"]) {
  const b = page.locator('[role="dialog"][aria-label="Settings"]').getByRole("button", { name, exact: true });
  const bb = await b.boundingBox();
  check(bb && bb.y >= 0 && bb.y + bb.height <= 640, `${name} is inside the window`, `${name} is not visible`);
}
await page.locator("#settings-reuse-tools").click({ timeout: 3000 });
check(true, "Reuse checkbox is clickable by a real pointer", "");
const scrolls = await page.locator(".dialog-body").evaluate((el) => el.scrollHeight > el.clientHeight);
check(scrolls, "the dialog body scrolls", "the dialog body does not scroll");
await page.locator('[role="dialog"][aria-label="Settings"]').getByRole("button", { name: "Save", exact: true }).click({ timeout: 3000 });
await page.waitForSelector('[role="dialog"][aria-label="Settings"]', { state: "detached", timeout: 3000 });
check(true, "Save closes the dialog", "");
await browser.close();
console.log(failed ? "FAIL" : "PASS");
process.exit(failed ? 1 : 0);
