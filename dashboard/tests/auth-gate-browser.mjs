import assert from "node:assert/strict";

import { chromium } from "playwright";

const origin = process.env.UI_ORIGIN ?? "http://127.0.0.1:19995";
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
);
try {
  for (const failure of [404, 503, "invalid", "network"]) {
    const context = await browser.newContext();
    const page = await context.newPage();
    let ready = false;
    const queries = [],
      identityRequests = [];
    await page.route("**/v1/auth/config", (route) => {
      if (ready) return route.fulfill({ json: { mode: "open" } });
      if (failure === "network") return route.abort();
      return route.fulfill(
        failure === "invalid"
          ? { json: { mode: "unknown" } }
          : { status: failure, body: "Backend unavailable" }
      );
    });
    await page.route("**/v1/query", (route) => {
      queries.push({
        sql: route.request().postData(),
        authorization: route.request().headers().authorization,
      });
      return route.fulfill({ json: [] });
    });
    await page.route("**/v1/alerts?**", (route) => route.fulfill({ json: {} }));
    page.on("request", (request) => {
      if (
        request.url().includes("/api/auth/") ||
        request.url().includes("/.well-known/oauth-client")
      )
        identityRequests.push(request.url());
    });
    // A stale hosted token must not be used by the local server.
    await page.addInitScript(() =>
      sessionStorage.setItem(
        "sigiro.dashboard.token",
        JSON.stringify({
          accessToken: "stale-hosted-token",
          expiresAt: Date.now() + 900000,
        })
      )
    );
    await page.goto(`${origin}/dashboard/`);
    await page.getByText("Cannot connect to Sigiro", { exact: true }).waitFor();
    assert.equal(
      await page.getByRole("button", { name: "Sign in", exact: true }).count(),
      0
    );
    assert.equal(queries.length, 0);
    ready = true;
    await page.getByRole("button", { name: "Retry connection" }).click();
    await page.getByRole("navigation", { name: "Main navigation" }).waitFor();
    await page.waitForFunction(() => document.querySelector("main") !== null);
    await page
      .getByRole("button", { name: "Local Sigiro Workspace menu" })
      .click();
    assert.equal(await page.getByText("Sign out", { exact: true }).count(), 0);
    assert.equal(
      await page.getByText("Switch workspace", { exact: true }).count(),
      0
    );
    assert.equal(identityRequests.length, 0);
    assert.ok(queries.length > 0);
    assert.ok(
      queries.every(
        (query) => !query.authorization && query.sql !== "SELECT 1 AS ok"
      )
    );
    await context.close();
    console.log(
      `PASS: ${failure} never means signed out; retry enters open mode without OAuth or bearer tokens`
    );
  }

  const page = await browser.newPage();
  await page.route("**/v1/auth/config", (route) =>
    route.fulfill({
      json: { mode: "oauth", issuer: "https://sigiro.com/api/auth" },
    })
  );
  await page.route("**/api/auth/get-session*", (route) =>
    route.fulfill({ json: null })
  );
  let available = false;
  await page.route("**/.well-known/oauth-client", (route) =>
    route.fulfill(
      available
        ? {
            json: {
              client_id: `${origin}/.well-known/oauth-client`,
              application_type: "native",
              token_endpoint_auth_method: "none",
              redirect_uris: [`${origin}/dashboard/`],
              response_types: ["code"],
            },
          }
        : { status: 404, body: "Not found" }
    )
  );
  await page.route("**/api/auth/oauth2/authorize?**", (route) =>
    route.fulfill({ body: "Authorization reached" })
  );
  await page.goto(`${origin}/dashboard/`);
  const button = page.getByRole("button", { name: "Sign in", exact: true });
  await button.click();
  const alert = page.locator('[data-state="signed-out"] [role="alert"]');
  await alert.waitFor();
  assert.match(await alert.innerText(), /HTTP 404.*authentication service/);
  assert.equal(await button.isEnabled(), true);
  available = true;
  await button.click();
  await page.waitForURL("**/api/auth/oauth2/authorize?**");
  const url = new URL(page.url());
  assert.equal(url.searchParams.get("redirect_uri"), `${origin}/dashboard/`);
  assert.equal(url.searchParams.get("code_challenge_method"), "S256");
  assert.ok(url.searchParams.get("state"));
  console.log(
    "PASS: OAuth mode retains visible errors and PKCE sign-in (mock identity service)"
  );

  await page.route("**/v1/auth/config", (route) =>
    route.fulfill({
      json: { mode: "oauth", issuer: "https://unsupported.example/auth" },
    })
  );
  await page.goto(`${origin}/dashboard/`);
  await page
    .getByText("Authentication setup required", { exact: true })
    .waitFor();
  assert.equal(
    await page.getByRole("button", { name: "Sign in", exact: true }).count(),
    0
  );
  console.log(
    "PASS: unsupported issuer fails closed rather than redirecting to Sigiro Cloud"
  );
} finally {
  await browser.close();
}
