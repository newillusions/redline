#!/usr/bin/env node
// Real-browser check of the Navigator (bookmarks, page labels, links; docs/navigation.md
// "Document structure"): the real Svelte app in headless Chromium over a MOCK Tauri IPC whose
// get_navigation / navigation_edit / navigation_undo / navigation_redo keep a small in-memory
// document (the real Rust side is covered by cargo tests and by the independent-reader check
// of the sample PDF; this proves the frontend wiring: panels, dialogs, viewport layer).
//
// Run:  npm run dev                      (serves the app at :1421, separate terminal)
//       OUT=/tmp/nav node tools/gui-harness-navigation.mjs
// Prints OK lines, then PASS and exits 0; prints FAIL lines and exits 1 otherwise.
// Screenshots go to $OUT (default /tmp).
import { chromium } from "playwright";

const URL = process.env.HARNESS_URL || "http://localhost:1421/";
const OUT = process.env.OUT || "/tmp";

function installMockTauri() {
  const PAGES = 5;
  const PAGE_W = 612;
  const PAGE_H = 792;
  window.__calls = [];

  // ---- the mock document ------------------------------------------------------------
  const clone = (x) => JSON.parse(JSON.stringify(x));
  const S = {
    labels: ["i", "ii", "E-101", "E-102", "E-103"],
    nodes: [
      { title: "Cover", target: { kind: "page", page: 0, view: { kind: "xyz", left: null, top: null, zoom: null } }, open: false, children: [] },
      {
        title: "Electrical",
        target: { kind: "page", page: 2, view: { kind: "xyz", left: null, top: null, zoom: null } },
        open: true,
        children: [
          { title: "Lighting plan E-101", target: { kind: "page", page: 2, view: { kind: "xyz", left: null, top: null, zoom: null } }, open: false, children: [] },
          { title: "Power plan E-102", target: { kind: "page", page: 3, view: { kind: "xyz", left: null, top: null, zoom: null } }, open: false, children: [] },
        ],
      },
      { title: "Specification (web)", target: { kind: "uri", uri: "https://example.com/spec" }, open: false, children: [] },
      { title: "Remote drawings", target: { kind: "unsupported", action: "GoToR" }, open: false, children: [] },
    ],
    links: [
      { id: "obj:90:0", page: 0, rect: [60, 560, 360, 620], target: { kind: "uri", uri: "https://example.com/spec" } },
      { id: "obj:91:0", page: 0, rect: [60, 440, 360, 500], target: { kind: "unsupported", action: "Launch" } },
    ],
    nextLinkId: 100,
  };
  let undo = [];
  let redo = [];

  function pathGet(nodes, path) {
    let list = nodes;
    let node = null;
    for (const i of path) { node = list[i]; if (!node) return null; list = node.children; }
    return node;
  }
  function parentList(nodes, path) {
    let list = nodes;
    for (const i of path.slice(0, -1)) list = list[i].children;
    return list;
  }
  function view(nodes, prefix = []) {
    return nodes.map((n, i) => {
      const id = [...prefix, i].join(".");
      return { id, title: n.title, open: n.open, target: n.target, color: n.color ?? null, bold: !!n.bold, italic: !!n.italic, children: view(n.children, [...prefix, i]) };
    });
  }
  function count(nodes) { return nodes.reduce((n, b) => n + 1 + count(b.children), 0); }
  function info() {
    return {
      page_count: PAGES,
      outline: { nodes: view(S.nodes), total: count(S.nodes), broken: 0, read_only: null },
      labels: S.labels,
      links: S.links.map((l) => ({ ...l })),
      links_truncated: false,
      read_only: null,
      undo_depth: undo.length,
      redo_depth: redo.length,
    };
  }
  const snap = () => clone({ nodes: S.nodes, labels: S.labels, links: S.links });
  const restore = (s) => { S.nodes = clone(s.nodes); S.labels = clone(s.labels); S.links = clone(s.links); };

  function edit(e) {
    const before = snap();
    switch (e.op) {
      case "bookmark_add": {
        const node = { title: e.title, target: e.target.kind === "uri" ? { kind: "uri", uri: e.target.uri } : { kind: "page", page: e.target.page, view: e.target.view }, open: false, children: [] };
        if (e.anchor === null) { e.place === "before" ? S.nodes.unshift(node) : S.nodes.push(node); break; }
        const p = e.anchor.split(".").map(Number);
        const a = pathGet(S.nodes, p);
        if (!a || a.title !== e.anchor_expect && e.anchor_expect !== null) throw "The bookmarks changed since the panel was drawn";
        if (e.place === "child") { a.open = true; a.children.push(node); }
        else { const list = parentList(S.nodes, p); const idx = p[p.length - 1]; list.splice(e.place === "before" ? idx : idx + 1, 0, node); }
        break;
      }
      case "bookmark_rename": { const n = pathGet(S.nodes, e.id.split(".").map(Number)); if (!n || n.title !== e.expect) throw "The bookmarks changed since the panel was drawn"; n.title = e.title; break; }
      case "bookmark_delete": { const p = e.id.split(".").map(Number); const n = pathGet(S.nodes, p); if (!n || n.title !== e.expect) throw "The bookmarks changed since the panel was drawn"; parentList(S.nodes, p).splice(p[p.length - 1], 1); break; }
      case "bookmark_set_open": { const open = new Set(e.open); const walk = (ns, pre) => ns.forEach((n, i) => { const id = [...pre, i].join("."); if (n.children.length) n.open = open.has(id); walk(n.children, [...pre, i]); }); walk(S.nodes, []); break; }
      case "labels_number": {
        e.pages.forEach((p, k) => {
          const n = e.start + k;
          const num = e.style === "D" ? String(n) : e.style === "r" ? ["", "i", "ii", "iii", "iv", "v", "vi"][n] ?? String(n) : e.style === "A" ? String.fromCharCode(64 + n) : e.style === "R" ? ["", "I", "II", "III", "IV", "V"][n] ?? String(n) : e.style === "a" ? String.fromCharCode(96 + n) : "";
          S.labels[p] = e.prefix + num;
        });
        break;
      }
      case "label_rename": S.labels[e.page] = e.text; break;
      case "link_create": {
        const target = e.target.kind === "uri" ? { kind: "uri", uri: e.target.uri } : { kind: "page", page: e.target.page, view: e.target.view };
        S.links.push({ id: `obj:${S.nextLinkId++}:0`, page: e.page, rect: e.rect, target });
        break;
      }
      case "link_delete": S.links = S.links.filter((l) => l.id !== e.id); break;
      case "link_set_target": { const l = S.links.find((x) => x.id === e.id); if (!l) throw "that link no longer exists"; l.target = e.target.kind === "uri" ? { kind: "uri", uri: e.target.uri } : { kind: "page", page: e.target.page, view: e.target.view }; break; }
      default: throw `unhandled mock edit ${e.op}`;
    }
    undo.push(before);
    redo = [];
    return info();
  }

  function tilePng() {
    const c = document.createElement("canvas");
    c.width = 64; c.height = 64;
    const g = c.getContext("2d");
    g.fillStyle = "#f4f6fb"; g.fillRect(0, 0, 64, 64);
    return { w: 64, h: 64, b64: c.toDataURL("image/png").split(",")[1] };
  }

  const H = {
    auto_open_path: () => "/mock/set.pdf",
    open_document: (a) => ({ doc_id: "mock-doc", path: a.path || "/mock/set.pdf", page_count: PAGES }),
    close_document: () => null,
    get_page_count: () => PAGES,
    get_page_size: (a) => ({ doc_id: a.docId, page_index: a.pageIndex, width_pts: PAGE_W, height_pts: PAGE_H }),
    render_tile: (a) => {
      const t = tilePng(); const r = a.req;
      return { doc_id: r.doc_id, page_index: r.page_index, tile_x: r.tile_x, tile_y: r.tile_y, width_px: t.w, height_px: t.h, zoom: r.zoom, dpr: r.dpr, png_base64: t.b64, render_ms: 1 };
    },
    process_rss_mb: () => 100,
    get_user_identity: () => ({ user_id: "00000000-0000-0000-0000-000000000001", display_name: "Harness" }),
    load_markups: () => [], list_markups: () => [], add_markup: () => null, update_markup: () => null, delete_markup: () => null,
    save_document: () => null, save_document_as: () => null,
    license_status: () => ({ state: "valid", staff_id: "harness", expires_at: "2099-01-01T00:00:00Z", days_remaining: 9999, renew_due: false }),
    list_scales: () => [], get_page_snap_targets: () => [], list_tool_sets: () => [], recent_tools: () => [],
    folder_index_status: () => ({ status: "idle", doc_count: 0 }),
    list_document_versions: () => [],
    load_recent_docs: () => [], save_recent_docs: () => null, check_file_exists: () => true, search_document: () => [],
    // ---- Navigator ----
    get_navigation: () => info(),
    navigation_edit: (a) => { window.__calls.push(["navigation_edit", a.edit.op]); return edit(a.edit); },
    navigation_undo: () => { const s = undo.pop(); if (!s) throw "Nothing to undo."; redo.push(snap()); restore(s); window.__calls.push(["navigation_undo"]); return info(); },
    navigation_redo: () => { const s = redo.pop(); if (!s) throw "Nothing to redo."; undo.push(snap()); restore(s); window.__calls.push(["navigation_redo"]); return info(); },
    prepare_external_open: (a) => { window.__pending = { token: "tok-1", uri: new URL(a.uri).href }; window.__calls.push(["prepare_external_open", a.uri]); return window.__pending; },
    confirm_external_open: (a) => { if (!window.__pending || a.token !== window.__pending.token) throw "That question is no longer open"; window.__calls.push(["open_external_uri", window.__pending.uri]); window.__pending = null; return null; },
  };

  window.__TAURI_INTERNALS__ = {
    // getCurrentWindow() reads these (the close guard in App.svelte).
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
    transformCallback(cb) { const id = (window.__cbid = (window.__cbid || 0) + 1); window[`__cb_${id}`] = cb; return id; },
    invoke(cmd, args) {
      const h = H[cmd];
      if (!h) { console.warn("MOCK_INVOKE_UNHANDLED", cmd); return Promise.resolve(null); }
      try { return Promise.resolve(h(args || {})); } catch (e) { return Promise.reject(typeof e === "string" ? e : String(e)); }
    },
  };
}

