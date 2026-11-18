import assert from "node:assert/strict";
import { chromium } from "playwright";

// No routes are mocked. Requires an open-mode Sigiro backend behind this origin.
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19995";
const response = await fetch(`${origin}/v1/auth/config`);
assert.equal(response.status, 200);
assert.equal(response.headers.get("cache-control"), "no-store");
assert.deepEqual(await response.json(), { mode: "open" });
const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
try {
  const page = await browser.newPage();
  const failures = [], queries = [], identity = [], errors = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => {
    if (request.url().includes("/v1/query")) queries.push({ sql: request.postData(), authorization: request.headers().authorization });
    if (request.url().includes("/api/auth/") || request.url().includes("/.well-known/oauth-client")) identity.push(request.url());
  });
  page.on("response", response => {
    if (response.url().includes("/v1/") && response.status() >= 400) failures.push([response.status(), response.url()]);
  });
  for (const path of ["", "logs/", "traces/", "profiles/", "sql/"]) {
    await page.goto(`${origin}/dashboard/${path}`);
    await page.getByRole("navigation", { name: "Main navigation" }).waitFor();
    await page.waitForTimeout(1800);
    assert.equal(await page.getByRole("button", { name: "Sign in", exact: true }).count(), 0);
    console.log(`PASS real local backend /dashboard/${path}`);
  }
  assert.ok(queries.length > 0);
  assert.ok(queries.every(query => !query.authorization));
  assert.deepEqual(identity, []);
  assert.deepEqual(failures, []);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ queries: queries.length, identityRequests: identity.length, failures, errors }));
} finally {
  await browser.close();
}
