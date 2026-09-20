import { createHash } from "node:crypto";
import { createServer } from "node:http";

import { APIError } from "better-auth/api";
import { toNodeHandler } from "better-auth/node";
import { fetchClientMetadataResource } from "@better-auth/cimd/node";
import Cloudflare from "cloudflare";
import { Redacted } from "effect";
import { Pool } from "pg";

import { createAuth } from "./auth.js";
import {
  checkDatabase,
  makePgRuntime,
  membershipRoles,
  membershipsForUser,
  sessionTenant,
} from "./database.js";
import { sendCloudflareEmail } from "./email.js";
import { postgresSecondaryStorage } from "./secondary-storage.js";
import { authUIResponse } from "./ui.js";

const required = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
};

const environment = required("ENVIRONMENT");
if (environment !== "production" && environment !== "test") {
  throw new Error("ENVIRONMENT must be production or test");
}

const baseURL = required("BETTER_AUTH_URL");
const secret = required("BETTER_AUTH_SECRET");
if (secret.length < 32) throw new Error("BETTER_AUTH_SECRET must be at least 32 characters");

const testOTP = environment === "test" ? required("AUTH_TEST_OTP") : undefined;
if (environment === "production" && process.env.AUTH_TEST_OTP !== undefined) {
  throw new Error("AUTH_TEST_OTP must not be configured in production");
}

const databaseUrl = required("DATABASE_URL");
const databaseSsl =
  environment === "production"
    ? {
        ca: Buffer.from(required("DATABASE_CA_CERT_B64"), "base64").toString("utf8"),
        rejectUnauthorized: true,
      }
    : undefined;
const betterAuthDatabase = new Pool({
  connectionString: databaseUrl,
  application_name: "sigiro-auth",
  max: 10,
  connectionTimeoutMillis: 5_000,
  ssl: databaseSsl,
});
const postgres = makePgRuntime({
  url: Redacted.make(databaseUrl),
  ssl: databaseSsl,
  connectTimeout: "5 seconds",
  multiplex: true,
  applicationName: "sigiro-auth",
});
const cloudflare =
  environment === "production"
    ? new Cloudflare({ apiToken: required("CLOUDFLARE_EMAIL_API_TOKEN"), maxRetries: 2, timeout: 10_000 })
    : undefined;
const cloudflareAccountId = environment === "production" ? required("CLOUDFLARE_ACCOUNT_ID") : undefined;

const auth = createAuth({
  baseURL,
  secret,
  resource: required("SIGIRO_RESOURCE"),
  // The runtime-specific plumbing. Everything else (plugins, PKCE
  // enforcement, the `tid` claim) lives in createAuthOptions and is shared
  // verbatim with the Worker.
  database: betterAuthDatabase,
  secondaryStorage: postgresSecondaryStorage(postgres.runPromise),
  sessionTenant: (sessionId) => postgres.runPromise(sessionTenant(sessionId)),
  membershipsForUser: (userId) => postgres.runPromise(membershipsForUser(userId)),
  fetchClientMetadataResource,
  testOTP,
  emailFrom: required("EMAIL_FROM"),
  async sendEmail(message) {
    if (!cloudflare || !cloudflareAccountId) throw new Error("email delivery is unavailable outside production");
    await sendCloudflareEmail(cloudflare, cloudflareAccountId, message);
  },
});

const ensurePersonalOrganization = async (request: Request): Promise<Response> => {
  const session = await auth.api.getSession({ headers: request.headers });
  if (!session) return Response.json({ error: "unauthorized" }, { status: 401 });

  let organizations = await auth.api.listOrganizations({ headers: request.headers });
  if (organizations.length === 0) {
    const slug = `personal-${createHash("sha256").update(session.user.id).digest("hex").slice(0, 20)}`;
    try {
      await auth.api.createOrganization({
        headers: request.headers,
        body: { name: `${session.user.name || session.user.email}'s workspace`, slug },
      });
    } catch (cause) {
      organizations = await auth.api.listOrganizations({ headers: request.headers });
      if (organizations.length === 0) throw cause;
    }
    organizations = await auth.api.listOrganizations({ headers: request.headers });
  }

  const organization = organizations[0];
  if (!organization) throw new Error("personal organization provisioning returned no organization");
  if (session.session.activeOrganizationId !== organization.id) {
    await auth.api.setActiveOrganization({
      headers: request.headers,
      body: { organizationId: organization.id },
    });
  }
  return Response.json({ organization: { id: organization.id, name: organization.name } });
};

