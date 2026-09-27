/**
 * Drive the OAuth device flow against production in a real browser, then prove
 * the issued access token is a usable, correctly-scoped JWT.
 *
 * This is the half of onboarding the sign-up check does not cover: a signed-up
 * user connecting an agent and getting an ingest credential. It stayed broken
 * in production while every surface check was green, because the device
 * endpoint's failure was a plain 400 that nothing asserted against.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

import { chromium } from "playwright";

const BASE = process.env.SIGIRO_E2E_BASE ?? "https://sigiro.com";
const INBOX = process.env.SIGIRO_E2E_INBOX ?? "akshat@agentmail.to";
const CLIENT_ID = `${BASE}/.well-known/oauth-client.json`;

interface DeviceCodeResponse {
  readonly device_code: string;
  readonly user_code: string;
  readonly verification_uri_complete: string;
}

interface TokenResponse {
  readonly access_token?: string;
  readonly token_type?: string;
  readonly scope?: string;
  readonly error?: string;
}

interface AgentMailMessage {
  readonly timestamp?: string;
  readonly created_at?: string;
  readonly preview?: string;
  readonly subject?: string;
}

const pass = (label: string, detail = ""): void => {
  console.log(`  ok   ${label}${detail ? `  ${detail}` : ""}`);
};

/** The newest verification code delivered after `since`. */
const awaitEmailedCode = async (since: Date): Promise<string> => {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const raw = execFileSync(
      "agentmail",
      [
        "inboxes",
        "messages",
        "list",
        "--inbox-id",
        INBOX,
        "--limit",
        "3",
        "--format",
        "json",
      ],
      { encoding: "utf8" }
    );
    // SAFETY: `--format json` output; a shape mismatch surfaces as no code found
    // and the loop times out rather than reading a wrong value.
    const parsed = JSON.parse(raw) as {
      messages?: readonly AgentMailMessage[];
    };
    for (const message of parsed.messages ?? []) {
      const stamp = message.timestamp ?? message.created_at;
      if (!stamp || new Date(stamp) <= since) continue;
      const code = /\b(\d{6})\b/.exec(
        `${message.subject ?? ""} ${message.preview ?? ""}`
      );
      if (code) return code[1];
    }
    await new Promise((resolve) => setTimeout(resolve, 6000));
  }
  throw new Error("no verification code arrived");
};

const form = (fields: Record<string, string>): string =>
  new URLSearchParams(fields).toString();

