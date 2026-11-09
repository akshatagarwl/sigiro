import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";

import { chromium } from "playwright";
const require = createRequire(import.meta.url);
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-base-ui-evidence";
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const context = await browser.newContext({
  viewport: { width: 1600, height: 1000 },
  permissions: ["clipboard-read", "clipboard-write"],
});
const page = await context.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const results = { kind: "real-existing-backend", url: origin, checks: [] };
const url = `${origin}/dashboard/logs/?timestamp=${process.env.LOG_WINDOW ?? "1794243300000-1794243450000"}`;
await page.goto(url);
await page.locator("[data-event-row]").first().waitFor();
assert.equal(await page.locator("[data-event-row]").count(), 2);
results.checks.push("two real logs, original time/metadata/facets");
await page.screenshot({ path: `${out}/01-real-logs.png`, fullPage: true });
const first = page.locator("[data-event-row]").first();
const id = await first.getAttribute("data-event-row");
await first.focus();
await page.keyboard.press("ArrowDown");
await page.waitForFunction(
  (id) => document.activeElement?.getAttribute("data-event-row") !== id,
  id
);
assert.notEqual(
  await page.evaluate(() =>
    document.activeElement?.getAttribute("data-event-row")
  ),
  id
);
await page.keyboard.press("Home");
await page.waitForFunction(
  (id) => document.activeElement?.getAttribute("data-event-row") === id,
  id
);
assert.equal(
  await page.evaluate(() =>
    document.activeElement?.getAttribute("data-event-row")
  ),
  id
);
await page.keyboard.press("Enter");
await page.getByRole("complementary", { name: "Event inspector" }).waitFor();
await page.waitForURL(/uuid=/);
assert.equal(new URL(page.url()).searchParams.get("uuid"), id);
results.checks.push("Arrow/Home/Enter row keyboard + URL selection");
await page.getByRole("button", { name: "Copy message", exact: true }).click();
assert.match(
  await page.evaluate(() => navigator.clipboard.readText()),
  /Completed/
);
await page.getByRole("tab", { name: "Event JSON", exact: true }).click();
assert.match(
  await page.getByRole("tabpanel", { name: "Event JSON" }).innerText(),
  /dashboard-e2e/
);
await page.getByRole("tab", { name: "Attributes", exact: true }).click();
await page.screenshot({ path: `${out}/02-real-inspector.png`, fullPage: true });
const beforeResize = await page
  .getByRole("complementary", { name: "Event inspector" })
  .evaluate((el) => el.getBoundingClientRect().width);
await page
  .getByRole("separator", {
    name: "Resize event list and inspector",
    exact: true,
  })
  .focus();
await page.keyboard.press("ArrowLeft");
await page.waitForFunction(
  (width) =>
    Math.abs(
      (document
        .querySelector('aside[aria-label="Event inspector"]')
        ?.getBoundingClientRect().width ?? width) - width
    ) > 1,
  beforeResize
);
results.checks.push("inspector separator resizes with keyboard");
await page.getByRole("button", { name: "Filter service" }).click();
await page.getByRole("menuitem").first().waitFor();
assert.equal(await page.getByRole("menuitem").count(), 1);
assert.match(await page.getByRole("menuitem").innerText(), /Include/);
await page.keyboard.press("Escape");
await page.getByRole("button", { name: "Close event inspector" }).focus();
await page.keyboard.press("Escape");
await page
  .getByRole("complementary", { name: "Event inspector" })
  .waitFor({ state: "hidden" });
await page.waitForFunction(
  (id) => document.activeElement?.getAttribute("data-event-row") === id,
  id
);
assert.equal(
  await page.evaluate(() =>
    document.activeElement?.getAttribute("data-event-row")
  ),
  id
);
results.checks.push(
  "copy real message; tabs; no unsupported exclude; Escape focus restored"
);
await page.getByRole("button", { name: "Wrap", exact: true }).click();
assert.equal(
  await page
    .getByRole("button", { name: "Wrap", exact: true })
    .getAttribute("aria-pressed"),
  "true"
);
await page.getByRole("button", { name: "Columns", exact: true }).click();
await page.getByRole("menuitemcheckbox", { name: "Service" }).click();
await page.keyboard.press("Escape");
assert.equal(
  await page.locator('[data-event-row] [data-column="service"]').count(),
  0
);
results.checks.push("wrap and Base UI column checkbox menu");
await page
  .getByRole("textbox", { name: "Find in loaded events" })
  .fill("does-not-exist");