let failed = false;
function fail(msg) { console.log(`FAIL: ${msg}`); failed = true; }
function ok(msg) { console.log(`OK: ${msg}`); }
function check(cond, good, bad) { cond ? ok(good) : fail(bad); }

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1500, height: 950 } });
page.on("pageerror", (e) => fail(`page error: ${e.message}`));
await page.addInitScript(installMockTauri);
await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".viewport-root", { timeout: 15000 });
await page.waitForSelector('[data-testid="bookmarks-panel"]', { timeout: 10000 });
await page.waitForTimeout(400);
const calls = () => page.evaluate(() => window.__calls.map((c) => c.join(":")));

// ---- 1. Bookmarks panel ---------------------------------------------------------------
check(await page.getByText("Electrical").first().isVisible(), "Bookmarks panel lists the outline", "outline not shown");
check(await page.getByText("Lighting plan E-101").isVisible(), "an open parent shows its children", "children of the open parent are hidden");
check((await page.locator('[data-testid="bookmarks-panel"] .count').innerText()).includes("6 bookmarks"), "bookmark count shown", "bookmark count wrong");
check(await page.getByText("Remote drawings").evaluate((el) => el.classList.contains("dim")), "an unsupported action is dimmed", "unsupported action is not dimmed");
await page.screenshot({ path: `${OUT}/nav-1-bookmarks.png` });
// The More menu opens fully on screen (it must not be clipped by the scrolling sidebar).
await page.locator('[data-testid="bookmarks-panel"]').getByRole("button", { name: "More" }).click();
const menuBox = await page.locator(".menu-list").boundingBox();
const lastItem = await page.getByRole("menuitem", { name: "Save collapse state" }).boundingBox();
check(
  menuBox && lastItem && menuBox.x >= 0 && menuBox.x + menuBox.width <= 1500 && lastItem.y + lastItem.height <= 950,
  "the More menu opens fully on screen",
  `the More menu is clipped: menu ${JSON.stringify(menuBox)}, last item ${JSON.stringify(lastItem)}`,
);
await page.screenshot({ path: `${OUT}/nav-1b-more-menu.png` });
await page.keyboard.press("Escape");
await page.mouse.click(700, 500);