const main = async (): Promise<void> => {
  console.log(`device flow against ${BASE}`);

  // 1. A client the server has never seen asks for a device code. This is the
  //    request that was returning invalid_client in production.
  const deviceResponse = await fetch(`${BASE}/api/auth/device/code`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: form({
      client_id: CLIENT_ID,
      scope: "openid profile email offline_access telemetry:ingest query:read",
      resource: BASE,
    }),
  });
  assert.equal(
    deviceResponse.status,
    200,
    `device/code returned ${deviceResponse.status}`
  );
  // SAFETY: status asserted 200 above; the device_code assertion below fails
  // loudly if the body is not the documented shape.
  const device = (await deviceResponse.json()) as DeviceCodeResponse;
  assert.ok(device.device_code, "no device_code issued");
  pass("device code issued", device.user_code);

  // 2. Polling before approval must say pending, not fail.
  // SAFETY: only `.error` is read, and it is asserted against an exact value.
  const pending = (await (
    await fetch(`${BASE}/api/auth/oauth2/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: form({
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        device_code: device.device_code,
        client_id: CLIENT_ID,
        resource: BASE,
      }),
    })
  ).json()) as TokenResponse;
  assert.equal(pending.error, "authorization_pending");
  pass("pending before approval");

  // 3. A real user approves it in a real browser.
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    const requestedAt = new Date();
    await page.goto(`${BASE}/sign-in`, { waitUntil: "domcontentloaded" });
    await page.fill('input[name="email"]', INBOX);
    await page.click('#email-form button[type="submit"]');
    await page.waitForSelector("#otp-form", {
      state: "visible",
      timeout: 30_000,
    });
    const code = await awaitEmailedCode(requestedAt);
    // The visual slots share one native input, so this mirrors mobile autofill
    // and password-manager input while retaining ordinary form semantics.
    await page.locator('#otp-form input[name="verification-code"]').fill(code);
    // Wait for the PAGE to finish signing in, then confirm the session once.
    //
    // Two wrong ways to do this, both of which produced failures blamed on the
    // product. Polling `status === 200` passes instantly, because signed-out
    // /get-session answers 200 with a `null` body — the navigation then races
    // the in-flight sign-in and aborts the response that sets the cookie.
    // Polling the body instead fires a fetch per animation frame, which trips
    // the server's rate limiter and makes the device page's own /get-session
    // fail, so it renders the sign-in form.
    //
    // The form hiding is the page's own signal that sign-in resolved; one
    // assertion after it confirms the session without hammering anything.
    await page.waitForFunction(
      () => document.querySelector("#otp-form") === null,
      undefined,
      {
        timeout: 30_000,
      }
    );
    const sessionUser = await page.evaluate(
      async (): Promise<string | null> => {
        const response = await fetch("/api/auth/get-session", {
          credentials: "include",
        });
        // SAFETY: signed-out responses are the JSON literal `null`, which parses
        // to null and yields null here; the assertion below rejects it.
        const body = (await response.json()) as {
          user?: { email?: string };
        } | null;
        return body?.user?.email ?? null;
      }
    );
    assert.equal(sessionUser, INBOX, "sign-in did not establish a session");
    pass("signed in", INBOX);

    const pageErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error")
        pageErrors.push(`console: ${message.text()}`);
    });
    page.on("pageerror", (error) =>
      pageErrors.push(`pageerror: ${error.message}`)
    );
    page.on("response", (response) => {
      if (!response.ok() && response.url().includes("/api/auth")) {
        pageErrors.push(
          `${response.status()} ${new URL(response.url()).pathname}`
        );
      }
    });
    await page.goto(device.verification_uri_complete, {
      waitUntil: "domcontentloaded",
    });
    // The approval control is #approve inside #device-flow, which the page
    // unhides once the code is resolved. Do NOT match a bare submit button:
    // the sign-in form's hidden button also matches and never becomes visible.
    const approve = page.locator("#approve");
    try {
      await approve.waitFor({ state: "visible", timeout: 20_000 });
    } catch (cause) {
      // The page hides #device-flow and writes the reason into #status/#error;
      // surface that, plus any console/network errors, instead of a bare
      // selector timeout that says nothing about why.
      const status = await page.locator("#status, #error").allTextContents();
      throw new Error(
        `device page never revealed the approval control: [${status.join(" | ")}] ${pageErrors.join("; ")}`,
        { cause }
      );
    }
    await approve.click();
    await page.waitForSelector("text=/Return to your terminal/i", {
      timeout: 20_000,
    });

    pass("device approved in browser");
  } finally {
    await browser.close();
  }

  // 4. The token must now be issued.
  // SAFETY: only `.access_token` is read, and it is asserted present before use.
  const issued = (await (
    await fetch(`${BASE}/api/auth/oauth2/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: form({
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        device_code: device.device_code,
        client_id: CLIENT_ID,
        resource: BASE,
      }),
    })
  ).json()) as TokenResponse;
  assert.ok(
    issued.access_token,
    `no access_token issued: ${JSON.stringify(issued)}`
  );
  pass("access token issued", issued.token_type ?? "");

  // 5. It must be a real JWT carrying the tenant and the ingest scope — this is
  //    what the Rust host verifies against the published JWKS.
  const [, payloadSegment] = issued.access_token.split(".");
  assert.ok(payloadSegment, "access token is not a JWT");
  // SAFETY: decoded JWT payload; every field read is asserted immediately below,
  // so a malformed token fails the check rather than passing silently.
  const claims = JSON.parse(
    Buffer.from(payloadSegment, "base64url").toString("utf8")
  ) as {
    scope?: string;
    tid?: string;
    aud?: string | readonly string[];
  };
  assert.match(claims.scope ?? "", /telemetry:ingest/);
  assert.ok(claims.tid, "token carries no tenant claim");
  pass("token scoped for ingest", claims.tid);

  console.log("device flow: an agent can obtain a tenant-scoped ingest token");
};

await main();
