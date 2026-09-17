/* eslint-disable anti-slop/no-unknown-parameters, anti-slop/no-unsafe-dictionary-type, anti-slop/require-safety-comment-for-type-assertion */
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { WebStandardStreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js';
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js';
import * as Schema from 'effect/Schema';

import {
  MCP_CATALOG,
  MCP_ISSUER,
  MCP_OPERATION_CATALOG,
  MCP_OPERATION_SCOPES,
  MCP_RESOURCE,
  catalogEntry,
  type CatalogEntry,
} from './mcp-catalog';

export type McpTool = Pick<
  CatalogEntry,
  'name' | 'title' | 'description' | 'inputSchema' | 'annotations'
>;
export const MCP_PROTECTED_RESOURCE_METADATA = {
  resource: MCP_RESOURCE,
  authorization_servers: [MCP_ISSUER],
  scopes_supported: ['openid', ...MCP_OPERATION_SCOPES],
} as const;
export const tools: McpTool[] = MCP_CATALOG;
export const operationTools: McpTool[] = MCP_OPERATION_CATALOG;

type DocsHandler = (request: Request) => Promise<Response>;
const ValidatedAuthResponse = Schema.Struct({ authenticated: Schema.Literal(true) });
const decode = (name: string, value: unknown) => {
  const entry = MCP_OPERATION_CATALOG.find((candidate) => candidate.name === name);
  if (!entry) return value as Record<string, unknown>;
  return Schema.decodeUnknownSync(entry.runtimeSchema)(value) as Record<string, unknown>;
};
const json = (value: unknown) => ({
  content: [{ type: 'text' as const, text: JSON.stringify(value, null, 2) }],
});
const error = (message: string) => ({
  content: [{ type: 'text' as const, text: message }],
  isError: true,
});
const apiCall = async (env: Env, token: string, path: string, init: RequestInit = {}) => {
  const headers = new Headers(init.headers);
  headers.set('authorization', token);
  headers.set('accept', 'application/json');
  return fetch(new Request(new URL(path, env.API_ORIGIN_URL), { ...init, headers }));
};
const readApi = async (env: Env, token: string, path: string, init?: RequestInit) => {
  const response = await apiCall(env, token, path, init);
  const body = await response.text();
  if (!response.ok) throw new Error(`API ${response.status}: ${body}`);
  try {
    return JSON.parse(body);
  } catch {
    return body;
  }
};

export const createMcpHandler =
  (docsHandler: DocsHandler, _legacyDocsTools?: McpTool[]) =>
  async (request: Request, env: Env): Promise<Response> => {
    if (request.method === 'OPTIONS')
      return new Response(null, {
        status: 204,
        headers: {
          'access-control-allow-origin': '*',
          'access-control-allow-methods': 'POST, OPTIONS',
          'access-control-allow-headers':
            'Authorization, Content-Type, Mcp-Protocol-Version, Mcp-Session-Id',
        },
      });
    const authorization = request.headers.get('authorization');
    if (!authorization || !/^Bearer\s+\S+$/i.test(authorization))
      return new Response(JSON.stringify({ error: 'Bearer authentication required' }), {
        status: 401,
        headers: {
          'content-type': 'application/json',
          'www-authenticate':
            'Bearer resource_metadata="https://sigiro.com/.well-known/oauth-protected-resource"',
        },
      });
    let validation: Response;
    try {
      validation = await fetch(
        new Request(new URL('/v1/auth/validate', env.API_ORIGIN_URL), {
          headers: { authorization, accept: 'application/json' },
        }),
      );
    } catch {
      return new Response(JSON.stringify({ error: 'Invalid access token' }), {
        status: 401,
        headers: {
          'content-type': 'application/json',
          'www-authenticate':
            'Bearer resource_metadata="https://sigiro.com/.well-known/oauth-protected-resource"',
        },
      });
    }
    if (!validation.ok)
      return new Response(JSON.stringify({ error: 'Invalid access token' }), {
        status: 401,
        headers: {
          'content-type': 'application/json',
          'www-authenticate':
            'Bearer resource_metadata="https://sigiro.com/.well-known/oauth-protected-resource"',
        },
      });
    let validationBody: unknown;
    try {
      validationBody = await validation.json();
    } catch {
      return new Response(JSON.stringify({ error: 'Invalid access token' }), { status: 401 });
    }
    try {
      Schema.decodeUnknownSync(ValidatedAuthResponse)(validationBody);
    } catch {
      return new Response(JSON.stringify({ error: 'Invalid access token' }), { status: 401 });
    }
    const server = new Server(
      { name: 'sigiro', version: '1.0.0' },
      { capabilities: { tools: {} } },
    );
    server.setRequestHandler(ListToolsRequestSchema, () => ({ tools }));
    server.setRequestHandler(CallToolRequestSchema, async ({ params }) => {
      const entry = catalogEntry(params.name);
      if (!entry) return error(`Unknown tool: ${params.name}`);
      const args = params.arguments ?? {};
      try {
        if (entry.kind === 'docs') {
          const response = await docsHandler(
            new Request('https://sigiro.com/mcp/docs', {
              method: 'POST',
              headers: {
                'content-type': 'application/json',
                accept: 'application/json, text/event-stream',
              },
              body: JSON.stringify({
                jsonrpc: '2.0',
                id: 1,
                method: 'tools/call',
                params: { name: entry.name, arguments: args },
              }),
            }),
          );
          const payload = (await response.json()) as {
            result?: { content?: unknown; isError?: boolean };
            error?: { message?: string };
          };
          if (!response.ok || payload.error)
            return error(
              `documentation MCP failed: ${response.status}: ${payload.error?.message ?? 'unknown error'}`,
            );
          return payload.result ?? error('documentation MCP returned no result');
        }
        const decoded = decode(entry.name, args);
        if (entry.name === 'list_anomalies') {
          const q = new URLSearchParams();
          if (decoded.service) q.set('service', String(decoded.service));
          if (decoded.from_ts !== undefined) q.set('from_ts', String(decoded.from_ts));
          if (decoded.to_ts !== undefined) q.set('to_ts', String(decoded.to_ts));
          return json(await readApi(env, authorization, `${entry.route}?${q}`));
        }
        if (entry.name === 'diagnose_service')
          return json(
            await readApi(env, authorization, entry.route!, {
              method: entry.method,
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify(decoded),
            }),
          );
        if (entry.name === 'run_query')
          return json(
            await readApi(env, authorization, entry.route!, {
              method: entry.method,
              headers: { 'content-type': 'text/plain' },
              body: String(decoded.sql),
            }),
          );
        return json(await readApi(env, authorization, entry.route!, { method: entry.method }));
      } catch (e) {
        return error(e instanceof Error ? e.message : String(e));
      }
    });
    const transport = new WebStandardStreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
      enableJsonResponse: true,
    });
    await server.connect(transport);
    const response = await transport.handleRequest(request);
    const headers = new Headers(response.headers);
    headers.set('access-control-allow-origin', '*');
    headers.set('access-control-expose-headers', 'Mcp-Session-Id');
    return new Response(response.body, { status: response.status, headers });
  };