// ---- 2. Page indicator shows the label; clicking a bookmark navigates -----------------
check((await page.locator('[data-testid="page-label"]').innerText()) === "i", "page indicator shows the label of page 1 (i)", "page label missing from the page indicator");
await page.getByText("Power plan E-102").click();
await page.waitForTimeout(400);
const pageNo = await page.locator(".page-indicator-display").innerText();
check(pageNo.trim() === "4", "clicking a bookmark goes to its page (4)", `expected page 4, indicator shows ${pageNo}`);
check((await page.locator('[data-testid="page-label"]').innerText()) === "E-102", "page indicator now shows E-102", "label did not follow the page");
// A bookmark for the page being viewed is marked.
check(await page.locator('[data-testid="bookmarks-panel"] [aria-label="On this page"]').count() >= 1, "the bookmark for this page is marked", "no marker on the current page's bookmark");

// ---- 3. Typing a label in the page box ------------------------------------------------
await page.locator(".page-indicator-display").click();
await page.getByLabel("Jump to page").fill("E-101");
await page.getByLabel("Jump to page").press("Enter");
await page.waitForTimeout(300);
check((await page.locator(".page-indicator-display").innerText()).trim() === "3", "typing a page label jumps to it (E-101 = page 3)", "label jump failed");

// ---- 4. Add, rename, undo -------------------------------------------------------------
await page.getByText("Details", { exact: true }).count(); // none yet
await page.locator('[data-testid="bookmarks-panel"]').getByRole("button", { name: "Add", exact: true }).click();
await page.waitForSelector('input[aria-label="Bookmark title"]');
check((await page.getByLabel("Bookmark title").inputValue()) === "E-101", "a new bookmark is titled with the page label and opens for renaming", "new bookmark not titled with the label");
await page.getByLabel("Bookmark title").fill("Lighting plan, level 1");
await page.getByLabel("Bookmark title").press("Enter");
await page.waitForTimeout(300);
check(await page.getByText("Lighting plan, level 1").isVisible(), "rename shows the new title", "rename not applied");
await page.screenshot({ path: `${OUT}/nav-2-added.png` });
await page.getByLabel("Undo").first().click();
await page.waitForTimeout(300);
check(!(await page.getByText("Lighting plan, level 1").isVisible().catch(() => false)), "Undo removes the rename", "Undo did not revert");
check((await calls()).some((c) => c.startsWith("navigation_undo")), "Undo went to the backend", "no undo call");

