// Blume creates this module before Alchemy bundles the deployment entrypoint.
import website from '../dist/server/blume-worker.mjs';

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
    if (isAuthPath(pathname)) return proxyRequest(request, env.AUTH_ORIGIN_URL);
    if (isApiPath(pathname)) return proxyRequest(request, env.API_ORIGIN_URL);
    return website.fetch(request, env, context);
  },
} satisfies ExportedHandler<Env>;
