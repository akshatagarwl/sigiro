import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';

import { BROWSER_TOOL_NAMES, browserToolSchemas, browserTools } from '../src/webmcp/browser';

const source = await readFile(new URL('../src/webmcp/browser.ts', import.meta.url), 'utf8');

test('browser catalog has exactly the canonical eight names and schemas', () => {
  assert.deepEqual(
    browserTools.map((tool) => tool.name),
    [...BROWSER_TOOL_NAMES],
  );
  assert.equal(new Set(browserTools.map((tool) => tool.name)).size, 8);
  for (const schema of Object.values(browserToolSchemas))
    assert.equal(schema.additionalProperties, false);
  assert.ok(browserTools.every((tool) => tool.annotations?.readOnlyHint === true));
});

test('docs calls are same-origin, anonymous, and use the official MCP path', async () => {
  Object.assign(globalThis, { location: new URL('https://sigiro.test/docs') });
  const requests: Request[] = [];
  const credentials: RequestCredentials[] = [];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init);
    requests.push(request);
    credentials.push(init?.credentials ?? 'same-origin');
    // SAFETY: The mock response is controlled by this deterministic test.
    const body = JSON.parse(await request.clone().text()) as { method?: string };
    if (body.method === 'initialize') {
      return Response.json({
        jsonrpc: '2.0',
        id: 0,
        result: {
          protocolVersion: '2025-06-18',
          capabilities: { tools: {} },
          serverInfo: { name: 'test', version: '1.0.0' },
        },
      });
    }
    return Response.json({
      jsonrpc: '2.0',
      id: 1,
      result: { content: [{ type: 'text', text: 'ok' }] },
    });
  };
  try {
    const tool = browserTools.find((candidate) => candidate.name === 'search_docs');
    assert.ok(tool);
    await tool.execute({ query: 'telemetry' });
    assert.equal(requests[0]?.url, 'https://sigiro.test/mcp/docs');
    assert.ok(credentials.every((value) => value === 'omit'));
    assert.equal(requests[0]?.headers.has('authorization'), false);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test('OAuth boundary has no persistent token APIs and cleans redirect state', () => {
  assert.match(source, /sessionStorage/);
  assert.match(source, /removeItem\(TRANSIENT_KEY\)/);
  assert.match(source, /validateAuthResponse/);
  assert.match(source, /calculatePKCECodeChallenge/);
  assert.doesNotMatch(source, /localStorage|indexedDB/i);
  assert.match(source, /pagehide/);
  assert.match(source, /credentials: 'omit'/);
  assert.match(source, /authorization: `Bearer/);
});

test('SDK unsupported browsers remain graceful and registration is torn down', () => {
  assert.match(source, /registerTools\(\[\.\.\.browserTools\]/);
  assert.match(source, /registration\.unregister\(\)/);
  assert.match(source, /telemetry: false/);
  assert.match(source, /if \(typeof document !== 'undefined'\)/);
});
