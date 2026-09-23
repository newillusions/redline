// Perf-measurement variant of wdio.conf.js — used ONLY by the tool-lag investigation
// (fix/tool-lag-angle-label). Same embedded-provider setup as wdio.conf.js, but points
// REDLINE_OPEN_PDF at e2e/fixtures/large-dense.pdf (40 pages, ~80 vector objects/page =
// dense snap-target geometry, generated via tools/gen-large-dense-fixture.py) instead of
// the 1-page e2e-sample.pdf, and runs only e2e/specs/perf-tool-lag.spec.js.
//
// Known harness gap this inherits from wdio.conf.js (see that file's header comment):
// PDFIUM_DYNAMIC_LIB_PATH is not resolved by `tauri build --no-bundle`, so it must be set
// manually to a real libpdfium.dylib (e.g. copied from an installed build's
// Contents/Resources/resources/) for the canvas to actually render - HANDOVER.md
// (2026-09-02 entry) documents this as an intermittent pass (roughly 1 in 4 runs) on this
// Mac, a separate PDFium-specific regression under the embedded WebKit spawn, unrelated to
// the DirectEval rewrite. Retry a failed run before concluding a real regression.
import { config as baseConfig } from "./wdio.conf.js";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
// REDLINE_PERF_FIXTURE: escape hatch for ad-hoc density-scaling investigation runs against
// a fixture NOT committed to the repo (e.g. a stress-test PDF built in scratch space) -
// defaults to the committed 40-page/~260-targets-per-page fixture.
const PERF_FIXTURE_PDF = process.env.REDLINE_PERF_FIXTURE || join(__dirname, "e2e", "fixtures", "large-dense.pdf");

export const config = {
  ...baseConfig,
  specs: ["./e2e/specs/perf-tool-lag.spec.js"],
  services: [
    [
      "@wdio/tauri-service",
      {
        ...baseConfig.services[0][1],
        env: {
          ...baseConfig.services[0][1].env,
          REDLINE_OPEN_PDF: PERF_FIXTURE_PDF,
        },
      },
    ],
  ],
};
