import assert from "node:assert/strict";

import { chromium } from "playwright";

const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
try {
  const page = await browser.newPage();
  // Rows 49 and 50 share a timestamp across the page boundary: the second
  // page must resume after (timestamp, key) of row 49, not skip row 50.
  const tsOf = (i) => 1794243364481000 - (i > 49 ? i - 1 : i) * 1000;
  const data = Array.from({ length: 60 }, (_, i) => ({
    timestamp: tsOf(i),
    sigiro_cursor_key: `${String(i).padStart(2, "0")}|synthetic-pagination|key`,
    level: "info",
    service_name: "synthetic-pagination",
    severity_text: "INFO",
    body: `Synthetic pagination event ${i}`,
    attributes: "{}",
    trace_id: "",
    span_id: String(i),
  }));
  let requests = 0;
  // The second page fails once, then succeeds on the inline retry.
  let pageFailures = 1;
  const cursors = [];
  await page.route("**/v1/query", async (route) => {
    const sql = route.request().postData() ?? "";
    let rows = [];
    if (sql.includes("ORDER BY timestamp DESC, sigiro_cursor_key DESC LIMIT")) {
      if (requests > 0 && pageFailures > 0) {
        pageFailures--;
        await route.fulfill({
          status: 500,
          contentType: "application/json",
          body: JSON.stringify({ error: "synthetic page failure" }),
        });
        return;
      }
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
  // A failed page is reported at the bottom, never as the table-level error,
  // and the loaded rows stay.
  await page
    .getByRole("alert")
    .filter({ hasText: "Could not load more events" })
    .waitFor({ timeout: 20_000 });
  assert.equal(
    await page.getByText(/Unable to (load|refresh) logs/).count(),
    0,
    "a failed page does not raise the table-level banner"
  );
  await page.getByText("50 / 50 loaded", { exact: true }).waitFor();
  await page
    .getByRole("alert")
    .filter({ hasText: "Could not load more events" })
    .getByRole("button", { name: "Retry", exact: true })
    .click();
  await page.getByText("60 / 60 loaded", { exact: true }).waitFor();
  assert.equal(pageFailures, 0);
  assert.equal(new URL(page.url()).searchParams.get("uuid"), selectedId);
  assert(
    await page
      .locator('[aria-label^="Log events."]')
      .evaluate((el) => el.scrollTop > 200),
    "appending does not jump back to the selected first event"
  );
  assert(
    cursors[1].includes(
      `(timestamp < make_timestamp(${data[49].timestamp}) OR (timestamp = make_timestamp(${data[49].timestamp}) AND `
    ) && cursors[1].includes(`< '${data[49].sigiro_cursor_key}'))`),
    `second page resumes after (timestamp, key) of the last row: ${cursors[1]}`
  );
  assert.equal(data[50].timestamp, data[49].timestamp);
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
          "a failed page shows inline at the bottom with retry, no table banner",
          "second page resumes after the last row's (timestamp, key), so a tied row is not skipped",
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
