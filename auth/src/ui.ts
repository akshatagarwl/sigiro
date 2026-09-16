const securityHeaders = {
  "cache-control": "no-store",
  "content-security-policy":
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
  "cross-origin-opener-policy": "same-origin",
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  "x-frame-options": "DENY",
} as const;

type PageKind = "sign-in" | "sign-up" | "device" | "agent-device" | "consent";

const page = (kind: PageKind): string => {
  const title =
    kind === "sign-up"
      ? "Create your Sigiro account"
      : kind === "agent-device"
        ? "Connect an agent to Sigiro"
        : kind === "device"
        ? "Connect the Sigiro CLI"
        : kind === "consent"
          ? "Authorize access"
          : "Sign in to Sigiro";
  const intro =
    kind === "sign-up"
      ? "Enter your email. We’ll send a one-time verification code and create your personal workspace."
      : kind === "device"
        ? "Sign in, review the code shown by your terminal, then approve this device."
        : kind === "agent-device"
          ? "Sign in, verify the approval code shown by your agent, then approve its requested capabilities."
        : kind === "consent"
          ? "Review and approve the access requested by your OAuth client."
          : "Enter your email. We’ll send a one-time verification code.";

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${title}</title>
  <meta name="description" content="Secure passwordless access to hosted Sigiro.">
  <link rel="stylesheet" href="/auth-ui.css">
  <script src="/auth-ui.js" defer></script>
</head>
<body data-page="${kind}">
  <main>
    <a class="brand" href="https://sigiro.com" aria-label="Sigiro home">sigiro</a>
    <section class="card" aria-labelledby="title">
      <p class="eyebrow">hosted sigiro</p>
      <h1 id="title">${title}</h1>
      <p class="intro">${intro}</p>

      <div id="auth-flow" hidden>
        <form id="email-form">
          <label id="name-label" for="name">Name</label>
          <input id="name" name="name" autocomplete="name" maxlength="100">
          <label for="email">Email</label>
          <input id="email" name="email" type="email" autocomplete="email" required>
          <button type="submit">Send verification code</button>
        </form>
        <form id="otp-form" hidden>
          <label for="otp">Verification code</label>
          <input id="otp" name="otp" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{6}" maxlength="6" required>
          <button type="submit">Verify and continue</button>
          <button class="secondary" id="restart" type="button">Use another email</button>
        </form>
      </div>

      <div id="device-flow" hidden>
        <p class="code-label">Confirm this code matches your terminal</p>
        <output id="user-code" class="code"></output>
        <dl id="request-details" hidden>
          <div><dt>Client</dt><dd id="client-id"></dd></div>
          <div><dt>Access</dt><dd id="scope"></dd></div>
          <div><dt>Resource</dt><dd id="resource"></dd></div>
        </dl>
        <div class="actions">
          <button id="approve" type="button">Approve request</button>
          <button class="secondary" id="deny" type="button">Deny</button>
        </div>
      </div>

      <div id="consent-flow" hidden>
        <p>The requesting client will receive only the scopes in its signed authorization request.</p>
        <div class="actions">
          <button id="accept-consent" type="button">Approve access</button>
          <button class="secondary" id="deny-consent" type="button">Deny</button>
        </div>
      </div>

      <div id="success" hidden>
        <h2>You're ready.</h2>
        <p id="success-message">Your Sigiro account is active.</p>
        <a href="https://sigiro.com/docs/how-to/hosted-onboarding">Continue to hosted setup</a>
      </div>

      <p id="status" role="status" aria-live="polite"></p>
      <p id="error" role="alert" hidden></p>
    </section>
    <p class="fine-print">Passwordless sign-in · 6-digit codes expire after five minutes</p>
  </main>
</body>
</html>`;
};

const script = String.raw`(() => {
  "use strict";
  const api = "/api/auth";
  const page = document.body.dataset.page;
  const status = document.querySelector("#status");
  const error = document.querySelector("#error");
  const authFlow = document.querySelector("#auth-flow");
  const emailForm = document.querySelector("#email-form");
  const otpForm = document.querySelector("#otp-form");
  const nameInput = document.querySelector("#name");
  const emailInput = document.querySelector("#email");
  const otpInput = document.querySelector("#otp");
  let email = "";

  const setStatus = (message) => {
    status.textContent = message;
    error.hidden = true;
  };
  const setError = (message) => {
    error.textContent = message;
    error.hidden = false;
    status.textContent = "";
  };
  const request = async (path, options = {}) => {
    const response = await fetch(api + path, {
      credentials: "same-origin",
      ...options,
      headers: options.body === undefined ? options.headers : { "content-type": "application/json", ...options.headers },
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.message || body.error_description || body.error || "Request failed");
    return body;
  };
  const ensureWorkspace = async () => request("/onboarding/ensure-personal-organization", {
    method: "POST",
    body: "{}",
  });
  const session = async () => request("/get-session");
  const signedIn = async () => Boolean((await session())?.session);
  const oauthQuery = new URLSearchParams(location.search).get("oauth_query");

  const finishOAuthLogin = async () => {
    if (!oauthQuery) return false;
    const body = { oauth_query: oauthQuery };
    if (page === "sign-up") body.created = true;
    const result = await request("/oauth2/continue", { method: "POST", body: JSON.stringify(body) });
    location.assign(result.redirect_uri);
    return true;
  };

  const showSuccess = (message) => {
    authFlow.hidden = true;
    document.querySelector("#device-flow").hidden = true;
    document.querySelector("#consent-flow").hidden = true;
    document.querySelector("#success-message").textContent = message;
    document.querySelector("#success").hidden = false;
    setStatus("");
  };

  const showAuth = () => {
    authFlow.hidden = false;
    if (page !== "sign-up") {
      document.querySelector("#name-label").hidden = true;
      nameInput.hidden = true;
      nameInput.removeAttribute("required");
    } else {
      nameInput.required = true;
    }
  };

  emailForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    email = emailInput.value.trim().toLowerCase();
    setStatus("Sending your code…");
    try {
      await request("/email-otp/send-verification-otp", {
        method: "POST",
        body: JSON.stringify({ email, type: "sign-in" }),
      });
      emailForm.hidden = true;
      otpForm.hidden = false;
      otpInput.focus();
      setStatus("Check your inbox for the six-digit code.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not send the code");
    }
  });

  otpForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    setStatus("Verifying…");
    try {
      const body = { email, otp: otpInput.value.trim() };
      if (page === "sign-up") body.name = nameInput.value.trim();
      await request("/sign-in/email-otp", { method: "POST", body: JSON.stringify(body) });
      await ensureWorkspace();
      if (page === "device") return showDevice();
      if (page === "agent-device") return showAgentDevice();
      if (await finishOAuthLogin()) return;
      showSuccess("Your Sigiro account and personal workspace are active.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Verification failed");
    }
  });

  document.querySelector("#restart").addEventListener("click", () => {
    email = "";
    otpInput.value = "";
    otpForm.hidden = true;
    emailForm.hidden = false;
    emailInput.focus();
    setStatus("");
  });

  const userCode = new URLSearchParams(location.search).get("user_code")?.trim() || "";
  const showDevice = async () => {
    if (!userCode) return setError("This device link is missing its user code. Run sigiro auth login again.");
    if (!(await signedIn())) {
      showAuth();
      return;
    }
    await ensureWorkspace();
    const details = await request("/device?user_code=" + encodeURIComponent(userCode));
    document.querySelector("#user-code").textContent = userCode;
    document.querySelector("#client-id").textContent = details.client_id || "Unknown client";
    document.querySelector("#scope").textContent = details.scope || "No scopes requested";
    document.querySelector("#resource").textContent = Array.isArray(details.resource)
      ? details.resource.join(", ")
      : details.resource || "No resource requested";
    document.querySelector("#request-details").hidden = false;
    authFlow.hidden = true;
    document.querySelector("#device-flow").hidden = false;
    setStatus("Review the request before approving it.");
  };

  const decideDevice = async (approve) => {
    setStatus(approve ? "Approving…" : "Denying…");
    try {
      await request(approve ? "/device/approve" : "/device/deny", {
        method: "POST",
        body: JSON.stringify({ userCode }),
      });
      showSuccess(approve ? "Return to your terminal. The CLI is connected." : "The device request was denied.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not process the device request");
    }
  };
  if (page !== "agent-device") {
    document.querySelector("#approve").addEventListener("click", () => decideDevice(true));
    document.querySelector("#deny").addEventListener("click", () => decideDevice(false));
  }

  const agentId = new URLSearchParams(location.search).get("agent_id")?.trim() || "";
  const agentCode = new URLSearchParams(location.search).get("code")?.trim() || "";
  const showAgentDevice = async () => {
    if (!agentId || !agentCode) return setError("This agent approval link is incomplete. Start the connection again.");
    if (!(await signedIn())) {
      showAuth();
      return;
    }
    await ensureWorkspace();
    document.querySelector("#user-code").textContent = agentCode;
    document.querySelector("#client-id").textContent = agentId;
    document.querySelector("#scope").textContent = "Capabilities requested by this agent";
    document.querySelector("#resource").textContent = "https://sigiro.com";
    document.querySelector("#request-details").hidden = false;
    authFlow.hidden = true;
    document.querySelector("#device-flow").hidden = false;
    setStatus("Confirm this code matches the requesting agent before approving.");
  };

  const decideAgentDevice = async (approve) => {
    setStatus(approve ? "Approving…" : "Denying…");
    try {
      await request("/agent/approve-capability", {
        method: "POST",
        body: JSON.stringify({
          agent_id: agentId,
          user_code: agentCode,
          action: approve ? "approve" : "deny",
        }),
      });
      showSuccess(approve ? "Return to your agent. Access is approved." : "The agent request was denied.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not process the agent request");
    }
  };

  const decideConsent = async (accept) => {
    if (!oauthQuery) return setError("This authorization request is missing its signed query.");
    setStatus(accept ? "Approving…" : "Denying…");
    try {
      const result = await request("/oauth2/consent", {
        method: "POST",
        body: JSON.stringify({ accept, oauth_query: oauthQuery }),
      });
      location.assign(result.redirect_uri);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not process consent");
    }
  };
  document.querySelector("#accept-consent").addEventListener("click", () => decideConsent(true));
  document.querySelector("#deny-consent").addEventListener("click", () => decideConsent(false));

  (async () => {
    try {
      if (page === "device") return await showDevice();
      if (page === "agent-device") {
        document.querySelector("#approve").onclick = () => decideAgentDevice(true);
        document.querySelector("#deny").onclick = () => decideAgentDevice(false);
        return await showAgentDevice();
      }
      if (page === "consent") {
        if (!(await signedIn())) {
          location.assign("/sign-in?oauth_query=" + encodeURIComponent(oauthQuery || ""));
          return;
        }
        await ensureWorkspace();
        document.querySelector("#consent-flow").hidden = false;
        return;
      }
      if (await signedIn()) {
        await ensureWorkspace();
        if (await finishOAuthLogin()) return;
        showSuccess("You are signed in and your personal workspace is active.");
        return;
      }
      showAuth();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Authentication failed");
    }
  })();
})();`;

const css = `:root{color-scheme:light dark;font-family:ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#0d0f12;color:#f5f7fa}*{box-sizing:border-box}body{margin:0;min-height:100vh;background:radial-gradient(circle at 50% 0,#1f2937 0,#0d0f12 48%)}main{width:min(100% - 32px,480px);margin:0 auto;padding:48px 0}.brand{display:inline-block;margin-bottom:24px;color:#f5f7fa;text-decoration:none;font-size:18px;font-weight:700;letter-spacing:-.03em}.card{border:1px solid #303741;border-radius:16px;background:#14181e;padding:28px;box-shadow:0 24px 80px #0008}.eyebrow{margin:0 0 10px;color:#88a6ff;font:600 11px/1.2 ui-monospace,monospace;text-transform:uppercase;letter-spacing:.12em}h1{margin:0;font-size:28px;line-height:1.15;letter-spacing:-.035em}h2{font-size:20px}.intro{margin:12px 0 24px;color:#aeb7c4;line-height:1.6}form{display:grid;gap:10px}label,.code-label{color:#cbd2dc;font-size:13px}input{width:100%;border:1px solid #3b4552;border-radius:9px;background:#0d1117;color:#fff;padding:12px 13px;font:inherit;outline:none}input:focus{border-color:#7d9cff;box-shadow:0 0 0 3px #5777df33}button,.card a{display:inline-flex;justify-content:center;border:0;border-radius:9px;background:#e8edff;color:#111827;padding:11px 15px;font:600 14px/1.2 inherit;text-decoration:none;cursor:pointer}button:hover,.card a:hover{background:#fff}.secondary{background:#252c35;color:#d9e0ea}.actions{display:flex;gap:10px;margin-top:20px}.code{display:block;margin:10px 0 20px;font:700 30px/1 ui-monospace,monospace;letter-spacing:.16em}dl{display:grid;gap:10px;margin:0}dl div{border-top:1px solid #303741;padding-top:10px}dt{color:#8792a2;font-size:11px;text-transform:uppercase;letter-spacing:.08em}dd{margin:4px 0 0;overflow-wrap:anywhere;color:#d9e0ea;font-size:13px}#status,#error{min-height:20px;margin:18px 0 0;font-size:13px}#status{color:#9fb0c7}#error{color:#ff9b9b}.fine-print{text-align:center;color:#6f7a89;font-size:12px}@media(max-width:520px){main{padding:24px 0}.card{padding:22px}.actions{flex-direction:column}}`;

export const authUIResponse = (pathname: string): Response | undefined => {
  const kind: PageKind | undefined = pathname === "/device/capabilities"
    ? "agent-device"
    : pathname === "/sign-in" || pathname === "/sign-up" || pathname === "/device" || pathname === "/consent"
      ? pathname.slice(1) as Exclude<PageKind, "agent-device">
      : undefined;
  if (kind) return new Response(page(kind), { headers: { ...securityHeaders, "content-type": "text/html; charset=utf-8" } });
  if (pathname === "/auth-ui.js") {
    return new Response(script, { headers: { ...securityHeaders, "content-type": "text/javascript; charset=utf-8" } });
  }
  if (pathname === "/auth-ui.css") {
    return new Response(css, { headers: { ...securityHeaders, "content-type": "text/css; charset=utf-8" } });
  }
  return undefined;
};
