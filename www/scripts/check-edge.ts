import assert from 'node:assert/strict';

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

const auth = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  fetch: (request) => jsonOrigin('auth', request),
});
const api = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  fetch: (request) => jsonOrigin('api', request),
});
const reservation = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  fetch: () => new Response(null, { status: 503 }),
});
const workerPort = reservation.port;
reservation.stop(true);
const worker = Bun.spawn(
  [
    'bunx',
    'wrangler',
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
  { stdout: 'pipe', stderr: 'pipe' },
);
const stdout = new Response(worker.stdout).text();
const stderr = new Response(worker.stderr).text();
const base = `http://127.0.0.1:${workerPort}`;

const stop = async (): Promise<void> => {
  worker.kill();
  await worker.exited;
  auth.stop(true);
  api.stop(true);
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
    await Bun.sleep(100);
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

  const canonical = await fetch(`${base}/sign-up`, {
    headers: { host: `www.sigiro.com:${workerPort}` },
    redirect: 'manual',
  });
  assert.equal(canonical.status, 308);
  assert.equal(canonical.headers.get('location'), `http://sigiro.com:${workerPort}/sign-up`);

  console.log('edge smoke: 10 routes passed');
} catch (error) {
  console.error(await stdout);
  console.error(await stderr);
  throw error;
} finally {
  await stop();
}
