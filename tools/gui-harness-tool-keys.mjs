#!/usr/bin/env node
// Real-browser check of the Revu-aligned tool keys, the Stamp tool, reading-mode disabling and
// Recent Tools (benchmark must-fix items 1, 3, 5, 6; /Users/martin/dev-reports/2026-11-04/
// redline-ux-benchmark.md). The real Svelte app in headless Chromium over a MOCK Tauri IPC.
//
// Checks:
//   1. every Revu tool key selects its tool in the palette (aria-pressed), incl. Shift variants;
//   2. keys do nothing while a text input has focus (the zoom-percent field);
//   3. Z hides the zoom toolbar, T no longer does (T is Text Box);
//   4. S opens the Stamp picker; choosing "Approved" and clicking the page places a
//      StampDynamic markup;
//   5. palette clicks and key presses both land in the Tool Chest Recent list;
//   6. in reading mode every palette tool is disabled and the hint is visible.
//
// Run:  npm run dev                       (serves the frontend at :1421)
//       node tools/gui-harness-tool-keys.mjs
//       HARNESS_URL=http://localhost:1431/ node tools/gui-harness-tool-keys.mjs
// Exits 0 and prints PASS on success; exits 1 with FAIL lines otherwise.
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
function fail(msg) { console.log(`FAIL: ${msg}`); failed = true; }
function ok(msg) { console.log(`OK: ${msg}`); }
function check(cond, good, bad) { cond ? ok(good) : fail(bad); }

async function openApp(browser, viewer) {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  await page.addInitScript((v) => { localStorage.setItem("__mock_viewer", v); }, viewer);
  await page.addInitScript(installMockTauri);
  page.on("pageerror", (e) => console.log("PAGEERROR", e.message));
  await page.goto(URL, { waitUntil: "load" });
  await page.waitForSelector(".tool-strip", { timeout: 15000 });
  await page.waitForTimeout(500);
  return page;
}

/** Titles of the pressed palette buttons. */
const pressed = (page) =>
  page.locator('.tool-strip .tool-btn[aria-pressed="true"]').evaluateAll((els) => els.map((e) => e.getAttribute("title")));

const browser = await chromium.launch();

