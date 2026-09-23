// Perf-measurement spec for the tool-lag investigation (fix/tool-lag-angle-label).
// Measures input-to-handler latency per synthetic pointermove/mousemove for the Line,
// Arrow and Measure-Angle tools against a real running app (real Tauri IPC, real render
// path, real DOM event dispatch) - not a mock. Run via `npm run e2e:perf` (wdio.perf.conf.js,
// which points REDLINE_OPEN_PDF at e2e/fixtures/large-dense.pdf, a 40-page fixture with
// dense line/rect vector content per page, generated to stress the per-page vector
// snap-target index - see tools/gen-large-dense-fixture.py).
//
// Methodology: each drag/preview gesture dispatches N synthetic events directly on
// `svg.markup-overlay` from INSIDE the page (same DirectEval pattern as app-launch.spec.js's
// `dragRectangle` - `browser.action("pointer", ...)` is broken on this crate, see that
// file's header comment). Two distinct measurements are taken per move, because they catch
// different bug classes:
//   1. "handler" - `performance.now()` bracketing the synchronous `dispatchEvent(...)` call
//      itself (the JS handler's own cost: buildMarkup/markupToSvg/snap lookup).
//   2. "frame" - the interval between successive `requestAnimationFrame` callbacks across
//      the drag (same "time between redraws" methodology Viewport.svelte's own pan code
//      uses for its §20 metric, see onMouseMove's panFrameMs) - this catches style/layout/
//      paint/compositing cost that happens AFTER dispatchEvent() returns and would be
//      invisible to (1) alone; it is closer to what a human perceives as "lag while drawing".
// Both are reported as p50/p95/max per tool so a single slow outlier doesn't hide in an
// average, printed via `console.log` (captured by the `spec` reporter) so a human/CI log
// shows the numbers without needing to open a JSON artifact.
//
// This spec inherits the same S2b-gate / PDFium-bundling gaps documented in wdio.conf.js's
// header comment and HANDOVER.md (2026-09-02 entry): PDFIUM_DYNAMIC_LIB_PATH must be set for
// the canvas to render at all, and even then this Mac's embedded-WebKit spawn has a known
// intermittent PDFium regression unrelated to this investigation (~1 in 4 clean runs
// historically) - retry once before treating a failure to reach '.viewport-root' as a real
// regression.

/** Query a single element by selector inside the real page (mirrors app-launch.spec.js). */
async function queryEl(selector, opts = {}) {
  return browser.tauri.execute(
    (tauri, sel, o) => {
      const el = document.querySelector(sel);
      if (!el) return { exists: false };
      const result = { exists: true };
      if (o.text) result.text = el.textContent;
      if (o.attr) result.attr = el.getAttribute(o.attr);
      return result;
    },
    selector,
    opts,
  );
}

async function waitForDisplayed(selector, { timeout = 20000, timeoutMsg } = {}) {
  await browser.waitUntil(
    async () => (await queryEl(selector)).exists,
    { timeout, timeoutMsg: timeoutMsg || `${selector} did not appear within ${timeout}ms` },
  );
}

async function clickEl(selector) {
  const result = await browser.tauri.execute((tauri, sel) => {
    const el = document.querySelector(sel);
    if (!el) return { ok: false };
    el.click();
    return { ok: true };
  }, selector);
  if (!result.ok) throw new Error(`clickEl: no element matched "${selector}"`);
}

function percentile(sorted, p) {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return sorted[idx];
}

function summarize(deltas) {
  const sorted = [...deltas].sort((a, b) => a - b);
  return {
    n: sorted.length,
    p50: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    max: sorted[sorted.length - 1] ?? 0,
  };
}

/**
 * Drag a Line/Arrow-style tool (pointerdown -> N x pointermove -> pointerup) on the markup
 * overlay. Returns TWO measurements, both matter and catch different bugs:
 *
 *  - `handlerDeltas`: wall-clock time for the synchronous `dispatchEvent("pointermove", ...)`
 *    call itself (onOverlayPointerMove's own JS cost - buildMarkup/markupToSvg/snap lookup).
 *  - `frameDeltas`: real requestAnimationFrame-to-rAF intervals across the drag (the same
 *    "time between successive redraws" methodology Viewport.svelte's own pan code already
 *    uses for its §20 metric - see onMouseMove's panFrameMs). This is what a human actually
 *    perceives as "lag while drawing": it captures style/layout/paint/compositing cost that
 *    happens AFTER dispatchEvent() returns and would NOT show up in handlerDeltas alone.
 */
