// Blume creates this module before Alchemy bundles the deployment entrypoint.
import website from "../dist/server/blume-worker.mjs";
import {
  BROWSER_CLIENT_METADATA_PATH,
  browserClientMetadata,
} from "./lib/browser-client";
import { MCP_RESOURCE } from "./mcp-catalog";
import {
  createMcpHandler,
  MCP_PROTECTED_RESOURCE_METADATA,
  tools,
} from "./mcp-handler";

/**
 * Paths the auth Worker owns.
 *
 * The auth PAGES are no longer in this set. `/sign-in`, `/sign-up`, `/consent`,
 * `/device` and `/device/capabilities` are Astro pages in this site now, so the
 * website renders them with its own header, footer and theme; only the `/api/auth/*`
 * endpoints they call still cross the service binding. `/auth-ui.css` and
 * `/auth-ui.js` are gone entirely — the stylesheet is Blume's and the script is
 * bundled into the page.
 */
const AUTH_PATHS = new Set(["/.well-known/agent-configuration"]);

const API_PATHS = new Set([
  "/.well-known/oauth-protected-resource",
  "/health",
  "/healthz",
  "/ingest",
  "/ingest/pyroscope",
  "/openapi.json",
  "/push.v1.PusherService/Push",
  "/v1development/profiles",
]);

/**
 * RFC 8414 §3 and OIDC discovery for the issuer `https://sigiro.com/api/auth`.
 *
 * A path-bearing issuer's metadata lives at `/.well-known/<suffix>/api/auth`,
 * which generic OAuth clients derive from the issuer and which answered 404.
 * Better Auth serves the document under the issuer path, so rewrite to it.
 */
const ISSUER_METADATA = new Map([
  [
    "/.well-known/oauth-authorization-server/api/auth",
    "/api/auth/.well-known/oauth-authorization-server",
  ],
  [
    "/.well-known/openid-configuration/api/auth",
    "/api/auth/.well-known/openid-configuration",
  ],
]);

const isAuthPath = (pathname: string): boolean =>
  pathname === "/api/auth" ||
  pathname.startsWith("/api/auth/") ||
  AUTH_PATHS.has(pathname);

const isApiPath = (pathname: string): boolean =>
  pathname.startsWith("/v1/") || API_PATHS.has(pathname);

const MCP_METADATA = "/.well-known/oauth-protected-resource";

const proxyRequest = async (
  request: Request,
  upstream: string
): Promise<Response> => {
  const url = new URL(request.url);
  const publicOrigin = url.origin;
  const upstreamOrigin = new URL(upstream).origin;
  if (upstreamOrigin === publicOrigin) {
    throw new Error("proxy origin must differ from the public origin");
  }
  const upstreamUrl = new URL(`${url.pathname}${url.search}`, upstreamOrigin);
  const headers = new Headers(request.headers);
  headers.set("x-forwarded-host", url.host);
  headers.set("x-forwarded-proto", url.protocol.slice(0, -1));

  const response = await fetch(new Request(upstreamUrl, request), {
    headers,
    redirect: "manual",
  });
  const responseHeaders = new Headers(response.headers);
  const location = responseHeaders.get("location");
  if (location) {
    const redirect = new URL(location, upstreamOrigin);
    if (redirect.origin === upstreamOrigin) {
      responseHeaders.set(
        "location",
        `${publicOrigin}${redirect.pathname}${redirect.search}${redirect.hash}`
      );
    }
  }

  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: responseHeaders,
  });
};

export default {
  fetch(
    request: Request,
    env: Env,
    context: ExecutionContext
  ): Promise<Response> {
    const url = new URL(request.url);
    if (url.hostname === "www.sigiro.com") {
      url.hostname = "sigiro.com";
      return Promise.resolve(Response.redirect(url, 308));
    }

    const pathname = url.pathname;
    // Sign-up and sign-in are one card: a one-time code creates the account the
    // first time an address uses it. 302, not 301, so the page can come back
    // without browsers having cached it away; the query (an `oauth_query`, UTM
    // tags) rides along.
    if (pathname === "/sign-up" || pathname === "/sign-up/") {
      url.pathname = "/sign-in";
      return Promise.resolve(Response.redirect(url, 302));
    }
    const handleDocsMcp = (docsRequest: Request) =>
      website.fetch(docsRequest, env, context);
    if (pathname === "/mcp")
      return createMcpHandler(handleDocsMcp)(request, env);
    if (pathname === "/mcp/docs") return handleDocsMcp(request);
    if (
      pathname === BROWSER_CLIENT_METADATA_PATH &&
      (request.method === "GET" || request.method === "HEAD")
    ) {
      return Promise.resolve(
        request.method === "HEAD"
          ? new Response(null, {
              headers: { "content-type": "application/json" },
            })
          : Response.json(browserClientMetadata(url.origin))
      );
    }
    if (pathname === MCP_METADATA) {
      return Promise.resolve(
        Response.json(MCP_PROTECTED_RESOURCE_METADATA, {
          headers: { "access-control-allow-origin": "*" },
        })
      );
    }
    if (
      pathname === "/.well-known/mcp.json" ||
      pathname === "/.well-known/mcp/server-card.json"
    ) {
      return Promise.resolve(
        Response.json({
          $schema:
            "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json",
          name: "com.sigiro/sigiro",
          title: "Sigiro",
          version: "1.0.0",
          description:
            "Authenticated Sigiro observability and documentation MCP server.",
          transport: "streamable-http",
          url: "https://sigiro.com/mcp",
          remotes: [{ type: "streamable-http", url: "https://sigiro.com/mcp" }],
          authorization: {
            required: true,
            resource_metadata: `${MCP_RESOURCE}/.well-known/oauth-protected-resource`,
          },
          tools,
        })
      );
    }
    // A service binding: the auth worker is invoked directly, so the request
    // never leaves Cloudflare and no redirect rewriting is needed (the auth
    // worker already sees the public sigiro.com URL).
    const issuerMetadata = ISSUER_METADATA.get(pathname);
    if (issuerMetadata) {
      url.pathname = issuerMetadata;
      return env.AUTH.fetch(new Request(url, request));
    }
    if (isAuthPath(pathname)) return env.AUTH.fetch(request);
    if (isApiPath(pathname)) return proxyRequest(request, env.API_ORIGIN_URL);
    return website.fetch(request, env, context);
  },
} satisfies ExportedHandler<Env>;
