import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";
const workload = JSON.parse(
  fs.readFileSync(
    process.env.TRACE_FIXTURE ?? "/tmp/sigiro-trace-workload.json"
  )
);
const out = path.resolve(
  process.env.SCREENSHOT_DIR ?? "../docs/assets/trace-workspace"
);
fs.mkdirSync(out, { recursive: true });
const base = process.env.DASHBOARD_URL ?? "http://127.0.0.1:19996/dashboard";
assert(
  ["localhost", "127.0.0.1"].includes(new URL(base).hostname),
  "Run against an isolated loopback server"
);
const browser = await chromium.launch({
  headless: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
const page = await browser.newPage({
  viewport: { width: 1600, height: 1000 },
  deviceScaleFactor: 1,
});
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const results = [];
const pass = (name) => {
  results.push(name);
  console.log("PASS", name);
};
const settle = () => page.waitForTimeout(650);
const shot = async (name) => {
  await settle();
  await page.screenshot({ path: out + "/" + name + ".png", fullPage: true });
};
await page.goto(base + "/traces/");
await page
  .getByRole("cell", {
    name: "UI verification workload (80 hashes)",
    exact: true,
  })
  .waitFor();
await shot("after-explorer");
await page
  .getByRole("combobox", { name: "Configure columns", exact: true })
  .click();
await page.getByPlaceholder("Search options...").waitFor();
await page.getByRole("option", { name: "Trace ID", exact: true }).click();
await page.keyboard.press("Escape");
await page
  .getByRole("columnheader", { name: "Trace ID", exact: true })
  .waitFor();
pass("explorer real root rows and working column picker");
await page.goto(base + "/traces/?name=80%20hashes");
await page
  .getByRole("cell", {
    name: "UI verification workload (80 hashes)",
    exact: true,
  })
  .waitFor();
assert.equal(
  await page
    .getByRole("cell", {
      name: "UI verification workload (2100 hashes)",
      exact: true,
    })
    .count(),
  0
);
await page.reload();
await page
  .getByRole("cell", {
    name: "UI verification workload (80 hashes)",
    exact: true,
  })
  .waitFor();
pass("root-name filter restored from URL after reload");
await page
  .getByRole("cell", {
    name: "UI verification workload (80 hashes)",
    exact: true,
  })
  .click();
await page.locator('[data-trace-workspace="82"]').waitFor();
await settle();
assert(new URL(page.url()).searchParams.get("uuid") === workload.normal.trace);
pass("row opens trace workspace with shareable uuid");
const timeline = page.getByLabel(
  "Span tree and timeline. Use arrow keys to select spans."
);
await page
  .getByRole("button", {
    name: "Collapse UI verification workload (80 hashes)",
    exact: true,
  })
  .click();
await page.waitForTimeout(150);
assert.equal(await page.locator("[data-span-row]").count(), 1);
await page
  .getByRole("button", {
    name: "Expand UI verification workload (80 hashes)",
    exact: true,
  })
  .click();
pass("span tree collapse and expand");
await page.getByRole("textbox", { name: "Find spans" }).fill("operation 13");
await page
  .getByRole("button", { name: "Inspect pbkdf2 operation 13,", exact: false })
  .click();
await page
  .getByRole("complementary", { name: "Span inspector" })
  .getByText("operation.input", { exact: true })
  .waitFor();
await page.waitForURL((url) => Boolean(url.searchParams.get("span")));
pass("span search, selected URL and recorded input/output");
await shot("after-detail");
await page.getByRole("tab", { name: "Events", exact: true }).click();
await page.getByText("hash computed", { exact: false }).waitFor();
await page.getByRole("tab", { name: "Attributes", exact: true }).click();
await page.getByRole("heading", { name: "Resource", exact: true }).waitFor();
pass("events and resource/span attributes");
await page.getByRole("textbox", { name: "Find spans" }).fill("");
await page.getByRole("button", { name: "Errors only", exact: true }).click();
await page
  .getByRole("button", { name: "Inspect GET /does-not-exist,", exact: false })
  .click();
await page.getByRole("alert").filter({ hasText: "404" }).waitFor();
assert.equal(await page.locator("[data-span-row]").count(), 1);
pass("errors-only filter and real HTTP 404 status message");
await shot("after-error");
await page.getByRole("button", { name: "Errors only", exact: true }).click();
const before = await timeline.evaluate((e) => e.scrollWidth);
await page.getByRole("button", { name: "Zoom in", exact: true }).click();
await page.waitForTimeout(100);
assert((await timeline.evaluate((e) => e.scrollWidth)) > before);
await page.getByRole("button", { name: "Reset zoom" }).click();
pass("timeline zoom controls");
await timeline.hover();
await page.keyboard.down("Control");
await page.mouse.wheel(0, -100);
await page.keyboard.up("Control");
await page
  .getByRole("button", { name: "Reset zoom" })
  .filter({ hasText: "1.5" })
  .waitFor();
await page.getByRole("button", { name: "Reset zoom" }).click();
pass("Laminar-adapted Ctrl+wheel zoom");
await timeline.focus();
await page.keyboard.press("Home");
await page.keyboard.press("ArrowDown");
await page
  .getByRole("complementary", { name: "Span inspector" })
  .getByRole("heading", { name: "pbkdf2 operation 0", exact: true })
  .waitFor();
pass("keyboard span navigation");
const separator = page.getByRole("separator", {
  name: "Resize timeline and inspector",
});
await separator.focus();
await page.keyboard.press("ArrowLeft");
pass("accessible panel resizing");
await page.waitForTimeout(200);
await page.reload();
await page
  .getByRole("complementary", { name: "Span inspector" })
  .getByRole("heading", { name: "pbkdf2 operation 0", exact: true })
  .waitFor();
pass("deep link survives reload");
await page.getByRole("button", { name: "Close", exact: true }).click();
await settle();
assert(!new URL(page.url()).searchParams.has("uuid"));
assert(!new URL(page.url()).searchParams.has("span"));
pass("close clears selected trace/span and restores explorer");
await page.goto(base + "/traces/?trace=" + workload.large.trace);
await page
  .getByRole("status")
  .filter({ hasText: "Showing the first 2,000 spans" })
  .waitFor();
assert((await page.locator("[data-span-row]").count()) < 70);
await timeline.evaluate((e) => {
  e.scrollTop = e.scrollHeight;
});
await page.waitForTimeout(200);
assert((await page.locator("[data-span-row]").count()) < 70);
pass("2102-span trace visibly capped at 2000; DOM virtualized at start/end");
await shot("after-large");
await page.goto(base + "/traces/?trace=not-a-trace");
await page.getByText("Not a trace id.", { exact: true }).waitFor();
pass("invalid trace ID");
await page.goto(base + "/traces/?trace=" + "0".repeat(32));
await page
  .getByText("No spans stored for this trace.", { exact: true })
  .waitFor();
pass("empty trace");
await page.goto(base + "/traces/?trace=" + workload.normal.trace);
await page.locator("[data-trace-workspace]").waitFor();
await page.getByRole("link", { name: "Trace logs", exact: true }).click();
await page
  .getByText("Completed 80 measured PBKDF2 operations", { exact: true })
  .waitFor();
assert(
  new URL(page.url()).searchParams.get("trace_id") === workload.normal.trace
);
pass("related logs link returns the real correlated log");
await page.goto(base + "/traces/?trace=" + workload.normal.trace);
await page.locator("[data-trace-workspace]").waitFor();
await page.getByRole("link", { name: "Span profile", exact: true }).click();
await page.waitForTimeout(1000);
assert(page.url().includes("/profiles/"));
assert(new URL(page.url()).searchParams.get("span") === workload.normal.root);
pass("profile link carries exact trace/span/service/window");
await page
  .getByText("No samples match these filters in this window.", { exact: true })
  .waitFor();
pass("profile destination shows honest empty state when no samples exist");
await page.setViewportSize({ width: 390, height: 844 });
await page.goto(base + "/traces/?trace=" + workload.normal.trace);
await page.locator("[data-trace-workspace]").waitFor();
await settle();
assert(
  (await page
    .locator('[data-slot="sheet-content"]')
    .evaluate((e) => e.scrollWidth - e.clientWidth)) < 2
);
pass("390px mobile sheet has no horizontal page overflow");
await shot("after-mobile");
await page.setViewportSize({ width: 1600, height: 1000 });
await page.emulateMedia({ colorScheme: "dark" });
await page.reload();
await page.locator("[data-trace-workspace]").waitFor();
await shot("after-dark");
assert.deepEqual(errors, []);
pass("no browser runtime errors");
fs.writeFileSync(
  out + "/browser-results.json",
  JSON.stringify({ results, errors, workload }, null, 2)
);
await browser.close();
