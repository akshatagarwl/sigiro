import type { BetterAuthPlugin } from "@better-auth/core";
import { DEVICE_CODE_GRANT_TYPE } from "@better-auth/oauth-provider";
import {
  APIError,
  createAuthEndpoint,
  createAuthMiddleware,
  getSessionFromCtx,
  sessionMiddleware,
} from "better-auth/api";
import * as z from "zod";

import { deviceTenantsForClient } from "./tenant-scope.js";

/**
 * Device login names the workspace it grants.
 *
 * Without this, the RFC 8628 grant carried no tenant: the `tid` claim was
 * derived after the fact from "the user's only membership", so a user who
 * belonged to two workspaces could not log in at all, and a user with one
 * auto-created empty workspace was logged into it with nothing on the
 * approval screen saying so.
 *
 * Now the approval screen lists the workspaces this client may receive, the
 * user confirms one, and that choice is bound to the device code. At token
 * exchange the binding becomes the token's `referenceId`, which OAuth
 * Provider persists on the refresh token, so every refresh keeps the same
 * workspace (and `auth.ts` re-checks membership on each one).
 */

interface Workspace {
  readonly id: string;
  readonly name: string;
  readonly slug: string;
  readonly role: string;
  /** Auto-created at first sign-in by `ensure-personal-organization`. */
  readonly personal: boolean;
}

interface DeviceCodeRecord {
  readonly id: string;
  readonly deviceCode: string;
  readonly userCode: string;
  readonly userId?: string | null;
  readonly status: string;
  readonly expiresAt: Date;
  readonly clientId?: string | null;
  readonly oauthClientId?: string | null;
}

// oxlint-disable-next-line typescript/no-explicit-any -- plugin hooks and endpoints share better-auth's untyped context
type AnyContext = any;

const bindingKey = (deviceCodeId: string): string =>
  `sigiro:device-tenant:${deviceCodeId}`;

const normalizeUserCode = (userCode: string): string =>
  userCode.replace(/[^a-zA-Z0-9]/g, "").toUpperCase();

const findByUserCode = async (
  ctx: AnyContext,
  userCode: string
): Promise<DeviceCodeRecord | null> => {
  for (const value of new Set([userCode, normalizeUserCode(userCode)])) {
    const record = (await ctx.context.adapter.findOne({
      model: "deviceCode",
      where: [{ field: "userCode", value }],
    })) as DeviceCodeRecord | null;
    if (record) return record;
  }
  return null;
};

const recordClientId = (record: DeviceCodeRecord): string | undefined =>
  record.oauthClientId ?? record.clientId ?? undefined;

/** The workspaces `userId` may grant to `clientId`, oldest membership first. */
const grantableWorkspaces = async (
  ctx: AnyContext,
  userId: string,
  clientId: string | undefined
): Promise<Workspace[]> => {
  const members = (await ctx.context.adapter.findMany({
    model: "member",
    where: [{ field: "userId", value: userId }],
    sortBy: { field: "createdAt", direction: "asc" },
  })) as { organizationId: string; role: string }[];
  const allowed = new Set(
    deviceTenantsForClient(
      clientId ?? "",
      members.map((member) => member.organizationId)
    )
  );
  const ids = members
    .map((member) => member.organizationId)
    .filter((id) => allowed.has(id));
  if (ids.length === 0) return [];
  const organizations = (await ctx.context.adapter.findMany({
    model: "organization",
    where: [{ field: "id", operator: "in", value: ids }],
  })) as { id: string; name: string; slug: string }[];
  const byId = new Map(organizations.map((org) => [org.id, org]));
  return members.flatMap((member) => {
    const org = byId.get(member.organizationId);
    if (!org || !allowed.has(org.id)) return [];
    return [
      {
        id: org.id,
        name: org.name,
        slug: org.slug,
        role: member.role,
        personal: org.slug.startsWith("personal-"),
      },
    ];
  });
};

/**
 * The workspace to pre-select. Only an unambiguous answer: the single grantable
 * workspace, or the single one that is not the auto-created personal
 * workspace. Anything else leaves the choice to the user.
 */
const suggestedWorkspace = (
  workspaces: ReadonlyArray<Workspace>
): string | null => {
  if (workspaces.length === 1) return workspaces[0]!.id;
  const shared = workspaces.filter((workspace) => !workspace.personal);
  return shared.length === 1 ? shared[0]!.id : null;
};

const requireClaimedRecord = async (
  ctx: AnyContext,
  userCode: string,
  userId: string
): Promise<DeviceCodeRecord> => {
  const record = await findByUserCode(ctx, userCode);
  if (!record || record.expiresAt < new Date()) {
    throw new APIError("BAD_REQUEST", {
      error: "invalid_request",
      error_description:
        "This device code is invalid or expired. Run `sigiro auth login` again.",
    });
  }
  if (record.userId && record.userId !== userId) {
    throw new APIError("FORBIDDEN", {
      error: "access_denied",
      error_description: "This device request belongs to another account.",
    });
  }
  return record;
};

