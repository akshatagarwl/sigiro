// Blume creates this module before Alchemy bundles the deployment entrypoint.
import website from '../dist/server/blume-worker.mjs';
import { MCP_PROTECTED_RESOURCE_METADATA } from './mcp-handler';
import { MCP_REQUIRED_SCOPE, MCP_RESOURCE } from './mcp-catalog';
import { handleDocsMcp, handleMcp, tools } from './mcp';

const AUTH_PATHS = new Set([
  '/.well-known/agent-configuration',
  '/auth-ui.css',
  '/auth-ui.js',
  '/consent',
  '/device',
  '/sign-in',
  '/sign-up',
]);

const AUTH_PAGE_PREFIXES = ['/consent/', '/device/', '/sign-in/', '/sign-up/'];

const API_PATHS = new Set([
  '/.well-known/oauth-protected-resource',
  '/health',
  '/healthz',
  '/ingest',
  '/ingest/pyroscope',
  '/openapi.json',
  '/push.v1.PusherService/Push',
  '/v1development/profiles',
]);

const isAuthPath = (pathname: string): boolean =>
  pathname === '/api/auth' ||
  pathname.startsWith('/api/auth/') ||
  AUTH_PATHS.has(pathname) ||
  AUTH_PAGE_PREFIXES.some((prefix) => pathname.startsWith(prefix));

const isApiPath = (pathname: string): boolean =>
  pathname.startsWith('/v1/') || API_PATHS.has(pathname);

const MCP_METADATA = '/.well-known/oauth-protected-resource';
const WEBMCP_CLIENT_METADATA = '/.well-known/oauth-client';

const proxyRequest = async (request: Request, upstream: string): Promise<Response> => {
  const url = new URL(request.url);
  const publicOrigin = url.origin;
  const upstreamOrigin = new URL(upstream).origin;
  if (upstreamOrigin === publicOrigin) {
    throw new Error('proxy origin must differ from the public origin');
  }
  const upstreamUrl = new URL(`${url.pathname}${url.search}`, upstreamOrigin);
  const headers = new Headers(request.headers);
  headers.set('x-forwarded-host', url.host);
  headers.set('x-forwarded-proto', url.protocol.slice(0, -1));

  const response = await fetch(new Request(upstreamUrl, request), {
    headers,
    redirect: 'manual',
  });
  const responseHeaders = new Headers(response.headers);
  const location = responseHeaders.get('location');
  if (location) {
    const redirect = new URL(location, upstreamOrigin);
    if (redirect.origin === upstreamOrigin) {
      responseHeaders.set(
        'location',
        `${publicOrigin}${redirect.pathname}${redirect.search}${redirect.hash}`,
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
  fetch(request: Request, env: Env, context: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    if (url.hostname === 'www.sigiro.com') {
      url.hostname = 'sigiro.com';
      return Promise.resolve(Response.redirect(url, 308));
    }

    const pathname = url.pathname;
    if (pathname === '/mcp') return handleMcp(request, env);
    if (pathname === '/mcp/docs') return handleDocsMcp(request);
    if (
      pathname === WEBMCP_CLIENT_METADATA &&
      (request.method === 'GET' || request.method === 'HEAD')
    ) {
      const metadata = {
        client_id: `${url.origin}${WEBMCP_CLIENT_METADATA}`,
        client_name: 'Sigiro browser WebMCP',
        redirect_uris: [`${url.origin}/`],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        token_endpoint_auth_method: 'none',
        application_type: 'native',
        scope: MCP_REQUIRED_SCOPE,
      };
      return Promise.resolve(
        request.method === 'HEAD'
          ? new Response(null, { headers: { 'content-type': 'application/json' } })
          : Response.json(metadata),
      );
    }
    if (pathname === MCP_METADATA) {
      return Promise.resolve(
        Response.json(MCP_PROTECTED_RESOURCE_METADATA, {
          headers: { 'access-control-allow-origin': '*' },
        }),
      );
    }
    if (pathname === '/.well-known/mcp.json' || pathname === '/.well-known/mcp/server-card.json') {
      return Promise.resolve(
        Response.json({
          $schema: 'https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json',
          name: 'com.sigiro/sigiro',
          title: 'Sigiro',
          version: '1.0.0',
          description: 'Authenticated Sigiro observability and documentation MCP server.',
          transport: 'streamable-http',
          url: 'https://sigiro.com/mcp',
          remotes: [{ type: 'streamable-http', url: 'https://sigiro.com/mcp' }],
          authorization: {
            required: true,
            resource_metadata: `${MCP_RESOURCE}/.well-known/oauth-protected-resource`,
          },
          tools,
        }),
      );
    }
    if (isAuthPath(pathname)) return proxyRequest(request, env.AUTH_ORIGIN_URL);
    if (isApiPath(pathname)) return proxyRequest(request, env.API_ORIGIN_URL);
    return website.fetch(request, env, context);
  },
} satisfies ExportedHandler<Env>;
