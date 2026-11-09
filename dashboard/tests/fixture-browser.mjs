import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);

import { chromium } from "playwright";
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-base-ui-evidence";
fs.mkdirSync(out, { recursive: true });
const b = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const c = await b.newContext({
  viewport: { width: 1600, height: 1000 },
  permissions: ["clipboard-read", "clipboard-write"],
});
const p = await c.newPage();
const sql = [],
  external = [],
  errors = [];
p.on("pageerror", (e) => errors.push(e.message));
p.on("request", (r) => {
  if (!r.url().startsWith(`${origin}/`) && !r.url().startsWith("data:"))
    external.push(r.url());
});
await p.addInitScript(() => {
  window.__xss = 0;
  window.__tasks = [];
  new PerformanceObserver((list) =>
    window.__tasks.push(...list.getEntries().map((e) => e.duration))
  ).observe({ type: "longtask", buffered: true });
});
const attack =
  '<img src="https://evil.invalid/pixel" onerror="window.__xss=1"><svg onload="window.__xss=2"></svg><a href="javascript:window.__xss=3">bad</a>\u001b[31mred\u001b[0m\u001b]8;;https://evil.invalid\u0007osc8\u001b]8;;\u0007';
const service = "fixture' OR true -- " + attack;
const huge =
  attack +
  "\n" +
  "one-megabyte synthetic fixture; not production data. ".repeat(21_000);
const rows = Array.from({ length: 5000 }, (_, i) => ({
  timestamp: 1794243364481000 - i * 1000,
  level: i % 7 === 0 ? "error" : "info",
  service_name: i === 0 ? service : "fixture-service",
  severity_text: i % 7 === 0 ? "ERROR" : "INFO",
  body:
    i === 0
      ? huge
      : `Synthetic fixture event ${i} – a measured browser workload, not a real log`,
  attributes:
    i === 0
      ? JSON.stringify({
          nil: null,
          number: 12,
          bool: false,
          list: [1, "two", true],
          nested: { attack },
          long: "x".repeat(40_000),
          ["__proto__"]: { polluted: true },
        })
      : "{}",
  trace_id: "fixture-trace",
  span_id: `fixture-span-${i}`,
}));
await p.route("**/v1/query", async (route) => {
  const q = route.request().postData() ?? "";
  sql.push(q);
  let result = [];
  if (q.includes("ORDER BY timestamp DESC LIMIT")) result = rows;
  else if (q.includes("AS f0"))
    result = [{ f0: "info", f1: "fixture-service", n: 5000 }];
  else if (q.includes("time_bucket"))
    result = [{ t: 1794243360000, s: "info", n: 5000 }];
  else if (q.includes("count(*)")) result = [{ n: 5000 }];
  await route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(result),
  });
});
const started = performance.now();
await p.goto(`${origin}/dashboard/logs/?timestamp=1794243300000-1794243450000`);
await p.locator("[data-event-row]").first().waitFor();
const readyMs = performance.now() - started;
await p.waitForTimeout(200);
const initialDom = await p.locator("[data-event-row]").count();
assert(initialDom < 60);
const open = performance.now();
await p.locator("[data-event-row]").first().click();
await p.getByRole("complementary", { name: "Event inspector" }).waitFor();
const inspectorMs = performance.now() - open;
assert((await p.locator("pre").first().innerText()).length <= 50_000);
await p.getByRole("button", { name: "Copy message", exact: true }).click();
assert.equal(await p.evaluate(() => navigator.clipboard.readText()), huge);
await p.getByRole("tab", { name: "Event JSON", exact: true }).click();
assert(
  (await p.getByRole("tabpanel", { name: "Event JSON" }).innerText()).length <
    51_000
);
await p.getByRole("tab", { name: "Attributes", exact: true }).click();
await p.getByRole("button", { name: "Filter service", exact: true }).click();
const filterRequest = p.waitForRequest(
  (request) =>
    request.url().endsWith("/v1/query") &&
    (request.postData() ?? "").includes(
      "service_name IN ('fixture'' OR true --"
    )
);
await p.getByRole("menuitem").first().click();
await filterRequest;
assert(sql.some((q) => q.includes("service_name IN ('fixture'' OR true --")));
await p.getByRole("button", { name: "Close event inspector" }).click();
await p.locator("[data-event-row]").first().focus();
await p.keyboard.press("End");
await p.waitForTimeout(150);
assert.match(
  await p.evaluate(() => document.activeElement.textContent),
  /fixture event 4999/
);
const endDom = await p.locator("[data-event-row]").count();
assert(endDom < 60);
assert.equal(await p.evaluate(() => window.__xss), 0);
assert.equal(await p.evaluate(() => ({}).polluted), undefined);
assert.equal(
  await p
    .locator(
      'section[aria-label="Structured log explorer"] img, section[aria-label="Structured log explorer"] svg'
    )
    .count(),
  0
);
assert.deepEqual(external, []);
assert.deepEqual(errors, []);
const workloadPerformance = await p.evaluate(() => ({
  heap: performance.memory?.usedJSHeapSize,
  longTasks: window.__tasks,
}));
await p.addScriptTag({ path: require.resolve("axe-core/axe.min.js") });
const lightAxe = await p.evaluate(async () =>
  (
    await axe.run(
      document.querySelector('section[aria-label="Structured log explorer"]')
    )
  ).violations.map((x) => x.id)
);
await p.getByRole("button", { name: "Toggle theme", exact: true }).click();
await p.waitForFunction(() =>
  document.documentElement.classList.contains("dark")
);
const darkAxe = await p.evaluate(async () =>
  (
    await axe.run(
      document.querySelector('section[aria-label="Structured log explorer"]')
    )
  ).violations.map((x) => x.id)
);
assert.deepEqual(lightAxe, []);
assert.deepEqual(darkAxe, []);
const result = {
  kind: "SYNTHETIC fixture responses intercepted in browser; not production telemetry",
  rows: 5000,
  messageBytes: Buffer.byteLength(huge),
  readyMs,
  inspectorMs,
  initialDom,
  endDom,
  security: {
    executedPayload: await p.evaluate(() => window.__xss),
    unexpectedRequests: external,
    sqlQuoteEscaping: true,
    prototypePollution: false,
    fullClipboardPreserved: true,
  },
  performance: workloadPerformance,
  lightAxe,
  darkAxe,
  errors,
};
fs.writeFileSync(
  `${out}/fixture-results.json`,
  JSON.stringify(result, null, 2)
);
console.log(JSON.stringify(result, null, 2));
await b.close();
