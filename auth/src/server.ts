import { createServer } from "node:http";

import { toNodeHandler } from "better-auth/node";
import Cloudflare from "cloudflare";
import { Pool } from "pg";

import { createAuth } from "./auth.js";
import { sendCloudflareEmail } from "./email.js";

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

const database = new Pool({
  connectionString: required("DATABASE_URL"),
  max: 10,
  connectionTimeoutMillis: 5_000,
  ssl:
    environment === "production"
      ? {
          ca: Buffer.from(required("DATABASE_CA_CERT_B64"), "base64").toString("utf8"),
          rejectUnauthorized: true,
        }
      : undefined,
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
  database,
  testOTP,
  emailFrom: required("EMAIL_FROM"),
  async sendEmail(message) {
    if (!cloudflare || !cloudflareAccountId) throw new Error("email delivery is unavailable outside production");
    await sendCloudflareEmail(cloudflare, cloudflareAccountId, message);
  },
});

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
    if ((request.method === "GET" || request.method === "HEAD") && url.pathname === "/health") {
      await database.query("SELECT 1");
      return withCors(Response.json({ status: "ok" }), request);
    }

    if (request.method === "POST" && url.pathname === "/api/auth/admin/oauth2/create-client") {
      const session = await auth.api.getSession({ headers: request.headers });
      const organizationId = session?.session.activeOrganizationId;
      if (!session || typeof organizationId !== "string") {
        return withCors(Response.json({ error: "unauthorized" }, { status: 401 }), request);
      }

      const membership = await database.query<{ role: string }>(
        'SELECT role FROM member WHERE "userId" = $1 AND "organizationId" = $2',
        [session.user.id, organizationId],
      );
      const roles = membership.rows[0]?.role.split(",") ?? [];
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
      await database.end();
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