const introspectSigiroAgent = async (request: Request): Promise<Response> => {
  const session = await auth.api.getAgentSession({ headers: request.headers });
  if (!session || typeof session.userId !== "string") return Response.json({ active: false });

  const memberships = await postgres.runPromise(membershipsForUser(session.userId));
  if (memberships.length !== 1) return Response.json({ active: false });
  return Response.json({
    active: true,
    agent_id: session.agentId,
    user_id: session.userId,
    tenant_id: memberships[0]!,
    mode: session.type,
    agent_capability_grants: session.agent.capabilityGrants,
  });
};

const errorResponse = () => Response.json({ error: "internal_server_error" }, { status: 500 });

const withCors = (response: Response, request: Request): Response => {
  const origin = request.headers.get("origin");
  const configuredOrigin = new URL(baseURL).origin;
  if (origin !== configuredOrigin && origin !== new URL(request.url).origin) return response;
  const headers = new Headers(response.headers);
  headers.set("access-control-allow-origin", configuredOrigin);
  headers.set("access-control-allow-credentials", "true");
  headers.append("vary", "Origin");
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
};

const dispatch = async (request: Request): Promise<Response> => {
  try {
    if (request.method === "OPTIONS") {
      return withCors(
        new Response(null, {
          status: 204,
          headers: {
            "access-control-allow-headers": "authorization, content-type",
            "access-control-allow-methods": "GET, HEAD, POST, OPTIONS",
          },
        }),
        request,
      );
    }

    const url = new URL(request.url);
    const origin = request.headers.get("origin");
    if (request.method === "POST" && origin !== null && origin !== new URL(baseURL).origin) {
      return Response.json({ error: "forbidden" }, { status: 403 });
    }
    const ui = authUIResponse(url.pathname);
    if (ui && (request.method === "GET" || request.method === "HEAD")) {
      return request.method === "HEAD" ? new Response(null, { status: ui.status, headers: ui.headers }) : ui;
    }
    if ((request.method === "GET" || request.method === "HEAD") && url.pathname === "/health") {
      await postgres.runPromise(checkDatabase);
      return withCors(Response.json({ status: "ok" }), request);
    }
    if ((request.method === "GET" || request.method === "HEAD") && url.pathname === "/.well-known/agent-configuration") {
      const configuration = await auth.api.getAgentConfiguration();
      return withCors(
        request.method === "HEAD" ? new Response(null) : Response.json(configuration),
        request,
      );
    }

    if (request.method === "POST" && url.pathname === "/api/auth/onboarding/ensure-personal-organization") {
      return withCors(await ensurePersonalOrganization(request), request);
    }

    if (request.method === "GET" && url.pathname === "/api/auth/sigiro/agent/session") {
      return withCors(await introspectSigiroAgent(request), request);
    }

    if (request.method === "POST" && url.pathname === "/api/auth/admin/oauth2/create-client") {
      const session = await auth.api.getSession({ headers: request.headers });
      const organizationId = session?.session.activeOrganizationId;
      if (!session || typeof organizationId !== "string") {
        return withCors(Response.json({ error: "unauthorized" }, { status: 401 }), request);
      }

      const roles = await postgres.runPromise(membershipRoles(session.user.id, organizationId));
      if (!roles.some((role) => role === "owner" || role === "admin")) {
        return withCors(Response.json({ error: "forbidden" }, { status: 403 }), request);
      }

      const body = (await request.json()) as NonNullable<
        Parameters<typeof auth.api.adminCreateOAuthClient>[0]
      >["body"];
      const client = await auth.api.adminCreateOAuthClient({ headers: request.headers, body });
      return withCors(Response.json(client, { status: 201 }), request);
    }

    return withCors(await auth.handler(request), request);
  } catch (cause) {
    if (cause instanceof APIError) {
      return Response.json(cause.body ?? { error: "auth_request_failed" }, {
        status: cause.statusCode,
        headers: cause.headers,
      });
    }
    console.error("auth request failed", cause);
    return errorResponse();
  }
};

const server = createServer(toNodeHandler(dispatch));
const port = Number(required("PORT"));
if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) throw new Error("PORT must be a valid TCP port");

server.listen(port, "0.0.0.0", () => {
  console.log(`Sigiro auth listening on ${port}`);
});

let stopping = false;
const stop = (signal: NodeJS.Signals) => {
  if (stopping) return;
  stopping = true;
  console.log(`received ${signal}; stopping Sigiro auth`);
  server.close(async (serverError) => {
    try {
      await Promise.all([betterAuthDatabase.end(), postgres.dispose()]);
      if (serverError) throw serverError;
      process.exit(0);
    } catch (cause) {
      console.error("auth shutdown failed", cause);
      process.exit(1);
    }
  });
};

process.on("SIGINT", () => stop("SIGINT"));
process.on("SIGTERM", () => stop("SIGTERM"));