// ---- 5. Thumbnails show labels; Number pages -------------------------------------------
const navAcc = page.locator('[data-testid="accordion-navigator"]');
await navAcc.scrollIntoViewIfNeeded();
check(await page.locator(".thumbnail-label", { hasText: "E-103" }).count() === 1, "thumbnails show the page labels", "thumbnail labels missing");
await page.getByRole("button", { name: "Document", exact: false }).first().click();
await page.getByText("Number pages…").click();
await page.waitForSelector(".number-pages");
await page.getByLabel("Prefix").fill("S-");
await page.getByLabel("Start").fill("10");
await page.screenshot({ path: `${OUT}/nav-3-number-pages.png` });
await page.locator(".number-pages").getByText("OK", { exact: true }).click();
await page.waitForTimeout(400);
check(await page.locator(".thumbnail-label", { hasText: "S-14" }).count() === 1, "Number pages relabelled the thumbnails (S-10 ... S-14)", "labels not updated after Number pages");

// ---- 6. Link tool: draw, choose a target, follow it -----------------------------------
await page.getByLabel("Link tool").click();
const box = await page.locator(".viewport-root").boundingBox();
const x0 = box.x + box.width * 0.35, y0 = box.y + 160;
await page.mouse.move(x0, y0);
await page.mouse.down();
await page.mouse.move(x0 + 160, y0 + 70, { steps: 6 });
await page.mouse.up();
await page.waitForSelector('dialog[aria-label="New link"]');
await page.locator('dialog[aria-label="New link"]').getByLabel("Page", { exact: true }).fill("S-13");
await page.screenshot({ path: `${OUT}/nav-4-new-link.png` });
await page.locator('dialog[aria-label="New link"]').getByText("Create link").click();
await page.waitForTimeout(400);
check((await calls()).includes("navigation_edit:link_create"), "the link dialog created a link", "link_create was not sent");
check(await page.locator(".link-hit").count() >= 1, "the new link is a clickable region in the Hand tool", "no clickable link region");
await page.screenshot({ path: `${OUT}/nav-5-links.png` });
// The Links panel lists it (and the two seeded links).
await page.locator('[data-testid="accordion-links"]').click();
await page.waitForSelector('[data-testid="links-panel"]');
check((await page.locator('[data-testid="links-panel"] [role="option"]').count()) === 3, "the Links panel lists every link", "Links panel count wrong");
await page.screenshot({ path: `${OUT}/nav-5b-links-panel.png` });
await page.locator('[data-testid="accordion-links"]').click();
// Follow it.
await page.mouse.click(x0 + 80, y0 + 35);
await page.waitForTimeout(400);
check((await page.locator(".page-indicator-display").innerText()).trim() === "4", "clicking the link goes to its page (S-13 = 4)", "link did not navigate");

