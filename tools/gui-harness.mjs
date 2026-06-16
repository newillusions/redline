/**
 * GUI harness — drive the real Svelte frontend in headless Chromium with a MOCK Tauri IPC.
 *
 * Why: the Tauri WKWebView can't be driven by cliclick, and the test suite mocks invoke
 * without rendering. This loads the actual Vite dev app (http://localhost:1421) and installs
 * a window.__TAURI_INTERNALS__ shim that returns SYNTHETIC labelled tiles + fake docs, so we
 * can script zoom / pan / page-nav / tool interactions and screenshot the result — verifying
 * frontend render-loop + interaction behaviour (the bugs live there, not in real PDF pixels).
 *
 * Tiles are checker-coloured and labelled "P{page} t{tx},{ty} z{zoom}" so position, page,
 * zoom, seams and stale-tile races are all visually obvious.
 *
 * Run:  node tools/gui-harness.mjs            (needs `npm run dev` / tauri:dev serving :1421)
 *       OUT=/tmp/h node tools/gui-harness.mjs  (screenshot dir; default /tmp)
 */
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";
const OUT = process.env.OUT || "/tmp";

// Runs in the PAGE context, before the app boots. Must be self-contained (no closures over
// Node scope). Sets up the mock Tauri IPC.
function installMockTauri() {
  const PAGES = 5;
  const PAGE_W = 612; // pts (US Letter) — wide enough to span multiple tiles when zoomed
  const PAGE_H = 792;

  function tilePng(req) {
    const { zoom, dpr, tile_size_css: css, tile_x: tx, tile_y: ty, page_index: pg } = req;
    const scale = zoom * dpr;
    const tilePx = Math.round(css * dpr);
    const fullW = Math.floor(PAGE_W * scale);
    const fullH = Math.floor(PAGE_H * scale);
    const ox = tx * tilePx;
    const oy = ty * tilePx;
    const w = Math.max(1, Math.min(tilePx, fullW - ox));
    const h = Math.max(1, Math.min(tilePx, fullH - oy));
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    const g = c.getContext("2d");
    const even = (tx + ty) % 2 === 0;
    g.fillStyle = even ? "#eef3ff" : "#dbe6ff";
    g.fillRect(0, 0, w, h);
    g.strokeStyle = "#2244aa";
    g.lineWidth = 2;
    g.strokeRect(1, 1, w - 2, h - 2);
    g.fillStyle = "#102040";
    g.font = `${Math.round(16 * dpr)}px sans-serif`;
    g.fillText(`P${pg} t${tx},${ty} z${zoom.toFixed(2)}`, 8, Math.round(26 * dpr));
    return { w, h, b64: c.toDataURL("image/png").split(",")[1] };
  }

  const H = {
    auto_open_path: () => "/mock/contract.pdf",
    open_document: (a) => ({ doc_id: "mock-doc", path: a.path || "/mock/contract.pdf", page_count: PAGES }),
    close_document: () => null,
    get_page_count: () => PAGES,
    get_page_size: (a) => ({ doc_id: a.docId, page_index: a.pageIndex, width_pts: PAGE_W, height_pts: PAGE_H }),
    render_tile: (a) => {
      const r = a.req;
      const t = tilePng(r);
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

const shot = async (page, name) => {
  const p = `${OUT}/harness-${name}.png`;
  await page.screenshot({ path: p });
  console.log("SHOT", p);
};

const browser = await chromium.launch();
const page = await browser.newPage({ deviceScaleFactor: 2, viewport: { width: 1280, height: 800 } });
page.on("console", (m) => {
  const t = m.text();
  if (m.type() === "error" || m.type() === "warning" || t.startsWith("MOCK_INVOKE_UNHANDLED")) {
    console.log(`[page:${m.type()}]`, t);
  }
});
page.on("pageerror", (e) => console.log("[pageerror]", e.message));

await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });

// Wait for the viewport to mount (doc auto-opens via mock auto_open_path).
await page.waitForSelector(".viewport-root", { timeout: 15000 });
await page.waitForTimeout(1200);
await shot(page, "100");

// Zoom in with the wheel, centred on the viewport.
const vp = await page.$(".viewport-root");
const box = await vp.boundingBox();
const cx = box.x + box.width / 2;
const cy = box.y + box.height / 2;
await page.mouse.move(cx, cy);
for (let i = 0; i < 6; i++) {
  await page.mouse.wheel(0, -120);
  await page.waitForTimeout(50);
}
await page.waitForTimeout(400);
await shot(page, "zoomin");

// Pan a bit (drag with the Hand tool — default tool is hand).
await page.mouse.move(cx, cy);
await page.mouse.down();
await page.mouse.move(cx - 150, cy - 120, { steps: 8 });
await page.mouse.up();
await page.waitForTimeout(300);
await shot(page, "panned");

// Next page (rapid double-click to also probe the stale-tile race).
const next = await page.$(".page-nav .btn-nav:last-child");
if (next) {
  await next.click();
  await next.click();
  await page.waitForTimeout(500);
  await shot(page, "page-switch");
}

await browser.close();
console.log("DONE");
