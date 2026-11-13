import assert from "node:assert/strict";

import { chromium } from "playwright";

// UI-only fixture: no production auth changes or backend requests.
const base = process.env.DASHBOARD_PREVIEW_URL ?? "http://127.0.0.1:19988";
const browser = await chromium.launch({ channel: "chrome", headless: true });
try {
  for (const width of [320, 390, 768, 1440]) {
    const page = await browser.newPage({
      viewport: { width, height: 900 },
      timezoneId: "Asia/Kolkata",
    });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.route("**/v1/query", (route) => route.fulfill({ json: [] }));
    await page.goto(`${base}/dashboard/preview/time-range/`);
    await page.getByText(/^Browser time · Asia\/(Kolkata|Calcutta)$/).waitFor();
    const trigger = page
      .locator("main")
      .last()
      .getByRole("button", { name: /^Time range:/ });
    const dialog = page.getByRole("dialog", { name: "Choose time range" });
    const serialized = page.locator("code");
    assert.equal(await serialized.textContent(), "from=now-15m&to=now");

    await trigger.click();
    await dialog.waitFor();
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    assert.equal(
      await trigger.evaluate((el) => el === document.activeElement),
      true
    );

    await trigger.click();
    const startAmount = dialog
      .locator(".datetime-range-relative-input")
      .first();
    await startAmount.fill("30");
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.equal(await serialized.textContent(), "from=now-15m&to=now");

    await trigger.click();
    assert.equal(
      await startAmount.inputValue(),
      "15",
      "cancel discards the draft"
    );
    await startAmount.fill("30");
    await dialog
      .getByRole("button", { name: "Apply range", exact: true })
      .click();
    await dialog.waitFor({ state: "hidden" });
    assert.equal(await serialized.textContent(), "from=now-30m&to=now");

    await trigger.click();
    await dialog
      .getByRole("button", { name: "Last hour", exact: true })
      .click();
    await dialog.waitFor({ state: "hidden" });
    assert.equal(await serialized.textContent(), "from=now-1h&to=now");

    // Check both editing modes, with real calendar rendering on narrow screens.
    await trigger.click();
    await dialog
      .getByRole("tab", { name: "Absolute", exact: true })
      .first()
      .click();
    for (const dark of [false, true]) {
      await page.evaluate(
        (value) => document.documentElement.classList.toggle("dark", value),
        dark
      );
      const geometry = await dialog.evaluate((el) => {
        const rect = el.getBoundingClientRect();
        return {
          left: rect.left,
          right: rect.right,
          width: innerWidth,
          client: el.clientWidth,
          scroll: el.scrollWidth,
          document: document.documentElement.scrollWidth,
        };
      });
      assert.ok(
        geometry.left >= 0 && geometry.right <= width + 1,
        JSON.stringify(geometry)
      );
      assert.ok(
        geometry.scroll <= geometry.client + 1,
        JSON.stringify(geometry)
      );
      assert.ok(geometry.document <= geometry.width, JSON.stringify(geometry));
    }
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(errors, []);
    await page.close();
    console.log(
      `PASS ${width}px: draft/apply/cancel, presets, Escape/focus, calendar, light/dark, overflow`
    );
  }
} finally {
  await browser.close();
}
