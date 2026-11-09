import assert from "node:assert/strict";
import fs from "node:fs";
import { gzipSync } from "node:zlib";

import { chromium } from "playwright";

const current = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const baseline = process.env.BASELINE_ORIGIN;
assert(
  baseline,
  "Provide BASELINE_ORIGIN serving an unmodified 516a2563 baseline build against the same backend"
);
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const output = {};
try {
  for (const [label, origin] of [
    ["baseline", baseline],
    ["current", current],
  ]) {
    const samples = [];
    let assets;
    for (let run = 0; run < 3; run++) {
      const context = await browser.newContext({
        viewport: { width: 1600, height: 1000 },
      });
      const page = await context.newPage();
      const bodies = [];
      page.on("response", (response) => {
        if (
          ["script", "stylesheet"].includes(response.request().resourceType())
        )
          bodies.push(
            response.body().then((body) => ({
              url: new URL(response.url()).pathname,
              bytes: body.length,
              offlineGzip: gzipSync(body).length,
            }))
          );
      });
      const start = performance.now();
      await page.goto(
        `${origin}/dashboard/logs/?timestamp=1794243300000-1794243450000`
      );
      await page
        .getByText("Completed 2100 measured PBKDF2 operations", { exact: true })
        .first()
        .waitFor();
      samples.push(performance.now() - start);
      await page.waitForLoadState("networkidle");
      assets = await Promise.all(bodies);
      await context.close();
    }
    output[label] = {
      coldRealRowsMs: samples,
      medianMs: [...samples].sort((a, b) => a - b)[1],
      requestedJsCssBytes: assets.reduce((sum, a) => sum + a.bytes, 0),
      offlineGzipBytes: assets.reduce((sum, a) => sum + a.offlineGzip, 0),
      assets,
    };
  }
  output.note =
    "Three cold Chromium contexts per build, same real read-only backend/two log rows. Asset set includes viewport prefetch. Offline gzip is a size estimate, not this local server’s on-wire compression. Timings are local observations, not a benchmark or performance guarantee.";
  output.deltaBytes =
    output.current.requestedJsCssBytes - output.baseline.requestedJsCssBytes;
  output.deltaOfflineGzip =
    output.current.offlineGzipBytes - output.baseline.offlineGzipBytes;
  const dir = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-base-ui-evidence";
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    `${dir}/bundle-comparison.json`,
    JSON.stringify(output, null, 2)
  );
  console.log(JSON.stringify(output, null, 2));
} finally {
  await browser.close();
}
