/**
 * Drives the dashboard app (../dashboard, served at /dashboard/) end to end in
 * a real browser against its built static export.
 *
 * A local server here plays sigiro.com: it serves `../dashboard/dist` under
 * `/dashboard/` and the same-origin CIMD document. The browser OAuth endpoints
 * and `/v1/query` are fulfilled by Playwright, so the PKCE flow runs for real
 * in the page with an `alg: none` token, and every panel's SQL is answered
 * from fixtures shaped like the query API's JSON rows (timestamps in epoch
 * microseconds). The first query is answered 429 once to prove the client
 * retries instead of failing.
 *
 *   DASHBOARD_DIST  default ../dashboard/dist (run `aube -C ../dashboard run build`)
 *   SHOTS           directory for light and dark screenshots (optional)
 */
import assert from "node:assert/strict";
import { mkdir, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";

const playwright = await import("playwright");
const { chromium } = playwright.default ?? playwright;

const dist = resolve(
  process.env.DASHBOARD_DIST ??
    new URL("../../dashboard/dist", import.meta.url).pathname
);
const shots = process.env.SHOTS;

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".txt": "text/plain",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
};

const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  if (url.pathname === "/.well-known/oauth-client") {
    const origin = `http://${request.headers.host}`;
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        client_id: `${origin}/.well-known/oauth-client`,
        client_name: "Sigiro browser",
        redirect_uris: [`${origin}/`, `${origin}/dashboard/`],
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
        application_type: "native",
        scope: "openid services:read query:read traces:read",
      })
    );
    return;
  }
  if (url.pathname === "/dashboard") {
    response.writeHead(307, { location: "/dashboard/" });
    response.end();
    return;
  }
  if (!url.pathname.startsWith("/dashboard/")) {
    response.writeHead(404);
    response.end("not found");
    return;
  }
  let file = normalize(join(dist, decodeURIComponent(url.pathname)));
  if (!file.startsWith(dist)) {
    response.writeHead(400);
    response.end();
    return;
  }
  try {
    if ((await stat(file)).isDirectory()) file = join(file, "index.html");
    const body = await readFile(file);
    response.writeHead(200, {
      "content-type": TYPES[extname(file)] ?? "application/octet-stream",
    });
    response.end(body);
  } catch {
    response.writeHead(404, { "content-type": "text/html" });
    response.end(
      await readFile(join(dist, "dashboard/404.html")).catch(() => "not found")
    );
  }
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${server.address().port}`;

const b64 = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const jwt = (tid) =>
  `${b64({ alg: "none" })}.${b64({ tid, scope: "openid query:read" })}.sig`;

const TRACE = "0af7651916cd43dd8448eb211c80319c";
const SERVICES = ["checkout", "payments", "shopfront-api"];
const LEVELS = ["error", "warning", "info", "debug"];
const UNITS = {
  minute: 60_000,
  minutes: 60_000,
  hour: 3_600_000,
  hours: 3_600_000,
  day: 86_400_000,
};

/** Buckets for a chart query: the window from its `make_timestamp` bounds, the width from its INTERVAL. */
const buckets = (sql) => {
  const [from, to] = [...sql.matchAll(/make_timestamp\((\d+)\)/g)].map(
    (m) => Number(m[1]) / 1000
  );
  const [, count, unit] = sql.match(/INTERVAL '(\d+) (\w+)'/);
  const ms = Number(count) * UNITS[unit];
  const out = [];
  for (let t = Math.floor(from / ms) * ms; t < to; t += ms) out.push(t);
  return out;
};

const traceSpans = () => {
  const start = Date.now() * 1000 - 5_000_000;
  const span = (id, parent, name, service, offset, duration, status = 1) => ({
    start_us: start + offset,
    duration,
    service_name: service,
    span_name: name,
    span_id: id,
    parent_span_id: parent,
    status_code: status,
  });
  return [
    span("a1", null, "POST /checkout", "checkout", 0, 420_000),
    span("b2", "a1", "SELECT cart", "checkout", 10_000, 80_000),
    span("c3", "a1", "POST /charge", "payments", 100_000, 300_000, 2),
    span("d4", "c3", "stripe.createIntent", "payments", 120_000, 250_000),
  ];
};

let emptyWorkspace = false;

/** The query API, answered from fixtures by the shape of each panel's SQL. */
const answer = (sql) => {
  if (emptyWorkspace)
    return {
      body: sql.includes("AS spans_before")
        ? [
            {
              spans: 0,
              spans_before: 0,
              errors: 0,
              errors_before: 0,
              error_rate: null,
              p95_ms: null,
              logs: 0,
              error_logs: 0,
              error_logs_before: 0,
            },
          ]
        : [],
    };
  const now = Date.now() * 1000;
  if (/\bnope\b/.test(sql))
    return {
      status: 400,
      body: {
        title: "Bad Request",
        detail: 'Parser Error: syntax error at or near "nope"',
      },
    };
  if (sql.includes("SELECT DISTINCT service_name AS s"))
    return { body: SERVICES.map((s) => ({ s })) };
  if (sql.includes("AS spans_before"))
    return {
      body: [
        {
          spans: 1200,
          spans_before: 1000,
          errors: 36,
          errors_before: 30,
          error_rate: 3.0,
          p95_ms: 250.5,
          logs: 3000,
          error_logs: 40,
          error_logs_before: 50,
        },
      ],
    };
  if (sql.includes("AS ok, count(*) FILTER"))
    return {
      body: buckets(sql).map((t, i) => ({
        t,
        ok: 20 + (i % 7),
        error: i % 5 === 0 ? 3 : 0,
      })),
    };
  if (
    sql.includes("AS p99 FROM sigiro_spans") &&
    sql.includes(" AS t,") &&
    sql.includes("INTERVAL '1 minute'")
  )
    return {
      body: buckets(sql)
        .slice(30, 31)
        .map((t) => ({ t, p50: 12, p95: 180, p99: 600 })),
    };
  if (sql.includes("AS p99 FROM sigiro_spans") && sql.includes(" AS t,"))
    return {
      body: buckets(sql).map((t, i) => ({
        t,
        p50: 12 + (i % 3),
        p95: 180 + (i % 11) * 5,
        p99: 600 + (i % 13) * 20,
      })),
    };
  if (sql.includes("AS p99 FROM sigiro_spans"))
    return { body: [{ p50: 12.4, p95: 250.5, p99: 910.2 }] };
  if (sql.includes("AS debug FROM sigiro_logs"))
    return {
      body: buckets(sql).map((t, i) => ({
        t,
        error: i % 6 === 0 ? 2 : 0,
        warning: 3,
        info: 40 + (i % 9),
        debug: 10,
      })),
    };
  if (sql.includes("count(DISTINCT span_name)"))
    return {
      body: SERVICES.map((service, i) => ({
        service,
        spans: 600 - i * 200,
        failed_spans: 10 + i,
        error_rate: 1.5 + i,
        p50_ms: 10,
        p95_ms: 200 + i * 50,
        p99_ms: 800,
        operations: 4 + i,
      })),
    };
  if (sql.includes("AS error_logs FROM sigiro_logs"))
    return {
      body: SERVICES.map((service, i) => ({
        service,
        logs: 1000 - i * 100,
        error_logs: 5 + i,
      })),
    };
  if (sql.includes(`trace_id = '${TRACE}' ORDER BY timestamp LIMIT 2000`))
    return { body: traceSpans() };
  if (sql.includes(" AS f0")) {
    const logs = sql.includes("FROM sigiro_logs");
    const series = logs ? LEVELS : ["ok", "error"];
    return {
      body: SERVICES.flatMap((service) =>
        series.map((s, i) => ({ f0: s, f1: service, n: 10 + i }))
      ),
    };
  }
  if (sql.includes(" AS s, count(*)")) {
    const series = sql.includes("FROM sigiro_logs") ? LEVELS : ["ok", "error"];
    return {
      body: buckets(sql).flatMap((t, i) =>
        series.map((s, j) => ({ t, s, n: (i + j) % 4 }))
      ),
    };
  }
  if (sql.includes(" AS lo0")) return { body: [{ lo0: 1, hi0: 900 }] };
  if (sql.startsWith("SELECT count(*) AS n FROM"))
    return { body: [{ n: 4321 }] };
  if (sql.includes("ORDER BY timestamp DESC LIMIT")) {
    if (sql.endsWith("LIMIT 500")) return { body: [] };
    const logs = sql.includes("FROM sigiro_logs");
    return {
      body: Array.from({ length: 25 }, (_, i) => {
        const timestamp = now - (i + 1) * 30_000_000;
        const service = SERVICES[i % 3];
        return logs
          ? {
              timestamp,
              level: LEVELS[i % 4],
              service_name: service,
              severity_text: LEVELS[i % 4].toUpperCase(),
              body: `request ${i} handled`,
              attributes: '{"http.route":"/checkout"}',
              trace_id: i === 0 ? TRACE : null,
              uuid: `log-${i}`,
            }
          : {
              timestamp,
              status: i % 4 === 0 ? "error" : "ok",
              service_name: service,
              span_name: `POST /route-${i}`,
              duration_ms: 10 + i * 7,
              status_message: i % 4 === 0 ? "deadline exceeded" : null,
              trace_id:
                i === 0
                  ? TRACE
                  : `${String(i).padStart(2, "0")}f7651916cd43dd8448eb211c80319c`,
            };
      }),
    };
  }
  if (sql.includes("FROM sigiro_logs"))
    return {
      body: SERVICES.map((service, i) => ({
        service_name: service,
        severity_text: "INFO",
        n: 100 - i,
      })),
    };
  return {
    status: 500,
    body: { title: "unexpected query in fixture", detail: sql },
  };
};

const browser = await chromium.launch({ headless: true });
const results = [];
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
  });
  const page = await context.newPage();
  page.setDefaultTimeout(20_000);
  const foreign = [];
  const errors = [];
  const queries = [];
  const calls = { authorize: [], token: [] };
  let throttled = false;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (
      url.origin !== origin &&
      !url.protocol.startsWith("data") &&
      !url.protocol.startsWith("blob")
    )
      foreign.push(url.href);
  });
  page.on("pageerror", (error) => errors.push(error.message));
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
        access_token: jwt("fixture-tenant"),
        token_type: "Bearer",
        expires_in: 900,
      }),
    });
  });
  await page.route(`${origin}/api/auth/organization/list`, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        { id: "fixture-tenant", name: "Fixture workspace" },
      ]),
    })
  );
  await page.route(`${origin}/v1/query`, (route) => {
    const request = route.request();
    assert.match(request.headers().authorization ?? "", /^Bearer /);
    const sql = request.postData();
    queries.push(sql);
    if (!throttled) {
      throttled = true;
      return route.fulfill({
        status: 429,
        contentType: "application/problem+json",
        body: JSON.stringify({ title: "Too Many Requests" }),
      });
    }
    const { status = 200, body } = answer(sql);
    return route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });

  const step = async (name, run) => {
    await run();
    results.push(name);
    console.log(`ok ${name}`);
  };
  const screenshot = async (name) => {
    if (!shots) return;
    await mkdir(shots, { recursive: true });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((value) => {
        localStorage.setItem("blume-theme", value);
        document.documentElement.classList.toggle("dark", value === "dark");
        document.documentElement.style.colorScheme = value;
      }, theme);
      await page.waitForTimeout(1500);
      await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
    }
  };

  await step("signed out shows sign-in", async () => {
    await page.goto(`${origin}/dashboard/`);
    await page.waitForSelector('[data-state="signed-out"]');
    await screenshot("0-signed-out");
  });

  await step("sign-in runs PKCE against the CIMD client", async () => {
    await page.locator('[data-action="sign-in"]').click();
    await page.waitForTimeout(400);
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
    await page.waitForFunction(() => !location.search.includes("code="));
    assert.equal(calls.token.length, 1, "callback must exchange the code once");
    assert.match(calls.token[0], /code_verifier=/);
  });

  await step("overview: KPIs and three EvilCharts panels", async () => {
    await page.waitForSelector('[data-state="ready"]', { timeout: 60_000 });
    assert.ok(throttled, "the first query was throttled");
    assert.match(await page.locator('[data-kpi="Spans"]').innerText(), /1\.2K/);
    assert.match(
      await page.locator('[data-kpi="Error rate"]').innerText(),
      /3\.00%/
    );
    await page.waitForSelector(
      '[data-chart-name="requests"] svg .recharts-bar-rectangle'
    );
    await page.waitForSelector(
      '[data-chart-name="logs"] svg .recharts-bar-rectangle'
    );
    await page.waitForSelector('[data-chart-name="latency"] canvas');
    assert.match(
      await page.locator("[data-workspace]").innerText(),
      /Fixture workspace/
    );
    await screenshot("1-overview");
  });

  await step("service filter scopes the overview SQL", async () => {
    const before = queries.length;
    await page.locator('[data-action="service"]').click();
    await page.getByRole("option", { name: "payments" }).click();
    await page.waitForFunction(() =>
      location.search.includes("service=payments")
    );
    await page.waitForSelector('[data-state="ready"]');
    await page.waitForTimeout(500);
    assert.ok(
      queries
        .slice(before)
        .some(
          (sql) =>
            sql.includes("AS spans_before") &&
            sql.includes("service_name = 'payments'")
        ),
      "service must reach the KPI SQL"
    );
    await page.locator('[data-action="service"]').click();
    await page.getByRole("option", { name: "All services" }).click();
    await page.waitForFunction(() => !location.search.includes("service="));
  });

  await step(
    "one-bucket latency still draws points; workspace shows its name",
    async () => {
      await page.locator('[data-action="service"]').waitFor();
      await page.getByRole("radio", { name: "1h" }).click();
      await page.waitForFunction(() => location.search.includes("range=1h"));
      await page.waitForSelector('[data-state="ready"]');
      await page.waitForTimeout(1500);
      await screenshot("1-overview-sparse");
      const workspace = await page.locator("[data-workspace]").innerText();
      assert.match(workspace, /Fixture workspace/);
      assert.doesNotMatch(workspace, /fixture-tenant/);
      await page.getByRole("radio", { name: "24h" }).click();
      await page.waitForFunction(() => !location.search.includes("range="));
    }
  );

  await step("live refresh re-runs the panels", async () => {
    const before = queries.length;
    await page.locator('[data-action="live"]').click();
    await page.waitForTimeout(11_000);
    assert.ok(
      queries.length > before + 3,
      `live mode must re-query (${before} -> ${queries.length})`
    );
    await page.locator('[data-action="live"]').click();
  });

  await step("traces list → waterfall", async () => {
    await page.getByRole("link", { name: "Traces" }).click();
    await page.getByText("POST /route-1", { exact: true }).first().waitFor();
    await screenshot("3-traces");
    await page
      .getByRole("row")
      .filter({ hasText: "POST /route-0" })
      .first()
      .click({ position: { x: 40, y: 10 } });
    await page.waitForSelector("[data-waterfall='4']");
    await screenshot("3-trace");
    await page.keyboard.press("Escape");
  });

  await step("trace deep link ?trace=", async () => {
    await page.goto(`${origin}/dashboard/traces/?trace=${TRACE}`);
    await page.waitForSelector("[data-waterfall='4']");
    await page.keyboard.press("Escape");
  });

  await step("logs explorer with facets and search", async () => {
    await page.goto(`${origin}/dashboard/logs/`);
    await page.getByText("request 3 handled").first().waitFor();
    await page.getByText("checkout", { exact: true }).first().waitFor();
    await page.waitForSelector(
      '[data-chart-name="timeline"] svg .recharts-bar-rectangle',
      { state: "attached" }
    );
    await screenshot("4-logs");
    const before = queries.length;
    await page.goto(`${origin}/dashboard/logs/?body=handled&level=error`);
    await page.getByText("request 0 handled").first().waitFor();
    assert.ok(
      queries
        .slice(before)
        .some(
          (sql) =>
            sql.includes("body ILIKE '%handled%'") &&
            sql.includes("IN ('error')")
        ),
      "search and facet must reach the SQL"
    );
  });

  await step("SQL page runs and reports errors", async () => {
    await page.goto(`${origin}/dashboard/sql/`);
    await page.locator('[data-action="run-sql"]').click();
    await page.waitForSelector('[data-table="sql"][data-rows="3"]');
    await screenshot("5-sql");
    await page.getByLabel("SQL query").fill("nope");
    await page.locator('[data-action="run-sql"]').click();
    await page.getByRole("alert").filter({ hasText: "syntax error" }).waitFor();
  });

  await step("empty workspace shows how to send data", async () => {
    emptyWorkspace = true;
    await page.goto(`${origin}/dashboard/`);
    const setup = page.locator('[data-state="setup"]');
    await setup.waitFor();
    const text = await setup.innerText();
    for (const needle of [
      "https://sigiro.com",
      "http/protobuf",
      "sigiro auth token",
      "Open the setup guide",
    ])
      assert.ok(text.includes(needle), `empty state must mention ${needle}`);
    assert.equal(
      await setup
        .getByRole("link", { name: "Open the setup guide" })
        .getAttribute("href"),
      "/docs/how-to/hosted-onboarding/"
    );
    await screenshot("6-empty");
    emptyWorkspace = false;
  });

  await step("390px wide: nothing cut off", async () => {
    const overflow = [];
    await page.setViewportSize({ width: 390, height: 844 });
    for (const [name, path, ready] of [
      ["overview", "/dashboard/", '[data-state="ready"]'],
      ["traces", "/dashboard/traces/", '[data-chart-name="timeline"]'],
      ["logs", "/dashboard/logs/", '[data-chart-name="timeline"]'],
      ["sql", "/dashboard/sql/", '[data-action="run-sql"]'],
    ]) {
      await page.goto(`${origin}${path}`);
      await page.waitForSelector(ready);
      await page.waitForTimeout(800);
      await screenshot(`7-mobile-${name}`);
      const scrollWidth = await page.evaluate(
        () => document.documentElement.scrollWidth
      );
      if (scrollWidth > 390)
        overflow.push(`${name}: page is ${scrollWidth}px wide`);
      const wide = await page.evaluate(() =>
        [...document.querySelectorAll("body *")]
          .filter((el) => {
            if (
              el.getBoundingClientRect().right <= 391 ||
              el.getClientRects().length === 0
            )
              return false;
            // Wide content inside its own sideways-scrolling box (a table) is reachable, not cut off.
            for (let up = el.parentElement; up; up = up.parentElement) {
              const { overflowX } = getComputedStyle(up);
              if (
                (overflowX === "auto" || overflowX === "scroll") &&
                up.getBoundingClientRect().right <= 391
              )
                return false;
            }
            return true;
          })
          .slice(-3)
          .map(
            (el) =>
              `${el.tagName.toLowerCase()}.${String(el.className).slice(0, 60)} → ${Math.round(el.getBoundingClientRect().right)}px`
          )
      );
      if (wide.length > 0) overflow.push(`${name}: ${wide.join(" | ")}`);
    }
    assert.deepEqual(
      overflow,
      [],
      "nothing may extend past 390px outside a scroll container"
    );
    await page.setViewportSize({ width: 1440, height: 900 });
  });

  await step(
    "an existing sigiro.com session signs in without a click",
    async () => {
      const tab = await context.newPage();
      const authorized = [];
      await tab.route(`${origin}/api/auth/get-session`, (route) =>
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            session: { id: "s" },
            user: { email: "a@b.c" },
          }),
        })
      );
      await tab.route(`${origin}/api/auth/oauth2/authorize**`, (route) => {
        authorized.push(route.request().url());
        return route.abort("aborted");
      });
      await tab.goto(`${origin}/dashboard/logs/`);
      await tab.waitForTimeout(1500);
      assert.equal(
        authorized.length,
        1,
        "a session must go straight to authorize"
      );
      assert.equal(
        await tab.locator('[data-state="signed-out"]').count(),
        0,
        "no sign-in card for a session"
      );
      await tab.close();
    }
  );

  assert.deepEqual(foreign, [], "no request may leave the origin");
  assert.deepEqual(errors, [], "no page errors");
  console.log(
    `ok ${results.length} steps, ${queries.length} queries, no foreign requests, no page errors`
  );
} finally {
  await browser.close();
  server.close();
}