// --- 1-5: page view -----------------------------------------------------------------------
{
  const page = await openApp(browser, "single");
  await page.waitForSelector(".viewport-root", { timeout: 15000 });
  await page.locator(".viewport-root").click({ position: { x: 5, y: 5 } }).catch(() => {});

  const KEYS = [
    ["r", "Rectangle (R)"], ["e", "Ellipse (E)"], ["l", "Line (L)"], ["a", "Arrow (A)"],
    ["h", "Highlight (H)"], ["c", "Cloud (C)"], ["y", "Cloud (C)"], ["k", "Cloud+ (K)"],
    ["q", "Callout (Q)"], ["t", "Text Box (T)"], ["n", "Sticky Note (N)"], ["p", "Pen (freehand) (P)"],
    ["m", "Measure Length (M)"], ["u", "Underline text (U)"], ["d", "Strikethrough text (D)"],
    ["Shift+P", "Polygon (Shift+P)"], ["Shift+N", "Polyline (Shift+N)"], ["Shift+T", "Select Text"],
    ["Shift+U", "Squiggly underline (Shift+U)"], ["v", "Select / Pointer (V)"],
  ];
  for (const [key, titlePrefix] of KEYS) {
    await page.keyboard.press("Escape");
    await page.keyboard.press(key);
    await page.waitForTimeout(40);
    const p = await pressed(page);
    check(p.length === 1 && p[0].startsWith(titlePrefix), `${key} selects ${titlePrefix}`, `${key}: expected ${titlePrefix}, pressed = ${JSON.stringify(p)}`);
  }

  // 2. typing in the zoom-percent input must not change the tool
  await page.keyboard.press("Escape");
  await page.keyboard.press("v");
  const before = await pressed(page);
  await page.locator(".zoom-percent-input").focus();
  for (const k of ["r", "c", "t", "s", "q"]) await page.keyboard.press(k);
  const after = await pressed(page);
  check(JSON.stringify(before) === JSON.stringify(after) && (await page.locator('[data-testid="stamp-picker"]').count()) === 0,
    "keys typed into the zoom input do not change the tool or open the stamp picker", `typing leaked: ${JSON.stringify(before)} -> ${JSON.stringify(after)}`);
  await page.locator(".zoom-percent-input").blur();
  await page.evaluate(() => document.activeElement && document.activeElement.blur());

  // 3. Z toggles the zoom toolbar, T does not
  const toolbarVisible = () => page.locator(".zoom-controls").isVisible();
  const v0 = await toolbarVisible();
  await page.keyboard.press("t");
  const vT = await toolbarVisible();
  await page.keyboard.press("z");
  const vZ = await toolbarVisible();
  check(v0 && vT && !vZ, "Z hides the zoom toolbar; T (Text Box) leaves it alone", `toolbar visibility start/T/Z = ${v0}/${vT}/${vZ}`);
  await page.keyboard.press("z");

  // 4. Stamp: S opens the picker, choosing Approved then clicking places a StampDynamic
  await page.keyboard.press("Escape");
  await page.keyboard.press("s");
  await page.waitForSelector('[data-testid="stamp-picker"]', { timeout: 3000 }).catch(() => {});
  const items = await page.locator('[data-testid="stamp-picker"] [role="menuitem"]').allTextContents();
  check(items.map((t) => t.trim()).join("|") === "Approved|Rejected|Revise and Resubmit|Reviewed|For Information",
    "S opens the Stamp picker with the five built-in stamps", `picker items: ${JSON.stringify(items)}`);
  await page.getByRole("menuitem", { name: "Approved" }).click();
  const tb = await page.locator(".markup-overlay").boundingBox();
  await page.mouse.click(tb.x + tb.width / 2, tb.y + 200);
  await page.waitForTimeout(500);
  const added = await page.evaluate(() => window.__added || []);
  const st = added.find((m) => m.markup_type === "StampDynamic");
  check(!!st && /APPROVED/.test(st.contents || ""), `Approved stamp placed (markup_type StampDynamic, contents "${st && st.contents}")`,
    `no stamp markup added: ${JSON.stringify(added.map((m) => [m.markup_type, m.contents]))}`);

  // 5. Recent: palette click + key press both recorded, shown in the Tool Chest
  await page.keyboard.press("Escape");
  await page.locator('.tool-strip .tool-btn[title^="Ellipse"]').click();
  await page.keyboard.press("r");
  await page.waitForTimeout(300);
  const rec = await page.evaluate(() => window.__recorded || []);
  check(rec.includes("Ellipse") && rec.includes("Rectangle") && rec.includes("Approved"),
    `Recent recorded palette click, key press and stamp (${JSON.stringify(rec)})`, `recorded = ${JSON.stringify(rec)}`);
  const chips = await page.locator(".tc-tool-chip .tc-tool-name").allTextContents();
  check(chips.includes("Rectangle") && chips.includes("Ellipse"), "Tool Chest Recent lists them", `Recent chips: ${JSON.stringify(chips)}`);
  await page.context().close();
}

// --- 6: reading mode --------------------------------------------------------------------
{
  const page = await openApp(browser, "reading");
  await page.waitForTimeout(500);
  const states = await page.locator(".tool-strip .tool-btn").evaluateAll((els) => els.map((e) => e.disabled));
  check(states.length > 20 && states.every(Boolean), `reading mode: all ${states.length} palette buttons disabled`, `reading mode: disabled = ${JSON.stringify(states)}`);
  const hint = (await page.locator('[data-testid="palette-read-only-hint"]').textContent().catch(() => "")) || "";
  check(hint.trim() === "Switch to page view to mark up", `hint shown: "${hint.trim()}"`, `hint missing/wrong: "${hint}"`);
  await page.context().close();
}

await browser.close();
if (failed) {
  console.log("FAIL: see lines above.");
  process.exit(1);
}
console.log("PASS: Revu tool keys select their tools and stay inert while typing; Z (not T) toggles the zoom toolbar; the Stamp picker places a stamp; palette and key use feed Recent; reading mode disables the palette with a hint.");
