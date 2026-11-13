import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve, extname } from "node:path";

import { chromium } from "playwright";

import { otlpProfileBody } from "../../tests/cloud/harness.ts";

// Real OTLP ingestion -> isolated Sigiro -> actual exported dashboard -> browser.
// No request mocking. Evidence/data are retained; only owned processes are stopped.
const binary = process.env.SIGIRO_BIN;
assert.ok(
  binary && existsSync(binary),
  "Set SIGIRO_BIN to a built local Sigiro executable"
);
const root = resolve("out");
assert.ok(existsSync(join(root, "index.html")), "Run next build first");
const evidence = mkdtempSync(join(tmpdir(), "sigiro-time-window-real-"));
const data = join(evidence, "data");
mkdirSync(data, { mode: 0o700 });
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
async function port() {
  const server = createServer();
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const n = server.address().port;
  await new Promise((done) => server.close(done));
  return n;
}
const [apiPort, otlpPort, grpcPort] = await Promise.all([
  port(),
  port(),
  port(),
]);
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !key.startsWith("SIGIRO_") && !key.startsWith("OTEL_")
  )
);
const log = createWriteStream(join(evidence, "backend.log"), { mode: 0o600 });
const backend = spawn(
  binary,
  [
    "serve",
    "--data-dir",
    data,
    "--http-port",
    String(apiPort),
    "--otlp-http-port",
    String(otlpPort),
    "--otlp-grpc-port",
    String(grpcPort),
  ],
  { env, stdio: ["ignore", "pipe", "pipe"] }
);
backend.stdout.pipe(log);
backend.stderr.pipe(log);
let browser;
const proxy = createServer(async (req, res) => {
  try {
    if (!req.url.startsWith("/dashboard")) {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const response = await fetch(`http://127.0.0.1:${apiPort}${req.url}`, {
        method: req.method,
        headers: {
          "content-type": req.headers["content-type"] ?? "text/plain",
          ...(req.headers.authorization
            ? { authorization: req.headers.authorization }
            : {}),
        },
        ...(!["GET", "HEAD"].includes(req.method)
          ? { body: Buffer.concat(chunks) }
          : {}),
      });
      res.writeHead(response.status, {
        "content-type":
          response.headers.get("content-type") ?? "application/json",
      });
      res.end(Buffer.from(await response.arrayBuffer()));
      return;
    }
    const path = decodeURIComponent(
      new URL(req.url, "http://localhost").pathname
    ).replace(/^\/dashboard\/?/, "");
    let file = resolve(root, path);
    if (file !== root && !file.startsWith(`${root}/`)) {
      res.writeHead(403);
      res.end();
      return;
    }
    if (existsSync(file) && statSync(file).isDirectory())
      file = join(file, "index.html");
    if (!existsSync(file)) {
      res.writeHead(404);
      res.end();
      return;
    }
    const types = {
      ".html": "text/html",
      ".js": "text/javascript",
      ".css": "text/css",
      ".json": "application/json",
      ".txt": "text/plain",
      ".woff2": "font/woff2",
      ".ico": "image/x-icon",
    };
    res.writeHead(200, {
      "content-type": types[extname(file)] ?? "application/octet-stream",
    });
    res.end(readFileSync(file));
  } catch (error) {
    res.writeHead(500);
    res.end(String(error));
  }
});
const start = 1699999999000;
const end = 1700000001000;
const fixtures = [
  [start - 1, "before"],
  [start, "start"],
  [start + 500, "inside"],
  [end - 1, "last"],
  [end, "end"],
  [end + 1, "after"],
];
const service = "range-real-e2e";
const attrs = [{ key: "service.name", value: { stringValue: service } }];
const nanos = (ms) => String(BigInt(ms) * 1_000_000n);
async function ingest(path, body, protobuf = false) {
  const response = await fetch(`http://127.0.0.1:${otlpPort}${path}`, {
    method: "POST",
    headers: {
      "content-type": protobuf ? "application/x-protobuf" : "application/json",
    },
    body: protobuf ? body : JSON.stringify(body),
  });
  assert.ok(
    response.ok,
    `${path}: ${response.status} ${await response.text()}`
  );
}
async function query(sql) {
  const response = await fetch(`http://127.0.0.1:${apiPort}/v1/query`, {
    method: "POST",
    body: sql,
  });
  assert.ok(response.ok, `Query failed: ${await response.clone().text()}`);
  return response.json();
}
const report = { binary, start, end, fixtures, checks: [], errors: [] };
try {
  for (let i = 0; i < 60; i++) {
    if (backend.exitCode !== null)
      throw new Error(`Backend exited ${backend.exitCode}`);
    try {
      if ((await fetch(`http://127.0.0.1:${apiPort}/health`)).ok) break;
    } catch {}
    await pause(500);
  }
  await ingest("/v1/logs", {
    resourceLogs: [
      {
        resource: { attributes: attrs },
        scopeLogs: [
          {
            logRecords: fixtures.map(([ms, label]) => ({
              timeUnixNano: nanos(ms),
              severityNumber: 9,
              severityText: "INFO",
              body: { stringValue: `time-window-${label}` },
            })),
          },
        ],
      },
    ],
  });
  await ingest("/v1/traces", {
    resourceSpans: [
      {
        resource: { attributes: attrs },
        scopeSpans: [
          {
            spans: fixtures.map(([ms, label], i) => ({
              traceId: (i + 1).toString(16).padStart(32, "0"),
              spanId: (i + 1).toString(16).padStart(16, "0"),
              name: `time-window-${label}`,
              kind: 2,
              startTimeUnixNano: nanos(ms),
              endTimeUnixNano: String(BigInt(nanos(ms)) + 100_000n),
              status: { code: 1 },
            })),
          },
        ],
      },
    ],
  });
  await ingest("/v1development/profiles", otlpProfileBody(service), true);
  for (let i = 0; i < 40; i++) {
    const logs = await query("SELECT count(*) AS n FROM sigiro_logs");
    const profiles = await query("SELECT count(*) AS n FROM sigiro_profiles");
    if (Number(logs[0].n) === 6 && Number(profiles[0].n) > 0) break;
    await pause(500);
  }
  const where = `timestamp >= make_timestamp(${start * 1000}) AND timestamp < make_timestamp(${end * 1000})`;
  assert.equal(
    Number(
      (await query(`SELECT count(*) AS n FROM sigiro_logs WHERE ${where}`))[0].n
    ),
    3
  );
  assert.equal(
    Number(
      (await query(`SELECT count(*) AS n FROM sigiro_spans WHERE ${where}`))[0]
        .n
    ),
    3
  );
  assert.ok(
    Number(
      (
        await query(`SELECT count(*) AS n FROM sigiro_profiles WHERE ${where}`)
      )[0].n
    ) > 0
  );
  report.checks.push(
    "Real ingestion: six logs/spans; exactly three inside [from,to); profiles stored in window"
  );
  await new Promise((done) => proxy.listen(0, "127.0.0.1", done));
  const origin = `http://127.0.0.1:${proxy.address().port}`;
  browser = await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage();
  const responses = [];
  page.on("pageerror", (e) => report.errors.push(e.message));
  page.on("response", async (r) => {
    if (r.url().endsWith("/v1/query"))
      responses.push({ status: r.status(), sql: r.request().postData() });
  });
  for (const route of ["", "services/", "logs/", "traces/", "profiles/"]) {
    responses.length = 0;
    await page.goto(
      `${origin}/dashboard/${route}?timeStart=${start}&timeEnd=${end}&service=${service}`
    );
    await page.getByRole("button", { name: /^Time range:/ }).waitFor();
    if (route === "logs/") {
      for (const label of ["start", "inside", "last"])
        await page
          .getByText(`time-window-${label}`, { exact: true })
          .first()
          .waitFor();
      for (const label of ["before", "end", "after"])
        assert.equal(
          await page.getByText(`time-window-${label}`, { exact: true }).count(),
          0
        );
    } else if (route === "traces/") {
      for (const label of ["start", "inside", "last"])
        await page
          .getByText(`time-window-${label}`, { exact: true })
          .first()
          .waitFor();
      for (const label of ["before", "end", "after"])
        assert.equal(
          await page.getByText(`time-window-${label}`, { exact: true }).count(),
          0
        );
    } else {
      const view = route.replace("/", "") || "overview";
      await page.locator(`[data-page=${view}][data-state=ready]`).waitFor();
    }
    await page.screenshot({
      path: join(evidence, `${route.replace("/", "") || "overview"}.png`),
      fullPage: true,
    });
    assert.equal(
      responses.some((r) => r.status >= 400),
      false,
      JSON.stringify(responses)
    );
    report.checks.push(
      `${route || "overview"}: real exported UI + real query responses; data visible`
    );
    console.log("PASS", report.checks.at(-1));
  }
  // Apply a genuinely different absolute range in the browser, not just a URL.
  await page.goto(
    `${origin}/dashboard/logs/?timeStart=${start}&timeEnd=${end}`
  );
  await page.getByText("time-window-inside", { exact: true }).first().waitFor();
  await page.getByRole("button", { name: /^Time range:/ }).click();
  const dialog = page.getByRole("dialog", { name: "Choose time range" });
  await dialog
    .getByRole("spinbutton", { name: "ss", exact: true })
    .first()
    .press("ArrowUp");
  await dialog
    .getByRole("button", { name: "Apply range", exact: true })
    .click();
  await page.waitForURL(
    (url) => url.searchParams.get("timeStart") === String(start + 1000)
  );
  await page.getByText("time-window-last", { exact: true }).first().waitFor();
  assert.equal(
    await page.getByText("time-window-inside", { exact: true }).count(),
    0
  );
  await page.reload();
  await page.getByText("time-window-last", { exact: true }).first().waitFor();
  assert.equal(
    await page.getByText("time-window-start", { exact: true }).count(),
    0
  );
  report.checks.push(
    "Actual absolute picker edit changes visible ingested rows and survives reload"
  );
  // Real timer, real SQL, real ingested rows: a near-boundary row must age out.
  const anchor = Date.now();
  await ingest("/v1/logs", {
    resourceLogs: [
      {
        resource: { attributes: attrs },
        scopeLogs: [
          {
            logRecords: [
              [anchor - 300_000 + 9000, "time-window-expiring"],
              [anchor - 60_000, "time-window-fresh"],
            ].map(([ms, body]) => ({
              timeUnixNano: nanos(ms),
              severityNumber: 9,
              severityText: "INFO",
              body: { stringValue: body },
            })),
          },
        ],
      },
    ],
  });
  await pause(1500);
  await page.goto(
    `${origin}/dashboard/logs/?timeStart=now-5m&timeEnd=now&service=${service}`
  );
  await page
    .getByText("time-window-expiring", { exact: true })
    .first()
    .waitFor();
  await page.getByText("time-window-fresh", { exact: true }).first().waitFor();
  await page
    .getByRole("button", { name: "Live refresh every 10 seconds", exact: true })
    .click();
  await page
    .getByText("time-window-expiring", { exact: true })
    .first()
    .waitFor({ state: "hidden", timeout: 20_000 });
  await page.getByText("time-window-fresh", { exact: true }).first().waitFor();
  assert.equal(new URL(page.url()).searchParams.get("timeStart"), "now-5m");
  report.checks.push(
    "Real 10-second live refresh advances the window and ages out only the boundary log"
  );
  await page.screenshot({
    path: join(evidence, "live-window.png"),
    fullPage: true,
  });
  assert.deepEqual(report.errors, []);
  report.status = "passed";
  console.log("PASS", report.checks.at(-1));
} catch (error) {
  report.status = "failed";
  report.failure = String(error);
  throw error;
} finally {
  writeFileSync(
    join(evidence, "report.json"),
    JSON.stringify(report, null, 2),
    { mode: 0o600 }
  );
  await browser?.close();
  proxy.closeAllConnections();
  await new Promise((done) => proxy.close(done));
  backend.kill("SIGTERM");
  await Promise.race([
    new Promise((done) => backend.once("exit", done)),
    pause(5000),
  ]);
  if (backend.exitCode === null) backend.kill("SIGKILL");
  console.log("Evidence:", evidence);
}
