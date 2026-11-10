import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";

import { chromium } from "playwright";
const require = createRequire(import.meta.url);
const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19990";
const out = process.env.EVIDENCE_DIR ?? "/tmp/sigiro-flow-evidence";
fs.mkdirSync(out, { recursive: true });
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
const measurements = [],
  checks = [],
  accessibility = [],
  errors = [];
const url = `${origin}/dashboard/logs/?timestamp=${process.env.LOG_WINDOW ?? "1794243300000-1794243450000"}`;
try {
  for (const [name, width, height] of [
    ["desktop", 1600, 1000],
    ["laptop", 1366, 900],
    ["mobile", 390, 844],
  ]) {
    const context = await browser.newContext({ viewport: { width, height } });
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(url);
    await page.locator("[data-event-row]").first().waitFor();
    await page.waitForTimeout(250);
    const measure = () =>
      page.evaluate(() => ({
        viewport: innerWidth,
        documentWidth: document.documentElement.scrollWidth,
        documentHeight: document.documentElement.scrollHeight,
        results: document
          .querySelector('section[aria-label="Structured log explorer"]')
          .getBoundingClientRect()
          .toJSON(),
        message: document
          .querySelector('[data-event-row] [data-column="message"]')
          .getBoundingClientRect()
          .toJSON(),
        scroll: document
          .querySelector('[aria-label^="Log events."]')
          .getBoundingClientRect()
          .toJSON(),
      }));
    const initial = await measure();
    measurements.push({ name, state: "default", ...initial });
    assert.equal(initial.documentWidth, width);
    assert(initial.documentHeight <= height + 1);
    if (name === "mobile") {
      assert(
        initial.scroll.top <= 360,
        "preserve results-first mobile first-row position"
      );
      assert(
        initial.scroll.height >= 390,
        "preserve results-first mobile list space"
      );
    }
    const filters = page.getByRole("button", { name: "Filters", exact: true });
    if (width > 767) {
      assert.equal(
        await page
          .locator('[data-slot="sidebar"][data-state]')
          .getAttribute("data-state"),
        "collapsed"
      );
      assert.equal(await filters.getAttribute("aria-expanded"), "false");
      assert.equal(
        await page.getByRole("checkbox", { name: /dashboard-e2e/ }).count(),
        0,
        "collapsed filters are inert, not exposed to accessibility tree"
      );
    }
    assert.equal(
      await page
        .getByRole("region", { name: "Event inspector", exact: true })
        .count(),
      0
    );
    await page.getByRole("button", { name: /^Remove Time:/ }).waitFor();
    await page.screenshot({
      path: `${out}/after-${name}.png`,
      fullPage: false,
    });
    await page.addScriptTag({ path: require.resolve("axe-core/axe.min.js") });
    const axe = await page.evaluate(async () =>
      (await axe.run(document.body)).violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        nodes: v.nodes.map((n) => ({
          target: n.target,
          summary: n.failureSummary,
        })),
      }))
    );
    accessibility.push({ name, state: "whole-page default", violations: axe });
    // This is a full-page diagnostic, not a blanket certification. Existing shell widgets remain partially Radix/Vaul/cmdk.
    assert.deepEqual(axe, []);
    await filters.click();
    await page.getByRole("checkbox", { name: /dashboard-e2e/ }).waitFor();
    if (width > 767) {
      assert.equal(await filters.getAttribute("aria-expanded"), "true");
      await page.screenshot({
        path: `${out}/after-${name}-facets.png`,
        fullPage: false,
      });
      await filters.click();
      await page.waitForTimeout(250);
    } else {
      const filterDialog = page.getByRole("dialog", {
        name: "Filters",
        exact: true,
      });
      await filterDialog.waitFor();
      const included = page.waitForResponse(
        (response) =>
          response.url().endsWith("/v1/query") &&
          (response.request().postData() ?? "").includes(
            "ORDER BY timestamp DESC LIMIT"
          ) &&
          (response.request().postData() ?? "").includes(
            "service_name IN ('dashboard-e2e')"
          )
      );
      await filterDialog
        .getByRole("checkbox", { name: /dashboard-e2e/ })
        .click();
      assert.equal((await (await included).json()).length, 2);
      await page.waitForURL(
        (current) => current.searchParams.get("service") === "dashboard-e2e"
      );
      await page.screenshot({
        path: `${out}/after-mobile-facets.png`,
        fullPage: false,
      });
      const facetAxe = await page.evaluate(async () =>
        (
          await axe.run(document.querySelector('[role="dialog"]'))
        ).violations.map((v) => ({
          id: v.id,
          impact: v.impact,
          nodes: v.nodes.map((n) => n.target),
        }))
      );
      accessibility.push({
        name,
        state: "opt-in existing mobile facets",
        violations: facetAxe,
      });
      assert.deepEqual(facetAxe, []);
      await filterDialog
        .getByRole("button", { name: "Close", exact: true })
        .click();
      await filterDialog.waitFor({ state: "hidden" });
      await page.waitForFunction(
        () => document.activeElement?.getAttribute("aria-label") === "Filters"
      );
      assert.equal(
        await page.evaluate(() =>
          document.activeElement?.getAttribute("aria-label")
        ),
        "Filters"
      );
      await page
        .getByRole("button", {
          name: "Remove Service: dashboard-e2e",
          exact: true,
        })
        .click();
      await page.waitForURL(
        (current) => current.searchParams.get("service") === null
      );
    }
    const navigation = page.getByRole("button", {
      name: "Toggle Sidebar",
      exact: true,
    });
    await navigation.click();
    if (width > 767) {
      await page.waitForFunction(
        () =>
          document
            .querySelector('[data-slot="sidebar"][data-state]')
            ?.getAttribute("data-state") === "expanded"
      );
      await page.getByRole("link", { name: "Traces", exact: true }).waitFor();
      await page.screenshot({
        path: `${out}/after-${name}-navigation.png`,
        fullPage: false,
      });
      await page.keyboard.press("Control+b");
      await page.waitForFunction(
        () =>
          document
            .querySelector('[data-slot="sidebar"][data-state]')
            ?.getAttribute("data-state") === "collapsed"
      );
      assert.equal(
        await filters.getAttribute("aria-expanded"),
        "false",
        "navigation shortcut must not also open facets"
      );
    } else {
      const navigationDialog = page.getByRole("dialog", {
        name: "Sidebar",
        exact: true,
      });
      await navigationDialog.waitFor();
      await navigationDialog
        .getByRole("link", { name: "Traces", exact: true })
        .waitFor();
      await page.screenshot({
        path: `${out}/after-mobile-navigation.png`,
        fullPage: false,
      });
      await page.keyboard.press("Escape");
      await navigationDialog.waitFor({ state: "hidden" });
      await page.waitForFunction(
        () =>
          document.activeElement?.getAttribute("data-slot") ===
          "sidebar-trigger"
      );
      assert.equal(
        await page.evaluate(() =>
          document.activeElement?.getAttribute("data-slot")
        ),
        "sidebar-trigger"
      );
    }
    await page.waitForTimeout(250);
    await page.locator("[data-event-row]").first().click();
    const detail =
      width > 767
        ? page.getByRole("region", { name: "Event inspector", exact: true })
        : page.getByRole("dialog", { name: "Event details", exact: true });
    await detail.waitFor();
    await page.waitForTimeout(250);
    const selected = await measure();
    measurements.push({ name, state: "selected", ...selected });
    assert.equal(selected.results.width, initial.results.width);
    assert.equal(selected.message.width, initial.message.width);
    await page.screenshot({
      path: `${out}/after-${name}-selected.png`,
      fullPage: false,
    });
    await detail.getByRole("button", { name: "Close event inspector" }).click();
    await detail.waitFor({ state: "hidden" });
    await page.waitForFunction(() =>
      Boolean(document.activeElement?.matches("[data-event-row]"))
    );
    assert(
      await page.evaluate(() =>
        Boolean(document.activeElement?.matches("[data-event-row]"))
      )
    );
    // Select the real smaller trace for navigation; the separate trace regression exercises the 2,102-span trace too.
    await page.locator("[data-event-row]").nth(1).click();
    const trace = page.getByRole("link", { name: "View trace", exact: true });
    const traceId = new URL(
      await trace.getAttribute("href"),
      origin
    ).searchParams.get("trace");
    assert.match(traceId, /^[a-f0-9]{32}$/);
    await trace.click();
    await page.locator('[data-trace-workspace="82"]').waitFor();
    assert.equal(new URL(page.url()).searchParams.get("trace"), traceId);
    await page.goBack();
    await page.locator("[data-event-row]").first().waitFor();
    if (width > 767)
      assert.equal(
        await page
          .locator('[data-slot="sidebar"][data-state]')
          .getAttribute("data-state"),
        "collapsed"
      );
    const restoredDetail =
      width > 767
        ? page.getByRole("region", { name: "Event inspector", exact: true })
        : page.getByRole("dialog", { name: "Event details", exact: true });
    if (await restoredDetail.isVisible())
      await restoredDetail
        .getByRole("button", { name: "Close event inspector" })
        .click();
    await navigation.click();
    await page.getByRole("link", { name: "Traces", exact: true }).click();
    await page
      .getByRole("cell", {
        name: "UI verification workload (80 hashes)",
        exact: true,
      })
      .waitFor();
    if (width < 768) {
      await page
        .getByRole("dialog", { name: "Sidebar", exact: true })
        .waitFor({ state: "hidden" });
      await page
        .getByRole("button", { name: "Toggle Sidebar", exact: true })
        .click();
    }
    await page.getByRole("link", { name: "Logs", exact: true }).click();
    await page.locator("[data-event-row]").first().waitFor();
    if (width < 768)
      await page
        .getByRole("dialog", { name: "Sidebar", exact: true })
        .waitFor({ state: "hidden" });
    else
      assert.equal(
        await page
          .locator('[data-slot="sidebar"][data-state]')
          .getAttribute("data-state"),
        "collapsed"
      );
    checks.push(
      `${name}: real events; initial compact navigation and closed facets; active time chip; opt-in filters/navigation; focus returns; full-width details; real View trace/back; Traces/Logs navigation closes mobile sheet and resets logs rail`
    );
    await context.close();
  }
  assert.deepEqual(errors, []);
  fs.writeFileSync(`${out}/after.json`, JSON.stringify(measurements, null, 2));
  fs.writeFileSync(
    `${out}/flow-results.json`,
    JSON.stringify(
      {
        kind: "real-existing-backend; no API interception",
        checks,
        accessibility,
        errors,
      },
      null,
      2
    )
  );
  console.log(
    JSON.stringify({ measurements, checks, accessibility, errors }, null, 2)
  );
} finally {
  await browser.close();
}
