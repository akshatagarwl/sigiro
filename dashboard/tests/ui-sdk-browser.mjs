import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";

import { chromium } from "playwright";

// Synthetic API fixtures, not production telemetry. The browser still exercises
// the real auth gate, query client, URL store, pages, and UI SDK components.
const require = createRequire(import.meta.url);
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19995";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-ui-sdk-evidence";
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const errors = [],
  checks = [],
  queries = [],
  axeResults = [];
const traceId = "a".repeat(32);
const stamp = 1794243364481000;
const events = Array.from({ length: 30 }, (_, i) => ({
  timestamp: stamp - i * 1000,
  level: "info",
  severity_text: "INFO",
  severity_number: 9,
  service_name: "sdk-fixture",
  body: `fixture event ${i}`,
  attributes: { nested: { value: "test" } },
  trace_id: traceId,
  span_id: String(i).padStart(16, "0"),
  parent_span_id: null,
  span_name: "GET /sdk",
  status: "ok",
  status_code: 1,
  duration: 42000,
  duration_ms: 42,
}));
function result(sql) {
  if (sql.includes("stack_trace AS stack"))
    return [{ stack: ["main", "handleRequest"], v: 100 }];
  if (sql.includes("profile_type AS type"))
    return [
      { type: "cpu", unit: "nanoseconds", n: 30 },
      { type: "alloc_space", unit: "bytes", n: 10 },
    ];
  if (sql.includes("service_name AS s"))
    return [
      { s: "sdk-fixture", n: 30 },
      { s: "other-service", n: 10 },
    ];
  if (sql.includes("count(DISTINCT timestamp)"))
    return [{ total: 100, snapshots: 1, rows: 30, linked: 0 }];
  if (sql.includes("AS key")) return [];
  if (sql.includes("AS spans_before"))
    return [
      {
        spans: 30,
        spans_before: 20,
        errors: 0,
        errors_before: 0,
        error_rate: 0,
        p95_ms: 42,
        logs: 30,
        error_logs: 0,
        error_logs_before: 0,
      },
    ];
  if (sql.includes("ORDER BY timestamp DESC, sigiro_cursor_key DESC LIMIT"))
    return events;
  if (sql.includes("AS f0"))
    return [
      {
        f0: sql.includes("sigiro_spans") ? "ok" : "info",
        f1: "sdk-fixture",
        n: 30,
      },
    ];
  if (sql.includes("time_bucket"))
    return [
      {
        t: stamp - 2000000,
        s: sql.includes("sigiro_spans") ? "ok" : "info",
        n: 30,
        v: 100,
        p50: 42,
        p95: 42,
        p99: 42,
        ok: 30,
        error: 0,
        info: 30,
        warning: 0,
        debug: 0,
      },
    ];
  if (sql.includes("quantile_cont")) return [{ p50: 42, p95: 42, p99: 42 }];
  if (sql.includes("count(*)")) return [{ n: 30 }];
  if (sql.includes("SELECT 42")) return [{ answer: 42 }];
  if (sql.includes("sigiro_spans")) return [events[0]];
  return [];
}
async function open(width = 1600, height = 1000) {
  const context = await browser.newContext({ viewport: { width, height } });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(e.stack));
  await page.route("**/v1/auth/config", (route) =>
    route.fulfill({ json: { mode: "open" } })
  );
  await page.route("**/v1/query", async (route) => {
    const sql = route.request().postData() ?? "";
    queries.push(sql);
    await route.fulfill({ json: result(sql) });
  });
  return { context, page };
}
async function audit(page, name) {
  await page.addScriptTag({ path: require.resolve("axe-core/axe.min.js") });
  const violations = await page.evaluate(async () =>
    (await axe.run(document.body)).violations.map(({ id, impact, nodes }) => ({
      id,
      impact,
      targets: nodes.map((n) => ({
        target: n.target,
        html: n.html,
        summary: n.failureSummary,
      })),
    }))
  );
  axeResults.push({ name, violations });
  assert.deepEqual(violations, [], `${name} accessibility`);
}
try {
  for (const [name, width, height] of [
    ["desktop", 1600, 1000],
    ["mobile", 390, 844],
  ]) {
    const { context, page } = await open(width, height);
    await page.goto(
      `${origin}/dashboard/logs/?timestamp=1794243300000-1794243450000`
    );
    await page.locator("[data-event-row]").first().waitFor();
    assert.equal(await page.locator('[data-slot="sidebar"]').count(), 0);
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth),
      width,
      `${name}: no horizontal overflow`
    );
    await audit(page, `${name} logs light`);
    await page.screenshot({ path: `${out}/sdk-${name}-logs.png` });
    const time = page.getByRole("button", { name: /^Time range:/ });
    await time.click();
    await page
      .getByRole("textbox", { name: "From", exact: true })
      .fill("now-30m");
    await page.getByRole("textbox", { name: "To", exact: true }).fill("now");
    await page
      .getByRole("button", { name: "Apply range", exact: true })
      .click();
    await page.waitForURL(/timeStart=now-30m/);
    assert.equal(new URL(page.url()).searchParams.get("timestamp"), null);
    assert.equal(new URL(page.url()).searchParams.get("timeEnd"), "now");
    await time.click();
    await page
      .getByRole("textbox", { name: "From", exact: true })
      .fill("not-a-date");
    await page
      .getByRole("button", { name: "Apply range", exact: true })
      .click();
    await page.getByRole("alert").filter({ hasText: "Use now" }).waitFor();
    await page.keyboard.press("Escape");
    await page.waitForTimeout(100);
    assert(
      await time.evaluate((el) => el === document.activeElement),
      "popup restores focus"
    );
    await time.click();
    await page
      .getByRole("textbox", { name: "From", exact: true })
      .fill("2026-11-09T16:55:00.123Z");
    await page
      .getByRole("textbox", { name: "To", exact: true })
      .fill("2026-11-09T16:57:30.456Z");
    await page
      .getByRole("button", { name: "Apply range", exact: true })
      .click();
    await page.waitForURL(/timeEnd=1794243450456/);
    assert.equal(
      new URL(page.url()).searchParams.get("timeStart"),
      "1794243300123"
    );
    await page.reload();
    await page.locator("[data-event-row]").first().waitFor();
    assert.equal(
      new URL(page.url()).searchParams.get("timeEnd"),
      "1794243450456"
    );
    await page.locator("[data-event-row]").first().click();
    const inspector = page.getByRole(width < 768 ? "dialog" : "region", {
      name: width < 768 ? "Event details" : "Event inspector",
      exact: true,
    });
    await inspector.waitFor();
    await inspector.getByRole("tab", { name: "Event JSON" }).click();
    await inspector
      .getByRole("tab", { name: "Attributes", exact: true })
      .click();
    if (width < 768) await page.keyboard.press("Escape");
    else
      await page.getByRole("button", { name: "Close event inspector" }).click();
    await page.getByRole("button", { name: "View", exact: true }).click();
    await page
      .getByRole("menuitemcheckbox", { name: "Wrap messages", exact: true })
      .click();
    await page.keyboard.press("Escape");
    assert.equal(
      (await page.locator("[data-event-row][data-wrap]").count()) > 0,
      true
    );
    await page.getByRole("button", { name: "Toggle theme" }).click();
    await page.waitForTimeout(100);
    await audit(page, `${name} logs dark`);
    await page.screenshot({ path: `${out}/sdk-${name}-logs-dark.png` });
    await page.getByRole("button", { name: "Workspace menu" }).click();
    await page
      .getByRole("menuitem", { name: "Documentation", exact: true })
      .waitFor();
    assert.equal(
      await page
        .getByRole("menuitem", { name: "Sign out", exact: true })
        .count(),
      0
    );
    assert.equal(
      await page
        .getByRole("menuitem", { name: "Switch workspace", exact: true })
        .count(),
      0
    );
    await page.keyboard.press("Escape");
    const snapshot = await page.evaluate(() => window.sigiroUI.snapshot());
    assert(
      snapshot.some((item) => /Time range/.test(item.label)),
      "SDK agent sees controls"
    );
    checks.push(
      `${name}: SDK controls, focus/Escape, relative and millisecond absolute URL round-trip, inspector tabs, menu checkboxes, account, theme, SDK agent snapshot`
    );
    await context.close();
  }
  for (const [path, name] of [
    ["", "Overview"],
    ["traces/", "Traces"],
    ["profiles/", "Profiles"],
    ["sql/", "SQL"],
  ]) {
    const { context, page } = await open();
    await page.goto(`${origin}/dashboard/${path}`);
    await page.getByRole("navigation", { name: "Main navigation" }).waitFor();
    if (name === "Traces") await page.locator("tbody tr[id]").first().waitFor();
    if (name === "Profiles") {
      await page.locator('[data-action="profile-service"]').click();
      await page
        .getByRole("option", { name: "other-service", exact: true })
        .click();
      await page.waitForURL(/service=other-service/);
      await page.locator('[data-action="profile-type"]').click();
      await page.getByRole("option", { name: /Allocated bytes/i }).click();
      await page.waitForURL(/ptype=alloc_space/);
    }
    if (name === "SQL") {
      await page.locator("textarea").fill("SELECT 42 AS answer");
      await page.getByRole("button", { name: "Run", exact: true }).click();
      await page.getByText("42", { exact: true }).waitFor();
    }
    await page.waitForTimeout(300);
    await audit(page, name);
    await page.screenshot({ path: `${out}/sdk-${name.toLowerCase()}.png` });
    checks.push(
      `${name}: renders with API fixtures and zero whole-page axe violations`
    );
    await context.close();
  }
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify(
      {
        kind: "SYNTHETIC browser API fixtures; no production telemetry",
        checks,
        axeResults,
        queries: queries.length,
        errors,
      },
      null,
      2
    )
  );
} finally {
  await browser.close();
}
