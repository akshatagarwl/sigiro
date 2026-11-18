/* eslint-disable anti-slop/no-unsafe-dictionary-type, anti-slop/require-safety-comment-for-type-assertion, anti-slop/no-runtime-typeof, anti-slop/no-unknown-returns, anti-slop/no-known-value-widening */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { defineTool, registerTools, type ToolRegistration } from '@nekuda/webmcp-sdk';
import * as oauth from 'oauth4webapi';
import * as Schema from 'effect/Schema';
import {
  MCP_CATALOG,
  MCP_REQUIRED_SCOPE,
  MCP_RESOURCE,
  MCP_ISSUER,
  type CatalogEntry,
} from '../mcp-catalog';

const CLIENT_METADATA_PATH = '/.well-known/oauth-client';
const REDIRECT_URI_PATH = '/';
const AUTH_PATH = '/api/auth';
const RESOURCE = MCP_RESOURCE;
const TRANSIENT_KEY = 'sigiro.webmcp.oauth';
const REQUIRED_SCOPE = MCP_REQUIRED_SCOPE;

type JsonObject = Record<string, unknown>;
type TokenState = { accessToken: string; refreshToken?: string; expiresAt: number };
type TransientState = { state: string; verifier: string; returnPath: string };

export const browserToolSchemas = Object.fromEntries(
  MCP_CATALOG.map((entry) => [entry.name, entry.inputSchema]),
) as Record<string, Record<string, unknown>>;
export const BROWSER_TOOL_NAMES = MCP_CATALOG.map((entry) => entry.name) as string[];

let tokenState: TokenState | undefined;
let callbackPromise: Promise<void> | undefined;

const transientStorage = (): Storage | undefined => {
  try {
    return globalThis.sessionStorage;
  } catch {
    return undefined;
  }
};
const readTransient = (): TransientState | undefined => {
  const raw = transientStorage()?.getItem(TRANSIENT_KEY);
  if (!raw) return undefined;
  try {
    const value = JSON.parse(raw) as Partial<TransientState>;
    if (
      typeof value.state !== 'string' ||
      typeof value.verifier !== 'string' ||
      typeof value.returnPath !== 'string'
    )
      return undefined;
    return value as TransientState;
  } catch {
    return undefined;
  }
};
const clearTransient = () => transientStorage()?.removeItem(TRANSIENT_KEY);
const safeReturnPath = (value: string): string => {
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin ? `${url.pathname}${url.search}${url.hash}` : '/';
  } catch {
    return '/';
  }
};
const json = async (response: Response): Promise<JsonObject | string> => {
  const text = await response.text();
  if (!response.ok) throw new Error(`Sigiro request failed (${response.status})`);
  try {
    return JSON.parse(text) as JsonObject;
  } catch {
    return text;
  }
};

const clientMetadataUrl = () => new URL(CLIENT_METADATA_PATH, location.origin);
const redirectUri = () => new URL(REDIRECT_URI_PATH, location.origin).href;
const authorizationServer = (): oauth.AuthorizationServer => ({
  issuer: MCP_ISSUER,
  authorization_endpoint: new URL(`${AUTH_PATH}/oauth2/authorize`, location.origin).href,
  token_endpoint: new URL(`${AUTH_PATH}/oauth2/token`, location.origin).href,
  code_challenge_methods_supported: ['S256'],
});

const loadClient = async (): Promise<oauth.Client> => {
  const metadataUrl = clientMetadataUrl();
  if (
    !['https:', 'http:'].includes(metadataUrl.protocol) ||
    metadataUrl.origin !== location.origin ||
    (metadataUrl.protocol === 'http:' &&
      !['localhost', '127.0.0.1', '[::1]'].includes(metadataUrl.hostname))
  )
    throw new Error('invalid same-origin CIMD client metadata URL');
  const response = await fetch(metadataUrl, { credentials: 'omit', cache: 'no-store' });
  if (!response.ok) throw new Error('CIMD metadata unavailable');
  const metadata = (await response.json()) as Partial<oauth.Client> & { redirect_uris?: unknown };
  if (
    metadata.client_id !== metadataUrl.href ||
    metadata.application_type !== 'native' ||
    metadata.token_endpoint_auth_method !== 'none' ||
    !Array.isArray(metadata.redirect_uris) ||
    metadata.redirect_uris.length !== 1 ||
    metadata.redirect_uris[0] !== redirectUri() ||
    !Array.isArray(metadata.response_types) ||
    metadata.response_types.length !== 1 ||
    metadata.response_types[0] !== 'code'
  )
    throw new Error('CIMD metadata failed the Better Auth MCP profile');
  return metadata as oauth.Client;
};

