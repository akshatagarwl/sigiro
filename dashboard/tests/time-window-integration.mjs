import assert from "node:assert/strict";

import { chromium } from "playwright";

const base = process.env.DASHBOARD_PREVIEW_URL ?? "http://127.0.0.1:19988";
const browser = await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage();
const queries = [];
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
await page.route("**/v1/query", async (route) => {
  queries.push(route.request().postData() ?? "");
  await route.fulfill({ json: [] });
});
const start = 1700000000123;
const end = 1700003600456;
try {
  for (const path of ["", "logs/", "traces/", "profiles/"]) {
    queries.length = 0;
    await page.goto(
      `${base}/dashboard/${path}?timeStart=${start}&timeEnd=${end}`
    );
    await page.getByRole("button", { name: /^Time range:/ }).waitFor();
    await page.waitForFunction(
      () => !document.body.innerText.includes("Signing in…")
    );
    // All data SQL must use the requested endpoints, not the old 24h fallback.
    await page.waitForTimeout(700);
    const data = queries.filter((q) => /timestamp\s*[<>]/.test(q));
    assert.ok(data.length > 0, `no scoped SQL on ${path}`);
    for (const sql of data) {
      assert.ok(sql.includes(`make_timestamp(${start * 1000})`), sql);
      assert.ok(sql.includes(`make_timestamp(${end * 1000})`), sql);
    }
    const refreshed = page.waitForRequest(
      (request) =>
        request.url().endsWith("/v1/query") &&
        (request.postData() ?? "").includes(`make_timestamp(${start * 1000})`)
    );
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await refreshed;
    console.log(
      `PASS ${path || "overview"}: exact absolute SQL bounds + fixed-window refresh`
    );
  }

  // Edit an arbitrary absolute endpoint through the actual date/time editor.
  await page.goto(`${base}/dashboard/?timeStart=${start}&timeEnd=${end}`);
  await page.getByRole("button", { name: /^Time range:/ }).click();
  const absoluteDialog = page.getByRole("dialog", {
    name: "Choose time range",
  });
  await absoluteDialog
    .getByRole("spinbutton", { name: "ss", exact: true })
    .first()
    .press("ArrowDown");
  queries.length = 0;
  await absoluteDialog
    .getByRole("button", { name: "Apply range", exact: true })
    .click();
  await page.waitForURL(
    (url) => url.searchParams.get("timeStart") === String(start - 1000)
  );
  await page.waitForTimeout(400);
  assert.ok(
    queries.some((sql) =>
      sql.includes(`make_timestamp(${(start - 1000) * 1000})`)
    )
  );
  await page.reload();
  await page.getByRole("button", { name: /^Time range:/ }).click();
  await absoluteDialog
    .getByRole("spinbutton", { name: "YYYY", exact: true })
    .nth(1)
    .press("ArrowDown");
  await absoluteDialog
    .getByRole("button", { name: "Apply range", exact: true })
    .click();
  await absoluteDialog
    .getByText("Start must be earlier than end.", { exact: true })
    .waitFor();
  assert.equal(new URL(page.url()).searchParams.get("timeEnd"), String(end));
  await absoluteDialog
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  console.log(
    "PASS absolute editor applies to SQL + reload; reversed selection cannot commit"
  );

  // Change via the actual header, clearing old local windows and page cursors.
  await page.goto(
    `${base}/dashboard/logs/?from=${start}&to=${end}&after=old&cursor=old&timestamp=old`
  );
  const trigger = page.getByRole("button", { name: /^Time range:/ });
  await trigger.click();
  const dialog = page.getByRole("dialog", { name: "Choose time range" });
  await dialog
    .getByRole("button", { name: "Last 5 minutes", exact: true })
    .click();
  await page.waitForURL(
    (url) => url.searchParams.get("timeStart") === "now-5m"
  );
  let url = new URL(page.url());
  for (const key of ["from", "to", "timestamp", "cursor", "after"])
    assert.equal(url.searchParams.has(key), false, key);
  await page.reload();
  await page
    .getByRole("button", { name: "Time range: Last 5 minutes", exact: true })
    .waitFor();
  console.log(
    "PASS header apply + URL roundtrip + cursor/legacy override reset"
  );

  queries.length = 0;
  await page.goto(`${base}/dashboard/?timeStart=nonsense&timeEnd=now`);
  await page
    .getByRole("heading", { name: "Invalid time range", exact: true })
    .waitFor();
  assert.equal(queries.filter((q) => q !== "SELECT 1 AS ok").length, 0);
  await page.getByRole("button", { name: /^Time range:/ }).click();
  await dialog.getByRole("button", { name: "Last hour", exact: true }).click();
  await page.waitForURL(
    (url) =>
      !url.searchParams.has("timeStart") &&
      url.searchParams.get("range") === "1h"
  );
  await page
    .getByRole("heading", { name: "Invalid time range", exact: true })
    .waitFor({ state: "hidden" });
  assert.deepEqual(errors, []);
  console.log("PASS malformed URL blocks queries and header recovers");
} finally {
  await browser.close();
}
