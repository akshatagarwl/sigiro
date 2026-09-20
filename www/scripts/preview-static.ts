/**
 * Serve the built site locally with `/api/auth/*` proxied to production.
 *
 * `blume dev` crashes in this worktree on a Vite dep-optimizer fault
 * (`deps_ssr/sequence-*.js` missing) that predates the auth work and has
 * nothing to do with it. This serves `dist/client` — the same artifact that
 * gets deployed — so what is inspected locally is what ships, and forwards the
 * auth API to production so the flow actually runs against real endpoints.
 *
 * Local inspection only; never deployed.
 */
import { createReadStream, existsSync, readFileSync, statSync } from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { extname, join, normalize } from 'node:path';

const ROOT = new URL('../dist/client/', import.meta.url).pathname;
const UPSTREAM = 'https://sigiro.com';
const PORT = Number(process.env.PORT ?? 4330);

const TYPES = new Map<string, string>([
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.mjs', 'text/javascript; charset=utf-8'],
  ['.css', 'text/css; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.svg', 'image/svg+xml'],
  ['.png', 'image/png'],
  ['.webp', 'image/webp'],
  ['.woff2', 'font/woff2'],
  ['.ico', 'image/x-icon'],
]);

const resolveFile = (pathname: string): string | null => {
  const clean = normalize(decodeURIComponent(pathname)).replace(/^(\.\.[/\\])+/, '');
  const candidates = [
    join(ROOT, clean),
    join(ROOT, clean, 'index.html'),
    `${join(ROOT, clean)}.html`,
  ];
  return (
    candidates.find((candidate) => existsSync(candidate) && statSync(candidate).isFile()) ?? null
  );
};

const readBody = async (request: IncomingMessage): Promise<Buffer | undefined> => {
  if (request.method === 'GET' || request.method === 'HEAD') return undefined;
  // `toArray` is typed on the async-iterable stream, so the chunks arrive as
  // Buffers without asserting anything about what the iterator yields.
  const chunks = await request.toArray();
  return Buffer.concat(chunks);
};

/** Forward upstream, rewriting cookies so the session sticks on localhost. */
const proxy = async (
  request: IncomingMessage,
  response: ServerResponse,
  target: string,
): Promise<void> => {
  const headers = new Headers();
  for (const [key, value] of Object.entries(request.headers)) {
    if (value === undefined) continue;
    headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  }
  headers.set('host', 'sigiro.com');
  headers.set('origin', UPSTREAM);

  const upstream = await fetch(target, {
    method: request.method ?? 'GET',
    headers,
    body: await readBody(request),
    redirect: 'manual',
  });

  const body = Buffer.from(await upstream.arrayBuffer());
  upstream.headers.forEach((value, key) => {
    const local =
      key === 'set-cookie'
        ? value.replace(/;\s*Domain=[^;]+/i, '').replace(/;\s*Secure/gi, '')
        : value;
    response.setHeader(key, local);
  });
  response.writeHead(upstream.status);
  response.end(body);
};

createServer((request, response) => {
  const url = new URL(request.url ?? '/', `http://localhost:${String(PORT)}`);

  if (url.pathname.startsWith('/api/auth')) {
    void proxy(request, response, `${UPSTREAM}${url.pathname}${url.search}`).catch(
      (cause: unknown) => {
        response.writeHead(502, { 'content-type': 'text/plain' });
        response.end(cause instanceof Error ? cause.message : 'proxy failed');
      },
    );
    return;
  }

  const file = resolveFile(url.pathname);
  if (!file) {
    const notFound = join(ROOT, '404.html');
    if (existsSync(notFound)) {
      response.writeHead(404, { 'content-type': 'text/html; charset=utf-8' });
      response.end(readFileSync(notFound));
      return;
    }
    response.writeHead(404, { 'content-type': 'text/plain' });
    response.end('not found');
    return;
  }

  response.writeHead(200, {
    'content-type': TYPES.get(extname(file)) ?? 'application/octet-stream',
  });
  createReadStream(file).pipe(response);
}).listen(PORT, () => {
  console.log(`preview: http://localhost:${String(PORT)}`);
});