const handleCallback = async (): Promise<void> => {
  if (!location.search) return;
  const params = new URLSearchParams(location.search);
  if (!params.has('code') && !params.has('error') && !params.has('state')) return;
  const transient = readTransient();
  clearTransient();
  try {
    if (!transient) throw new Error('authorization response is missing or replayed');
    if (params.get('state') !== transient.state) throw new Error('authorization state mismatch');
    const client = await loadClient();
    const callback = oauth.validateAuthResponse(
      authorizationServer(),
      client,
      params,
      transient.state,
    );
    const grantOptions: oauth.TokenEndpointRequestOptions = {
      additionalParameters: { resource: RESOURCE },
    };
    if (
      location.protocol === 'http:' &&
      ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname)
    ) {
      grantOptions[oauth.allowInsecureRequests] = true;
    }
    const response = await oauth.authorizationCodeGrantRequest(
      authorizationServer(),
      client,
      oauth.None(),
      callback,
      redirectUri(),
      transient.verifier,
      grantOptions,
    );
    const tokens = await oauth.processAuthorizationCodeResponse(
      authorizationServer(),
      client,
      response,
    );
    if (!tokens.access_token) throw new Error('authorization response omitted an access token');
    tokenState = {
      accessToken: tokens.access_token,
      refreshToken: tokens.refresh_token,
      expiresAt: Date.now() + (tokens.expires_in ?? 900) * 1000,
    };
    history.replaceState(null, '', safeReturnPath(transient.returnPath));
  } catch (error) {
    // The page displays this through the next tool invocation; protocol details and tokens are never logged.
    tokenState = undefined;
    history.replaceState(null, '', safeReturnPath(transient?.returnPath ?? '/'));
    throw error;
  }
};

const beginAuthorization = async (): Promise<never> => {
  const client = await loadClient();
  const state = oauth.generateRandomState();
  const verifier = oauth.generateRandomCodeVerifier();
  const challenge = await oauth.calculatePKCECodeChallenge(verifier);
  transientStorage()?.setItem(
    TRANSIENT_KEY,
    JSON.stringify({
      state,
      verifier,
      returnPath: `${location.pathname}${location.search}${location.hash}`,
    } satisfies TransientState),
  );
  const url = new URL(authorizationServer().authorization_endpoint!);
  url.search = new URLSearchParams({
    client_id: client.client_id,
    redirect_uri: redirectUri(),
    response_type: 'code',
    scope: REQUIRED_SCOPE,
    resource: RESOURCE,
    code_challenge: challenge,
    code_challenge_method: 'S256',
    state,
  }).toString();
  location.assign(url.href);
  throw new Error('authorization required; the visible sign-in flow has started');
};

const ensureAccessToken = async (): Promise<string> => {
  if (callbackPromise) await callbackPromise;
  if (tokenState && tokenState.expiresAt > Date.now() + 15_000) return tokenState.accessToken;
  // Tokens are deliberately never refreshed: a reload must reauthorize and all
  // token state is confined to this module's memory.
  tokenState = undefined;
  return beginAuthorization();
};

const callDocs = async (name: string, input: JsonObject): Promise<unknown> => {
  const client = new Client({ name: 'sigiro-webmcp', version: '1.0.0' });
  const transport = new StreamableHTTPClientTransport(new URL('/mcp/docs', location.origin), {
    requestInit: { credentials: 'omit' },
  });
  try {
    await client.connect(transport);
    return await client.callTool({ name, arguments: input });
  } finally {
    await client.close();
  }
};
const callApi = async (path: string, init: RequestInit = {}): Promise<unknown> => {
  const token = await ensureAccessToken();
  const headers = new Headers(init.headers);
  headers.set('accept', 'application/json');
  headers.set('authorization', `Bearer ${token}`);
  const response = await fetch(new URL(path, location.origin), {
    ...init,
    credentials: 'omit',
    headers,
  });
  if (response.status === 401) {
    tokenState = undefined;
    return ensureAccessToken()
      .then((retryToken) =>
        fetch(new URL(path, location.origin), {
          ...init,
          credentials: 'omit',
          headers: new Headers({
            ...Object.fromEntries(headers),
            authorization: `Bearer ${retryToken}`,
          }),
        }),
      )
      .then(json);
  }
  return json(response);
};

const executeCatalogTool = async (entry: CatalogEntry, input: JsonObject): Promise<unknown> => {
  if (entry.kind === 'docs') return callDocs(entry.name, input);
  const decoded = Schema.decodeUnknownSync(entry.runtimeSchema)(input) as JsonObject;
  if (entry.name === 'list_anomalies') {
    const q = new URLSearchParams();
    if (decoded.service) q.set('service', String(decoded.service));
    if (decoded.from_ts !== undefined) q.set('from_ts', String(decoded.from_ts));
    if (decoded.to_ts !== undefined) q.set('to_ts', String(decoded.to_ts));
    return callApi(`${entry.route}?${q}`);
  }
  if (entry.name === 'diagnose_service')
    return callApi(entry.route!, {
      method: entry.method,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(decoded),
    });
  if (entry.name === 'run_query')
    return callApi(entry.route!, {
      method: entry.method,
      headers: { 'content-type': 'text/plain' },
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
  }),
);

export const registerBrowserTools = (): { registration: ToolRegistration; destroy: () => void } => {
  const registration = registerTools([...browserTools], { telemetry: false });
  const destroy = () => registration.unregister();
  return { registration, destroy };
};

if (typeof document !== 'undefined') {
  callbackPromise = handleCallback().finally(() => {
    callbackPromise = undefined;
  });
  const { destroy } = registerBrowserTools();
  addEventListener('pagehide', destroy, { once: true });
}
