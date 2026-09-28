// A separate consented, owner-only client for the optimizer control plane.
// This document is served by the auth Worker; the Worker also returns the
// in-memory copy to CIMD so same-zone Worker self-fetches never hit CF 1014.
export const OPTIMIZER_CLIENT_ID = "https://sigiro.com/api/auth/optimizer-client.json";
// Until tenant-specific client registration is available, bind the live-test
// operator client to exactly this previously provisioned Sigiro tenant.
export const OPTIMIZER_CLIENT_TENANT_ID = "benchapp";
export const OPTIMIZER_CLIENT_METADATA = {
  client_id: OPTIMIZER_CLIENT_ID,
  client_name: "Sigiro Optimizer operator (Benchapp)",
  client_uri: "https://sigiro.com/",
  application_type: "native",
  redirect_uris: ["com.sigiro.cli:/oauth/callback"],
  grant_types: ["urn:ietf:params:oauth:grant-type:device_code", "refresh_token"],
  token_endpoint_auth_method: "none",
  scope: "openid profile offline_access tenant:admin optimizer:write query:read",
} as const;
export const OPTIMIZER_CLIENT_METADATA_JSON = JSON.stringify(OPTIMIZER_CLIENT_METADATA);
