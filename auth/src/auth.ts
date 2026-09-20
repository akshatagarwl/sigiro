import { cimd } from "@better-auth/cimd";
import { agentAuth } from "@better-auth/agent-auth";
import { mcp } from "@better-auth/mcp";
import { oauthDeviceAuthorization, type ClientMetadataResourceFetch } from "@better-auth/oauth-provider";
import { passkey } from "@better-auth/passkey";
import { betterAuth } from "better-auth";
import { APIError, createAuthMiddleware } from "better-auth/api";
import { emailOTP, jwt, organization } from "better-auth/plugins";
import type { SecondaryStorage } from "@better-auth/core/db";

export const SIGIRO_SCOPES = [
  "openid",
  "profile",
  "email",
  "offline_access",
  "telemetry:ingest",
  "services:read",
  "diagnose:read",
  "traces:read",
  "query:read",
  "anomalies:read",
  "alerts:read",
  "alerts:write",
  "tenant:members",
  "tenant:agents",
  "tenant:admin",
] as const;

export const SIGIRO_AGENT_CAPABILITIES = [
  "telemetry:ingest",
  "services:read",
  "diagnose:read",
  "traces:read",
  "query:read",
  "anomalies:read",
  "alerts:read",
  "alerts:write",
] as const;

export interface AuthRuntime {
  baseURL: string;
  secret: string;
  resource: string;
  /**
   * Whatever better-auth should use as its primary store: a `pg` Pool on the
   * Node host, a drizzle D1 adapter on Workers. Typed loosely on purpose —
   * narrowing it to one of the two is what would force a second copy of this
   * config to exist, and a forked copy is how PKCE enforcement or the `tid`
   * claim quietly diverges between runtimes.
   */
  database: Parameters<typeof betterAuth>[0]["database"];
  secondaryStorage: SecondaryStorage;
  /** Resolves a session's active organization. Backed by pg or D1. */
  sessionTenant(sessionId: string): Promise<string | null>;
  /** Every organization a user belongs to, oldest first. Backed by pg or D1. */
  membershipsForUser(userId: string): Promise<ReadonlyArray<string>>;
  testOTP?: string;
  sendEmail(message: {
    to: string;
    from: string;
    subject: string;
    text: string;
  }): Promise<void>;
  emailFrom: string;
  /**
   * Required, with no default. The Node transport (`@better-auth/cimd/node`)
   * uses node:dns and cannot load on Workers, so importing it here to provide
   * a fallback would break the Worker build at module scope.
   */
  fetchClientMetadataResource: ClientMetadataResourceFetch;
}

const requireResource = (value: unknown, expected: string): void => {
  const resources = Array.isArray(value) ? value : [value];
  if (resources.length !== 1 || resources[0] !== expected) {
    throw new APIError("BAD_REQUEST", {
      error: "invalid_target",
      message: `resource must be ${expected}`,
    });
  }
};

export const createAuthOptions = (runtime: AuthRuntime) =>
  ({
    appName: "Sigiro",
    baseURL: runtime.baseURL,
    secret: runtime.secret,
    database: runtime.database,
    secondaryStorage: runtime.secondaryStorage,
    session: { storeSessionInDatabase: true },
    verification: { storeInDatabase: true },
    disabledPaths: ["/token"],
    telemetry: { enabled: false },
    hooks: {
      before: createAuthMiddleware(async (context) => {
        if (context.path === "/oauth2/authorize") {
          requireResource(context.query?.resource, runtime.resource);
          if (
            typeof context.query?.code_challenge !== "string" ||
            context.query.code_challenge === "" ||
            context.query.code_challenge_method !== "S256"
          ) {
            throw new APIError("BAD_REQUEST", {
              error: "invalid_request",
              message: "authorization requires PKCE with code_challenge_method S256",
            });
          }
        }
        if (context.path === "/oauth2/token" || context.path === "/device/code") {
          requireResource(context.body?.resource, runtime.resource);
        }
      }),
    },
    plugins: [
      organization({ organizationLimit: 1 }),
      jwt({
        disableSettingJwtHeader: true,
        jwks: { keyPairConfig: { alg: "EdDSA", crv: "Ed25519" } },
      }),
      emailOTP({
        storeOTP: "hashed",
        sendVerificationOnSignUp: true,
        ...(runtime.testOTP ? { generateOTP: () => runtime.testOTP! } : {}),
        async sendVerificationOTP({ email: recipient, otp }) {
          if (runtime.testOTP) return;
          await runtime.sendEmail({
            to: recipient,
            from: runtime.emailFrom,
            subject: "Your Sigiro verification code",
            text: `Your Sigiro verification code is ${otp}.`,
          });
        },
      }),
      passkey(),
      agentAuth({
        providerName: "Sigiro",
        providerDescription: "Open-source observability for agents",
        modes: ["delegated"],
        allowDynamicHostRegistration: true,
        jtiCacheStorage: "secondary-storage",
        jwksCacheStorage: "secondary-storage",
        onEvent(event) {
          console.info("agent_auth_event", JSON.stringify(event));
        },
        capabilities: SIGIRO_AGENT_CAPABILITIES.map((name) => ({
          name,
          description: `Use Sigiro ${name}`,
          location: runtime.resource,
          approvalStrength: "session" as const,
        })),
      }),
      mcp({
        loginPage: "/sign-in",
        consentPage: "/consent",
        resource: runtime.resource,
        scopes: [...SIGIRO_SCOPES],
        resources: [
          {
            identifier: runtime.resource,
            allowedScopes: [...SIGIRO_SCOPES],
            accessTokenTtl: 900,
          },
        ],
        clientRegistrationDefaultResources: [runtime.resource],
        clientRegistrationDefaultScopes: ["openid", "profile", "email"],
        clientRegistrationAllowedScopes: [...SIGIRO_SCOPES],
        clientRegistrationRequirePKCE: true,
        refreshTokenReuseInterval: 0,
        extensions: [
          {
            claims: {
              accessToken: async ({ client, referenceId, sessionId, user }) => {
                const registeredTenantId = referenceId ?? client.referenceId;
                if (registeredTenantId) return { tid: registeredTenantId };
                if (sessionId) {
                  const activeOrganizationId = await runtime.sessionTenant(sessionId);
                  if (!activeOrganizationId) throw new Error("token session has no active organization");
                  return { tid: activeOrganizationId };
                }
                if (!user?.id) throw new Error("token issuance has no tenant-bearing user, session, or client");

                const memberships = await runtime.membershipsForUser(user.id);
                if (memberships.length !== 1) {
                  throw new Error("device token user must belong to exactly one organization");
                }
                return { tid: memberships[0]! };
              },
            },
          },
        ],
        clientReference: ({ session }) =>
          typeof session?.activeOrganizationId === "string" ? session.activeOrganizationId : undefined,
        clientPrivileges: ({ session }) => typeof session?.activeOrganizationId === "string",
      }),
      oauthDeviceAuthorization({ verificationUri: `${new URL(runtime.baseURL).origin}/device` }),
      cimd({
        fetchClientMetadataResource: runtime.fetchClientMetadataResource,
        metadataProfile: "mcp-2026-07-28",
      }),
    ],
  }) satisfies Parameters<typeof betterAuth>[0];

export const createAuth = (runtime: AuthRuntime) => betterAuth(createAuthOptions(runtime));
