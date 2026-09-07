#!/usr/bin/env node
// Real-browser regression check for the search-input-focus-loss defect (owner-reported
// 2026-09-07, v0.3.18: "the highlight and jump to result functions aren't working" after
// a search that DOES return hits).
//
// WHY THIS SCRIPT EXISTS, NOT JUST A VITEST TEST: SearchPanel.test.ts asserts the root
// cause directly (the input must never carry `disabled` while a search is in flight) —
// that's the fast, CI-enforced (`npm test`) regression guard. But the actual FAILURE
// MODE — a disabled element loses keyboard focus, and nothing restores it — is a real
// browser behavior jsdom does not model (confirmed: a jsdom-mounted SearchPanel keeps
// `document.activeElement === input` across the exact same searching-state transition
// that blurs it to `<body>` in real Chromium/WebKit/WebView2). Six prior render-loop
// bugs in this project (IPC casing, flipped matrix, DPR scaling, zoom runaway, sub-pixel
// seams — see CLAUDE.md "Build Order") were all invisible to headless tests and only
// surfaced on a real `cargo tauri dev`/browser session; this is the same class. This
// script is the real-browser proof, following tools/gui-harness.mjs's existing pattern
// (mocked Tauri IPC over the real Vite dev app, driven with Playwright).
//
// SYMPTOM MAPPED: mouse clicks on a result row and the global F3/Shift+F3 shortcut
// (App.svelte, window-level `<svelte:window onkeydown>`) are UNAFFECTED by this bug —
// both were verified working in every scenario tried during triage. The break is
// specifically pressing Enter a second time *inside the search input* (SearchPanel's own
// `handleKeydown`, bound to that element's `onkeydown`) once a search has completed at
// least once — which is also the single most natural "type a query, hit Enter, hit Enter
// again to jump" flow, so it plausibly reads to a user as "jump/highlight aren't working"
// even though it isn't the only navigation path in the app.
//
// Run:  npm run dev            (serves the real frontend at :1421, separate terminal)
//       node tools/gui-harness-search-focus.mjs
// Exits 0 and prints PASS on success, exits 1 and prints FAIL with a diagnosis otherwise.
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";

// Runs in the PAGE context (window.__TAURI_INTERNALS__ shim) — mirrors tools/gui-harness.mjs's
// mock, trimmed to what this scenario needs plus a canned single search hit.
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
    load_recent_docs: () => [],
    save_recent_docs: () => null,
    check_file_exists: () => true,
    // One hit, well off the opening page, so a real jump is unambiguous.
    search_document: () => [{ page: 3, rect: [50, 700, 150, 720], snippet: "the test phrase" }],
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

const before = (await page.locator(".page-label").textContent()).trim();
if (before !== "Page 1 / 5") fail(`unexpected starting state: page-label="${before}"`);

// Type a query and press Enter — this is the FIRST search (Enter also bypasses the
// debounce, matching SearchPanel.svelte's own documented behavior).
await page.click('button[title^="Find"]');
await page.click('[data-testid="search-input"]');
await page.type('[data-testid="search-input"]', "test");
await page.keyboard.press("Enter");
await page.waitForTimeout(400);

const resultCount = await page.locator(".search-result").count();
if (resultCount !== 1) fail(`expected 1 search result after the first Enter, got ${resultCount}`);

const focusedAfterSearch = await page.evaluate(() => {
  const input = document.querySelector('[data-testid="search-input"]');
  return document.activeElement === input;
});
if (!focusedAfterSearch) {
  console.log(
    "NOTE: the search input lost focus after the first search completed (the defect this " +
      "script targets) — continuing to prove the downstream symptom (Enter no longer jumps)."
  );
}

// Second Enter — in a working app this steps to the (only) result and jumps/highlights it.
await page.keyboard.press("Enter");
await page.waitForTimeout(500);

const afterLabel = (await page.locator(".page-label").textContent()).trim();
const hitCount = await page.locator(".search-hit").count();

if (afterLabel !== "Page 4 / 5") {
  fail(`Enter-key jump did not navigate: page-label is "${afterLabel}", expected "Page 4 / 5"`);
}
if (hitCount !== 1) {
  fail(`Enter-key jump did not render the highlight overlay: found ${hitCount} .search-hit element(s), expected 1`);
}

if (process.exitCode === 1) {
  console.log(
    "DIAGNOSIS: pressing Enter a second time inside the search input, after a search has " +
      "already completed once, silently did nothing. Root cause: the input was disabled " +
      "while `store.searching` was true, and disabling a focused element blurs it in real " +
      "browsers — SearchPanel's Enter handler is bound to that input's onkeydown, so once " +
      "focus is gone it never fires again. See SearchPanel.svelte's comment above the " +
      "search-input for the fix (drop the `disabled` binding — SearchStore.run()'s runToken " +
      "already makes it safe to keep typing/searching during an in-flight search)."
  );
} else {
  console.log("PASS: search-input keeps focus across a completed search, and a second Enter jumps to and highlights the result.");
}

await browser.close();
