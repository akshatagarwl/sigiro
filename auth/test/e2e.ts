import { createRemoteJWKSet, jwtVerify } from "jose";
import { createHash, randomBytes } from "node:crypto";
import { validateCimdMetadata } from "@better-auth/cimd";
import { fetchClientMetadataResource as fetchClientMetadataResourceNode } from "@better-auth/cimd/node";
import { AgentAuthClient } from "@auth/agent";
import { Redacted } from "effect";
import { Pool } from "pg";

import { createAuth } from "../src/auth.js";
import { makePgRuntime, membershipsForUser, sessionTenant } from "../src/database.js";
import { postgresSecondaryStorage } from "../src/secondary-storage.js";

const required = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
};

const env = {
  BETTER_AUTH_URL: required("BETTER_AUTH_URL"),
  AUTH_TEST_OTP: required("AUTH_TEST_OTP"),
  SIGIRO_RESOURCE: required("SIGIRO_RESOURCE"),
  BETTER_AUTH_SECRET: required("BETTER_AUTH_SECRET"),
  DATABASE_URL: required("DATABASE_URL"),
};

const failures: string[] = [];

let cookie = "";

async function request(path: string, body?: unknown): Promise<Response> {
  const response = await fetch(`${env.BETTER_AUTH_URL}/api/auth${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(cookie === "" ? {} : { cookie }),
      origin: env.BETTER_AUTH_URL,
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const setCookies = response.headers.getSetCookie();
  if (setCookies.length > 0) {
    cookie = setCookies.map((value) => value.split(";", 1)[0]).join("; ");
  }
  return response;
}

async function expectOk(response: Response, operation: string): Promise<Response> {
  if (!response.ok) {
    throw new Error(`${operation} failed: ${response.status} ${await response.text()}`);
  }
  return response;
}

{
  const health = await expectOk(await fetch(`${env.BETTER_AUTH_URL}/health`), "health check");
  if ((await health.json() as { status?: string }).status !== "ok") {
    throw new Error("health check did not report readiness");
  }

  const corsPath = "/api/auth/.well-known/oauth-authorization-server";
  const trustedCors = await fetch(`${env.BETTER_AUTH_URL}${corsPath}`, {
    headers: { origin: env.BETTER_AUTH_URL },
  });
  if (trustedCors.headers.get("access-control-allow-origin") !== env.BETTER_AUTH_URL) {
    throw new Error("trusted auth origin was not allowed by CORS");
  }
  const untrustedCors = await fetch(`${env.BETTER_AUTH_URL}${corsPath}`, {
    headers: { origin: "https://untrusted.example" },
  });
  if (untrustedCors.headers.has("access-control-allow-origin")) {
    throw new Error("untrusted auth origin was allowed by CORS");
  }

  const signupPage = await expectOk(await fetch(`${env.BETTER_AUTH_URL}/sign-up`), "sign-up page");
  if (
    !signupPage.headers.get("content-security-policy")?.includes("frame-ancestors 'none'") ||
    !(await signupPage.text()).includes("Create your Sigiro account")
  ) {
    throw new Error("sign-up page omitted its security policy or account form");
  }

  const email = `m2m-owner-${randomBytes(8).toString("hex")}@sigiro.local`;
  await expectOk(
    await request("/email-otp/send-verification-otp", { email, type: "sign-in" }),
    "OTP dispatch",
  );
  await expectOk(
    await request("/sign-in/email-otp", { email, otp: env.AUTH_TEST_OTP!, name: "M2M Owner" }),
    "email OTP sign-in",
  );
  const firstProvision = (await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/onboarding/ensure-personal-organization`, {
      method: "POST",
      headers: { "content-type": "application/json", cookie, origin: env.BETTER_AUTH_URL },
      body: "{}",
    }),
    "personal organization provisioning",
  ).then((response) => response.json())) as { organization?: { id?: string } };
  const secondProvision = (await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/onboarding/ensure-personal-organization`, {
      method: "POST",
      headers: { "content-type": "application/json", cookie, origin: env.BETTER_AUTH_URL },
      body: "{}",
    }),
    "idempotent personal organization provisioning",
  ).then((response) => response.json())) as { organization?: { id?: string } };
  if (!firstProvision.organization?.id || firstProvision.organization.id !== secondProvision.organization?.id) {
    throw new Error("personal organization provisioning was not idempotent");
  }
  const unauthenticatedProvision = await fetch(
    `${env.BETTER_AUTH_URL}/api/auth/onboarding/ensure-personal-organization`,
    { method: "POST", headers: { "content-type": "application/json", origin: env.BETTER_AUTH_URL }, body: "{}" },
  );
  if (unauthenticatedProvision.status !== 401) {
    throw new Error("personal organization provisioning accepted an unauthenticated request");
  }
  const crossOriginProvision = await fetch(
    `${env.BETTER_AUTH_URL}/api/auth/onboarding/ensure-personal-organization`,
    {
      method: "POST",
      headers: { "content-type": "application/json", cookie, origin: "https://untrusted.example" },
      body: "{}",
    },
  );
  if (crossOriginProvision.status !== 403) {
    throw new Error("personal organization provisioning accepted a cross-origin request");
  }

  const hostEnrollment = (await expectOk(
    await request("/host/create", {
      name: "Sigiro auth E2E",
      default_capabilities: ["telemetry:ingest"],
    }),
    "Agent Auth host provisioning",
  ).then((response) => response.json())) as { enrollmentToken?: string };
  if (!hostEnrollment.enrollmentToken) throw new Error("Agent Auth host enrollment token was omitted");

  const agentClient = new AgentAuthClient({
    urls: [env.BETTER_AUTH_URL],
    allowDirectDiscovery: true,
    hostName: "Sigiro auth E2E",
  });
  await agentClient.init();
  await agentClient.enrollHost({
    provider: env.BETTER_AUTH_URL,
    enrollmentToken: hostEnrollment.enrollmentToken,
  });
  const agentConnection = await agentClient.connectAgent({
    provider: env.BETTER_AUTH_URL,
    mode: "delegated",
    name: "Sigiro telemetry agent",
    capabilities: ["telemetry:ingest"],
  });
  if (agentConnection.status !== "active") throw new Error("Agent Auth connection did not become active");
  const agentToken = await agentClient.signJwt({
    agentId: agentConnection.agentId,
    capabilities: ["telemetry:ingest"],
    audience: env.SIGIRO_RESOURCE,
  });
  const agentSession = await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/sigiro/agent/session`, {
      headers: { authorization: `Bearer ${agentToken.token}` },
    }),
    "Agent Auth Sigiro session",
  );
  const agentSessionBody = (await agentSession.json()) as {
    active?: boolean;
    tenant_id?: string;
    agent_capability_grants?: Array<{ capability?: string; status?: string }>;
  };
  if (
    agentSessionBody.active !== true ||
    agentSessionBody.tenant_id !== firstProvision.organization.id ||
    !agentSessionBody.agent_capability_grants?.some(
      (grant) => grant.capability === "telemetry:ingest" && grant.status === "active",
    )
  ) {
    throw new Error(`Agent Auth session omitted tenant or capability binding: ${JSON.stringify(agentSessionBody)}`);
  }
  const secondaryAuthURL = process.env.SIGIRO_SECONDARY_AUTH_URL;
  if (!secondaryAuthURL) throw new Error("SIGIRO_SECONDARY_AUTH_URL is required");
  const replay = await fetch(`${secondaryAuthURL}/api/auth/sigiro/agent/session`, {
    headers: { authorization: `Bearer ${agentToken.token}` },
  });
  if (
    replay.status !== 401 ||
    !replay.headers.get("www-authenticate")?.startsWith("AgentAuth ")
  ) {
    throw new Error("Agent Auth replay was not rejected with an AgentAuth challenge");
  }
  await agentClient.rotateAgentKey(agentConnection.agentId);
  const revokedAgentToken = await agentClient.signJwt({
    agentId: agentConnection.agentId,
    capabilities: ["telemetry:ingest"],
    audience: env.SIGIRO_RESOURCE,
  });
  await agentClient.disconnectAgent(agentConnection.agentId);
  const revokedAgentSession = await fetch(`${env.BETTER_AUTH_URL}/api/auth/sigiro/agent/session`, {
    headers: { authorization: `Bearer ${revokedAgentToken.token}` },
  });
  const revokedAgentBody = await revokedAgentSession.text();
  if (revokedAgentSession.status !== 403 || !revokedAgentBody.includes('"error":"agent_revoked"')) {
    throw new Error(
      `Disconnected Agent Auth session was not rejected as revoked: ${revokedAgentSession.status} ${revokedAgentBody}`,
    );
  }
  agentClient.destroy();

  const metadataResponse = await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/.well-known/oauth-authorization-server`),
    "OAuth authorization-server metadata",
  );
  const metadata = (await metadataResponse.json()) as {
    issuer?: string;
    jwks_uri?: string;
    grant_types_supported?: string[];
    code_challenge_methods_supported?: string[];
    registration_endpoint?: string;
  };
  if (
    metadata.issuer !== `${env.BETTER_AUTH_URL}/api/auth` ||
    metadata.jwks_uri !== `${env.BETTER_AUTH_URL}/api/auth/jwks` ||
    !metadata.grant_types_supported?.includes("authorization_code") ||
    !metadata.grant_types_supported.includes("urn:ietf:params:oauth:grant-type:device_code") ||
    metadata.code_challenge_methods_supported?.join(" ") !== "S256" ||
    metadata.registration_endpoint !== undefined
  ) {
    throw new Error("OAuth discovery metadata is incomplete");
  }
  const protectedResourceResponse = await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/.well-known/oauth-protected-resource`),
    "MCP protected-resource metadata",
  );
  const protectedResource = (await protectedResourceResponse.json()) as {
    resource?: string;
    authorization_servers?: string[];
  };
  if (
    protectedResource.resource !== env.SIGIRO_RESOURCE ||
    !protectedResource.authorization_servers?.includes(`${env.BETTER_AUTH_URL}/api/auth`)
  ) {
    throw new Error("MCP protected-resource metadata is incorrect");
  }
  const jwks = (await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/jwks`),
    "JWKS",
  ).then((response) => response.json())) as { keys?: unknown[] };
  if (!jwks.keys?.length) {
    throw new Error("JWKS contains no signing keys");
  }
  const passkeyOptions = (await expectOk(
    await fetch(`${env.BETTER_AUTH_URL}/api/auth/passkey/generate-register-options`, {
      headers: { cookie },
    }),
    "passkey registration options",
  ).then((response) => response.json())) as { challenge?: string; rp?: { id?: string } };
  if (!passkeyOptions.challenge || passkeyOptions.rp?.id !== "127.0.0.1") {
    throw new Error("passkey plugin did not return registration options");
  }
  const cimdClientId = "https://client.example/.well-known/oauth-client";
  const cimdValidation = validateCimdMetadata(
    cimdClientId,
    {
      client_id: cimdClientId,
      client_name: "Sigiro CIMD kill-test client",
      redirect_uris: ["http://127.0.0.1:3110/callback"],
    },
    { metadataProfile: "mcp-2026-07-28" },
  );
  if (!cimdValidation.valid) {
    throw new Error(`CIMD MCP profile rejected valid metadata: ${cimdValidation.error}`);
  }
  let loopbackRejected = false;
  try {
    await fetchClientMetadataResourceNode("https://127.0.0.1/.well-known/oauth-client");
  } catch {
    loopbackRejected = true;
  }
  if (!loopbackRejected) throw new Error("official CIMD transport allowed a loopback address");
  const dcrResponse = await fetch(`${env.BETTER_AUTH_URL}/api/auth/oauth2/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_name: "deprecated DCR client",
      redirect_uris: ["http://127.0.0.1:3110/callback"],
    }),
  });
  if (dcrResponse.ok) throw new Error("deprecated dynamic client registration is enabled");

  const organizationsResponse = await expectOk(await request("/organization/list"), "organization list");
  const organizations = (await organizationsResponse.json()) as Array<{ id: string }>;
  if (organizations.length === 0) {
    await expectOk(
      await request("/organization/create", { name: "M2M Kill Test", slug: "m2m-kill-test" }),
      "organization create",
    );
  } else {
    await expectOk(
      await request("/organization/set-active", { organizationId: organizations[0]?.id }),
      "organization activation",
    );
  }
  const sessionResponse = await expectOk(await request("/get-session"), "session lookup");
  const currentSession = (await sessionResponse.json()) as {
    session?: { activeOrganizationId?: string };
  };
  const tenantId = currentSession.session?.activeOrganizationId;
  if (!tenantId) {
    throw new Error("organization activation did not set the tenant context");
  }

  const client = (await (
    await expectOk(
      await request("/admin/oauth2/create-client", {
        client_name: "Sigiro kill-test agent",
        grant_types: ["client_credentials"],
        token_endpoint_auth_method: "client_secret_post",
        client_credentials_scopes: ["diagnose:read"],
      }),
      "admin OAuth client creation",
    )
  ).json()) as { client_id: string; client_secret?: string };
  if (!client.client_secret) {
    throw new Error("OAuth client was created without a secret");
  }

  const missingResourceTokenResponse = await fetch(`${env.BETTER_AUTH_URL}/api/auth/oauth2/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "client_credentials",
      client_id: client.client_id,
      client_secret: client.client_secret,
      scope: "diagnose:read",
    }),
  });
  if (missingResourceTokenResponse.ok) {
    throw new Error("client_credentials accepted a request without an RFC 8707 resource");
  }

  const tokenResponse = await fetch(`${env.BETTER_AUTH_URL}/api/auth/oauth2/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "client_credentials",
      client_id: client.client_id,
      client_secret: client.client_secret,
      scope: "diagnose:read",
      resource: env.SIGIRO_RESOURCE,
    }),
  });
  await expectOk(tokenResponse, "client_credentials");

  const token = (await tokenResponse.json()) as { access_token?: string; scope?: string };
  if (!token.access_token) {
    throw new Error("token response omitted access_token");
  }
  const { payload } = await jwtVerify(
    token.access_token,
    createRemoteJWKSet(new URL(`${env.BETTER_AUTH_URL}/api/auth/jwks`)),
    {
      issuer: `${env.BETTER_AUTH_URL}/api/auth`,
      audience: env.SIGIRO_RESOURCE,
    },
  );
  if (
    payload.scope !== "diagnose:read" ||
    token.scope !== "diagnose:read" ||
    payload.tid !== tenantId
  ) {
    throw new Error(
      `issued token claim mismatch: payload_scope=${String(payload.scope)} response_scope=${String(token.scope)} tid=${String(payload.tid)} expected_tid=${tenantId}`,
    );
  }
  const excessScopeResponse = await fetch(`${env.BETTER_AUTH_URL}/api/auth/oauth2/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "client_credentials",
      client_id: client.client_id,
      client_secret: client.client_secret,
      scope: "query:read",
      resource: env.SIGIRO_RESOURCE,
    }),
  });
  if (excessScopeResponse.ok) {
    throw new Error("client_credentials accepted an unassigned scope");
  }

  const redirectUri = "http://127.0.0.1:3110/callback";
  const cimdClientMetadata = {
    client_id: cimdClientId,
    client_name: "Sigiro CIMD kill-test client",
    redirect_uris: [redirectUri],
    grant_types: [
      "authorization_code",
      "refresh_token",
      "urn:ietf:params:oauth:grant-type:device_code",
    ],
    response_types: ["code"],
    token_endpoint_auth_method: "none",
    application_type: "native",
    scope: "openid profile email offline_access diagnose:read",
  };
  let metadataFetches = 0;
  const cimdDatabase = new Pool({
    connectionString: env.DATABASE_URL,
    application_name: "sigiro-auth-cimd-e2e",
    allowExitOnIdle: true,
  });
  const cimdPostgres = makePgRuntime({
    url: Redacted.make(env.DATABASE_URL),
    connectTimeout: "5 seconds",
    multiplex: true,
    applicationName: "sigiro-auth-cimd-e2e",
  });
  const cimdAuth = createAuth({
    baseURL: env.BETTER_AUTH_URL,
    secret: env.BETTER_AUTH_SECRET,
    resource: env.SIGIRO_RESOURCE,
    database: cimdDatabase,
    secondaryStorage: postgresSecondaryStorage(cimdPostgres.runPromise),
    sessionTenant: (sessionId) => cimdPostgres.runPromise(sessionTenant(sessionId)),
    membershipsForUser: (userId) => cimdPostgres.runPromise(membershipsForUser(userId)),
    testOTP: env.AUTH_TEST_OTP,
    emailFrom: "noreply@sigiro.com",
    async sendEmail() {},
    async fetchClientMetadataResource(input) {
      const metadataUrl = input instanceof Request ? input.url : input.toString();
      if (metadataUrl !== cimdClientId) throw new Error(`unexpected CIMD fetch: ${metadataUrl}`);
      metadataFetches += 1;
      return Response.json(cimdClientMetadata, {
        headers: { "cache-control": "public, max-age=3600" },
      });
    },
  });
  try {
  const publicClient = { client_id: cimdClientId };
  const cimdRequest = (path: string, init?: RequestInit) =>
    cimdAuth.handler(new Request(`${env.BETTER_AUTH_URL}/api/auth${path}`, init));
  const verifier = randomBytes(48).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const authorizeUrl = new URL(`${env.BETTER_AUTH_URL}/api/auth/oauth2/authorize`);
  authorizeUrl.search = new URLSearchParams({
    response_type: "code",
    client_id: publicClient.client_id,
    redirect_uri: redirectUri,
    scope: "openid profile email offline_access diagnose:read",
    state: "kill-test-state",
    code_challenge: challenge,
    code_challenge_method: "S256",
    resource: env.SIGIRO_RESOURCE,
  }).toString();
  const missingResourceAuthorizeUrl = new URL(authorizeUrl);
  missingResourceAuthorizeUrl.searchParams.delete("resource");
  const missingResourceAuthorizeResponse = await cimdAuth.handler(
    new Request(missingResourceAuthorizeUrl, { headers: { cookie, origin: env.BETTER_AUTH_URL } }),
  );
  if (missingResourceAuthorizeResponse.ok || missingResourceAuthorizeResponse.status === 302) {
    throw new Error("authorization accepted a request without an RFC 8707 resource");
  }
  const missingPkceAuthorizeUrl = new URL(authorizeUrl);
  missingPkceAuthorizeUrl.searchParams.delete("code_challenge");
  missingPkceAuthorizeUrl.searchParams.delete("code_challenge_method");
  const missingPkceAuthorizeResponse = await cimdAuth.handler(
    new Request(missingPkceAuthorizeUrl, { headers: { cookie, origin: env.BETTER_AUTH_URL } }),
  );
  if (missingPkceAuthorizeResponse.ok || missingPkceAuthorizeResponse.status === 302) {
    throw new Error("authorization accepted a request without PKCE S256");
  }
  const authorizeResponse = await cimdAuth.handler(
    new Request(authorizeUrl, { headers: { cookie, origin: env.BETTER_AUTH_URL } }),
  );
  if (!authorizeResponse.ok && authorizeResponse.status !== 302) {
    throw new Error(`authorize failed: ${authorizeResponse.status} ${await authorizeResponse.text()}`);
  }
  if (metadataFetches !== 1) throw new Error(`CIMD metadata fetched ${metadataFetches} times`);
  const clientRows = await cimdDatabase.query<{
    clientId: string;
    clientDiscoveryId: string | null;
    clientSecret: string | null;
    name: string | null;
    redirectUris: string[];
    tokenEndpointAuthMethod: string | null;
  }>(
    'SELECT "clientId", "clientDiscoveryId", "clientSecret", name, "redirectUris", "tokenEndpointAuthMethod" FROM "oauthClient" WHERE "clientId" = $1',
    [cimdClientId],
  );
  const cimdClient = clientRows.rows[0];
  if (
    clientRows.rowCount !== 1 ||
    !cimdClient ||
    cimdClient.clientDiscoveryId !== "cimd" ||
    cimdClient.clientSecret !== null ||
    cimdClient.name !== cimdClientMetadata.client_name ||
    cimdClient.redirectUris.join(" ") !== redirectUri ||
    cimdClient.tokenEndpointAuthMethod !== "none"
  ) {
    throw new Error(`persisted CIMD client is incorrect: ${JSON.stringify(cimdClient)}`);
  }
  const resourceRows = await cimdDatabase.query<{ identifier: string }>(
    'SELECT resource.identifier FROM "oauthClientResource" link JOIN "oauthResource" resource ON resource.identifier = link."resourceId" WHERE link."clientId" = $1',
    [cimdClientId],
  );
  if (resourceRows.rowCount !== 1 || resourceRows.rows[0]?.identifier !== env.SIGIRO_RESOURCE) {
    throw new Error(`CIMD client was not bound to the Sigiro resource: ${JSON.stringify(resourceRows.rows)}`);
  }
  const authorizeRedirect = authorizeResponse.status === 302
    ? authorizeResponse.headers.get("location")
    : ((await authorizeResponse.json()) as { url?: string }).url;
  const consentUrl = new URL(authorizeRedirect ?? "", env.BETTER_AUTH_URL);
  const oauthQuery = consentUrl.search.slice(1);
  if (oauthQuery === "") {
    throw new Error(`authorize did not produce a signed consent query: ${consentUrl}`);
  }
  const consentResponse = await cimdRequest("/oauth2/consent", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      cookie,
      origin: env.BETTER_AUTH_URL,
    },
    body: JSON.stringify({ accept: true, oauth_query: oauthQuery }),
    redirect: "manual",
  });
  if (!consentResponse.ok && consentResponse.status !== 302) {
    throw new Error(`consent failed: ${consentResponse.status} ${await consentResponse.text()}`);
  }
  const consentRedirect = consentResponse.status === 302
    ? consentResponse.headers.get("location")
    : ((await consentResponse.json()) as { url?: string }).url;
  const callback = new URL(consentRedirect ?? "");
  const code = callback.searchParams.get("code");
  if (!code || callback.searchParams.get("state") !== "kill-test-state") {
    throw new Error(`consent callback omitted code or state: ${callback}`);
  }
  const pkceTokenResponse = await cimdRequest("/oauth2/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      client_id: publicClient.client_id,
      code,
      code_verifier: verifier,
      redirect_uri: redirectUri,
      resource: env.SIGIRO_RESOURCE,
    }),
  });
  await expectOk(pkceTokenResponse, "authorization_code + PKCE");
  const pkceToken = (await pkceTokenResponse.json()) as {
    access_token?: string;
    refresh_token?: string;
  };
  if (!pkceToken.access_token || !pkceToken.refresh_token) {
    throw new Error("PKCE token response omitted access_token or refresh_token");
  }
  const { payload: pkcePayload } = await jwtVerify(
    pkceToken.access_token,
    createRemoteJWKSet(new URL(`${env.BETTER_AUTH_URL}/api/auth/jwks`)),
    {
      issuer: `${env.BETTER_AUTH_URL}/api/auth`,
      audience: env.SIGIRO_RESOURCE,
    },
  );
  if (pkcePayload.tid !== tenantId) {
    throw new Error("PKCE access token omitted the tenant claim");
  }
  const rotatedResponse = await expectOk(
    await cimdRequest("/oauth2/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: publicClient.client_id,
        refresh_token: pkceToken.refresh_token,
        resource: env.SIGIRO_RESOURCE,
      }),
    }),
    "refresh-token rotation",
  );
  const rotatedToken = (await rotatedResponse.json()) as {
    access_token?: string;
    refresh_token?: string;
    error?: string;
    error_description?: string;
  };
  if (!rotatedToken.refresh_token) {
    failures.push(
      `refresh token did not rotate (response keys: ${Object.keys(rotatedToken).join(",")})`,
    );
  } else {
    if (!rotatedToken.access_token) {
      throw new Error("refresh-token exchange omitted access_token");
    }
    const { payload: rotatedPayload } = await jwtVerify(
      rotatedToken.access_token,
      createRemoteJWKSet(new URL(`${env.BETTER_AUTH_URL}/api/auth/jwks`)),
      {
        issuer: `${env.BETTER_AUTH_URL}/api/auth`,
        audience: env.SIGIRO_RESOURCE,
      },
    );
    if (rotatedPayload.tid !== tenantId) {
      throw new Error("rotated access token omitted the tenant claim");
    }
    if (rotatedToken.refresh_token === pkceToken.refresh_token) {
      failures.push("refresh-token exchange returned the original token");
    }
    const replayResponse = await cimdRequest("/oauth2/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: publicClient.client_id,
        refresh_token: pkceToken.refresh_token,
        resource: env.SIGIRO_RESOURCE,
      }),
    });
    if (replayResponse.ok) {
      failures.push("rotated refresh token replay was accepted");
    }
  }

  const deviceResponse = await expectOk(
    await cimdRequest("/device/code", {
      method: "POST",
      headers: { "content-type": "application/json", cookie, origin: env.BETTER_AUTH_URL },
      body: JSON.stringify({
        client_id: publicClient.client_id,
        scope: "diagnose:read",
        resource: env.SIGIRO_RESOURCE,
      }),
    }),
    "device authorization",
  );
  const device = (await deviceResponse.json()) as {
    device_code?: string;
    user_code?: string;
    verification_uri?: string;
    verification_uri_complete?: string;
    interval?: number;
  };
  if (
    !device.device_code ||
    !device.user_code ||
    device.verification_uri !== `${new URL(env.BETTER_AUTH_URL).origin}/device` ||
    device.verification_uri_complete !==
      `${new URL(env.BETTER_AUTH_URL).origin}/device?user_code=${encodeURIComponent(device.user_code)}`
  ) {
    throw new Error("device authorization response omitted codes or the user-facing verification URL");
  }
  await expectOk(
    await cimdRequest(`/device?${new URLSearchParams({ user_code: device.user_code })}`, {
      headers: { cookie },
    }),
    "device verification claim",
  );
  await expectOk(
    await cimdRequest("/device/approve", {
      method: "POST",
      headers: { "content-type": "application/json", cookie, origin: env.BETTER_AUTH_URL },
      body: JSON.stringify({ userCode: device.user_code }),
    }),
    "device approval",
  );
  await new Promise((resolve) => setTimeout(resolve, (device.interval ?? 5) * 1_000));
  const deviceTokenResponse = await cimdRequest("/oauth2/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:device_code",
      client_id: publicClient.client_id,
      device_code: device.device_code,
      resource: env.SIGIRO_RESOURCE,
    }),
  });
  await expectOk(deviceTokenResponse, "device token exchange");
  const deviceToken = (await deviceTokenResponse.json()) as { access_token?: string };
  if (!deviceToken.access_token) {
    throw new Error("device token response omitted access_token");
  }
  const { payload: devicePayload } = await jwtVerify(
    deviceToken.access_token,
    createRemoteJWKSet(new URL(`${env.BETTER_AUTH_URL}/api/auth/jwks`)),
    {
      issuer: `${env.BETTER_AUTH_URL}/api/auth`,
      audience: env.SIGIRO_RESOURCE,
    },
  );
  if (devicePayload.tid !== tenantId) {
    throw new Error("device access token omitted the tenant claim");
  }

  const applicationNameRows = await cimdDatabase.query<{ application_name: string }>(
    "SELECT DISTINCT application_name FROM pg_stat_activity WHERE datname = current_database()",
  );
  const applicationNames = new Set(applicationNameRows.rows.map((row) => row.application_name));
  for (const expected of ["sigiro-auth", "sigiro-auth-cimd-e2e"]) {
    if (!applicationNames.has(expected)) {
      throw new Error(`pg_stat_activity omitted application_name ${expected}`);
    }
  }

  process.stdout.write(
    "client_credentials, CIMD authorization_code + PKCE, and device JWTs verified\n",
  );
  if (failures.length > 0) {
    throw new Error(`kill-test failed: ${failures.join("; ")}`);
  }
  } finally {
    await Promise.all([cimdDatabase.end(), cimdPostgres.dispose()]);
  }
}
