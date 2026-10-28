import { APIError } from "better-auth/api";

import {
  OPTIMIZER_CLIENT_ID,
  OPTIMIZER_CLIENT_TENANT_ID,
} from "./optimizer-client.js";

/** A device client may select only its fixed tenant, never an arbitrary membership. */
export function tenantForDeviceClient(
  clientId: string,
  memberships: ReadonlyArray<string>
): string {
  if (clientId === OPTIMIZER_CLIENT_ID) {
    if (!memberships.includes(OPTIMIZER_CLIENT_TENANT_ID)) {
      throw new APIError("FORBIDDEN", {
        message: "operator has no membership in the optimizer client tenant",
      });
    }
    return OPTIMIZER_CLIENT_TENANT_ID;
  }
  if (memberships.length !== 1) {
    throw new APIError("FORBIDDEN", {
      message: "device token user must belong to exactly one organization",
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
