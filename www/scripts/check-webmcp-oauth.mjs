/* eslint-disable anti-slop/no-runtime-typeof */
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { dirname, extname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const playwright = await import(process.env.WEBMCP_PLAYWRIGHT ?? 'playwright');
const { chromium } = playwright.default ?? playwright;

const here = dirname(fileURLToPath(import.meta.url));
const clientRoot = resolve(here, '../dist/client');
const polyfill =
  process.env.WEBMCP_POLYFILL ??
  resolve(here, '../node_modules/@mcp-b/webmcp-polyfill/dist/index.iife.js');
const issuer = 'https://sigiro.com/api/auth';
const resource = 'https://sigiro.com';
const scope = 'openid services:read anomalies:read diagnose:read query:read';
const token = 'in-memory-browser-token';
const mimeTypes = {
  '.css': 'text/css',
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.woff2': 'font/woff2',
};

const serveClient = async (request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  const relative = pathname === '/' ? 'index.html' : pathname.slice(1);
  const candidate = resolve(clientRoot, relative);
  const rootPrefix = `${clientRoot}/`;
  let file = candidate;
  if (candidate !== clientRoot && !candidate.startsWith(rootPrefix)) {
    response.writeHead(400);
    response.end('bad path');
    return;
  }
  try {
    await readFile(file);
  } catch {
    file = resolve(clientRoot, 'index.html');
  }
  const body = await readFile(file);
  response.writeHead(200, {
    'content-type': mimeTypes[extname(file)] ?? 'application/octet-stream',
  });
  if (request.method !== 'HEAD') response.end(body);
  else response.end();
};

const startClientServer = async () => {
  const server = createServer((request, response) => {
    serveClient(request, response).catch((error) => {
      response.writeHead(500);
      response.end(String(error));
    });
  });
  await new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === 'string')
    throw new Error('local WebMCP server has no bound address');
  return { server, baseUrl: `http://127.0.0.1:${address.port}/` };
};

let baseUrl;
let origin;
let clientId;
let redirectUri;
let metadata;
let discovery;

const json = (body) => ({
  status: 200,
  contentType: 'application/json',
  body: JSON.stringify(body),
});
const installRoutes = async (page, calls, { tokenResponse = true } = {}) => {
  await page.route(`${origin}/.well-known/oauth-client`, (route) => route.fulfill(json(metadata)));
  await page.route(`${origin}/.well-known/openid-configuration`, (route) =>
    route.fulfill(json(discovery)),
  );
  await page.route(`${origin}/api/auth/oauth2/authorize**`, (route) => {
    calls.authorize.push(new URL(route.request().url()));
    return route.abort('aborted');
  });
  await page.route(`${origin}/api/auth/oauth2/token`, async (route) => {
    calls.token.push(route.request());
    if (!tokenResponse) return route.fulfill(json({ error: 'access_denied' }));
    return route.fulfill(json({ access_token: token, token_type: 'Bearer', expires_in: 900 }));
  });
  await page.route(`${origin}/v1/services`, async (route) => {
    calls.services.push(route.request());
    return route.fulfill(json([{ service: 'checkout' }]));
  });
};

const tool = async (page, name, input = {}) =>
  page.evaluate(
    async ({ name, input }) => {
      const candidate = (await document.modelContext.getTools()).find((item) => item.name === name);
      if (!candidate) throw new Error(`missing WebMCP tool: ${name}`);
      return document.modelContext.executeTool(candidate, JSON.stringify(input));
    },
    { name, input },
  );

const parseResult = (result) => {
  const value = typeof result === 'string' ? JSON.parse(result) : result;
  if (
    value &&
    Array.isArray(value.content) &&
    value.content.length === 1 &&
    typeof value.content[0]?.text === 'string'
  ) {
    return JSON.parse(value.content[0].text);
  }
  return value;
};
const transient = (page) => page.evaluate(() => sessionStorage.getItem('sigiro.webmcp.oauth'));
const storageAudit = (page) =>
  page.evaluate(async () => ({
    localStorage: localStorage.length,
    indexedDb: typeof indexedDB,
    cookies: document.cookie,
  }));

const launch = async (browser) => {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const page = await context.newPage();
  page.setDefaultTimeout(5_000);
  page.setDefaultNavigationTimeout(5_000);
  await page.addInitScript({ path: polyfill });
  return { context, page };
};

