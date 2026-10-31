/* eslint-disable anti-slop/no-unsafe-dictionary-type, anti-slop/require-safety-comment-for-type-assertion, anti-slop/no-runtime-typeof, anti-slop/no-unknown-returns, anti-slop/no-known-value-widening */
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import {
  defineTool,
  registerTools,
  type ToolRegistration,
} from "@nekuda/webmcp-sdk";
import * as Schema from "effect/Schema";

import { createBrowserOAuth } from "../lib/browser-oauth";
import {
  MCP_CATALOG,
  MCP_REQUIRED_SCOPE,
  type CatalogEntry,
} from "../mcp-catalog";

const oauthFlow = createBrowserOAuth({
  redirectPath: "/",
  scope: MCP_REQUIRED_SCOPE,
  transientKey: "sigiro.webmcp.oauth",
});

type JsonObject = Record<string, unknown>;

export const browserToolSchemas = Object.fromEntries(
  MCP_CATALOG.map((entry) => [entry.name, entry.inputSchema])
) as Record<string, Record<string, unknown>>;
export const BROWSER_TOOL_NAMES = MCP_CATALOG.map(
  (entry) => entry.name
) as string[];

let callbackPromise: Promise<unknown> | undefined;

const json = async (response: Response): Promise<JsonObject | string> => {
  const text = await response.text();
  if (!response.ok)
    throw new Error(`Sigiro request failed (${response.status})`);
  try {
    return JSON.parse(text) as JsonObject;
  } catch {
    return text;
  }
};

// Tokens are deliberately never refreshed or persisted: a reload must
// reauthorize and all token state is confined to this module's memory.
const ensureAccessToken = async (): Promise<string> => {
  if (callbackPromise) await callbackPromise;
  return oauthFlow.ensureAccessToken();
};

const callDocs = async (name: string, input: JsonObject): Promise<unknown> => {
  const client = new Client({ name: "sigiro-webmcp", version: "1.0.0" });
  const transport = new StreamableHTTPClientTransport(
    new URL("/mcp/docs", location.origin),
    {
      requestInit: { credentials: "omit" },
    }
  );
  try {
    await client.connect(transport);
    return await client.callTool({ name, arguments: input });
  } finally {
    await client.close();
  }
};
const callApi = async (
  path: string,
  init: RequestInit = {}
): Promise<unknown> => {
  const token = await ensureAccessToken();
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  headers.set("authorization", `Bearer ${token}`);
  const response = await fetch(new URL(path, location.origin), {
    ...init,
    credentials: "omit",
    headers,
  });
  if (response.status === 401) {
    oauthFlow.forget();
    return ensureAccessToken()
      .then((retryToken) =>
        fetch(new URL(path, location.origin), {
          ...init,
          credentials: "omit",
          headers: new Headers({
            ...Object.fromEntries(headers),
            authorization: `Bearer ${retryToken}`,
          }),
        })
      )
      .then(json);
  }
  return json(response);
};

const executeCatalogTool = async (
  entry: CatalogEntry,
  input: JsonObject
): Promise<unknown> => {
  if (entry.kind === "docs") return callDocs(entry.name, input);
  const decoded = Schema.decodeUnknownSync(entry.runtimeSchema)(
    input
  ) as JsonObject;
  if (entry.name === "run_query")
    return callApi(entry.route!, {
      method: entry.method,
      headers: { "content-type": "text/plain" },
      body: String(decoded.sql),
    });
  return callApi(entry.route!, { method: entry.method });
};

export const browserTools = MCP_CATALOG.map((entry) =>
  defineTool<JsonObject>({
    stableKey: `${entry.kind}.${entry.name}`,
    name: entry.name,
    title: entry.title,
    description: entry.description,
    inputSchema: entry.inputSchema,
    annotations: entry.annotations,
    execute: (input) => executeCatalogTool(entry, input),
  })
);

export const registerBrowserTools = (): {
  registration: ToolRegistration;
  destroy: () => void;
} => {
  const registration = registerTools([...browserTools], { telemetry: false });
  const destroy = () => registration.unregister();
  return { registration, destroy };
};

if (typeof document !== "undefined") {
  callbackPromise = oauthFlow.handleCallback().finally(() => {
    callbackPromise = undefined;
  });
  const { destroy } = registerBrowserTools();
  addEventListener("pagehide", destroy, { once: true });
}
