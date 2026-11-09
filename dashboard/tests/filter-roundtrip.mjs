import assert from "node:assert/strict";
import fs from "node:fs";

import { chromium } from "playwright";

const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-base-ui-evidence";
fs.mkdirSync(out, { recursive: true });
const url = `${origin}/dashboard/logs/?timestamp=${process.env.LOG_WINDOW ?? "1794243300000-1794243450000"}`;
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
try {
  const page = await browser.newPage({
    viewport: { width: 1600, height: 1000 },
  });
  const queries = [];
  page.on("response", async (response) => {
    if (!response.url().endsWith("/v1/query")) return;
    const sql = response.request().postData() ?? "";
    if (!sql.includes("ORDER BY timestamp DESC LIMIT")) return;
    queries.push({
      sql,
      status: response.status(),
      rows: (await response.json()).length,
    });
  });
  const resultRequest = (service) =>
    page.waitForResponse(
      (response) =>
        response.url().endsWith("/v1/query") &&
        (response.request().postData() ?? "").includes(
          "ORDER BY timestamp DESC LIMIT"
        ) &&
        (service
          ? (response.request().postData() ?? "").includes(
              `service_name IN ('${service.replaceAll("'", "''")}')`
            )
          : !(response.request().postData() ?? "").includes("service_name IN"))
    );
  await page.goto(url);
  await page.locator("[data-event-row]").first().waitFor();
  assert.equal(await page.locator("[data-event-row]").count(), 2);
  await page.locator("[data-event-row]").first().click();
  await page
    .getByRole("button", { name: "Filter service", exact: true })
    .click();
  const included = resultRequest("dashboard-e2e");
  await page
    .getByRole("menuitem", { name: "Include dashboard-e2e", exact: true })
    .click();
  const includeResponse = await included;
  assert.equal(includeResponse.status(), 200);
  assert.equal((await includeResponse.json()).length, 2);
  assert.equal(
    new URL(page.url()).searchParams.get("service"),
    "dashboard-e2e"
  );
  const reloaded = resultRequest("dashboard-e2e");
  await page.reload();
  assert.equal((await (await reloaded).json()).length, 2);
  await page.locator("[data-event-row]").first().waitFor();
  await page.getByRole("button", { name: "Close event inspector" }).click();
  const removed = resultRequest();
  await page.getByRole("checkbox", { name: /dashboard-e2e/ }).click();
  assert.equal((await (await removed).json()).length, 2);
  await page.waitForURL(
    (current) => current.searchParams.get("service") === null
  );
  const special = "quote' and \\backslash";
  const specialUrl = new URL(url);
  specialUrl.searchParams.set("service", special);
  const specialResponse = resultRequest(special);
  await page.goto(specialUrl.href);
  const response = await specialResponse;
  assert.equal(response.status(), 200);
  assert.deepEqual(await response.json(), []);
  assert.equal(new URL(page.url()).searchParams.get("service"), special);
  await page.getByText("No loaded events match", { exact: true }).waitFor();
  const specialReload = resultRequest(special);
  await page.reload();
  assert.deepEqual(await (await specialReload).json(), []);
  await page.goBack();
  await page.locator("[data-event-row]").first().waitFor();
  assert.equal(await page.locator("[data-event-row]").count(), 2);
  assert.equal(new URL(page.url()).searchParams.get("service"), null);
  await page.goForward();
  await page.getByText("No loaded events match", { exact: true }).waitFor();
  assert.equal(new URL(page.url()).searchParams.get("service"), special);
  assert(
    (await page.getByRole("alert").allTextContents()).every(
      (text) => !text.trim()
    )
  );
  const result = {
    kind: "real read-only backend round-trip, no fixture interception",
    service: "dashboard-e2e",
    matchedRows: 2,
    specialService: special,
    specialMatchedRows: 0,
    checks: [
      "include UI → URL → parsed filter → actual SQL → actual rows",
      "reload persists",
      "checkbox removal clears URL and SQL predicate",
      "quotes/backslashes preserved and SQL-quoted correctly, actual 200 empty response",
      "back/forward restores parsed URL state and displayed results",
    ],
    queries,
  };
  fs.writeFileSync(
    `${out}/filter-roundtrip.json`,
    JSON.stringify(result, null, 2)
  );
  console.log(JSON.stringify(result, null, 2));
} finally {
  await browser.close();
}
