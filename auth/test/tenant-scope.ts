import assert from "node:assert/strict";
import test from "node:test";

import { OPTIMIZER_CLIENT_ID } from "../src/optimizer-client.js";
import {
  assertTenantScopeAccess,
  tenantForDeviceClient,
} from "../src/tenant-scope.js";

test("dedicated device client selects only its fixed Benchapp membership", () => {
  assert.equal(
    tenantForDeviceClient(OPTIMIZER_CLIENT_ID, ["personal-org", "benchapp"]),
    "benchapp"
  );
  assert.throws(
    () => tenantForDeviceClient(OPTIMIZER_CLIENT_ID, ["personal-org"]),
    /no membership/
  );
  assert.throws(
    () =>
      tenantForDeviceClient("ordinary-cli", ["personal-org", "benchapp"]),
    /exactly one/
  );
  assert.equal(
    tenantForDeviceClient("ordinary-cli", ["personal-org"]),
    "personal-org"
  );
});

test("read scopes do not require privileged membership", async () => {
  await assertTenantScopeAccess(
    ["query:read"],
    undefined,
    "benchapp",
    async () => {
      throw new Error("read scopes must not query roles");
    }
  );
});

test("write and tenant-admin scopes require owner or admin in the exact tenant", async () => {
  for (const scope of ["tenant:admin", "optimizer:write"]) {
    for (const role of ["member", "viewer", ""]) {
      await assert.rejects(
        assertTenantScopeAccess(
          [scope],
          "user",
          "benchapp",
          async (user, tenant) => {
            assert.equal(user, "user");
            assert.equal(tenant, "benchapp");
            return [role];
          }
        ),
        /owner or admin/
      );
    }
    await assert.rejects(
      assertTenantScopeAccess([scope], undefined, "benchapp", async () => {
        throw new Error("no user must not query membership");
      }),
      /owner or admin/
    );
    for (const role of ["owner", "admin"]) {
      await assertTenantScopeAccess(
        [scope],
        "user",
        "benchapp",
        async () => [role]
      );
    }
  }
});
