import { AgentAuthClient } from "@auth/agent";

const BASE = process.env.SIGIRO_E2E_BASE ?? "http://127.0.0.1:9876";
const SECONDARY = process.env.SIGIRO_SECONDARY_AUTH_URL;

const jar: string[] = [];
const request = async (path: string, body?: unknown, init?: RequestInit) => {
  const response = await fetch(`${BASE}/api/auth${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      "content-type": "application/json",
      origin: BASE,
      ...(jar.length ? { cookie: jar.join("; ") } : {}),
      ...(init?.headers ?? {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    ...init,
  });
  const setCookie = response.headers.getSetCookie?.() ?? [];
  for (const cookie of setCookie) jar.push(cookie.split(";")[0]!);
  return response;
};

const expectOk = async (response: Response, label: string) => {
  if (!response.ok)
    throw new Error(
      `${label} failed: ${response.status} ${await response.text()}`
    );
  return response;
};

// 1. sign in
const email = `agent-e2e-${Date.now()}@sigiro.local`;
await expectOk(
  await request("/email-otp/send-verification-otp", { email, type: "sign-in" }),
  "OTP send"
);
await expectOk(
  await request("/sign-in/email-otp", {
    email,
    otp: "123456",
    name: "Agent Owner",
  }),
  "sign-in"
);

// 2. personal org (the tenant the agent must bind to)
const org = (await (
  await expectOk(
    await fetch(`${BASE}/api/auth/onboarding/ensure-personal-organization`, {
      method: "POST",
      headers: { origin: BASE, cookie: jar.join("; ") },
    }),
    "org provisioning"
  )
).json()) as { organization: { id: string } };
console.log("tenant:", org.organization.id);

// 3. enroll a host, connect an agent, mint an agent+jwt
const hostEnrollment = (await (
  await expectOk(
    await request("/host/create", {
      name: "Worker E2E",
      default_capabilities: ["telemetry:ingest"],
    }),
    "host provisioning"
  )
).json()) as { enrollmentToken?: string };
if (!hostEnrollment.enrollmentToken) throw new Error("no enrollment token");

const agentClient = new AgentAuthClient({
  urls: [BASE],
  allowDirectDiscovery: true,
  hostName: "Worker E2E",
});
await agentClient.init();
await agentClient.enrollHost({
  provider: BASE,
  enrollmentToken: hostEnrollment.enrollmentToken,
});
const connection = await agentClient.connectAgent({
  provider: BASE,
  mode: "delegated",
  name: "Worker telemetry agent",
  capabilities: ["telemetry:ingest"],
});
if (connection.status !== "active")
  throw new Error(`agent not active: ${connection.status}`);

const agentToken = await agentClient.signJwt({
  agentId: connection.agentId,
  capabilities: ["telemetry:ingest"],
  audience: BASE,
});

// 4. THE path src/auth.rs uses for typ=agent+jwt
const session = await fetch(`${BASE}/api/auth/sigiro/agent/session`, {
  headers: { authorization: `Bearer ${agentToken.token}` },
});
const body = (await session.json()) as Record<string, unknown>;
console.log("agent session:", session.status, JSON.stringify(body));

if (body.active !== true) throw new Error("agent session not active");
if (body.tenant_id !== org.organization.id)
  throw new Error(`tenant mismatch: ${body.tenant_id}`);
const grants = body.agent_capability_grants as
  | Array<{ capability?: string; status?: string }>
  | undefined;
if (
  !grants?.some(
    (g) => g.capability === "telemetry:ingest" && g.status === "active"
  )
) {
  throw new Error("capability grant missing");
}
console.log("PASS: active + tenant bound + capability granted");

// 5. REPLAY — the security property. Same token, second presentation.
const replay = await fetch(
  `${SECONDARY ?? BASE}/api/auth/sigiro/agent/session`,
  {
    headers: { authorization: `Bearer ${agentToken.token}` },
  }
);
console.log(
  `replay against ${SECONDARY ? "SECOND instance" : "same instance"}:`,
  replay.status,
  (await replay.text()).slice(0, 120)
);
if (replay.status !== 401)
  throw new Error(
    `REPLAY ACCEPTED (${replay.status}) -- jti cache is failing open`
  );
console.log("PASS: replay rejected 401");

// 6. revocation
await agentClient.rotateAgentKey(connection.agentId);
const revokedToken = await agentClient.signJwt({
  agentId: connection.agentId,
  capabilities: ["telemetry:ingest"],
  audience: BASE,
});
await agentClient.disconnectAgent(connection.agentId);
const revoked = await fetch(`${BASE}/api/auth/sigiro/agent/session`, {
  headers: { authorization: `Bearer ${revokedToken.token}` },
});
const revokedBody = await revoked.text();
console.log("after disconnect:", revoked.status, revokedBody.slice(0, 120));
if (revoked.status !== 403 || !revokedBody.includes("agent_revoked")) {
  throw new Error("revoked agent was not rejected");
}
console.log("PASS: revoked agent rejected 403");
console.log("\nALL AGENT CHECKS PASSED");
