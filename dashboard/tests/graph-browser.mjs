import assert from "node:assert/strict";
import { createServer } from "node:http";

import { build } from "esbuild";
import { chromium } from "playwright";

const result = await build({
  entryPoints: ["tests/fixtures/graph.tsx"],
  bundle: true,
  jsx: "automatic",
  format: "esm",
  outdir: "/virtual",
  write: false,
});
const files = new Map(
  result.outputFiles.map((file) => [file.path.split("/").at(-1), file.contents])
);
const server = createServer((req, res) => {
  const name = req.url.slice(1);
  if (files.has(name)) {
    res.setHeader(
      "content-type",
      name.endsWith(".css") ? "text/css" : "application/javascript"
    );
    res.end(files.get(name));
  } else {
    res.setHeader("content-type", "text/html");
    res.end(
      '<!doctype html><html lang="en"><title>Synthetic reusable-component test</title><style>:root{--background:white;--foreground:#171717;--muted:#f5f5f5;--muted-foreground:#737373;--border:#ddd;--accent:#eee;--accent-foreground:#111;--primary:#222;--ring:#444;--popover:white;--popover-foreground:#111}body{font:14px system-ui}#root{height:calc(100svh - 16px);display:flex;flex-direction:column}</style><link rel="stylesheet" href="/graph.css"><div id="root"></div><script type="module" src="/graph.js"></script></html>'
    );
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
try {
  const context = await browser.newContext({
    permissions: ["clipboard-read", "clipboard-write"],
    viewport: { width: 1400, height: 1000 },
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(origin);
  await page.locator("[data-event-row]").click();
  await page.getByRole("region", { name: "Event inspector" }).waitFor();
  assert(
    (await page
      .locator('section[aria-label="Structured log explorer"] *')
      .count()) < 1000
  );
  await page.getByRole("button", { name: /^wide/ }).click();
  assert(
    (await page
      .locator('section[aria-label="Structured log explorer"] *')
      .count()) < 5000
  );
  await page.getByRole("button", { name: "Copy message", exact: true }).click();
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    "synthetic oversized message ".repeat(50_000)
  );
  await page.getByRole("tab", { name: "Event JSON", exact: true }).click();
  await page
    .getByRole("button", { name: "Copy event JSON", exact: true })
    .click();
  const copied = JSON.parse(
    await page.evaluate(() => navigator.clipboard.readText())
  );
  assert.equal(
    copied.attributes.oversized.length,
    "synthetic graph payload ".repeat(50_000).length
  );
  assert.equal(copied.attributes.integer, "123456789012345678901234567890");
  assert.equal(copied.attributes.self, "[Circular]");
  assert.deepEqual(copied.attributes.first, copied.attributes.second);
  assert(
    (await page.getByRole("tabpanel", { name: "Event JSON" }).innerText())
      .length < 51_000
  );
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async () => {
          throw new Error("synthetic clipboard denial");
        },
      },
    })
  );
  await page
    .getByRole("button", { name: "Copy event JSON", exact: true })
    .click();
  await page
    .getByText(
      "Copy failed. Full value was not copied; check clipboard permission.",
      { exact: true }
    )
    .waitFor();
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: undefined,
    })
  );
  await page
    .getByRole("button", { name: "Copy event JSON", exact: true })
    .click();
  await page
    .getByText("Clipboard unavailable. Full value was not copied.", {
      exact: true,
    })
    .waitFor();
  await page.goto(`${origin}/?deep=1`);
  await page.locator("[data-event-row]").click();
  await page.getByRole("region", { name: "Event inspector" }).waitFor();
  await page
    .getByRole("button", { name: "Actions for attributes", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Copy JSON", exact: true }).click();
  await page
    .getByText(
      "Value could not be serialized. Copy individual fields instead.",
      { exact: true }
    )
    .waitFor();
  await page.getByRole("tab", { name: "Event JSON", exact: true }).click();
  assert(
    await page
      .getByRole("button", { name: "Copy event JSON", exact: true })
      .isDisabled()
  );
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify(
      {
        kind: "standalone same-source SYNTHETIC component test, not dashboard API",
        fullMessageCharacters: copied.message.length,
        fullAttributeCharacters: copied.attributes.oversized.length,
        cycles: "[Circular]",
        bigInt: "decimal string",
        repeatedReferencesPreserved: true,
        boundedPreview: true,
        highFanoutInitiallyCollapsed: true,
        deepSerializationFailureVisible: true,
        clipboardRejectionVisible: true,
        clipboardUnavailableVisible: true,
        errors,
      },
      null,
      2
    )
  );
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