const main = async () => {
  const local = await startClientServer();
  baseUrl = local.baseUrl;
  origin = new URL(baseUrl).origin;
  clientId = `${origin}/.well-known/oauth-client`;
  redirectUri = `${origin}/`;
  metadata = {
    client_id: clientId,
    client_name: 'Sigiro browser WebMCP',
    redirect_uris: [redirectUri],
    grant_types: ['authorization_code', 'refresh_token'],
    response_types: ['code'],
    token_endpoint_auth_method: 'none',
    application_type: 'native',
    scope,
  };
  discovery = {
    issuer,
    authorization_endpoint: `${origin}/api/auth/oauth2/authorize`,
    token_endpoint: `${origin}/api/auth/oauth2/token`,
    code_challenge_methods_supported: ['S256'],
  };
  let browser;
  try {
    browser = await chromium.launch({ headless: true, timeout: 5_000 });
    const calls = { authorize: [], token: [], services: [] };
    const { context, page } = await launch(browser);
    try {
      await installRoutes(page, calls);
      await page.goto(baseUrl);
      const listServices = await page.evaluate(
        async () =>
          (await document.modelContext.getTools()).find((item) => item.name === 'list_services')
            ?.name,
      );
      assert.equal(listServices, 'list_services');
      const authAttempt = tool(page, 'list_services');
      await assert.rejects(authAttempt, /authorization required|aborted|Target page/);
      await page.waitForTimeout(100);
      assert.equal(calls.authorize.length, 1);
      const authorization = calls.authorize[0];
      assert.equal(authorization.origin, origin);
      assert.equal(authorization.pathname, '/api/auth/oauth2/authorize');
      assert.equal(authorization.searchParams.get('client_id'), clientId);
      assert.equal(authorization.searchParams.get('redirect_uri'), redirectUri);
      assert.equal(authorization.searchParams.get('response_type'), 'code');
      assert.equal(authorization.searchParams.get('scope'), scope);
      assert.equal(authorization.searchParams.get('resource'), resource);
      assert.equal(authorization.searchParams.get('code_challenge_method'), 'S256');
      assert.match(authorization.searchParams.get('state') ?? '', /^.+$/);
      assert.match(authorization.searchParams.get('code_challenge') ?? '', /^.+$/);
      assert.ok(transient(await page));

      const state = JSON.parse(await transient(page)).state;
      await page.goto(`${origin}/?code=valid-code&state=${encodeURIComponent(state)}`);
      await page.waitForFunction(() => sessionStorage.getItem('sigiro.webmcp.oauth') === null);
      assert.equal(await transient(page), null);
      const result = parseResult(await tool(page, 'list_services'));
      assert.deepEqual(result, [{ service: 'checkout' }]);
      assert.equal(calls.services.length, 1);
      assert.equal(calls.services[0].headers()['authorization'], `Bearer ${token}`);
      assert.deepEqual(await storageAudit(page), {
        localStorage: 0,
        indexedDb: 'object',
        cookies: '',
      });

      await page.reload();
      const reloadAttempt = tool(page, 'list_services');
      await assert.rejects(reloadAttempt, /authorization required|aborted|Target page/);
      await page.waitForTimeout(100);
      assert.equal(calls.authorize.length, 2);
      assert.equal(calls.services.length, 1);
    } finally {
      await context.close();
    }

    for (const scenario of ['replay', 'denial', 'mismatch']) {
      const scenarioCalls = { authorize: [], token: [], services: [] };
      const { context, page } = await launch(browser);
      try {
        await installRoutes(page, scenarioCalls, { tokenResponse: scenario !== 'denial' });
        await page.goto(baseUrl);
        await assert.rejects(
          tool(page, 'list_services'),
          /authorization required|aborted|Target page/,
        );
        await page.waitForTimeout(50);
        const saved = JSON.parse(await transient(page));
        const callbackState = scenario === 'mismatch' ? 'wrong-state' : saved.state;
        const callback = `${origin}/?code=${scenario === 'replay' ? 'valid-code' : 'denied'}&state=${encodeURIComponent(callbackState)}${scenario === 'denial' ? '&error=access_denied' : ''}`;
        await page.goto(callback);
        await page.waitForLoadState('domcontentloaded');
        assert.equal(await transient(page), null, `${scenario} must clear transient state`);
        if (scenario === 'replay') {
          await page.goto(callback);
          await page.waitForLoadState('domcontentloaded');
          assert.equal(scenarioCalls.token.length, 1, 'replay setup must exchange only once');
        } else {
          assert.equal(
            scenarioCalls.token.length,
            0,
            `${scenario} must not exchange an invalid callback`,
          );
        }
        await assert.rejects(
          tool(page, 'list_services'),
          /authorization required|aborted|Target page/,
        );
        assert.equal(scenarioCalls.services.length, 0, `${scenario} must not call operation`);
      } finally {
        await context.close();
      }
    }
    console.log(
      'WebMCP OAuth lifecycle verified: redirect contract, callback exchange, memory-only bearer, reload reauthorization, replay/denial/mismatch rejection, storage hygiene',
    );
  } finally {
    if (browser) await browser.close();
    await new Promise((resolvePromise, reject) =>
      local.server.close((error) => (error ? reject(error) : resolvePromise())),
    );
  }
};

await main();