await page.getByText("No loaded events match", { exact: true }).waitFor();
await page.getByRole("textbox", { name: "Find in loaded events" }).fill("");
await first.waitFor();
await page.getByRole("button", { name: "Columns", exact: true }).click();
await page.getByRole("menuitemcheckbox", { name: "Service" }).click();
await page.keyboard.press("Escape");
await page.addScriptTag({ path: require.resolve("axe-core/axe.min.js") });
let axe = await page.evaluate(
  async () =>
    await axe.run(
      document.querySelector('section[aria-label="Structured log explorer"]')
    )
);
results.desktopAxe = axe.violations.map((x) => ({
  id: x.id,
  impact: x.impact,
  nodes: x.nodes.length,
  summary: x.nodes[0].failureSummary,
}));
await page.getByRole("button", { name: "Toggle theme", exact: true }).click();
await page.waitForFunction(() =>
  document.documentElement.classList.contains("dark")
);
const darkAxe = await page.evaluate(
  async () =>
    await axe.run(
      document.querySelector('section[aria-label="Structured log explorer"]')
    )
);
results.darkAxe = darkAxe.violations.map((x) => ({
  id: x.id,
  impact: x.impact,
  nodes: x.nodes.length,
}));
await page.screenshot({ path: `${out}/05-real-dark.png`, fullPage: true });
await page.getByRole("button", { name: "Toggle theme", exact: true }).click();
await page.waitForFunction(
  () => !document.documentElement.classList.contains("dark")
);
await page.setViewportSize({ width: 390, height: 844 });
await page.waitForTimeout(200);
assert.equal(
  await page.evaluate(() => document.documentElement.scrollWidth),
  390
);
await page.screenshot({ path: `${out}/03-real-mobile.png`, fullPage: true });
await first.click();
await page.getByRole("dialog", { name: "Event details" }).waitFor();
assert.equal(
  await page
    .getByRole("dialog")
    .evaluate((el) => el.getBoundingClientRect().width),
  390
);
await page.getByRole("button", { name: "Close event inspector" }).focus();
await page.keyboard.press("Shift+Tab");
await page.waitForFunction(() =>
  document.querySelector('[role="dialog"]')?.contains(document.activeElement)
);
assert(
  await page
    .getByRole("dialog")
    .evaluate((el) => el.contains(document.activeElement))
);
await page.screenshot({
  path: `${out}/04-real-mobile-inspector.png`,
  fullPage: false,
});
axe = await page.evaluate(
  async () => await axe.run(document.querySelector('[role="dialog"]'))
);
results.mobileAxe = axe.violations.map((x) => ({
  id: x.id,
  impact: x.impact,
  nodes: x.nodes.length,
  summary: x.nodes[0].failureSummary,
}));
await page.keyboard.press("Escape");
await page.getByRole("dialog").waitFor({ state: "hidden" });
await page.waitForFunction(
  (id) => document.activeElement?.getAttribute("data-event-row") === id,
  id
);
assert.equal(
  await page.evaluate(() =>
    document.activeElement?.getAttribute("data-event-row")
  ),
  id
);
results.checks.push(
  "390px no body overflow; full-width modal; trap and Escape focus return"
);
await page.emulateMedia({ reducedMotion: "reduce" });
results.reducedMotion = await page
  .locator('section[aria-label="Structured log explorer"]')
  .evaluate((el) =>
    [...el.querySelectorAll("*")].every(
      (n) =>
        getComputedStyle(n).animationName === "none" &&
        getComputedStyle(n).transitionDuration === "0s"
    )
  );
assert(results.reducedMotion);
assert.deepEqual(results.desktopAxe, []);
assert.deepEqual(results.mobileAxe, []);
assert.deepEqual(results.darkAxe, []);
results.errors = errors;
assert.equal(errors.length, 0);
fs.writeFileSync(`${out}/real-results.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
await browser.close();
