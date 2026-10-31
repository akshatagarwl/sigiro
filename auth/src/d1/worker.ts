import type { D1Database } from "@cloudflare/workers-types";
import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { APIError } from "better-auth/api";
import Cloudflare from "cloudflare";
import { drizzle } from "drizzle-orm/d1";

import { createAuthOptions } from "../auth.js";
import { sendCloudflareEmail } from "../email.js";
import { OPTIMIZER_CLIENT_METADATA_JSON } from "../optimizer-client.js";
import * as schema from "../schema/auth-schema.js";
import { fetchClientMetadataResource } from "./cimd-fetch.js";
import {
  checkDatabase,
  membershipRoles,
  membershipsForUser,
  organizationName,
  sessionTenant,
} from "./queries.js";
import { d1SecondaryStorage } from "./secondary-storage.js";

/**
 * Worker entrypoint for the sigiro auth service.
 *
 * The plugin configuration is NOT redefined here \u2014 it is `createAuthOptions`
 * from ../auth.ts, the same object the Node host builds. That matters: it
 * carries the PKCE-with-S256 enforcement, the `resource` validation on
 * /oauth2/authorize, /oauth2/token and /device/code, and the `tid` claim
 * derivation. A hand-copied second config would let any of those silently
 * diverge between the two runtimes, and the divergence would be invisible until
 * it mattered.
 *
 * What does change per runtime is only the plumbing: the database, the
 * secondary storage, two tenant lookups, and the CIMD fetch transport.
 *
 * All construction is deferred into `getAuth(env)` because Worker bindings do
 * not exist at module scope, unlike the Node server that ran this at import
 * time.
 */

export interface Env {
  DB: D1Database;
  ENVIRONMENT: string;
  BETTER_AUTH_URL: string;
  BETTER_AUTH_SECRET: string;
  SIGIRO_RESOURCE: string;
  EMAIL_FROM: string;
  CLOUDFLARE_ACCOUNT_ID?: string;
  CLOUDFLARE_EMAIL_API_TOKEN?: string;
  AUTH_TEST_OTP?: string;
}

const required = (env: Env, name: keyof Env): string => {
  const value = env[name];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`Missing environment variable: ${String(name)}`);
  }
  return value;
};

const sendEmail = async (
  env: Env,
  message: { to: string; from: string; subject: string; text: string }
): Promise<void> => {
  const accountId = env.CLOUDFLARE_ACCOUNT_ID;
  const token = env.CLOUDFLARE_EMAIL_API_TOKEN;
  if (!accountId || !token)
    throw new Error(
      "email delivery is unavailable without Cloudflare credentials"
    );
  // Use the SDK rather than a hand-rolled fetch. The hand-rolled version posted
  // to `/email/routing/send` with a SendGrid-shaped `personalizations` body;
  // that route does not exist (400, code 7000 "No route for that URI"), so every
  // verification code was silently dropped while the endpoint still returned
  // `{"success":true}` to the caller. Email Sending is `/email/sending/send`
  // with a flat `{from, to[], subject, text}` body, which is exactly what
  // `client.emailSending.send` builds.
  await sendCloudflareEmail(
    new Cloudflare({ apiToken: token }),
    accountId,
    message
  );
};

const buildAuth = (env: Env) => {
  const environment = required(env, "ENVIRONMENT");
  if (environment !== "production" && environment !== "test") {
    throw new Error("ENVIRONMENT must be production or test");
  }
  const secret = required(env, "BETTER_AUTH_SECRET");
  if (secret.length < 32)
    throw new Error("BETTER_AUTH_SECRET must be at least 32 characters");

  // Load-bearing, carried over verbatim from the Node host: a test OTP
  // reachable in production is a complete authentication bypass.
  if (environment === "production" && env.AUTH_TEST_OTP !== undefined) {
    throw new Error("AUTH_TEST_OTP must not be configured in production");
  }
  const testOTP =
    environment === "test" ? required(env, "AUTH_TEST_OTP") : undefined;

  const db = drizzle(env.DB, { schema });
  const options = createAuthOptions({
    baseURL: required(env, "BETTER_AUTH_URL"),
    secret,
    resource: required(env, "SIGIRO_RESOURCE"),
    emailFrom: required(env, "EMAIL_FROM"),
    // Passing `schema` arms better-auth's internal schema check, so a drifted
    // table is a boot error rather than a runtime surprise.
    database: drizzleAdapter(db, {
      provider: "sqlite",
      schema,
      camelCase: true,
    }),
    secondaryStorage: d1SecondaryStorage(env.DB),
    sessionTenant: (sessionId) => sessionTenant(env.DB, sessionId),
    membershipsForUser: (userId) => membershipsForUser(env.DB, userId),
    membershipRoles: (userId, tenantId) =>
      membershipRoles(env.DB, userId, tenantId),
    organizationName: (tenantId) => organizationName(env.DB, tenantId),
    fetchClientMetadataResource,
    testOTP,
    sendEmail: (message) => sendEmail(env, message),
  });
  return betterAuth(options);
};

