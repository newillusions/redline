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
 * Exits 0 and prints PASS on success; exits 1 with FAIL lines otherwise.
 * Run:  node tools/gui-harness.mjs            (needs `npm run dev` / tauri:dev serving :1421)
 *       OUT=/tmp/h node tools/gui-harness.mjs  (screenshot dir; default /tmp)
 */
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";
const OUT = process.env.OUT || "/tmp";
const WIDTH = Number(process.env.HARNESS_WIDTH) || 1280;

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
    // license_status has no handler here since this harness predates S2b (PR #49,
    // 2026-07-08) - the missing entry falls through to Promise.resolve(null), and
    // App.svelte's `licenseState.state !== "valid"` throws on that null, which never
    // resolves .viewport-root (confirmed 2026-08-05 during a GUI validation pass).
    // Mocked "valid" here so the harness can mount the app again.
    license_status: () => ({
      state: "valid", staff_id: "harness", expires_at: "2099-01-01T00:00:00Z",
      days_remaining: 9999, renew_due: false,
    }),
    list_scales: () => [],
    get_page_snap_targets: () => [],
    list_tool_sets: () => [],
    recent_tools: () => [],
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    load_recent_docs: () => [],
    save_recent_docs: () => null,
    check_file_exists: () => true,
    // The panel layout loads from settings; null would raise "Could not load the panel layout".
    load_settings: () => ({
      theme: "dark", default_tool: null, measurement_unit: "m", author_name: "", last_window: null,
      recent_colors: [], auto_ocr_on_open: false, viewer_mode: "single", panel_layout: null,
    }),
    save_settings: () => null,
    // VersionPanel iterates this; a null return crashes the sidebar.
    list_document_versions: () => [],
  };

  window.__TAURI_INTERNALS__ = {
    // getCurrentWindow() reads these (the close guard in App.svelte).
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
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
const fail = (msg) => { console.log(`FAIL: ${msg}`); failed = true; };

const shot = async (page, name) => {
  const p = `${OUT}/harness-${name}.png`;
  await page.screenshot({ path: p });
  console.log("SHOT", p);
};

const browser = await chromium.launch();

async function openApp(dpr, width, height = 900) {
  const page = await browser.newPage({ deviceScaleFactor: dpr, viewport: { width, height } });
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
  return page;
}

const intersects = (a, b) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

// Layout check: the zoom toolbar must not overlap the page-nav bar (or leave the viewport)
// at any width, at both dpr 1 and 2.5. Regression: at 1280 with both sidebars open the
// toolbar sat on top of the page-nav buttons.
async function checkToolbarOverlap(dpr, width) {
  const page = await openApp(dpr, width);
  const label = `${width}px dpr ${dpr}`;
  const nav = await page.$(".page-nav");
  const bar = await page.$(".zoom-controls-wrap");
  const root = await page.$(".viewport-root");
  if (!nav || !bar || !root) {
    fail(`${label}: missing element (.page-nav=${!!nav} .zoom-controls-wrap=${!!bar} .viewport-root=${!!root})`);
    await page.close();
    return;
  }
  const n = await nav.boundingBox();
  const b = await bar.boundingBox();
  const r = await root.boundingBox();
  if (!n || !b || !r) {
    fail(`${label}: element has no bounding box`);
  } else {
    if (intersects(n, b)) fail(`${label}: zoom toolbar overlaps page nav (nav ${JSON.stringify(n)} toolbar ${JSON.stringify(b)})`);
    if (b.x < r.x - 0.5 || b.x + b.width > r.x + r.width + 0.5) {
      fail(`${label}: zoom toolbar leaves the viewport horizontally (toolbar ${JSON.stringify(b)} viewport ${JSON.stringify(r)})`);
    }
    // Every button of both bars must stay clickable, i.e. not covered by the other bar.
    for (const sel of [".page-nav button", ".zoom-controls button"]) {
      for (const btn of await page.$$(sel)) {
        const bb = await btn.boundingBox();
        const other = sel.startsWith(".page-nav") ? b : n;
        if (bb && intersects(bb, other)) fail(`${label}: ${sel} button overlaps the other bar`);
      }
    }
    console.log(`overlap ${label}: nav y=${Math.round(n.y)} h=${Math.round(n.height)}, toolbar y=${Math.round(b.y)} h=${Math.round(b.height)} w=${Math.round(b.width)}`);
  }
  await page.close();
}

const page = await openApp(2, WIDTH);
await shot(page, "100");

// Zoom in with the wheel, centred on the viewport.
const vp = await page.$(".viewport-root");
if (!vp) {
  fail("missing .viewport-root");
} else {
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

  // Pan a bit (drag with the Hand tool - default tool is hand).
  await page.mouse.move(cx, cy);
  await page.mouse.down();
  await page.mouse.move(cx - 150, cy - 120, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(300);
  await shot(page, "panned");
}

// Next page (rapid double-click to also probe the stale-tile race).
// The Next-page button follows the page label; the last .btn-nav is "Next view".
const label = () => page.$eval(".page-label", (e) => e.textContent.replace(/\s+/g, " ").trim()).catch(() => null);
const next = await page.$(".page-nav .page-label + .btn-nav");
if (!next) {
  fail("Next-page button (.page-nav .page-label + .btn-nav) not found");
} else {
  const before = await label();
  await next.click();
  await next.click();
  await page.waitForTimeout(500);
  await shot(page, "page-switch");
  const after = await label();
  if (after !== "Page 3 / 5") fail(`expected "Page 3 / 5" after two Next clicks, got ${JSON.stringify(after)} (was ${JSON.stringify(before)})`);
}
await page.close();

// Overlays inside .viewport-root must stay relative to the WINDOW: a context menu opens at the
// click and a modal backdrop covers the whole window, with both sidebars open (the viewport is
// then far narrower than the window). Guards against anything on .viewport-root (containment,
// transform, filter) becoming the containing block of position:fixed descendants.
{
  const w = WIDTH;
  const h = 900;
  const op = await openApp(1, w, h);
  const root = await op.$(".viewport-root");
  const rb = root ? await root.boundingBox() : null;
  if (!rb) {
    fail("overlay check: .viewport-root missing");
  } else {
    if (rb.x < 50) fail(`overlay check: expected sidebars to offset the viewport, got x=${rb.x}`);
    const px = Math.round(rb.x + rb.width / 3);
    const py = Math.round(rb.y + rb.height / 3);
    await op.mouse.click(px, py, { button: "right" });
    const menu = op.locator('[data-testid="context-menu"]');
    await menu.waitFor({ timeout: 3000 }).catch(() => {});
    const mb = (await menu.count()) ? await menu.boundingBox() : null;
    if (!mb) fail("overlay check: right-click did not open the context menu");
    else if (Math.abs(mb.x - px) > 4 || Math.abs(mb.y - py) > 4) {
      fail(`overlay check: context menu at (${mb.x},${mb.y}), click at (${px},${py})`);
    } else console.log(`context menu at click point OK (${mb.x},${mb.y})`);
    await op.keyboard.press("Escape");

    const tool = op.locator('.tool-strip .tool-btn[title^="Viewport"]');
    if ((await tool.count()) !== 1) fail("overlay check: Viewport tool button not found");
    else {
      await tool.click();
      await op.mouse.move(px, py);
      await op.mouse.down();
      await op.mouse.move(px + 60, py + 50, { steps: 4 });
      await op.mouse.move(px + 120, py + 100, { steps: 4 });
      await op.mouse.up();
      const dlg = op.getByRole("dialog", { name: "New Viewport" });
      await dlg.waitFor({ timeout: 3000 }).catch(() => {});
      const bd = op.locator(".dialog-backdrop");
      const bb = (await bd.count()) ? await bd.first().boundingBox() : null;
      if (!bb) fail("overlay check: dragging with the Viewport tool did not open a dialog backdrop");
      else if (Math.abs(bb.x) > 1 || Math.abs(bb.y) > 1 || bb.width < w - 1 || bb.height < h - 1) {
        fail(`overlay check: dialog backdrop ${JSON.stringify(bb)} does not cover the ${w}x${h} window`);
      } else console.log("dialog backdrop covers the window OK");
    }
  }
  await op.close();
}

for (const dpr of [1, 2.5]) {
  for (const width of [1024, 1280, 1600]) await checkToolbarOverlap(dpr, width);
}

await browser.close();
if (failed) {
  console.log("FAIL: see lines above.");
  process.exit(1);
}
console.log("PASS");