// ---- 7. A web-address link asks before opening ----------------------------------------
await page.getByText("Specification (web)").click();
await page.waitForSelector(".confirm-prompt");
check((await page.locator(".confirm-prompt").innerText()).includes("https://example.com/spec"), "a web address is shown to the user before opening", "address not shown in the confirm dialog");
await page.screenshot({ path: `${OUT}/nav-6-confirm-open.png` });
await page.locator(".confirm-prompt").getByText("Cancel").click();
check(!(await calls()).some((c) => c.startsWith("open_external_uri")), "Cancel opens nothing", "an address was opened without confirmation");
check((await calls()).includes("prepare_external_open:https://example.com/spec"), "the address was validated by the backend before the dialog", "no prepare call before the dialog");
await page.getByText("Specification (web)").click();
await page.waitForSelector(".confirm-prompt");
await page.locator(".confirm-prompt").getByText("Open", { exact: true }).click();
await page.waitForTimeout(200);
check((await calls()).includes("open_external_uri:https://example.com/spec"), "Open sends the confirmed address to the backend", "open_external_uri not called after Open");

// ---- 8. An unsupported action runs nothing ---------------------------------------------
const before = (await calls()).length;
await page.getByText("Remote drawings").click();
await page.waitForTimeout(200);
check((await calls()).length === before, "an unsupported bookmark action runs nothing", "an unsupported action caused a backend call");

await page.screenshot({ path: `${OUT}/nav-7-final.png` });
await browser.close();
if (failed) {
  console.log("FAIL: see the lines above.");
  process.exit(1);
}
console.log("PASS: bookmarks, page labels and links work end to end in the real app over the mocked backend.");