type Auth = ReturnType<typeof buildAuth>;
let memoized: { auth: Auth; db: D1Database } | undefined;

/**
 * Per-isolate memo. Rebuilt when the binding identity changes, which is what
 * happens when celld hands a fresh D1 per dev session.
 */
const getAuth = (env: Env): Auth => {
  if (!memoized || memoized.db !== env.DB)
    memoized = { auth: buildAuth(env), db: env.DB };
  return memoized.auth;
};

const withCors = (
  response: Response,
  request: Request,
  baseURL: string
): Response => {
  const origin = request.headers.get("origin");
  const configuredOrigin = new URL(baseURL).origin;
  if (origin !== configuredOrigin && origin !== new URL(request.url).origin)
    return response;
  const headers = new Headers(response.headers);
  headers.set("access-control-allow-origin", configuredOrigin);
  headers.set("access-control-allow-credentials", "true");
  headers.append("vary", "Origin");
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
};

/**
 * Agent-token introspection.
 *
 * NOT a better-auth route \u2014 hand-written, and the Rust ingest depends on it:
 * src/auth.rs sends any token with `typ == "agent+jwt"` to
 * GET {issuer}/sigiro/agent/session and parses these exact snake_case fields.
 * The "exactly one membership" rule is part of that contract: more than one
 * makes the tenant ambiguous, and guessing would scope an agent to the wrong
 * tenant's data.
 */
const introspectSigiroAgent = async (
  env: Env,
  request: Request
): Promise<Response> => {
  const session = await getAuth(env).api.getAgentSession({
    headers: request.headers,
  });
  if (!session || typeof session.userId !== "string")
    return Response.json({ active: false });

  const memberships = await membershipsForUser(env.DB, session.userId);
  if (memberships.length !== 1) return Response.json({ active: false });

  return Response.json({
    active: true,
    agent_id: session.agentId,
    user_id: session.userId,
    tenant_id: memberships[0],
    mode: session.type,
    agent_capability_grants: session.agent.capabilityGrants,
  });
};

/**
 * Make sure a signed-in user has a workspace, and an active one.
 *
 * A user with no membership gets a personal workspace. The session's active
 * workspace is kept whenever the user is still a member of it: forcing it back
 * to the oldest membership on every page load silently switched invited users
 * to their empty personal workspace.
 */
const ensurePersonalOrganization = async (
  env: Env,
  request: Request
): Promise<Response> => {
  const auth = getAuth(env);
  const session = await auth.api.getSession({ headers: request.headers });
  if (!session)
    return Response.json({ error: "unauthorized" }, { status: 401 });

  let organizations = await auth.api.listOrganizations({
    headers: request.headers,
  });
  let created = false;
  if (organizations.length === 0) {
    // SubtleCrypto, not node:crypto createHash \u2014 the Node builtin is absent here.
    const digest = await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(session.user.id)
    );
    const hex = [...new Uint8Array(digest)]
      .map((byte) => byte.toString(16).padStart(2, "0"))
      .join("");
    try {
      await auth.api.createOrganization({
        headers: request.headers,
        body: {
          name: `${session.user.name || session.user.email}'s workspace`,
          slug: `personal-${hex.slice(0, 20)}`,
        },
      });
      created = true;
    } catch (cause) {
      organizations = await auth.api.listOrganizations({
        headers: request.headers,
      });
      if (organizations.length === 0) throw cause;
    }
    organizations = await auth.api.listOrganizations({
      headers: request.headers,
    });
  }

  const active = organizations.find(
    (organization) => organization.id === session.session.activeOrganizationId
  );
  const organization = active ?? organizations[0];
  if (!organization)
    throw new Error(
      "personal organization provisioning returned no organization"
    );
  if (!active) {
    await auth.api.setActiveOrganization({
      headers: request.headers,
      body: { organizationId: organization.id },
    });
  }
  return Response.json({
    organization: { id: organization.id, name: organization.name },
    created,
    organizations: organizations.length,
  });
};

