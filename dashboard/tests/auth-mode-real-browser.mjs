import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import axe from "axe-core";

// Uses the compiled server and real browser; no request interception or API fixtures.
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19995";
const evidence = path.resolve(
  process.env.SIGIRO_UI_EVIDENCE ?? "/tmp/sigiro-ui-real"
);
const session = `sigiro-real-${randomUUID()}`;
const launch = process.env.CHROMIUM_PATH
  ? ["--executable-path", process.env.CHROMIUM_PATH]
  : [];
await mkdir(evidence, { recursive: true });

function browser(...args) {
  let input;
  if (args[0] === "eval") {
    input = args[1];
    args = ["eval", "--stdin"];
  }
  const result = JSON.parse(
    execFileSync(
      "agent-browser",
      ["--session", session, ...launch, "--json", ...args],
      { encoding: "utf8", timeout: 60_000, input }
    )
  );
  assert.equal(result.success, true, JSON.stringify(result));
  return result.data;
}

const response = await fetch(`${origin}/v1/auth/config`);
assert.equal(response.status, 200);
assert.equal(response.headers.get("cache-control"), "no-store");
assert.deepEqual(await response.json(), { mode: "open" });

try {
  browser("open", `${origin}/dashboard/`);
  browser("wait", "nav[aria-label='Main navigation']");
  browser(
    "eval",
    "sessionStorage.setItem('sigiro.dashboard.token', JSON.stringify({ accessToken: 'stale-token-must-not-be-sent', expiresAt: Date.now() + 60000 }))"
  );
  browser("network", "har", "start");
  browser("set", "viewport", "1440", "1000");
  browser("set", "media", "dark", "reduced-motion");
  for (const route of ["", "logs/", "traces/", "profiles/", "sql/"]) {
    browser("open", `${origin}/dashboard/${route}`);
    browser("wait", "nav[aria-label='Main navigation']");
    browser("wait", "--load", "networkidle");
    const name = route.replace("/", "") || "overview";
    if (name === "overview" || name === "profiles") {
      const states = ["ready", "empty", "no-samples"];
      browser(
        "wait",
        states
          .map((state) => `[data-page='${name}'][data-state='${state}']`)
          .join(",")
      );
    }
    const text = browser("get", "text", "body").text;
    assert.match(text, /\bLocal\b/);
    assert.doesNotMatch(text, /Sign in to Sigiro|Cannot connect to Sigiro/);
    await writeFile(
      path.join(evidence, `${name}.snapshot.txt`),
      browser("snapshot").snapshot
    );
    browser("screenshot", path.join(evidence, `${name}.png`));
    console.log(`PASS real backend /dashboard/${route}`);
  }

  browser(
    "find",
    "label",
    "SQL query",
    "fill",
    "SELECT 'sigiro-browser-verified' AS browser_verified"
  );
  browser("find", "role", "button", "click", "--name", "Run", "--exact");
  browser("wait", "[data-page='sql'][data-state='ready']");
  browser("wait", "--text", "sigiro-browser-verified");
  assert.match(browser("get", "text", "body").text, /browser_verified/);
  browser("screenshot", path.join(evidence, "sql-executed.png"));
  browser("set", "viewport", "390", "844");
  browser("screenshot", path.join(evidence, "sql-mobile.png"));
  const overflow = browser(
    "eval",
    "document.documentElement.scrollWidth > innerWidth"
  ).result;
  assert.equal(overflow, false, "mobile document overflows horizontally");
  browser("network", "har", "stop", path.join(evidence, "network.har"));

  const { requests } = browser("network", "requests");
  const queries = requests.filter(
    (request) => new URL(request.url).pathname === "/v1/query"
  );
  assert.ok(queries.length > 0);
  for (const query of queries) {
    assert.equal(
      Object.keys(query.headers).some(
        (header) => header.toLowerCase() === "authorization"
      ),
      false
    );
  }
  const har = JSON.parse(
    await readFile(path.join(evidence, "network.har"), "utf8")
  );
  // React cancels superseded service/time-window queries; these are not HTTP failures.
  const failures = har.log.entries.filter(({ request, response }) => {
    if (!new URL(request.url).pathname.startsWith("/v1/")) return false;
    if (response.status >= 200 && response.status < 300) return false;
    return response.status !== 0 || response.statusText !== "net::ERR_ABORTED";
  });
  assert.deepEqual(failures, []);
  assert.deepEqual(
    requests.filter((request) =>
      /\/api\/auth\/|\/\.well-known\/oauth-client/.test(request.url)
    ),
    []
  );
  assert.deepEqual(browser("errors").errors, []);
  browser("eval", axe.source);
  const audit = browser("eval", "window.axe.run()").result;
  await writeFile(
    path.join(evidence, "accessibility.json"),
    JSON.stringify(audit, null, 2)
  );
  assert.deepEqual(audit.violations, []);
  console.log(JSON.stringify({ queries: queries.length, evidence }));
} finally {
  browser("close");
}