async function measureDragToolLatency(overlaySelector, dx0, dy0, dx1, dy1, steps) {
  return browser.tauri.execute(
    async (tauri, sel, dx0, dy0, dx1, dy1, steps) => {
      const el = document.querySelector(sel);
      if (!el) return { ok: false, reason: "overlay not found" };
      const rect = el.getBoundingClientRect();
      const startX = rect.left + dx0, startY = rect.top + dy0;
      const endX = rect.left + dx1, endY = rect.top + dy1;
      function firePointer(type, x, y, buttons) {
        el.dispatchEvent(new PointerEvent(type, {
          bubbles: true, cancelable: true, composed: true, view: window,
          pointerId: 1, pointerType: "mouse", isPrimary: true, button: 0, buttons,
          clientX: x, clientY: y,
        }));
      }
      const raf = () => new Promise((resolve) => requestAnimationFrame(resolve));
      firePointer("pointerdown", startX, startY, 1);
      const handlerDeltas = [];
      const frameDeltas = [];
      let lastFrameTs = await raf();
      for (let i = 1; i <= steps; i++) {
        const t = i / steps;
        const x = startX + (endX - startX) * t;
        const y = startY + (endY - startY) * t;
        const t0 = performance.now();
        firePointer("pointermove", x, y, 1);
        handlerDeltas.push(performance.now() - t0);
        const frameTs = await raf();
        frameDeltas.push(frameTs - lastFrameTs);
        lastFrameTs = frameTs;
      }
      firePointer("pointerup", endX, endY, 0);
      return { ok: true, handlerDeltas, frameDeltas };
    },
    overlaySelector, dx0, dy0, dx1, dy1, steps,
  );
}

/**
 * Angle-tool gesture: click ray-start, click vertex, then N x mousemove for the live
 * second-ray preview (Viewport.svelte's onOverlayMouseMove, native `mousemove` not
 * `pointermove` - see that function's isAngleTool branch), then a final click to commit.
 * Times each mousemove dispatch the same way as measureDragToolLatency.
 */
async function measureAngleToolLatency(overlaySelector, p0, vertex, p1, steps) {
  return browser.tauri.execute(
    async (tauri, sel, p0, vertex, p1, steps) => {
      const el = document.querySelector(sel);
      if (!el) return { ok: false, reason: "overlay not found" };
      const rect = el.getBoundingClientRect();
      const toClient = (p) => ({ x: rect.left + p.x, y: rect.top + p.y });
      function fireClick(pt) {
        const c = toClient(pt);
        el.dispatchEvent(new MouseEvent("click", {
          bubbles: true, cancelable: true, composed: true, view: window,
          clientX: c.x, clientY: c.y, button: 0,
        }));
      }
      function fireMouseMove(pt) {
        const c = toClient(pt);
        el.dispatchEvent(new MouseEvent("mousemove", {
          bubbles: true, cancelable: true, composed: true, view: window,
          clientX: c.x, clientY: c.y,
        }));
      }
      const raf = () => new Promise((resolve) => requestAnimationFrame(resolve));
      fireClick(p0);
      fireClick(vertex);
      const startFrom = vertex;
      const handlerDeltas = [];
      const frameDeltas = [];
      let lastFrameTs = await raf();
      for (let i = 1; i <= steps; i++) {
        const t = i / steps;
        const pt = { x: startFrom.x + (p1.x - startFrom.x) * t, y: startFrom.y + (p1.y - startFrom.y) * t };
        const t0 = performance.now();
        fireMouseMove(pt);
        handlerDeltas.push(performance.now() - t0);
        const frameTs = await raf();
        frameDeltas.push(frameTs - lastFrameTs);
        lastFrameTs = frameTs;
      }
      fireClick(p1);
      return { ok: true, handlerDeltas, frameDeltas };
    },
    overlaySelector, p0, vertex, p1, steps,
  );
}