const dispatch = async (env: Env, request: Request): Promise<Response> => {
  const baseURL = required(env, "BETTER_AUTH_URL");
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
        baseURL
      );
    }

    const url = new URL(request.url);
    const origin = request.headers.get("origin");
    if (
      request.method === "POST" &&
      origin !== null &&
      origin !== new URL(baseURL).origin
    ) {
      return Response.json({ error: "forbidden" }, { status: 403 });
    }

    if (
      request.method === "GET" &&
      url.pathname === "/api/auth/optimizer-client.json"
    ) {
      return new Response(OPTIMIZER_CLIENT_METADATA_JSON, {
        headers: {
          "content-type": "application/json",
          "cache-control": "public, max-age=300",
        },
      });
    }

    // Local-only schema bootstrap. D1 cannot be reached by better-auth's CLI,
    // and celld's `d1 execute` targets remote buckets rather than the dev
    // database, so applying the generated DDL through the binding itself is the
    // only path that works locally. Gated on ENVIRONMENT=test so it can never
    // be reachable in production.
    if (request.method === "POST" && url.pathname === "/__bootstrap-schema") {
      if (env.ENVIRONMENT !== "test")
        return new Response(null, { status: 404 });
      const sql = await request.text();
      // D1Database.exec takes ONE statement per call.
      const statements = sql
        .split(";\n")
        .map((statement) => statement.trim())
        .filter(
          (statement) => statement.length > 0 && !statement.startsWith("--")
        );
      let applied = 0;
      for (const statement of statements) {
        await env.DB.prepare(statement).run();
        applied += 1;
      }
      return Response.json({ applied });
    }

    if (
      (request.method === "GET" || request.method === "HEAD") &&
      url.pathname === "/health"
    ) {
      await checkDatabase(env.DB);
      return withCors(Response.json({ status: "ok" }), request, baseURL);
    }

    if (
      (request.method === "GET" || request.method === "HEAD") &&
      url.pathname === "/.well-known/agent-configuration"
    ) {
      const configuration = await getAuth(env).api.getAgentConfiguration();
      return withCors(
        request.method === "HEAD"
          ? new Response(null)
          : Response.json(configuration),
        request,
        baseURL
      );
    }

    if (
      request.method === "GET" &&
      url.pathname === "/api/auth/sigiro/agent/session"
    ) {
      return withCors(
        await introspectSigiroAgent(env, request),
        request,
        baseURL
      );
    }

    if (
      request.method === "POST" &&
      url.pathname === "/api/auth/onboarding/ensure-personal-organization"
    ) {
      return withCors(
        await ensurePersonalOrganization(env, request),
        request,
        baseURL
      );
    }

    if (
      request.method === "POST" &&
      url.pathname === "/api/auth/admin/oauth2/create-client"
    ) {
      const auth = getAuth(env);
      const session = await auth.api.getSession({ headers: request.headers });
      const organizationId = session?.session.activeOrganizationId;
      if (!session || typeof organizationId !== "string") {
        return withCors(
          Response.json({ error: "unauthorized" }, { status: 401 }),
          request,
          baseURL
        );
      }
      const roles = await membershipRoles(
        env.DB,
        session.user.id,
        organizationId
      );
      if (!roles.some((role) => role === "owner" || role === "admin")) {
        return withCors(
          Response.json({ error: "forbidden" }, { status: 403 }),
          request,
          baseURL
        );
      }
      const body = (await request.json()) as never;
      const client = await auth.api.adminCreateOAuthClient({
        headers: request.headers,
        body,
      });
      return withCors(Response.json(client, { status: 201 }), request, baseURL);
    }

    return withCors(await getAuth(env).handler(request), request, baseURL);
  } catch (cause) {
    if (cause instanceof APIError) {
      return Response.json(cause.body ?? { error: "auth_request_failed" }, {
        status: cause.statusCode,
        headers: cause.headers,
      });
    }
    console.error("auth request failed", cause);
    return Response.json({ error: "internal_server_error" }, { status: 500 });
  }
};

export default {
  fetch: (request: Request, env: Env): Promise<Response> =>
    dispatch(env, request),
};
