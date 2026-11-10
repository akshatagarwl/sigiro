import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";

import { chromium } from "playwright";
const require = createRequire(import.meta.url);
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-dashboard-evidence";
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const checks = [],
  measurements = [],
  accessibility = [],
  errors = [];
const fixed = "1794243300000-1794243450000";
const url = (path = "", query = "") =>
  `${origin}/dashboard/${path}${query ? "?" + query : ""}`;
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const bounds = (sql) =>
  [...sql.matchAll(/timestamp (?:>=|<) make_timestamp\((\d+)\)/g)]
    .slice(0, 2)
    .map((x) => Number(x[1]));
const rowSql = (sql) => /ORDER BY timestamp DESC LIMIT/.test(sql ?? "");
async function page(viewport = { width: 1600, height: 1000 }) {
  const p = await browser.newPage({ viewport });
  p.on("pageerror", (e) => errors.push(e.message));
  return p;
}
async function check(name, source, fn) {
  try {
    await fn();
  } catch (error) {
    const p = browser
      .contexts()
      .flatMap((c) => c.pages())
      .at(-1);
    if (p) {
      fs.writeFileSync(
        `${out}/failure.txt`,
        await p.locator("body").innerText()
      );
      await p.screenshot({ path: `${out}/failure.png` });
      console.error("Options:", await p.getByRole("option").allTextContents());
    }
    throw error;
  }
  checks.push({ name, source, status: "passed" });
  console.log("PASS", source, name);
}
async function state(p, view, kind) {
  await p.locator(`[data-page=${view}][data-state=${kind}]`).waitFor();
}
try {
  if (!process.env.SKIP_ROUTE_CAPTURE)
    for (const [size, width, height] of [
      ["desktop", 1600, 1000],
      ["laptop", 1366, 900],
      ["mobile", 390, 844],
    ])
      for (const view of [
        "overview",
        "services",
        "traces",
        "logs",
        "profiles",
        "sql",
      ]) {
        await check(
          `${view} ${size} real route, bounded shell, screenshot and scoped axe`,
          "real backend",
          async () => {
            const p = await page({ width, height });
            await p.goto(url(view === "overview" ? "" : view + "/"));
            if (["overview", "services"].includes(view))
              await state(p, view, "ready");
            if (view === "profiles") await state(p, view, "empty");
            if (view === "logs")
              await p.locator("[data-event-row]").first().waitFor();
            if (view === "traces")
              await p.locator("tbody tr[id]").first().waitFor();
            if (view === "sql") {
              await state(p, view, "idle");
              assert.equal(
                await p
                  .getByRole("button", { name: "Refresh", exact: true })
                  .count(),
                0
              );
              await p.getByRole("button", { name: "Run", exact: true }).click();
              await state(p, view, "ready");
            }
            await p.waitForTimeout(200);
            const m = await p.evaluate(() => ({
              width: innerWidth,
              height: innerHeight,
              documentWidth: document.documentElement.scrollWidth,
              documentHeight: document.documentElement.scrollHeight,
            }));
            assert.equal(m.documentWidth, width);
            assert(m.documentHeight <= height + 1);
            measurements.push({ view, size, ...m });
            await p.screenshot({ path: `${out}/real-${view}-${size}.png` });
            await p.addScriptTag({
              path: require.resolve("axe-core/axe.min.js"),
            });
            const violations = await p.evaluate(async () =>
              (await axe.run(document.querySelector("main"))).violations.map(
                (v) => ({
                  id: v.id,
                  impact: v.impact,
                  nodes: v.nodes.map((n) => n.target),
                })
              )
            );
            accessibility.push({ view, size, scope: "main", violations });
            await p.close();
          }
        );
      }
  await check(
    "Logs/Traces refresh moves relative bounds, preserves explicit bounds; relative header choice clears override",
    "real backend",
    async () => {
      for (const view of ["logs", "traces"]) {
        const p = await page();
        const qs = [];
        p.on("request", (r) => {
          if (r.url().endsWith("/v1/query") && rowSql(r.postData()))
            qs.push(r.postData());
        });
        await p.goto(url(view + "/"));
        await p.waitForResponse((r) => rowSql(r.request().postData()));
        await pause(300);
        const before = bounds(qs.at(-1));
        await pause(1100);
        const next = p.waitForResponse((r) => rowSql(r.request().postData()));
        await p.getByRole("button", { name: "Refresh", exact: true }).click();
        await next;
        const after = bounds(qs.at(-1));
        assert(after[1] > before[1]);
        assert.equal(after[1] - after[0], 86400000000);
        measurements.push({
          view,
          probe: "relative-refresh-bounds",
          before,
          after,
        });
        await p.goto(url(view + "/", `timestamp=${fixed}`));
        await p.waitForResponse((r) => rowSql(r.request().postData()));
        await pause(200);
        const fixedBefore = bounds(qs.at(-1));
        const refreshed = p.waitForResponse((r) =>
          rowSql(r.request().postData())
        );
        await p.getByRole("button", { name: "Refresh", exact: true }).click();
        await refreshed;
        assert.deepEqual(bounds(qs.at(-1)), fixedBefore);
        const changed = p.waitForResponse(
          (r) =>
            rowSql(r.request().postData()) &&
            bounds(r.request().postData())[1] -
              bounds(r.request().postData())[0] ===
              3600000000
        );
        await p.getByRole("radio", { name: "1h", exact: true }).click();
        await changed;
        assert(!new URL(p.url()).searchParams.has("timestamp"));
        await p.reload();
        await p.waitForResponse((r) => rowSql(r.request().postData()));
        assert.equal(bounds(qs.at(-1))[1] - bounds(qs.at(-1))[0], 3600000000);
        await p.close();
      }
    }
  );
  await check(
    "Traces known/missing/quoted service actually reaches SQL, reload and row keyboard/close focus",
    "real backend",
    async () => {
      const p = await page();
      const qs = [];
      p.on("request", (r) => {
        if (r.url().endsWith("/v1/query") && rowSql(r.postData()))
          qs.push(r.postData());
      });
      for (const service of ["missing-ui-inventory-service", "quote'\\slash"]) {
        await p.goto(
          url(
            "traces/",
            `timestamp=${fixed}&service=${encodeURIComponent(service)}`
          )
        );
        await p
          .getByText("No matching traces. Adjust the time range or filters.")
          .waitFor();
        assert(
          qs
            .at(-1)
            .includes(`service_name IN ('${service.replaceAll("'", "''")}')`)
        );
        await p.reload();
        await p
          .getByText("No matching traces. Adjust the time range or filters.")
          .waitFor();
      }
      await p.goto(url("traces/", `timestamp=${fixed}`));
      await p.locator("tbody tr[id]").first().waitFor();
      await p.getByRole("button", { name: "Filters", exact: true }).click();
      const serviceApplied = p.waitForResponse(
        (response) =>
          rowSql(response.request().postData()) &&
          (response.request().postData() ?? "").includes(
            "service_name IN ('dashboard-e2e')"
          )
      );
      await p.getByRole("checkbox", { name: /dashboard-e2e/ }).check();
      await serviceApplied;
      await p.waitForURL(
        (current) => current.searchParams.get("service") === "dashboard-e2e"
      );
      await p.getByRole("button", { name: "Filters", exact: true }).click();
      const row = p.locator("tbody tr[id]").first();
      await row.waitFor();
      assert(qs.at(-1).includes("service_name IN ('dashboard-e2e')"));
      const id = await row.getAttribute("id");
      await row.focus();
      await row.press("Enter");
      await p.getByRole("dialog").waitFor();
      await p.keyboard.press("Escape");
      await p.getByRole("dialog").waitFor({ state: "hidden" });
      assert.equal(await p.evaluate(() => document.activeElement?.id), id);
      assert(!new URL(p.url()).searchParams.has("uuid"));
      await p.getByRole("button", { name: "Filters", exact: true }).click();
      await p.getByRole("checkbox", { name: /dashboard-e2e/ }).uncheck();
      await p.waitForURL((current) => !current.searchParams.has("service"));
      await p.reload();
      await p.locator("tbody tr[id]").first().waitFor();
      assert(!qs.at(-1).includes("service_name IN"));
      await p.close();
    }
  );
  await check(
    "Compatible service/date navigation, active-route retention, back/forward/reload; real empty Overview/Services",
    "real backend",
    async () => {
      const p = await page();
      await p.goto(url("", "service=dashboard-e2e&range=7d"));
      await state(p, "overview", "ready");
      await p.getByRole("link", { name: "Services", exact: true }).click();
      await state(p, "services", "ready");
      assert.equal(
        new URL(p.url()).searchParams.get("service"),
        "dashboard-e2e"
      );
      await p.goBack();
      await state(p, "overview", "ready");
      assert.equal(new URL(p.url()).searchParams.get("range"), "7d");
      await p.goForward();
      await state(p, "services", "ready");
      await p.reload();
      await state(p, "services", "ready");
      await p.goto(
        url("logs/", `timestamp=${fixed}&service=dashboard-e2e&level=info`)
      );
      await p.locator("[data-event-row]").first().waitFor();
      const old = p.url();
      await p.getByRole("link", { name: "Logs", exact: true }).click();
      assert.equal(p.url(), old);
      await p.getByRole("link", { name: "Traces", exact: true }).click();
      await p.locator("tbody tr[id]").first().waitFor();
      assert.equal(new URL(p.url()).searchParams.get("timestamp"), fixed);
      assert.equal(
        new URL(p.url()).searchParams.get("service"),
        "dashboard-e2e"
      );
      assert(!new URL(p.url()).searchParams.has("level"));
      for (const view of ["overview", "services"]) {
        await p.goto(
          url(
            view === "overview" ? "" : view + "/",
            "service=missing-ui-inventory-service"
          )
        );
        await state(p, view, "empty");
        assert.equal(await p.locator("main").getByRole("alert").count(), 0);
      }
      await p.close();
    }
  );
  await check(
    "SQL real SELECT/empty/error, draft identity, executed disclosure, keyboard",
    "real backend",
    async () => {
      const p = await page();
      await p.goto(url("sql/"));
      const input = p.getByLabel("SQL query");
      await input.fill("SELECT 42 AS answer");
      await input.press("Control+Enter");
      await state(p, "sql", "ready");
      await p.getByText("42", { exact: true }).first().waitFor();
      await input.fill("SELECT 99 AS other");
      await p
        .getByText(
          "Draft changed · this outcome belongs to the last executed query."
        )
        .waitFor();
      await p
        .getByRole("button", { name: "Executed SQL", exact: true })
        .click();
      assert.equal(
        await p.getByLabel("Executed SQL text").textContent(),
        "SELECT 42 AS answer"
      );
      await input.fill("SELECT 1 AS n WHERE FALSE");
      await input.press("Control+Enter");
      await p
        .getByRole("heading", { name: "No rows matched the executed query" })
        .waitFor();
      await input.fill("SELECT * FROM missing_ui_inventory_table");
      await input.press("Control+Enter");
      await state(p, "sql", "error");
      assert.equal(await p.locator("[data-table=sql]").count(), 0);
      await input.fill("   ");
      assert(
        await p.getByRole("button", { name: "Run", exact: true }).isDisabled()
      );
      await p.close();
    }
  );
  await check(
    "Logs request failure is not a successful empty/end state; retry restores real events",
    "synthetic failure over real logs",
    async () => {
      const p = await page();
      let fail = true;
      await p.route("**/v1/query", (route) =>
        fail &&
        rowSql(route.request().postData()) &&
        (route.request().postData() ?? "").includes("FROM sigiro_logs")
          ? route.fulfill({ status: 503, body: "synthetic logs query failed" })
          : route.continue()
      );
      await p.goto(url("logs/", `timestamp=${fixed}`));
      await p.getByRole("heading", { name: "Unable to load logs" }).waitFor();
      await p.getByText("Results unavailable", { exact: true }).waitFor();
      assert.equal(
        await p
          .getByText("No more events in this query.", { exact: true })
          .count(),
        0
      );
      fail = false;
      await p.getByRole("button", { name: "Retry", exact: true }).click();
      await p.locator("[data-event-row]").first().waitFor();
      await p.close();
    }
  );
  await check(
    "Traces list/detail/related-profile failure, loading, retry and preserved workspace",
    "synthetic failures over real trace data",
    async () => {
      const p = await page();
      let failList = true,
        failDetail = true,
        failRelated = true;
      await p.route("**/v1/query", async (route) => {
        const sql = route.request().postData() ?? "";
        if (failList && rowSql(sql) && sql.includes("FROM sigiro_spans"))
          return route.fulfill({
            status: 503,
            body: "synthetic trace list failed",
          });
        if (
          failDetail &&
          sql.startsWith("SELECT epoch_us(timestamp) AS start_us")
        ) {
          await pause(450);
          return route.fulfill({
            status: 503,
            body: "synthetic trace detail failed",
          });
        }
        if (
          failRelated &&
          sql.startsWith("SELECT span_id, any_value(service_name)")
        )
          return route.fulfill({
            status: 503,
            body: "synthetic related profiles failed",
          });
        return route.continue();
      });
      await p.goto(url("traces/", `timestamp=${fixed}`));
      await p
        .getByRole("alert")
        .filter({ hasText: "synthetic trace list failed" })
        .waitFor();
      failList = false;
      await p.getByRole("button", { name: "Retry", exact: true }).click();
      const row = p.locator("tbody tr[id]").first();
      await row.waitFor();
      await row.focus();
      await row.press("Enter");
      await p.getByRole("heading", { name: "Loading trace spans" }).waitFor();
      await p
        .getByRole("alert")
        .filter({ hasText: "synthetic trace detail failed" })
        .waitFor();
      failDetail = false;
      await p.getByRole("button", { name: "Retry trace query" }).click();
      await p.locator("[data-trace-workspace]").waitFor();
      await p
        .getByRole("heading", { name: "Related profiles are unavailable" })
        .waitFor();
      failRelated = false;
      await p.getByRole("button", { name: "Retry", exact: true }).click();
      await p
        .getByRole("heading", { name: "Related profiles are unavailable" })
        .waitFor({ state: "hidden" });
      assert.equal(await p.locator("[data-trace-workspace]").count(), 1);
      await p.close();
    }
  );
  for (const view of ["overview", "services", "profiles"])
    await check(
      `${view} deterministic request failure/retry without intercepting auth health`,
      "synthetic failure over real page",
      async () => {
        const p = await page();
        let fail = true;
        await p.route("**/v1/query", async (route) => {
          const sql = route.request().postData() ?? "";
          if (
            fail &&
            sql.includes(
              view === "profiles"
                ? "FROM sigiro_profiles"
                : view === "services"
                  ? "AS failed_spans"
                  : "AS spans_before"
            )
          )
            return route.fulfill({
              status: 503,
              body: "synthetic dashboard retry failure",
            });
          return route.continue();
        });
        await p.goto(url(view === "overview" ? "" : view + "/"));
        await state(p, view, "error");
        await p
          .getByRole("alert")
          .filter({ hasText: "synthetic dashboard retry failure" })
          .waitFor();
        fail = false;
        await p.getByRole("button", { name: "Retry", exact: true }).click();
        await state(p, view, view === "profiles" ? "empty" : "ready");
        await p.close();
      }
    );
  await check(
    "SQL single-flight repeated shortcuts, cancel/rerun, navigation abort",
    "synthetic delayed read-only SELECT",
    async () => {
      const p = await page();
      let calls = 0,
        aborted = 0;
      p.on("requestfailed", (r) => {
        if ((r.postData() ?? "").includes("fixture_value")) aborted++;
      });
      await p.route("**/v1/query", async (route) => {
        if (!(route.request().postData() ?? "").includes("fixture_value"))
          return route.continue();
        calls++;
        await pause(700);
        try {
          await route.fulfill({ json: [{ fixture_value: 7 }] });
        } catch {}
      });
      await p.goto(url("sql/"));
      const input = p.getByLabel("SQL query");
      await input.fill("SELECT 7 AS fixture_value");
      await input.press("Control+Enter");
      await input.press("Control+Enter");
      await input.press("Control+Enter");
      await pause(100);
      assert.equal(calls, 1);
      await p.getByRole("button", { name: "Cancel", exact: true }).click();
      await state(p, "sql", "idle");
      await input.press("Control+Enter");
      await state(p, "sql", "ready");
      assert.equal(calls, 2);
      await input.press("Control+Enter");
      await state(p, "sql", "running");
      await p.getByRole("link", { name: "Services", exact: true }).click();
      await state(p, "services", "ready");
      await pause(750);
      assert(aborted >= 1);
      await p.close();
    }
  );
  await check(
    "Services client filter/sort, live selected-payload refresh, detail focus and SQL column namespace",
    "synthetic service aggregates; real SQL SELECT",
    async () => {
      const p = await page();
      let round = 0;
      await p.route("**/v1/query", async (route) => {
        const sql = route.request().postData() ?? "";
        if (!sql.startsWith("SELECT service_name AS service"))
          return route.continue();
        return route.fulfill({
          json: sql.includes("AS failed_spans")
            ? [
                {
                  service: "api",
                  spans: 20 + round,
                  failed_spans: 1,
                  error_rate: 5,
                  p50_ms: 1,
                  p95_ms: 2,
                  p99_ms: 3,
                  operations: 2,
                },
                {
                  service: "worker",
                  spans: 5,
                  failed_spans: 0,
                  error_rate: 0,
                  p50_ms: 2,
                  p95_ms: 3,
                  p99_ms: 4,
                  operations: 1,
                },
              ]
            : [
                { service: "api", logs: 4, error_logs: 1 },
                { service: "worker", logs: 9, error_logs: 0 },
              ],
        });
      });
      await p.goto(url("services/"));
      await state(p, "services", "ready");
      await p.getByRole("button", { name: "Spans", exact: true }).click();
      assert.equal(
        await p
          .getByRole("columnheader", { name: "Spans", exact: true })
          .getAttribute("aria-sort"),
        "descending"
      );
      await p.getByRole("button", { name: "Spans", exact: true }).click();
      await p
        .locator("tbody tr[id]")
        .first()
        .filter({ hasText: "worker" })
        .waitFor();
      assert.equal(
        await p.locator("tbody tr[id]").first().getAttribute("id"),
        "worker"
      );
      await p.getByRole("button", { name: /Filter loaded rows/ }).click();
      await p.getByPlaceholder("Filter loaded rows…").fill("service:api");
      await p.getByPlaceholder("Filter loaded rows…").press("Enter");
      await p.getByPlaceholder("Filter loaded rows…").press("Escape");
      await p.getByRole("button", { name: "Reset", exact: true }).waitFor();
      assert.equal(await p.locator("tbody tr[id]").count(), 1);
      await p.getByRole("button", { name: "Reset", exact: true }).click();
      await p.clock.install();
      await p
        .getByRole("button", { name: "Live refresh every 10 seconds" })
        .click();
      const row = p.locator("tr[id=api]");
      await row.focus();
      await row.press("Enter");
      await p.getByRole("dialog").waitFor();
      round = 5;
      await p.clock.fastForward(10500);
      await p.getByRole("dialog").getByText("25", { exact: true }).waitFor();
      await p.keyboard.press("Escape");
      await p.getByRole("dialog").waitFor({ state: "hidden" });
      assert.equal(await p.evaluate(() => document.activeElement?.id), "api");
      await p.getByRole("combobox", { name: "Configure columns" }).click();
      await p.getByRole("option", { name: "Spans", exact: true }).click();
      await p.keyboard.press("Escape");
      assert.equal(
        await p.getByRole("button", { name: "Spans", exact: true }).count(),
        0
      );
      await p.getByRole("link", { name: "SQL", exact: true }).click();
      await p.getByLabel("SQL query").fill("SELECT 1 AS spans");
      await p.getByRole("button", { name: "Run", exact: true }).click();
      await state(p, "sql", "ready");
      await p.getByRole("button", { name: "Spans", exact: true }).waitFor();
      await p.getByRole("link", { name: "Services", exact: true }).click();
      await state(p, "services", "ready");
      assert.equal(
        await p.getByRole("button", { name: "Spans", exact: true }).count(),
        0
      );
      await p.screenshot({ path: `${out}/fixture-services-preferences.png` });
      await p.close();
    }
  );
  for (const [size, width, height] of [
    ["desktop", 1600, 1000],
    ["mobile", 390, 844],
  ])
    await check(
      `Profiles ${size} populated/explicit scope/delay/type/compare/labels/view/keyboard/invalid time`,
      "synthetic profiles SQL responses; other endpoints real",
      async () => {
        const p = await page({ width, height });
        const qs = [];
        let failStacks = false,
          holdWorker = false,
          holdBaseline = false;
        await p.route("**/v1/query", async (route) => {
          const sql = route.request().postData() ?? "";
          if (!sql.includes("FROM sigiro_profiles")) return route.continue();
          qs.push(sql);
          if (sql.includes("stack_trace") && failStacks)
            return route.fulfill({
              status: 503,
              body: "synthetic profile stacks failed",
            });
          if (
            (holdWorker && sql.includes("service_name = 'worker'")) ||
            (holdBaseline && sql.includes("stack_trace"))
          )
            await pause(450);
          let data = [];
          if (sql.startsWith("SELECT service_name AS s"))
            data = [
              { s: "api", n: 3 },
              { s: "worker", n: 2 },
            ];
          else if (sql.startsWith("SELECT profile_type"))
            data = [{ type: "cpu", unit: "nanoseconds", n: 3 }];
          else if (sql.startsWith("SELECT stack_trace"))
            data = [
              {
                stack: [
                  "main",
                  sql.includes("'worker'") ? "workerFn" : "apiFn",
                ],
                v: 2000,
              },
              { stack: ["main", "other"], v: 1000 },
            ];
          else if (sql.startsWith("SELECT sum(value)"))
            data = [{ total: 3000, snapshots: 2, rows: 3, linked: 1 }];
          else if (sql.startsWith("SELECT epoch_us"))
            data = [{ t: 1794243359000000, v: 3000 }];
          else if (sql.startsWith("SELECT k AS key"))
            data = [{ key: "region", val: "test", total: 3000 }];
          return route.fulfill({ json: data });
        });
        await p.goto(url("profiles/", "service=api"));
        await state(p, "profiles", "ready");
        await p
          .getByRole("button", { name: "Highlight apiFn", exact: true })
          .waitFor();
        await p
          .getByRole("button", { name: "Highlight apiFn", exact: true })
          .focus();
        await p.keyboard.press("Enter");
        assert.equal(
          await p
            .getByLabel("Highlight function", { exact: true })
            .inputValue(),
          "apiFn"
        );
        await p
          .getByRole("button", { name: "Reset zoom", exact: true })
          .click();
        await p
          .getByRole("tab", { name: "Top functions", exact: true })
          .click();
        await p.waitForURL(/[?&]view=table/);
        assert.equal(new URL(p.url()).searchParams.get("view"), "table");
        await p.reload();
        await state(p, "profiles", "ready");
        assert.equal(
          await p
            .getByRole("tab", { name: "Top functions", exact: true })
            .getAttribute("aria-selected"),
          "true"
        );
        await p
          .getByRole("button", { name: "Filter by label", exact: true })
          .click();
        await p.getByRole("option").filter({ hasText: "test" }).click();
        await state(p, "profiles", "ready");
        await p.getByRole("button", { name: "Remove region=test" }).waitFor();
        assert(
          qs.some(
            (q) =>
              q.includes("json_extract_string(labels") && q.includes("= 'test'")
          )
        );
        await p.getByRole("button", { name: "Remove region=test" }).click();
        holdBaseline = true;
        await p.getByRole("combobox", { name: "Compare", exact: true }).click();
        await p
          .getByRole("option", { name: "vs previous window", exact: true })
          .click();
        await state(p, "profiles", "loading");
        assert.equal(
          await p.locator("[data-stat]").count(),
          0,
          "baseline pending is not displayed as a zero baseline"
        );
        await p.waitForURL(/[?&]compare=previous/);
        await state(p, "profiles", "ready");
        holdBaseline = false;
        assert.equal(new URL(p.url()).searchParams.get("compare"), "previous");
        await p
          .getByRole("tab", { name: "Table + flame graph", exact: true })
          .click();
        await p.waitForURL((current) => !current.searchParams.has("view"));
        await p
          .locator('[role="tab"][aria-selected="true"]')
          .filter({ hasText: "Table + flame graph" })
          .waitFor();
        await p.getByText("Diff flame graph", { exact: true }).waitFor();
        await p.screenshot({
          path: `${out}/fixture-profiles-${size}.png`,
          animations: "disabled",
        });
        await p.addScriptTag({ path: require.resolve("axe-core/axe.min.js") });
        accessibility.push({
          view: "profiles",
          size,
          source: "synthetic populated comparison",
          scope: "main",
          violations: await p.evaluate(async () =>
            (await axe.run(document.querySelector("main"))).violations.map(
              (v) => ({
                id: v.id,
                impact: v.impact,
                nodes: v.nodes.map((n) => n.target),
              })
            )
          ),
        });
        holdWorker = true;
        await p.getByRole("combobox", { name: "Service", exact: true }).click();
        await p.getByRole("option", { name: "worker", exact: true }).click();
        await state(p, "profiles", "loading");
        assert.equal(
          await p.locator("[data-stat]").count(),
          0,
          "no old api statistics under worker label"
        );
        await state(p, "profiles", "ready");
        await p
          .getByRole("button", { name: "Highlight workerFn", exact: true })
          .waitFor();
        await p.goto(url("profiles/", "service=missing"));
        await state(p, "profiles", "empty");
        assert(!qs.at(-1).includes("service_name = 'api'"));
        await p.goBack();
        await state(p, "profiles", "ready");
        await p
          .getByRole("button", { name: "Highlight workerFn", exact: true })
          .waitFor();
        await p.goForward();
        await state(p, "profiles", "empty");
        await p
          .getByRole("button", { name: "Use an available service" })
          .click();
        await state(p, "profiles", "ready");
        await p.goto(url("profiles/", "service=api&ptype=missing"));
        await state(p, "profiles", "empty");
        await p
          .getByRole("button", { name: "Use an available profile type" })
          .click();
        await state(p, "profiles", "ready");
        failStacks = true;
        await p.goto(
          url("profiles/", "service=api&from=1794243300000&to=1794243450000")
        );
        await state(p, "profiles", "error");
        assert.equal(await p.locator("[data-stat]").count(), 0);
        failStacks = false;
        await p.getByRole("button", { name: "Retry", exact: true }).click();
        await state(p, "profiles", "ready");
        if (width >= 1024)
          await p.getByRole("radio", { name: "1h", exact: true }).click();
        else {
          await p
            .getByRole("combobox", { name: "Time range", exact: true })
            .click();
          await p
            .getByRole("option", { name: "Last hour", exact: true })
            .click();
        }
        await p.waitForURL(
          (current) =>
            current.searchParams.get("range") === "1h" &&
            !current.searchParams.has("from") &&
            !current.searchParams.has("to")
        );
        await state(p, "profiles", "ready");
        assert.equal(bounds(qs.at(-1))[1] - bounds(qs.at(-1))[0], 3600000000);
        await p.goto(url("profiles/", "service=api&from=100&to=50"));
        await state(p, "profiles", "invalid");
        await p
          .getByRole("heading", { name: "Invalid custom time range" })
          .waitFor();
        await p
          .getByRole("button", { name: "Use the header's time range" })
          .click();
        await state(p, "profiles", "ready");
        assert(!new URL(p.url()).searchParams.has("from"));
        await p.goto(url("profiles/", "service=api&trace=invalid"));
        await state(p, "profiles", "invalid");
        await p.getByRole("button", { name: "Clear trace filter" }).click();
        await state(p, "profiles", "ready");
        assert.equal(
          await p.evaluate(() => document.documentElement.scrollWidth),
          width
        );
        await p.close();
      }
    );
  assert.deepEqual(
    accessibility.filter((x) => x.violations.length),
    []
  );
  assert.deepEqual(errors, []);
} finally {
  fs.writeFileSync(
    `${out}/dashboard-results.json`,
    JSON.stringify(
      { checks, measurements, accessibility, runtimeErrors: errors },
      null,
      2
    )
  );
  await browser.close();
}
