/* eslint-disable anti-slop/require-safety-comment-for-type-assertion, anti-slop/no-runtime-typeof */
import * as oauth from "oauth4webapi";

import { MCP_ISSUER, MCP_RESOURCE } from "../mcp-catalog";
import {
  BROWSER_CLIENT_METADATA_PATH,
  type BROWSER_CLIENT_REDIRECT_PATHS,
} from "./browser-client";

type RedirectPath = (typeof BROWSER_CLIENT_REDIRECT_PATHS)[number];
type TokenState = { accessToken: string; expiresAt: number };
type TransientState = { state: string; verifier: string; returnPath: string };

export type BrowserOAuthOptions = {
  redirectPath: RedirectPath;
  scope: string;
  transientKey: string;
  /** Where the bearer lives. `undefined` keeps it in module memory only. */
  tokenKey?: string;
};

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

const sessionStore = (): Storage | undefined => {
  try {
    return globalThis.sessionStorage;
  } catch {
    return undefined;
  }
};

const readJson = <T>(key: string, valid: (value: Partial<T>) => boolean) => {
  const raw = sessionStore()?.getItem(key);
  if (!raw) return undefined;
  try {
    const value = JSON.parse(raw) as Partial<T>;
    return valid(value) ? (value as T) : undefined;
  } catch {
    return undefined;
  }
};

const safeReturnPath = (value: string, fallback: string): string => {
  try {
    const url = new URL(value, location.origin);
    return url.origin === location.origin
      ? `${url.pathname}${url.search}${url.hash}`
      : fallback;
  } catch {
    return fallback;
  }
};

const authorizationServer = (): oauth.AuthorizationServer => ({
  issuer: MCP_ISSUER,
  authorization_endpoint: new URL("/api/auth/oauth2/authorize", location.origin)
    .href,
  token_endpoint: new URL("/api/auth/oauth2/token", location.origin).href,
  code_challenge_methods_supported: ["S256"],
  // Better Auth signs ID tokens with Ed25519 (see the issuer's metadata).
  // Without this, oauth4webapi assumes RS256 and rejects every ID token with
  // `unexpected JWT "alg" header parameter`.
  id_token_signing_alg_values_supported: ["EdDSA"],
});

