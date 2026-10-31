import { agentAuth } from "@better-auth/agent-auth";
import { cimd } from "@better-auth/cimd";
import { defineRequestState } from "@better-auth/core/context";
import type { SecondaryStorage } from "@better-auth/core/db";
import { mcp } from "@better-auth/mcp";
import {
  oauthDeviceAuthorization,
  type ClientMetadataResourceFetch,
} from "@better-auth/oauth-provider";
import { passkey } from "@better-auth/passkey";
import { betterAuth } from "better-auth";
import { APIError, createAuthMiddleware } from "better-auth/api";
import { emailOTP, jwt, organization } from "better-auth/plugins";

import { deviceTenant } from "./device-tenant.js";
import {
  assertTenantScopeAccess,
  tenantForDeviceClient,
} from "./tenant-scope.js";

export const SIGIRO_SCOPES = [
  "openid",
  "profile",
  "email",
  "offline_access",
  "telemetry:ingest",
  "services:read",
  "traces:read",
  "query:read",
  "alerts:read",
  "alerts:write",
  "tenant:members",
  "tenant:agents",
  "tenant:admin",
  "optimizer:write",
] as const;

export const SIGIRO_AGENT_CAPABILITIES = [
  "telemetry:ingest",
  "services:read",
  "traces:read",
  "query:read",
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
  /** Organization roles for elevated tenant write scopes. */
  membershipRoles(
    userId: string,
    tenantId: string
  ): Promise<ReadonlyArray<string>>;
  /** Display name of an organization, for the `tenant_name` claim. */
  organizationName(tenantId: string): Promise<string | null>;
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

/**
 * Better Auth runs `sendVerificationOTP` through `runInBackgroundOrAwait`,
 * which awaits it and then LOGS and SWALLOWS any error, so the endpoint
 * answered `{"success":true}` while Cloudflare refused every email (403,
 * `sending_disabled`). The send records its failure here, per request, and an
 * after hook turns it into an error the sign-in form shows.
 */
const emailDelivery = defineRequestState<{ failure?: unknown }>(() => ({}));

export const EMAIL_DELIVERY_FAILED_MESSAGE =
  "We could not send the code. Try again in a minute.";

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
      after: createAuthMiddleware(async () => {
        const { failure } = await emailDelivery.get();
        if (failure === undefined) return;
        throw new APIError("SERVICE_UNAVAILABLE", {
          code: "EMAIL_DELIVERY_FAILED",
          message: EMAIL_DELIVERY_FAILED_MESSAGE,
        });
      }),
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
              message:
                "authorization requires PKCE with code_challenge_method S256",
            });
          }
        }
        if (
          context.path === "/oauth2/token" ||
          context.path === "/device/code"
        ) {
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
        expiresIn: 300,
        sendVerificationOnSignUp: true,
        ...(runtime.testOTP ? { generateOTP: () => runtime.testOTP! } : {}),
        async sendVerificationOTP({ email: recipient, otp }) {
          if (runtime.testOTP) return;
          try {
            await runtime.sendEmail({
              to: recipient,
              from: runtime.emailFrom,
              subject: "Your Sigiro verification code",
              text: `Your Sigiro verification code is ${otp}. It expires in five minutes. If you didn't request this code, you can ignore this email.`,
            });
          } catch (cause) {
            console.error("verification email was not sent", cause);
            (await emailDelivery.get()).failure = cause;
            throw cause;
          }
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
              accessToken: async ({
                client,
                referenceId,
                sessionId,
                user,
                scopes,
              }) => {
                const registeredTenantId = referenceId ?? client.referenceId;
                let tenantId: string;
                if (referenceId && user?.id) {
                  // Bound to a user's chosen workspace (device approval).
                  // Re-checked on every refresh: removing a member from a
                  // workspace must end their access to it.
                  const memberships = await runtime.membershipsForUser(user.id);
                  if (!memberships.includes(referenceId))
                    throw new APIError("BAD_REQUEST", {
                      error: "invalid_grant",
                      error_description:
                        "you are no longer a member of this workspace; run `sigiro auth login`",
                    });
                  tenantId = referenceId;
                } else if (registeredTenantId) {
                  tenantId = registeredTenantId;
                } else if (sessionId) {
                  const activeOrganizationId =
                    await runtime.sessionTenant(sessionId);
                  if (!activeOrganizationId)
                    throw new Error("token session has no active organization");
                  tenantId = activeOrganizationId;
                } else {
                  if (!user?.id)
                    throw new Error(
                      "token issuance has no tenant-bearing user, session, or client"
                    );
                  const memberships = await runtime.membershipsForUser(user.id);
                  tenantId = tenantForDeviceClient(
                    client.clientId,
                    memberships
                  );
                }
                await assertTenantScopeAccess(
                  scopes,
                  user?.id,
                  tenantId,
                  runtime.membershipRoles
                );
                const tenantName = await runtime.organizationName(tenantId);
                return tenantName
                  ? { tid: tenantId, tenant_name: tenantName }
                  : { tid: tenantId };
              },
            },
          },
        ],
        clientReference: ({ session }) =>
          typeof session?.activeOrganizationId === "string"
            ? session.activeOrganizationId
            : undefined,
        clientPrivileges: ({ session }) =>
          typeof session?.activeOrganizationId === "string",
      }),
      oauthDeviceAuthorization({
        verificationUri: `${new URL(runtime.baseURL).origin}/device`,
      }),
      // After oauthDeviceAuthorization(): it wraps that plugin's grant.
      deviceTenant(),
      cimd({
        fetchClientMetadataResource: runtime.fetchClientMetadataResource,
        metadataProfile: "mcp-2026-07-28",
      }),
    ],
  }) satisfies Parameters<typeof betterAuth>[0];

export const createAuth = (runtime: AuthRuntime) =>
  betterAuth(createAuthOptions(runtime));