const deviceWorkspaces = createAuthEndpoint(
  "/device/workspaces",
  {
    method: "GET",
    query: z.object({ user_code: z.string().min(1) }),
    use: [sessionMiddleware],
  },
  async (ctx) => {
    const session = ctx.context.session;
    const record = await requireClaimedRecord(
      ctx,
      ctx.query.user_code,
      session.user.id
    );
    const workspaces = await grantableWorkspaces(
      ctx,
      session.user.id,
      recordClientId(record)
    );
    const clientId = recordClientId(record);
    const client = clientId
      ? ((await ctx.context.adapter.findOne({
          model: "oauthClient",
          where: [{ field: "clientId", value: clientId }],
        })) as { name?: string | null } | null)
      : null;
    return ctx.json({
      client_name: client?.name ?? null,
      expires_at: record.expiresAt.toISOString(),
      workspaces,
      suggested: suggestedWorkspace(workspaces),
    });
  }
);

const bindWorkspaceOnApprove = createAuthMiddleware(async (ctx) => {
  const body = (ctx.body ?? {}) as {
    userCode?: unknown;
    organizationId?: unknown;
  };
  if (typeof body.userCode !== "string") return;
  const session = await getSessionFromCtx(ctx);
  if (!session) return; // the endpoint itself answers 401
  const record = await requireClaimedRecord(
    ctx,
    body.userCode,
    session.user.id
  );
  const workspaces = await grantableWorkspaces(
    ctx,
    session.user.id,
    recordClientId(record)
  );
  let organizationId: string;
  if (typeof body.organizationId === "string" && body.organizationId !== "") {
    if (!workspaces.some((workspace) => workspace.id === body.organizationId)) {
      throw new APIError("FORBIDDEN", {
        error: "access_denied",
        error_description:
          "You can't grant that workspace to this client. Pick one of your workspaces.",
      });
    }
    organizationId = body.organizationId;
  } else if (workspaces.length === 1) {
    // Nothing to choose: the CLI's `sigiro signup` approves right after it
    // provisions the user's only workspace.
    organizationId = workspaces[0]!.id;
  } else if (workspaces.length === 0) {
    throw new APIError("FORBIDDEN", {
      error: "access_denied",
      error_description:
        "Your account has no workspace this client can use. Ask a workspace owner to invite you.",
    });
  } else {
    throw new APIError("BAD_REQUEST", {
      error: "invalid_request",
      error_description: "Choose which workspace to connect before approving.",
    });
  }
  const ttl = Math.max(
    60,
    Math.ceil((record.expiresAt.getTime() - Date.now()) / 1000) + 60
  );
  const storage = ctx.context.secondaryStorage;
  if (!storage) throw new Error("deviceTenant() requires secondaryStorage");
  await storage.set(bindingKey(record.id), organizationId, ttl);
});

/** `referenceId` for a device-code exchange: the workspace bound at approval. */
const boundWorkspace = async (
  ctx: AnyContext
): Promise<{ recordId: string; organizationId: string } | null> => {
  const deviceCode = ctx.body?.device_code;
  if (typeof deviceCode !== "string" || deviceCode === "") return null;
  const record = (await ctx.context.adapter.findOne({
    model: "deviceCode",
    where: [{ field: "deviceCode", value: deviceCode }],
  })) as DeviceCodeRecord | null;
  if (!record || record.status !== "approved") return null;
  const organizationId = await ctx.context.secondaryStorage.get(
    bindingKey(record.id)
  );
  return typeof organizationId === "string" && organizationId !== ""
    ? { recordId: record.id, organizationId }
    : null;
};

interface GrantInput {
  readonly ctx: AnyContext;
  readonly provider: {
    issueTokens: (params: Record<string, unknown>) => Promise<unknown>;
  } & Record<string, unknown>;
  readonly [key: string]: unknown;
}
type GrantHandler = (input: GrantInput) => Promise<unknown>;
interface GrantExtension {
  grants?: Record<string, GrantHandler>;
}

const wrapped = new WeakSet<GrantHandler>();

export const deviceTenant = () =>
  ({
    id: "sigiro-device-tenant",
    endpoints: { deviceWorkspaces },
    hooks: {
      before: [
        {
          matcher: (context) => context.path === "/device/approve",
          handler: bindWorkspaceOnApprove,
        },
      ],
    },
    init(ctx) {
      const provider = ctx.getPlugin("oauth-provider") as {
        options: { extensions?: GrantExtension[] };
      } | null;
      const extensions = provider?.options.extensions ?? [];
      const index = extensions.findIndex(
        (extension) => extension.grants?.[DEVICE_CODE_GRANT_TYPE]
      );
      const extension = extensions[index];
      const exchange = extension?.grants?.[DEVICE_CODE_GRANT_TYPE];
      if (!extension || !exchange) {
        throw new Error(
          "deviceTenant() must come after oauthDeviceAuthorization()"
        );
      }
      if (wrapped.has(exchange)) return;
      const bound: GrantHandler = async (input) => {
        const binding = await boundWorkspace(input.ctx);
        if (!binding) return exchange(input);
        const tokens = await exchange({
          ...input,
          provider: {
            ...input.provider,
            issueTokens: (params) =>
              input.provider.issueTokens({
                ...params,
                referenceId: binding.organizationId,
              }),
          },
        });
        await input.ctx.context.secondaryStorage.delete(
          bindingKey(binding.recordId)
        );
        return tokens;
      };
      wrapped.add(bound);
      extensions[index] = {
        ...extension,
        grants: { ...extension.grants, [DEVICE_CODE_GRANT_TYPE]: bound },
      };
    },
  }) satisfies BetterAuthPlugin;
