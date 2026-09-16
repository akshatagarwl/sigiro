import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import {
  createServer,
  type IncomingMessage,
  request as httpRequest,
  type Server,
  type ServerResponse,
} from 'node:http';
import type { AddressInfo } from 'node:net';

const toHeaders = (source: Record<string, string | string[] | undefined>): Headers => {
  const headers = new Headers();
  for (const [name, value] of Object.entries(source)) {
    if (Array.isArray(value)) {
      for (const item of value) headers.append(name, item);
    } else if (value !== undefined) {
      headers.set(name, value);
    }
  }
  return headers;
};

const requestBody = async (request: IncomingMessage): Promise<Buffer> => {
  const chunks: Buffer[] = [];
  for await (const chunk of request)
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  return Buffer.concat(chunks);
};

const sendResponse = async (response: Response, target: ServerResponse): Promise<void> => {
  target.statusCode = response.status;
  response.headers.forEach((value, name) => target.setHeader(name, value));
  target.end(response.body === null ? undefined : Buffer.from(await response.arrayBuffer()));
};

const startServer = async (
  handler: (request: Request) => Promise<Response> | Response,
): Promise<{ server: Server; port: number }> => {
  const server = createServer(async (incoming, outgoing) => {
    try {
      const host = incoming.headers.host ?? '127.0.0.1';
      const body =
        incoming.method === 'GET' || incoming.method === 'HEAD'
          ? undefined
          : await requestBody(incoming);
      const request = new Request(`http://${host}${incoming.url ?? '/'}`, {
        method: incoming.method,
        headers: toHeaders(incoming.headers),
        body,
      });
      await sendResponse(await handler(request), outgoing);
    } catch (error) {
      outgoing.statusCode = 500;
      outgoing.end(error instanceof Error ? error.message : String(error));
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  // SAFETY: this server is bound to a TCP host, so Node returns AddressInfo.
  const address = server.address() as AddressInfo;
  return { server, port: address.port };
};

const closeServer = async (server: Server): Promise<void> => {
  server.close();
  await once(server, 'close');
};

const requestWithHost = (
  url: string,
  host: string,
): Promise<{ status: number; headers: Headers }> =>
  new Promise((resolve, reject) => {
    const request = httpRequest(url, { headers: { host } }, (response) => {
      response.resume();
      response.on('end', () => {
        resolve({ status: response.statusCode ?? 0, headers: toHeaders(response.headers) });
      });
    });
    request.on('error', reject);
    request.end();
  });

const jsonOrigin = async (origin: string, request: Request): Promise<Response> => {
  const url = new URL(request.url);
  if (origin === 'auth' && url.pathname === '/sign-in') {
    return Response.redirect(`${url.origin}/device?from=auth`, 302);
  }
  return Response.json({
    origin,
    method: request.method,
    path: `${url.pathname}${url.search}`,
    body: await request.text(),
    forwardedHost: request.headers.get('x-forwarded-host'),
    forwardedProto: request.headers.get('x-forwarded-proto'),
  });
};

const auth = await startServer((request) => jsonOrigin('auth', request));
const api = await startServer((request) => jsonOrigin('api', request));
const reservation = await startServer(() => new Response(null, { status: 503 }));
const workerPort = reservation.port;
await closeServer(reservation.server);

const worker = spawn(
  'wrangler',
  [
    'dev',
    '--config',
    'wrangler.preview.jsonc',
    '--port',
    String(workerPort),
    '--var',
    `AUTH_ORIGIN_URL:http://127.0.0.1:${auth.port}`,
    '--var',
    `API_ORIGIN_URL:http://127.0.0.1:${api.port}`,
  ],
  { stdio: ['ignore', 'pipe', 'pipe'] },
);
let stdout = '';
let stderr = '';
worker.stdout.setEncoding('utf8').on('data', (chunk: string) => {
  stdout += chunk;
});
worker.stderr.setEncoding('utf8').on('data', (chunk: string) => {
  stderr += chunk;
});
const base = `http://127.0.0.1:${workerPort}`;

const stop = async (): Promise<void> => {
  if (worker.exitCode === null) {
    worker.kill();
    await once(worker, 'exit');
  }
  await Promise.all([closeServer(auth.server), closeServer(api.server)]);
};

try {
  const deadline = Date.now() + 20_000;
  while (true) {
    try {
      const response = await fetch(`${base}/`);
      if (response.status === 200) break;
    } catch {
      // The bounded readiness loop owns transient connection refusal.
    }
    if (Date.now() >= deadline) throw new Error('Wrangler did not become ready within 20 seconds');
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  const home = await fetch(`${base}/`);
  assert.equal(home.status, 200);
  assert.match(home.headers.get('content-type') ?? '', /^text\/html/);

  const docs = await fetch(`${base}/docs`);
  assert.equal(docs.status, 200);

  const authResponse = await fetch(`${base}/api/auth/jwks?probe=1`);
  const authBody = await authResponse.json();
  assert.equal(authBody.origin, 'auth');
  assert.equal(authBody.path, '/api/auth/jwks?probe=1');
  assert.equal(authBody.forwardedHost, `127.0.0.1:${workerPort}`);
  assert.equal(authBody.forwardedProto, 'http');

  const agentConfiguration = await fetch(`${base}/.well-known/agent-configuration`);
  assert.equal((await agentConfiguration.json()).origin, 'auth');

  const redirect = await fetch(`${base}/sign-in`, { redirect: 'manual' });
  assert.equal(redirect.status, 302);
  assert.equal(redirect.headers.get('location'), `${base}/device?from=auth`);

  const apiResponse = await fetch(`${base}/v1/traces?probe=1`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-protobuf' },
    body: 'trace-body',
  });
  const apiBody = await apiResponse.json();
  assert.equal(apiBody.origin, 'api');
  assert.equal(apiBody.method, 'POST');
  assert.equal(apiBody.path, '/v1/traces?probe=1');
  assert.equal(apiBody.body, 'trace-body');

  const metadata = await fetch(`${base}/.well-known/oauth-protected-resource`);
  assert.equal((await metadata.json()).origin, 'api');

  const health = await fetch(`${base}/health`);
  assert.equal((await health.json()).origin, 'api');

  const admin = await fetch(`${base}/admin/status`);
  assert.notEqual(admin.headers.get('content-type'), 'application/json');

  const canonical = await requestWithHost(`${base}/sign-up`, `www.sigiro.com:${workerPort}`);
  assert.equal(canonical.status, 308);
  assert.equal(canonical.headers.get('location'), `http://sigiro.com:${workerPort}/sign-up`);

  console.log('edge smoke: 10 routes passed');
} catch (error) {
  console.error(stdout);
  console.error(stderr);
  throw error;
} finally {
  await stop();
}
