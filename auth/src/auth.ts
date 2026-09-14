import { cimd } from "@better-auth/cimd";
import { mcp } from "@better-auth/mcp";
import { oauthDeviceAuthorization, type ClientMetadataResourceFetch } from "@better-auth/oauth-provider";
import { passkey } from "@better-auth/passkey";
import { isPublicRoutableHost } from "@better-auth/core/utils/host";
import { betterAuth } from "better-auth";
import { emailOTP, jwt, organization } from "better-auth/plugins";
import type { Pool } from "pg";

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

export interface AuthRuntime {
  baseURL: string;
  secret: string;
  resource: string;
  database: Pool;
  testOTP?: string;
  sendEmail(message: {
    to: string;
    from: string;
    subject: string;
    text: string;
  }): Promise<void>;
  emailFrom: string;
}

const fetchClientMetadataResource: ClientMetadataResourceFetch = async (input, init) => {
  const request = new Request(input, init);
  const url = new URL(request.url);
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    url.hash ||
    !isPublicRoutableHost(url.hostname)
  ) {
    throw new Error("CIMD metadata must use a public HTTPS URL without credentials or fragments");
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    throw new Error("CIMD metadata requests must use GET or HEAD");
  }

  const response = await fetch(request, { redirect: "manual" });
  if (response.status >= 300 && response.status < 400) {
    throw new Error("CIMD metadata redirects are not allowed");
  }
  return response;
};

export const createAuthOptions = (runtime: AuthRuntime) =>
  ({
    appName: "Sigiro",
    baseURL: runtime.baseURL,
    secret: runtime.secret,
    database: runtime.database,
    disabledPaths: ["/token"],
    telemetry: { enabled: false },
    plugins: [
      organization(),
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
              accessToken: ({ client, referenceId }) => ({
                tid: referenceId ?? client.referenceId,
              }),
            },
          },
        ],
        clientReference: ({ session }) =>
          typeof session?.activeOrganizationId === "string" ? session.activeOrganizationId : undefined,
        clientPrivileges: ({ session }) => typeof session?.activeOrganizationId === "string",
      }),
      oauthDeviceAuthorization(),
      cimd({
        fetchClientMetadataResource,
        metadataProfile: "mcp-2026-07-28",
      }),
    ],
  }) satisfies Parameters<typeof betterAuth>[0];

export const createAuth = (runtime: AuthRuntime) => betterAuth(createAuthOptions(runtime));
