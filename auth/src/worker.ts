import { Effect } from "effect";
import { Pool } from "pg";

import { createAuth } from "./auth.js";

type RuntimeEnv = Omit<Env, "ENVIRONMENT"> & {
  ENVIRONMENT: "production" | "test";
  BETTER_AUTH_SECRET: string;
  AUTH_TEST_OTP?: string;
};

const errorResponse = () => Response.json({ error: "internal_server_error" }, { status: 500 });

const withCors = (
  response: Response,
  origin: string | null,
  requestOrigin: string,
  env: RuntimeEnv,
): Response => {
  const configuredOrigin = new URL(env.BETTER_AUTH_URL).origin;
  if (origin !== configuredOrigin && origin !== requestOrigin) return response;
  const headers = new Headers(response.headers);
  headers.set("access-control-allow-origin", configuredOrigin);
  headers.set("access-control-allow-credentials", "true");
  headers.append("vary", "Origin");
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
};

const dispatch = async (request: Request, env: RuntimeEnv): Promise<Response> => {
  if (
    typeof env.BETTER_AUTH_SECRET !== "string" ||
    env.BETTER_AUTH_SECRET.length < 32 ||
    (env.ENVIRONMENT !== "test" && env.AUTH_TEST_OTP !== undefined)
  ) {
    console.error("invalid auth Worker configuration");
    return errorResponse();
  }

  const origin = request.headers.get("origin");
  const requestOrigin = new URL(request.url).origin;
  if (request.method === "OPTIONS") {
    const response = new Response(null, {
      status: 204,
      headers: {
        "access-control-allow-headers": "authorization, content-type",
        "access-control-allow-methods": "GET, HEAD, POST, OPTIONS",
      },
    });
    return withCors(response, origin, requestOrigin, env);
  }

  const database = new Pool({
    connectionString: env.HYPERDRIVE.connectionString,
    max: 1,
    connectionTimeoutMillis: 5_000,
  });

  const program = Effect.tryPromise({
    try: async () => {
      if (
        (request.method === "GET" || request.method === "HEAD") &&
        new URL(request.url).pathname === "/health"
      ) {
        await database.query("SELECT 1");
        return Response.json({ status: "ok" });
      }

      const auth = createAuth({
        baseURL: env.BETTER_AUTH_URL,
        secret: env.BETTER_AUTH_SECRET,
        resource: env.SIGIRO_RESOURCE,
        database,
        testOTP: env.ENVIRONMENT === "test" ? env.AUTH_TEST_OTP : undefined,
        emailFrom: env.EMAIL_FROM,
        async sendEmail(message) {
          await env.EMAIL.send(message);
        },
      });

      const url = new URL(request.url);
      if (request.method === "POST" && url.pathname === "/api/auth/admin/oauth2/create-client") {
        const session = await auth.api.getSession({ headers: request.headers });
        const organizationId = session?.session.activeOrganizationId;
        if (!session || typeof organizationId !== "string") {
          return Response.json({ error: "unauthorized" }, { status: 401 });
        }

        const membership = await database.query<{ role: string }>(
          'SELECT role FROM member WHERE "userId" = $1 AND "organizationId" = $2',
          [session.user.id, organizationId],
        );
        const roles = membership.rows[0]?.role.split(",") ?? [];
        if (!roles.some((role) => role === "owner" || role === "admin")) {
          return Response.json({ error: "forbidden" }, { status: 403 });
        }

        const body = (await request.json()) as NonNullable<
          Parameters<typeof auth.api.adminCreateOAuthClient>[0]
        >["body"];
        const client = await auth.api.adminCreateOAuthClient({ headers: request.headers, body });
        return Response.json(client, { status: 201 });
      }

      return auth.handler(request);
    },
    catch: (cause) => cause,
  }).pipe(
    Effect.catch((cause) =>
      Effect.sync(() => {
        console.error("auth request failed", cause);
        return errorResponse();
      }),
    ),
    Effect.ensuring(Effect.promise(() => database.end())),
  );

  return withCors(await Effect.runPromise(program), origin, requestOrigin, env);
};

export default {
  fetch: dispatch,
} satisfies ExportedHandler<RuntimeEnv>;