describe("Redline tool-drawing latency (perf, large dense fixture)", () => {
  before(async function () {
    this.timeout(120000);
    await browser.waitUntil(
      async () => {
        const state = await browser.tauri.execute((tauri) => ({
          gate: !!document.querySelector("h1.gate-title"),
          viewport: !!document.querySelector(".viewport-root"),
        }));
        return state.gate || state.viewport;
      },
      { timeout: 60000, timeoutMsg: "neither the activation gate nor the viewport appeared" },
    );
    const gated = (await queryEl("h1.gate-title")).exists;
    if (gated) {
      throw new Error(
        "REDLINE E2E PERF: S2b ActivationGate is blocking and no activation flow is run by " +
        "this spec (this Mac is expected to already be licensed - see HANDOVER.md 2026-09 " +
        "entries) - cannot reach the viewport to measure anything.",
      );
    }
    await waitForDisplayed("canvas.tile-canvas", { timeout: 30000 });
    await waitForDisplayed(".page-label", { timeout: 15000 });
  });

  it("reports the fixture loaded with the expected page count (sanity check before timing)", async () => {
    const label = await queryEl(".page-label", { text: true });
    console.log(`REDLINE PERF: page-label = "${label.text}"`);
    expect(label.text).toContain("Page 1 /");
  });

  it("measures Line-tool pointermove latency during a drag", async () => {
    await clickEl('button[title="Line"]');
    const pressed = await queryEl('button[title="Line"]', { attr: "aria-pressed" });
    expect(pressed.attr).toBe("true");

    const result = await measureDragToolLatency("svg.markup-overlay", 60, 60, 500, 400, 60);
    expect(result.ok).toBe(true);
    const h = summarize(result.handlerDeltas);
    const f = summarize(result.frameDeltas);
    console.log(`REDLINE PERF: Line handler (ms) - n=${h.n} p50=${h.p50.toFixed(3)} p95=${h.p95.toFixed(3)} max=${h.max.toFixed(3)}`);
    console.log(`REDLINE PERF: Line frame-interval (ms) - n=${f.n} p50=${f.p50.toFixed(3)} p95=${f.p95.toFixed(3)} max=${f.max.toFixed(3)}`);
    global.__redlinePerf = global.__redlinePerf || {};
    global.__redlinePerf.line = { handler: h, frame: f };
  });

  it("measures Arrow-tool pointermove latency during a drag", async () => {
    await clickEl('button[title="Arrow"]');
    const pressed = await queryEl('button[title="Arrow"]', { attr: "aria-pressed" });
    expect(pressed.attr).toBe("true");

    const result = await measureDragToolLatency("svg.markup-overlay", 60, 60, 500, 400, 60);
    expect(result.ok).toBe(true);
    const h = summarize(result.handlerDeltas);
    const f = summarize(result.frameDeltas);
    console.log(`REDLINE PERF: Arrow handler (ms) - n=${h.n} p50=${h.p50.toFixed(3)} p95=${h.p95.toFixed(3)} max=${h.max.toFixed(3)}`);
    console.log(`REDLINE PERF: Arrow frame-interval (ms) - n=${f.n} p50=${f.p50.toFixed(3)} p95=${f.p95.toFixed(3)} max=${f.max.toFixed(3)}`);
    global.__redlinePerf = global.__redlinePerf || {};
    global.__redlinePerf.arrow = { handler: h, frame: f };
  });

  it("measures Measure-Angle-tool mousemove latency during the live second-ray preview", async () => {
    const angleTitle = "Measure Angle (click first ray, vertex, second ray)";
    await clickEl(`button[title="${angleTitle}"]`);
    const pressed = await queryEl(`button[title="${angleTitle}"]`, { attr: "aria-pressed" });
    expect(pressed.attr).toBe("true");

    const result = await measureAngleToolLatency(
      "svg.markup-overlay",
      { x: 60, y: 200 },
      { x: 200, y: 200 },
      { x: 200, y: 60 },
      60,
    );
    expect(result.ok).toBe(true);
    const h = summarize(result.handlerDeltas);
    const f = summarize(result.frameDeltas);
    console.log(`REDLINE PERF: Angle handler (ms) - n=${h.n} p50=${h.p50.toFixed(3)} p95=${h.p95.toFixed(3)} max=${h.max.toFixed(3)}`);
    console.log(`REDLINE PERF: Angle frame-interval (ms) - n=${f.n} p50=${f.p50.toFixed(3)} p95=${f.p95.toFixed(3)} max=${f.max.toFixed(3)}`);
    global.__redlinePerf = global.__redlinePerf || {};
    global.__redlinePerf.angle = { handler: h, frame: f };
  });

  it("prints a combined summary line for easy before/after diffing", () => {
    const p = global.__redlinePerf || {};
    const fmt = (s) => (s ? `p50=${s.p50.toFixed(3)}ms p95=${s.p95.toFixed(3)}ms max=${s.max.toFixed(3)}ms` : "MISSING");
    const fmtTool = (t) => (t ? `handler[${fmt(t.handler)}] frame[${fmt(t.frame)}]` : "MISSING");
    console.log(
      `REDLINE PERF SUMMARY: Line{${fmtTool(p.line)}} Arrow{${fmtTool(p.arrow)}} Angle{${fmtTool(p.angle)}}`,
    );
  });
});
