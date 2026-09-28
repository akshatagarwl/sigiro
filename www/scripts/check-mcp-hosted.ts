import {
  MCP_CATALOG,
  MCP_ISSUER,
  MCP_RESOURCE,
  MCP_REQUIRED_SCOPE,
} from "../src/mcp-catalog";
/* eslint-disable anti-slop/no-conditional-empty-object-spread, anti-slop/no-unknown-parameters, anti-slop/no-unsafe-dictionary-type */
import {
  createMcpHandler,
  MCP_PROTECTED_RESOURCE_METADATA,
  operationTools,
  type McpTool,
} from "../src/mcp-handler";

const assert = (condition: unknown, message: string): asserts condition => {
  if (!condition) throw new Error(message);
};
const equal = (actual: unknown, expected: unknown, message: string) => {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${message}: ${JSON.stringify(actual)}`
  );
};

const token = "Bearer test-access-token";
const docsTools: McpTool[] = MCP_CATALOG.filter(
  (entry) => entry.kind === "docs"
);
const apiOrigin = "https://api.test.invalid";
const calls: Array<{ url: string; request: Request }> = [];
const originalFetch = globalThis.fetch;
globalThis.fetch = async (input: Request | URL | string): Promise<Response> => {
  const request = input instanceof Request ? input : new Request(input);
  const url = new URL(request.url);
  if (url.pathname === "/v1/auth/validate") {
    assert(
      request.headers.get("authorization") === token,
      "validation did not receive bearer"
    );
    assert(
      !request.headers.has("cookie"),
      "validation received a Cookie header"
    );
    return Response.json({ authenticated: true });
  }
  calls.push({ url: request.url, request: request.clone() });
  if (url.pathname === "/v1/anomalies") {
    return Response.json({
      anomalies: [{ service: url.searchParams.get("service") }],
    });
  }
  if (url.pathname === "/v1/diagnose")
    return Response.json({ findings: ["nested"] });
  if (url.pathname === "/v1/query")
    return Response.json({ rows: [{ ok: true }] });
  if (url.pathname === "/v1/services")
    return Response.json({ services: ["api"] });
  return new Response("upstream exploded", { status: 503 });
};

const docsCalls: Request[] = [];
const docsHandler = async (request: Request) => {
  docsCalls.push(request.clone());
  // SAFETY: this test sends the MCP tools/call envelope immediately below.
  const body = (await request.json()) as { params: { name: string } };
  return Response.json({
    jsonrpc: "2.0",
    id: 1,
    result: {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            page: body.params.name,
            nested: { ok: true },
          }),
        },
      ],
    },
  });
};
const handleMcp = createMcpHandler(docsHandler, docsTools);
// SAFETY: the handler only reads API_ORIGIN_URL in this deterministic test.
const env = { API_ORIGIN_URL: apiOrigin } as Env;

const rpc = async (method: string, params?: Record<string, unknown>) => {
  const request = new Request("https://sigiro.com/mcp", {
    method: "POST",
    headers: {
      authorization: token,
      accept: "application/json, text/event-stream",
      "content-type": "application/json",
      cookie: "must-not-forward",
      "mcp-protocol-version": "2025-06-18",
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method,
      ...(params ? { params } : {}),
    }),
  });
  const response = await handleMcp(request, env);
  const body = await response.text();
  assert(
    response.status === 200,
    `${method} returned ${response.status}: ${body}`
  );
  // SAFETY: the SDK transport returns a JSON-RPC response envelope.
  return JSON.parse(body) as {
    result?: { tools?: McpTool[]; content?: Array<{ text: string }> };
  };
};

try {
  const missing = await handleMcp(
    new Request("https://sigiro.com/mcp", { method: "POST" }),
    env
  );
  equal(missing.status, 401, "missing auth status");
  assert(
    missing.headers.get("www-authenticate") ===
      'Bearer resource_metadata="https://sigiro.com/.well-known/oauth-protected-resource"',
    "OAuth challenge metadata"
  );
  equal(
    MCP_PROTECTED_RESOURCE_METADATA,
    {
      resource: MCP_RESOURCE,
      authorization_servers: [MCP_ISSUER],
      scopes_supported: [
        "openid",
        "services:read",
        "query:read",
      ],
    },
    "protected resource metadata"
  );
  const malformed = await handleMcp(
    new Request("https://sigiro.com/mcp", {
      headers: { authorization: "Basic nope" },
    }),
    env
  );
  equal(malformed.status, 401, "malformed auth status");

  const outageFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error("validation unavailable");
  };
  const outage = await handleMcp(
    new Request("https://sigiro.com/mcp", {
      method: "POST",
      headers: { authorization: token },
    }),
    env
  );
  equal(outage.status, 401, "validation outage fails closed");
  globalThis.fetch = outageFetch;
  assert(
    MCP_REQUIRED_SCOPE ===
      "openid services:read query:read",
    "canonical scope set"
  );

  const options = await handleMcp(
    new Request("https://sigiro.com/mcp", { method: "OPTIONS" }),
    env
  );
  equal(options.status, 204, "OPTIONS status");
  assert(
    options.headers.get("access-control-allow-methods") === "POST, OPTIONS",
    "OPTIONS methods"
  );
  assert(
    options.headers
      .get("access-control-allow-headers")
      ?.includes("Mcp-Session-Id"),
    "OPTIONS headers"
  );

  await rpc("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "deterministic-test", version: "1.0" },
  });
  const listed = await rpc("tools/list");
  equal(listed.result?.tools?.length, 6, "unified tool count");
  equal(
    listed.result?.tools?.map((tool) => tool.name),
    [...docsTools, ...operationTools].map((tool) => tool.name),
    "tool catalog"
  );

  const docs = await rpc("tools/call", {
    name: "get_page",
    arguments: { route: "/docs/reference" },
  });
  assert(
    docs.result?.content?.[0]?.text.includes("nested"),
    "nested documentation response"
  );
  equal(docsCalls.length, 1, "documentation call count");
  assert(
    docsCalls[0].url === "https://sigiro.com/mcp/docs",
    "documentation pass-through URL"
  );
  assert(
    !docsCalls[0].headers.has("authorization") &&
      !docsCalls[0].headers.has("cookie"),
    "docs auth isolation"
  );

  await rpc("tools/call", { name: "list_services", arguments: {} });
  await rpc("tools/call", {
    name: "run_query",
    arguments: { sql: "SELECT 1" },
  });
  equal(
    calls.map(({ url }) => url),
    [
      `${apiOrigin}/v1/services`,
      `${apiOrigin}/v1/query`,
    ],
    "operational route mapping"
  );

  const failingFetch = globalThis.fetch;
  globalThis.fetch = async (input: Request | URL | string) => {
    const request = input instanceof Request ? input : new Request(input);
    if (request.url === `${apiOrigin}/v1/auth/validate`)
      return Response.json({ authenticated: true });
    assert(request.url === `${apiOrigin}/v1/services`, "error propagation URL");
    return new Response("bad gateway", {
      status: 502,
      statusText: "Bad Gateway",
    });
  };
  const failed = await rpc("tools/call", {
    name: "list_services",
    arguments: {},
  });
  assert(
    failed.result?.content?.[0]?.text.includes("API 502: bad gateway"),
    "upstream error propagation"
  );
  globalThis.fetch = failingFetch;
  console.log(
    "Hosted MCP behavioral contract verified: auth, metadata challenge, 6 tools, docs isolation, 2 API routes, errors, negotiation"
  );
} finally {
  globalThis.fetch = originalFetch;
}