export const createBrowserOAuth = (options: BrowserOAuthOptions) => {
  let memoryToken: TokenState | undefined;
  const redirectUri = () => new URL(options.redirectPath, location.origin).href;

  const readToken = (): TokenState | undefined =>
    options.tokenKey
      ? readJson<TokenState>(
          options.tokenKey,
          (value) =>
            typeof value.accessToken === "string" &&
            typeof value.expiresAt === "number"
        )
      : memoryToken;
  const writeToken = (value: TokenState | undefined) => {
    if (!options.tokenKey) {
      memoryToken = value;
      return;
    }
    if (value) sessionStore()?.setItem(options.tokenKey, JSON.stringify(value));
    else sessionStore()?.removeItem(options.tokenKey);
  };

  const loadClient = async (): Promise<oauth.Client> => {
    const metadataUrl = new URL(BROWSER_CLIENT_METADATA_PATH, location.origin);
    if (
      metadataUrl.protocol !== "https:" &&
      !(metadataUrl.protocol === "http:" && LOOPBACK.has(metadataUrl.hostname))
    )
      throw new Error("invalid same-origin CIMD client metadata URL");
    const response = await fetch(metadataUrl, {
      credentials: "omit",
      cache: "no-store",
    });
    if (!response.ok) throw new Error("CIMD metadata unavailable");
    const metadata = (await response.json()) as Partial<oauth.Client> & {
      redirect_uris?: unknown;
    };
    if (
      metadata.client_id !== metadataUrl.href ||
      metadata.application_type !== "native" ||
      metadata.token_endpoint_auth_method !== "none" ||
      !Array.isArray(metadata.redirect_uris) ||
      !metadata.redirect_uris.includes(redirectUri()) ||
      !Array.isArray(metadata.response_types) ||
      metadata.response_types.length !== 1 ||
      metadata.response_types[0] !== "code"
    )
      throw new Error("CIMD metadata failed the Better Auth MCP profile");
    return metadata as oauth.Client;
  };

  /**
   * Exchanges an authorization response on this surface's redirect path.
   * Resolves `false` when the URL carries no response for this surface.
   */
  const handleCallback = async (): Promise<boolean> => {
    if (location.pathname !== options.redirectPath || !location.search)
      return false;
    const params = new URLSearchParams(location.search);
    if (!params.has("code") && !params.has("error") && !params.has("state"))
      return false;
    const transient = readJson<TransientState>(
      options.transientKey,
      (value) =>
        typeof value.state === "string" &&
        typeof value.verifier === "string" &&
        typeof value.returnPath === "string"
    );
    sessionStore()?.removeItem(options.transientKey);
    const fallback = options.redirectPath;
    try {
      if (!transient)
        throw new Error("authorization response is missing or replayed");
      if (params.get("state") !== transient.state)
        throw new Error("authorization state mismatch");
      const client = await loadClient();
      const callback = oauth.validateAuthResponse(
        authorizationServer(),
        client,
        params,
        transient.state
      );
      const grantOptions: oauth.TokenEndpointRequestOptions = {
        additionalParameters: { resource: MCP_RESOURCE },
      };
      if (location.protocol === "http:" && LOOPBACK.has(location.hostname))
        grantOptions[oauth.allowInsecureRequests] = true;
      const response = await oauth.authorizationCodeGrantRequest(
        authorizationServer(),
        client,
        oauth.None(),
        callback,
        redirectUri(),
        transient.verifier,
        grantOptions
      );
      const tokens = await oauth.processAuthorizationCodeResponse(
        authorizationServer(),
        client,
        response
      );
      if (!tokens.access_token)
        throw new Error("authorization response omitted an access token");
      writeToken({
        accessToken: tokens.access_token,
        expiresAt: Date.now() + (tokens.expires_in ?? 900) * 1000,
      });
      history.replaceState(
        null,
        "",
        safeReturnPath(transient.returnPath, fallback)
      );
      return true;
    } catch (error) {
      writeToken(undefined);
      history.replaceState(
        null,
        "",
        safeReturnPath(transient?.returnPath ?? fallback, fallback)
      );
      throw error;
    }
  };

  /** Navigates to the authorization endpoint; the returned promise never settles usefully. */
  const beginAuthorization = async (): Promise<never> => {
    const client = await loadClient();
    const state = oauth.generateRandomState();
    const verifier = oauth.generateRandomCodeVerifier();
    const challenge = await oauth.calculatePKCECodeChallenge(verifier);
    sessionStore()?.setItem(
      options.transientKey,
      JSON.stringify({
        state,
        verifier,
        returnPath: `${location.pathname}${location.search}${location.hash}`,
      } satisfies TransientState)
    );
    const url = new URL(authorizationServer().authorization_endpoint!);
    url.search = new URLSearchParams({
      client_id: client.client_id,
      redirect_uri: redirectUri(),
      response_type: "code",
      scope: options.scope,
      resource: MCP_RESOURCE,
      code_challenge: challenge,
      code_challenge_method: "S256",
      state,
    }).toString();
    location.assign(url.href);
    throw new Error(
      "authorization required; the visible sign-in flow has started"
    );
  };

  /** A live bearer, or `undefined` when the reader has to sign in again. Never refreshed. */
  const currentToken = (): string | undefined => {
    const token = readToken();
    if (token && token.expiresAt > Date.now() + 15_000)
      return token.accessToken;
    writeToken(undefined);
    return undefined;
  };

  const ensureAccessToken = async (): Promise<string> =>
    currentToken() ?? beginAuthorization();

  const forget = () => writeToken(undefined);

  return {
    handleCallback,
    beginAuthorization,
    currentToken,
    ensureAccessToken,
    forget,
  };
};
