// Tier-2 real-app E2E harness (docs/TESTING.md). Drives the actual compiled `redline`
// binary - real Tauri commands over real IPC, a real webview, no mocked backend (the
// mocked-IPC harness is tools/gui-harness.mjs, a different tier). No native OS file-picker
// dialogs are driven anywhere in this file - WebDriver cannot reach those (they run outside
// the webview) - fixture opening instead goes through the app's own pre-existing
// REDLINE_OPEN_PDF auto-open path (see wdio.conf.js), which is the same App.svelte code
// path ("File > Open") minus the dialog.
//
// IMPORTANT, read before trusting a green run: this repo ships an S2b client-entitlement
// gate (src/components/ActivationGate.svelte) that blocks ALL app content - including the
// REDLINE_OPEN_PDF auto-open - until `license_status` resolves "valid" or "grace" for this
// specific device. This dev Mac has never been activated (no token under
// `~/Library/Application Support/com.emittiv.redline/`; confirmed by
// `recent-docs.json` predating the gate's introduction in PR #49). Producing an
// activation code means calling the production emittiv-staff license service and
// consuming/creating a real device-bound seat - that is an owner-gated production action,
// not something this harness does on its own. So: spec 1 always runs and reports which of
// the two real states the app actually reached; specs 2 and 3 need a licensed/grace device
// and mark themselves `pending` (not `passed`, not silently skipped) via `this.skip()` with
// an explicit reason when the gate is blocking, so a stale "all green" report is never
// possible. Re-run after activating this device to get a true pass on 2 and 3.

describe("Redline app launch (Tier-2 real-app E2E)", () => {
  /** True once we've confirmed the S2b activation gate is NOT blocking this run. */
  let licensed = false;

  before(async () => {
    // Real launch: wait for either real terminal state (never both, never neither - a
    // browser reaching `about:blank`/blank body forever means the harness itself is
    // broken, matching the exact failure class satchel-gui's TESTING.md documents).
    const gate = await browser.$("h1.gate-title");
    const empty = await browser.$(".empty-state");
    await browser.waitUntil(
      async () => (await gate.isExisting()) || (await empty.isExisting()),
      {
        timeout: 20000,
        timeoutMsg:
          "neither the S2b ActivationGate (h1.gate-title) nor the empty-state document " +
          "shell (.empty-state) appeared - the app failed to reach any known real state",
      },
    );
    licensed = await empty.isExisting();
    console.log(
      licensed
        ? "REDLINE E2E: device is licensed/grace - empty-state (or auto-opened fixture) reached"
        : "REDLINE E2E: S2b ActivationGate is blocking (device not activated) - specs 2 and 3 will be marked pending",
    );
  });

  it("launches and reaches a real terminal UI state (licensed empty-state, or the S2b activation gate)", async () => {
    // The state itself was already established in `before()` against real IPC
    // (`license_status`) - this test's job is just to assert exactly one landmark is
    // visible and to name which, so a report reader never has to guess.
    if (licensed) {
      const empty = await browser.$(".empty-state");
      await expect(empty).toBeDisplayed();
      // If REDLINE_OPEN_PDF's auto-open already fired (see wdio.conf.js), the app moved
      // straight past the empty state into the fixture tab - both are valid "reached a
      // real state" outcomes, so accept either without failing this specific assertion.
    } else {
      const gate = await browser.$("h1.gate-title");
      await expect(gate).toBeDisplayed();
      await expect(gate).toHaveText("Activate Redline");
    }
  });

  it("opens the fixture PDF via the real auto-open path and renders it (canvas present, correct page count)", async function () {
    if (!licensed) {
      this.skip();
      // Reason (mocha does not carry a skip-reason string; logged for the report instead):
      // S2b ActivationGate is blocking on this unlicensed dev Mac - see file header and
      // docs/TESTING.md. REDLINE_OPEN_PDF's auto-open never runs while the gate is up
      // (App.svelte's `maybeInitializeAppContent` gates on `isUsable(licenseState)`).
    }

    const canvas = await browser.$("canvas.tile-canvas");
    await canvas.waitForDisplayed({ timeout: 20000 });
    await expect(canvas).toBeDisplayed();

    // e2e-sample.pdf (e2e/fixtures/e2e-sample.pdf) is a single deterministic page.
    const pageLabel = await browser.$(".page-label");
    await pageLabel.waitForDisplayed({ timeout: 10000 });
    await expect(pageLabel).toHaveText("Page 1 / 1", { containing: true });

    const pagesBadge = await browser.$(".doc-pages");
    await expect(pagesBadge).toHaveText("1 page", { containing: true });
  });

  it("places one rectangle markup and confirms it is persisted in the document's markup list", async function () {
    if (!licensed) {
      this.skip(); // same S2b gate block as the previous spec.
    }

    // Select the Rectangle tool (ToolPalette.svelte - unique by its `title` attribute).
    const rectTool = await browser.$('button[title="Rectangle"]');
    await rectTool.waitForDisplayed({ timeout: 10000 });
    await rectTool.click();
    await expect(rectTool).toHaveAttribute("aria-pressed", "true");

    // Draw by dragging on the real markup-overlay SVG (Viewport.svelte's
    // `onpointerdown={onOverlayPointerDown}` element) - a real pointer-down/move/up
    // sequence, not a synthetic DOM event or a mocked IPC call.
    const overlay = await browser.$("svg.markup-overlay");
    await overlay.waitForDisplayed({ timeout: 10000 });
    const loc = await overlay.getLocation();
    const startX = loc.x + 80;
    const startY = loc.y + 80;
    const endX = loc.x + 260;
    const endY = loc.y + 220;

    await browser
      .action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ x: Math.round(startX), y: Math.round(startY) })
      .down({ button: 0 })
      .move({ duration: 300, x: Math.round(endX), y: Math.round(endY) })
      .up({ button: 0 })
      .perform();

    // Ground truth: the real backend markup list (list_markups over real Tauri IPC), not
    // just a DOM/visual guess - the `.markup-overlay` SVG also renders selection-handle
    // and drag-preview `<rect>` elements from other code paths, so counting overlay
    // `<rect>`s would be a fragile proxy for what actually persisted. This queries the
    // same real `#[tauri::command]` the frontend itself calls in `src/lib/ipc.ts`.
    // `data-doc-id` on `.viewport-root` (Viewport.svelte) is a one-line, purely additive
    // test hook added in this PR for exactly this - it carries no behavior.
    const viewportRoot = await browser.$(".viewport-root");
    const docId = await viewportRoot.getAttribute("data-doc-id");
    if (!docId) {
      throw new Error("viewport-root has no data-doc-id - the fixture never actually opened");
    }

    await browser.waitUntil(
      async () => {
        const markups = await browser.execute(
          (id) => window.__TAURI__.core.invoke("list_markups", { docId: id }),
          docId,
        );
        return Array.isArray(markups) && markups.some((m) => m.markup_type === "Rectangle");
      },
      {
        timeout: 10000,
        timeoutMsg:
          "no Rectangle markup appeared in list_markups after the pointer-drag - either " +
          "the drag coordinates missed the drawable page area, or the placement did not " +
          "persist",
      },
    );
  });
});
