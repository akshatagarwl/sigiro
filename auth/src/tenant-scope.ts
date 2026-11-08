import { APIError } from "better-auth/api";

import {
  OPTIMIZER_CLIENT_ID,
  optimizerClientTenant,
} from "./optimizer-client.js";

/**
 * The tenants a device-login user may grant to `clientId`: the optimizer
 * client's fixed tenant, or any of the user's memberships for everyone else.
 */
export function deviceTenantsForClient(
  clientId: string,
  memberships: ReadonlyArray<string>
): string[] {
  if (clientId === OPTIMIZER_CLIENT_ID) {
    const tenant = optimizerClientTenant();
    return tenant !== undefined && memberships.includes(tenant) ? [tenant] : [];
  }
  return [...memberships];
}

/**
 * Tenant for a device token issued without a workspace bound at approval
 * (codes approved before `deviceTenant()` existed). A device client may select
 * only its fixed tenant, never an arbitrary membership.
 */
export function tenantForDeviceClient(
  clientId: string,
  memberships: ReadonlyArray<string>
): string {
  if (clientId === OPTIMIZER_CLIENT_ID) {
    const tenant = optimizerClientTenant();
    if (tenant === undefined || !memberships.includes(tenant)) {
      throw new APIError("FORBIDDEN", {
        message: "operator has no membership in the optimizer client tenant",
      });
    }
    return tenant;
  }
  if (memberships.length !== 1) {
    // Only tokens from before workspace binding reach this, for a user who
    // has since joined a second workspace. Guessing would be wrong, so end the
    // session the OAuth way: `invalid_grant` makes clients ask for a new login.
    throw new APIError("BAD_REQUEST", {
      error: "invalid_grant",
      error_description:
        "this login predates workspace selection and you now belong to several workspaces; run `sigiro auth login` and choose one",
    });
  }
  return memberships[0]!;
}

/** Enforce organization ownership at issuance and on every token refresh. */
export async function assertTenantScopeAccess(
  scopes: ReadonlyArray<string>,
  userId: string | undefined,
  tenantId: string,
  membershipRoles: (
    userId: string,
    tenantId: string
  ) => Promise<ReadonlyArray<string>>
): Promise<void> {
  if (
    !scopes.some(
      (scope) => scope === "tenant:admin" || scope === "optimizer:write"
    )
  )
    return;
  const roles = userId ? await membershipRoles(userId, tenantId) : [];
  if (!roles.some((role) => role === "owner" || role === "admin")) {
    throw new APIError("FORBIDDEN", {
      message: "elevated tenant scope requires an owner or admin",
    });
  }
}
