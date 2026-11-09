import assert from "node:assert/strict";

import { chromium } from "playwright";

const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
try {
  const page = await browser.newPage();
  const data = Array.from({ length: 60 }, (_, i) => ({
    timestamp: 1794243364481000 - i * 1000,
    level: "info",
    service_name: "synthetic-pagination",
    severity_text: "INFO",
    body: `Synthetic pagination event ${i}`,
    attributes: "{}",
    trace_id: "",
    span_id: String(i),
  }));
  let requests = 0;
  const cursors = [];
  await page.route("**/v1/query", async (route) => {
    const sql = route.request().postData() ?? "";
    let rows = [];
    if (sql.includes("ORDER BY timestamp DESC LIMIT")) {
      cursors.push(sql);
      rows = requests++ === 0 ? data.slice(0, 50) : data.slice(50);
    } else if (sql.includes("AS f0"))
      rows = [{ f0: "info", f1: "synthetic-pagination", n: 60 }];
    else if (sql.includes("time_bucket"))
      rows = [{ t: 1794243360000, s: "info", n: 60 }];
    else if (sql.includes("count(*)")) rows = [{ n: 60 }];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(rows),
    });
  });
  await page.goto(
    `${origin}/dashboard/logs/?timestamp=1794243300000-1794243450000`
  );
  await page.getByText("50 / 50 loaded", { exact: true }).waitFor();
  await page.locator("[data-event-row]").first().click();
  await page
    .getByRole("region", { name: "Event inspector", exact: true })
    .waitFor();
  await page.waitForURL((current) => current.searchParams.has("uuid"));
  const selectedId = new URL(page.url()).searchParams.get("uuid");
  await page.keyboard.press("End");
  await page.waitForFunction(() =>
    document.activeElement?.textContent?.includes(
      "Synthetic pagination event 49"
    )
  );
  await page
    .getByRole("button", { name: "Load more events", exact: true })
    .click();
  await page.getByText("60 / 60 loaded", { exact: true }).waitFor();
  assert.equal(new URL(page.url()).searchParams.get("uuid"), selectedId);
  assert(
    await page
      .locator('[aria-label^="Log events."]')
      .evaluate((el) => el.scrollTop > 200),
    "appending does not jump back to the selected first event"
  );
  assert(
    cursors[1].includes(`timestamp < make_timestamp(${data[49].timestamp})`)
  );
  await page
    .getByText("No more events in this query.", { exact: true })
    .waitFor();
  assert.equal(requests, 2);
  await page.locator("[data-event-row]").first().focus();
  await page.keyboard.press("End");
  await page.waitForFunction(() =>
    document.activeElement?.textContent?.includes(
      "Synthetic pagination event 59"
    )
  );
  console.log(
    JSON.stringify(
      {
        kind: "SYNTHETIC intercepted pagination responses",
        checks: [
          "50 + 10 real-adapter cursor pages append",
          "last row timestamp used unchanged",
          "load more stops at authoritative 60",
          "selection persists and appending at bottom does not scroll back to selected first event",
          "virtualized End reaches appended final row",
        ],
      },
      null,
      2
    )
  );
} finally {
  await browser.close();
}
