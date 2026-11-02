/* eslint-disable anti-slop/no-runtime-typeof */
/**
 * Drives `/dashboard/` end to end against a running website Worker
 * (`aube run preview`, or `wrangler dev --config wrangler.preview.jsonc`).
 *
 * The browser OAuth endpoints are fulfilled here, so the PKCE flow runs for real
 * in the page without a person signing in. `/v1/query` is NOT mocked in the
 * `live` scenario: it goes through the Worker's API proxy to whatever
 * `API_ORIGIN_URL` the Worker was started with — a local `sigiro serve` with
 * seeded telemetry in development.
 *
 *   DASHBOARD_URL   default http://127.0.0.1:8788
 *   SHOTS           directory for 1440 and 390 screenshots (optional)
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";

const playwright = await import("playwright");
const { chromium } = playwright.default ?? playwright;

const base = new URL(process.env.DASHBOARD_URL ?? "http://127.0.0.1:8788");
const origin = base.origin;
const shots = process.env.SHOTS;
const b64 = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const jwt = (tid) =>
  `${b64({ alg: "none" })}.${b64({ tid, scope: "openid query:read" })}.sig`;

const signIn = async (page, tid, calls) => {
  await page.route(`${origin}/api/auth/oauth2/authorize**`, (route) => {
    calls.authorize.push(new URL(route.request().url()));
    return route.abort("aborted");
  });
  await page.route(`${origin}/api/auth/oauth2/token`, (route) => {
    calls.token.push(route.request().postData());
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        access_token: jwt(tid),
        token_type: "Bearer",
        expires_in: 900,
      }),
    });
  });
  await page.goto(new URL("/dashboard/", origin).href);
  await page.locator('[data-action="sign-in"]').click();
  await page.waitForTimeout(300);
  assert.equal(calls.authorize.length, 1, "sign-in must reach authorize");
  const authorize = calls.authorize[0];
  assert.equal(
    authorize.searchParams.get("client_id"),
    `${origin}/.well-known/oauth-client`
  );
  assert.equal(
    authorize.searchParams.get("redirect_uri"),
    `${origin}/dashboard/`
  );
  assert.equal(
    authorize.searchParams.get("scope"),
    "openid services:read query:read traces:read"
  );
  assert.equal(authorize.searchParams.get("code_challenge_method"), "S256");
  assert.equal(authorize.searchParams.get("resource"), "https://sigiro.com");
  const state = authorize.searchParams.get("state");
  await page.goto(
    `${origin}/dashboard/?code=e2e-code&state=${encodeURIComponent(state)}`
  );
  await page.waitForFunction(() => location.search === "");
  assert.equal(calls.token.length, 1, "callback must exchange the code once");
  assert.match(calls.token[0], /code_verifier=/);
  assert.match(calls.token[0], /redirect_uri=http/);
};

const ready = (page) =>
  page.waitForSelector('[data-state="ready"]', { timeout: 120_000 });

const rows = (page, name) =>
  page.locator(`[data-table="${name}"]`).getAttribute("data-rows").then(Number);

const screenshot = async (page, name) => {
  if (!shots) return;
  await mkdir(shots, { recursive: true });
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(400);
    await page.screenshot({
      path: `${shots}/${name}-${width}.png`,
      fullPage: true,
    });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
};

const scenario = async (browser, name, run) => {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
  });
  const page = await context.newPage();
  page.setDefaultTimeout(20_000);
  const foreign = [];
  const errors = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    // The site's Cloudflare Web Analytics beacon is the one expected third party.
    const analytics = [
      "static.cloudflareinsights.com",
      "cloudflareinsights.com",
    ].includes(url.hostname);
    if (
      url.origin !== origin &&
      !analytics &&
      !url.protocol.startsWith("data") &&
      !url.protocol.startsWith("blob")
    )
      foreign.push(url.href);
  });
  page.on("pageerror", (error) => errors.push(error.message));
  try {
    await run(page);
    assert.deepEqual(foreign, [], `${name}: no request may leave the origin`);
    assert.deepEqual(errors, [], `${name}: no page errors`);
    console.log(`ok ${name}`);
  } finally {
    await context.close();
  }
};

const browser = await chromium.launch({ headless: true });
try {
  await scenario(browser, "live", async (page) => {
    const calls = { authorize: [], token: [] };
    const queries = [];
    page.on("request", (request) => {
      if (request.url().endsWith("/v1/query")) {
        queries.push(request.postData());
        assert.match(request.headers().authorization ?? "", /^Bearer /);
      }
    });
    await signIn(page, "fixture-tenant", calls);
    await ready(page);
    assert.equal(queries.length, 8, "eight bounded server queries");
    for (const sql of queries) {
      assert.match(
        sql,
        /timestamp >= TIMESTAMP '\d{4}-\d\d-\d\d \d\d:\d\d:\d\d'/
      );
      assert.doesNotMatch(sql, /now\(\)/i);
    }
    const tab = (name) => page.getByRole("tab", { name }).click();
    const pick = async (label, option) => {
      await page.getByLabel(label).click();
      await page.getByRole("option", { name: option }).click();
    };
    await page.waitForSelector('[data-table="services"][data-rows]');
    assert.equal(
      await page.locator('[data-slot="chart"]').count(),
      2,
      "two overview charts"
    );
    await screenshot(page, "fixture");
    await tab("Services");
    assert.ok((await rows(page, "operations")) > 0, "operations has rows");
    await tab("Traces");
    await page.waitForSelector('[data-table="failed"][data-rows]');
    assert.ok((await rows(page, "failed")) > 0, "failed has rows");
    await tab("Logs");
    await page.waitForSelector('[data-table="logs"][data-rows]');
    for (const table of ["templates", "logs"])
      assert.ok((await rows(page, table)) > 0, `${table} has rows`);

    await pick("Minimum severity", "Error and above");
    await page.waitForFunction(() =>
      [
        ...document.querySelectorAll(
          '[data-table="logs"] tbody td:nth-child(3)'
        ),
      ].every((cell) => cell.textContent === "ERROR")
    );

    await page.getByLabel("Service").click();
    const service = await page.getByRole("option").nth(1).textContent();
    await page.getByRole("option", { name: service }).click();
    await tab("Traces");
    await page.waitForTimeout(500);
    const services = await page
      .locator('[data-table="failed"] tbody td:nth-child(1)')
      .allTextContents();
    assert.ok(
      services.length > 0 && services.every((value) => value === service),
      "service filter"
    );

    await tab("Logs");
    await page.getByLabel("Search").fill("timeout");
    await page.waitForTimeout(500);
    const bodies = await page
      .locator('[data-table="logs"] tbody td:nth-child(4)')
      .allTextContents();
    assert.ok(
      bodies.every((value) => /timeout/i.test(value)),
      "text search"
    );

    const queriesBefore = queries.length;
    await tab("SQL");
    await page
      .getByLabel("SQL query")
      .fill(
        "SELECT span_name, count(*) AS n FROM sigiro_spans WHERE status_code = 2 GROUP BY ALL ORDER BY n DESC"
      );
    await page.locator('[data-action="run-sql"]').click();
    await page.waitForSelector('[data-table="sql"]');
    assert.ok((await rows(page, "sql")) > 0, "SQL box runs locally");
    assert.equal(
      queries.length,
      queriesBefore,
      "slicing and SQL never call the server"
    );
    await page.getByLabel("SQL query").fill("SELECT * FROM nope");
    await page.locator('[data-action="run-sql"]').click();
    await page.getByRole("alert").filter({ hasText: "nope" }).waitFor();
    await page.getByLabel("Search").fill("");
    await screenshot(page, "fixture-filtered");
  });

  await scenario(browser, "empty tenant", async (page) => {
    await page.route(`${origin}/v1/query`, (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        headers: { "x-sigiro-truncated": "false" },
        body: "[]",
      })
    );
    await signIn(page, "sH75IV0W3H1fvXR30EASeAAHm7RuuLoT", {
      authorize: [],
      token: [],
    });
    await ready(page);
    await page.waitForSelector('[data-state="empty"]');
    assert.match(
      await page.locator('[data-state="empty"]').innerText(),
      /sH75IV0W3H1fvXR30EASeAAHm7RuuLoT/
    );
    await page.getByRole("tab", { name: "SQL" }).click();
    await page
      .getByLabel("SQL query")
      .fill("SELECT count(*) AS n FROM sigiro_logs");
    await page.locator('[data-action="run-sql"]').click();
    await page.waitForSelector('[data-table="sql"]');
    await screenshot(page, "empty");
  });

  await scenario(browser, "rejected token", async (page) => {
    await page.route(`${origin}/v1/query`, (route) =>
      route.fulfill({
        status: 401,
        contentType: "application/json",
        body: '{"status":401,"title":"Invalid access token"}',
      })
    );
    await signIn(page, "t", { authorize: [], token: [] });
    await page.waitForSelector('[data-state="signed-out"]');
    assert.match(
      await page.locator('[data-state="signed-out"]').innerText(),
      /rejected the access token/
    );
    assert.equal(
      await page.evaluate(() =>
        sessionStorage.getItem("sigiro.dashboard.token")
      ),
      null
    );
  });

  await scenario(browser, "state mismatch", async (page) => {
    await page.goto(`${origin}/dashboard/?code=x&state=forged`);
    await page.waitForSelector('[data-state="signed-out"]');
    assert.match(
      await page.locator('[data-state="signed-out"]').innerText(),
      /missing or replayed/
    );
  });
} finally {
  await browser.close();
}
console.log(
  "dashboard verified: PKCE sign-in, bounded server queries, local DuckDB-WASM slicing, SQL box, empty tenant, rejected token, forged callback, no third-party requests"
);
