import { describe, expect, test } from "bun:test";
import celldChannel from "../agent/channels/celld.ts";
import githubChannel, { CELLD_AUTHENTICATOR, publishThenDelegate, publishTurn, trustedAttributes } from "../agent/channels/github.ts";
import {
  authForJob,
  authenticateWorker,
  celldJobSchema,
  dispatchCelldJob,
  messageForJob,
  parseCelldJobBody,
} from "../agent/lib/celld-dispatch.ts";
import { deterministicDraftBranch } from "../agent/lib/github-draft-pr.ts";
import type { PublicationReceipt, ReceiptSink } from "../agent/lib/publication-receipts.ts";

process.env.EVE_WORKER_TOKEN = "secret-token";

const job = {
  executionId: "exec-1",
  workerId: "worker-7",
  repository: "acme/demo",
  baseRef: "main",
  baseSha: "a".repeat(40),
  prompt: "Fix the failing test.",
  tests: [{ name: "unit", command: "bun test" }],
  // Bare directories: the publisher normalizes each to the path and its subtree.
  allowedPaths: ["src", "test"],
  github: { installationId: 42, issueNumber: 9 },
};

function request(body: unknown, token = "secret-token") {
  return new Request("http://example.test/internal/celld/job", {
    method: "POST",
    headers: { authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
}

const route = celldChannel.routes[0]!;
if (route.method !== "POST") throw new Error("expected POST route");
const routeHandler = route.handler as (request: Request, args: { readonly to: Parameters<typeof dispatchCelldJob>[1] }) => Promise<Response>;

function routeArgs(to: Parameters<typeof dispatchCelldJob>[1]) {
  return { to } as Parameters<typeof routeHandler>[1];
}

describe("Eve Celld ingress", () => {
  test("authenticates with timing-safe equal-length comparison and fails closed", () => {
    expect(authenticateWorker("Bearer secret-token", "secret-token")).toBe(true);
    expect(authenticateWorker("Bearer wrong-token", "secret-token")).toBe(false);
    expect(authenticateWorker(undefined, "secret-token")).toBe(false);
    const configured = process.env.EVE_WORKER_TOKEN;
    delete process.env.EVE_WORKER_TOKEN;
    expect(authenticateWorker("Bearer secret-token")).toBe(false);
    process.env.EVE_WORKER_TOKEN = configured;
    expect(() => authenticateWorker("Bearer secret-token", "secret-token")).not.toThrow("secret-token");
  });

  test("enforces schema bounds and the GitHub target union", () => {
    expect(celldJobSchema.safeParse(job).success).toBe(true);
    expect(celldJobSchema.safeParse({ ...job, baseSha: "A".repeat(40) }).success).toBe(false);
    expect(celldJobSchema.safeParse({ ...job, github: { installationId: 42, issueNumber: 1, pullRequestNumber: 2 } }).success).toBe(false);
    expect(celldJobSchema.safeParse({ ...job, github: { installationId: 42 } }).success).toBe(false);
    expect(celldJobSchema.safeParse({ ...job, prompt: "x".repeat(8_193) }).success).toBe(false);
    expect(parseCelldJobBody(new TextEncoder().encode(JSON.stringify({ ...job, allowedPaths: [] })))).toBeNull();
  });

  test("dispatches an issue target with exact trusted attributes and message", async () => {
    let seen: { channel: unknown; target: unknown; message: string; options: unknown } | undefined;
    const to = ((channel: unknown, target: unknown) => ({
      send: async (message: string, options: unknown) => {
        seen = { channel, target, message, options };
        return { id: "session-issue" };
      },
    })) as Parameters<typeof dispatchCelldJob>[1];
    await expect(dispatchCelldJob(job, to)).resolves.toEqual({ sessionId: "session-issue" });
    expect(seen?.channel).toBe(githubChannel);
    expect(seen?.target).toEqual({ installationId: 42, issueNumber: 9, owner: "acme", repo: "demo" });
    expect(seen?.options).toEqual({ auth: authForJob(job), turnPolicy: "queue" });
    expect(seen?.message).toBe(messageForJob(job));
    expect(seen?.message).toContain(`git rev-parse HEAD equals exactly ${job.baseSha}`);
    expect(seen?.message).toContain("Leave all changes uncommitted");
    expect(seen?.message).not.toContain("secret-token");
  });

  test("dispatches a pull request target", async () => {
    let target: unknown;
    const to = ((_: unknown, value: unknown) => ({ send: async () => { target = value; return { id: "pr-session" }; } })) as Parameters<typeof dispatchCelldJob>[1];
    await dispatchCelldJob({ ...job, github: { installationId: 42, pullRequestNumber: 12 } }, to);
    expect(target).toEqual({ installationId: 42, pullRequestNumber: 12, owner: "acme", repo: "demo" });
  });

  test("returns 502 without exposing handoff errors", async () => {
    const to = (() => ({ send: async () => { throw new Error("internal secret cause"); } })) as Parameters<typeof dispatchCelldJob>[1];
    const response = await routeHandler(request(job), routeArgs(to));
    expect(response.status).toBe(502);
    expect(await response.text()).toBe('{"error":"handoff failed"}');
  });

  test("rejects before dispatch for bad auth and invalid input", async () => {
    let calls = 0;
    const to = (() => ({ send: async () => { calls += 1; return { id: "never" }; } })) as Parameters<typeof dispatchCelldJob>[1];
    expect((await routeHandler(request(job, "bad"), routeArgs(to))).status).toBe(401);
    expect((await routeHandler(request({ ...job, baseSha: "bad" }), routeArgs(to))).status).toBe(400);
    expect((await routeHandler(new Request("http://example.test/internal/celld/job", { method: "POST", headers: { authorization: "Bearer secret-token", "content-length": "999999" }, body: "{}" }), routeArgs(to))).status).toBe(400);
    expect(calls).toBe(0);
  });

  test("returns 202 with session id after send resolves", async () => {
    const to = (() => ({ send: async () => ({ id: "session-accepted" }) })) as Parameters<typeof dispatchCelldJob>[1];
    const response = await routeHandler(request(job), routeArgs(to));
    expect(response.status).toBe(202);
    const payload = await response.json() as unknown;
    expect(payload).toEqual({ sessionId: "session-accepted" });
  });
});

const celldPrincipal = { attributes: authForJob(job).attributes, authenticator: CELLD_AUTHENTICATOR };
const humanPrincipal = { attributes: { user_login: "someone" }, authenticator: "github-webhook" };

/**
 * Eve's GitHub continuation token carries no execution nonce, so a later human
 * `@bot` comment on the same anchor delivers into the celld-initiated session
 * with only `auth.current` replaced. These tests pin the fail-closed rule.
 */
describe("Celld attribute trust", () => {
  test("trusts attributes only when the current turn is the celld worker", () => {
    expect(trustedAttributes({ current: celldPrincipal, initiator: celldPrincipal })).toEqual({ kind: "celld", attributes: celldPrincipal.attributes });
    expect(trustedAttributes({ current: humanPrincipal, initiator: celldPrincipal })).toEqual({ kind: "suppressed" });
    expect(trustedAttributes({ current: humanPrincipal, initiator: humanPrincipal })).toEqual({ kind: "human" });
    expect(trustedAttributes({ current: null, initiator: celldPrincipal })).toEqual({ kind: "suppressed" });
    expect(trustedAttributes({})).toEqual({ kind: "human" });
  });

  function harness(auth: Parameters<typeof publishTurn>[0]["session"]["auth"]) {
    const posts: string[] = [];
    const requests: { readonly method: string; readonly path: string }[] = [];
    let sandboxCalls = 0;
    const channel = {
      state: { checkoutPath: "/workspace/repo", headSha: job.baseSha, baseRef: "main", defaultBranch: "main" },
      github: {
        repository: { owner: "acme", name: "demo", fullName: "acme/demo", id: 1, private: true },
        request: async <T>(input: { readonly method: string; readonly path: string }) => {
          requests.push(input);
          if (input.path.endsWith("/git/commits")) return { ok: true, status: 201, body: { sha: "commit" } as T };
          if (input.path.includes("/git/commits/")) return { ok: true, status: 200, body: { tree: { sha: "base-tree" } } as T };
          if (input.path.endsWith("/git/blobs")) return { ok: true, status: 201, body: { sha: "blob" } as T };
          if (input.path.endsWith("/git/trees")) return { ok: true, status: 201, body: { sha: "tree" } as T };
          if (input.path.includes("/git/ref/heads/")) return { ok: true, status: 200, body: { object: { sha: "commit" } } as T };
          if (input.path.includes("/pulls?")) return { ok: true, status: 200, body: [] as T };
          if (input.path.endsWith("/pulls")) return { ok: true, status: 201, body: { number: 3, html_url: "https://example.test/pr/3" } as T };
          return { ok: true, status: 200, body: {} as T };
        },
      },
      thread: { post: async (message: string) => { posts.push(message); } },
    };
    const sandbox = {
      run: async () => { sandboxCalls += 1; return { stdout: `:000000 100644 1111111 2222222 A\0src/a.txt\0`, stderr: "", exitCode: 0 }; },
      readBinaryFile: async () => Uint8Array.from([1]),
    };
    const session = { id: "session-1", auth };
    // An in-memory sink keeps the channel tests off the filesystem; the store
    // itself is proven against real files in test/eve-publication-receipts.test.ts.
    const written: PublicationReceipt[] = [];
    const receipts: ReceiptSink = { write: async (receipt) => { written.push(receipt); } };
    return {
      channel, session, receipts, getSandbox: async () => sandbox, posts, requests,
      sandboxCalls: () => sandboxCalls, written,
    };
  }

  test("publishes nothing on a human turn inside a celld-initiated session", async () => {
    const harnessed = harness({ current: humanPrincipal, initiator: celldPrincipal });
    await publishTurn(harnessed);
    expect(harnessed.requests).toHaveLength(0);
    expect(harnessed.posts).toHaveLength(0);
    expect(harnessed.sandboxCalls()).toBe(0);
    // A suppressed turn is not this execution's publication, so it stamps nothing.
    expect(harnessed.written).toHaveLength(0);
  });

  test("publishes on a celld current turn using the trusted execution id", async () => {
    const harnessed = harness({ current: celldPrincipal, initiator: celldPrincipal });
    await publishTurn(harnessed);
    expect(harnessed.posts).toEqual(["Draft PR: https://example.test/pr/3"]);
    expect(harnessed.requests.some((call) => call.path.endsWith("/pulls"))).toBe(true);
    const created = harnessed.requests.find((call) => call.path.includes("/git/ref/heads/"));
    expect(created?.path).toContain(deterministicDraftBranch(job.executionId));
  });

  test("publishes a human-only session under the session publication id", async () => {
    const harnessed = harness({ current: humanPrincipal, initiator: humanPrincipal });
    await publishTurn(harnessed);
    expect(harnessed.posts).toEqual(["Draft PR: https://example.test/pr/3"]);
    const created = harnessed.requests.find((call) => call.path.includes("/git/ref/heads/"));
    expect(created?.path).toContain(deterministicDraftBranch("session-1"));
  });

  /**
   * The wrapper runs the delegate in a `finally`, so a failed publication still
   * lets a configured handler observe the turn.
   */
  test("runs the delegated turn.completed handler even when publishing throws", async () => {
    let delegated = 0;
    const harnessed = harness({ current: celldPrincipal, initiator: celldPrincipal });
    const failing = { ...harnessed, channel: { ...harnessed.channel, state: { ...harnessed.channel.state, headSha: "different-sha" } } };
    await expect(publishThenDelegate(() => publishTurn(failing), () => { delegated += 1; }))
      .rejects.toThrow("GitHub base SHA mismatch");
    expect(delegated).toBe(1);
  });

  test("refuses a celld turn whose anchor resolved a different base ref", async () => {
    const harnessed = harness({ current: celldPrincipal, initiator: celldPrincipal });
    const drifted = { ...harnessed, channel: { ...harnessed.channel, state: { ...harnessed.channel.state, baseRef: "release" } } };
    await expect(publishTurn(drifted)).rejects.toThrow("GitHub base ref mismatch");
    expect(harnessed.requests).toHaveLength(0);
  });

  /** An issue anchor leaves `state.baseRef` null, so the check cannot apply there. */
  test("publishes a celld turn on an issue anchor with no resolved base ref", async () => {
    const harnessed = harness({ current: celldPrincipal, initiator: celldPrincipal });
    const anchored = { ...harnessed, channel: { ...harnessed.channel, state: { ...harnessed.channel.state, baseRef: null } } };
    await publishTurn(anchored);
    expect(harnessed.posts).toEqual(["Draft PR: https://example.test/pr/3"]);
  });

  test("runs the delegate after a successful publication", async () => {
    let delegated = 0;
    const harnessed = harness({ current: celldPrincipal, initiator: celldPrincipal });
    await publishThenDelegate(() => publishTurn(harnessed), () => { delegated += 1; });
    expect(delegated).toBe(1);
    expect(harnessed.posts).toHaveLength(1);
  });
});

/**
 * Eve's `callAdapterEventHandler` wraps an authored handler in `try`/`catch`,
 * logs a throw, and stamps `turn.completed` regardless. The throw below is
 * therefore invisible to the worker's stream; the receipt is the evidence that
 * survives, so it has to be written before the throw is re-raised.
 */
describe("Publication receipts", () => {
  function harness(auth: Parameters<typeof publishTurn>[0]["session"]["auth"], overrides: {
    readonly headSha?: string;
    readonly staged?: string;
    readonly testExitCode?: number;
  } = {}) {
    const posts: string[] = [];
    const written: PublicationReceipt[] = [];
    let sandboxRuns = 0;
    const channel = {
      state: {
        checkoutPath: "/workspace/repo",
        headSha: overrides.headSha ?? job.baseSha,
        baseRef: "main",
        defaultBranch: "main",
      },
      github: {
        repository: { owner: "acme", name: "demo", fullName: "acme/demo", id: 1, private: true },
        request: async <T>(input: { readonly method: string; readonly path: string }) => {
          if (input.path.endsWith("/git/commits")) return { ok: true, status: 201, body: { sha: "commit-sha" } as T };
          if (input.path.includes("/git/commits/")) return { ok: true, status: 200, body: { tree: { sha: "base-tree" } } as T };
          if (input.path.endsWith("/git/blobs")) return { ok: true, status: 201, body: { sha: "blob" } as T };
          if (input.path.endsWith("/git/trees")) return { ok: true, status: 201, body: { sha: "tree" } as T };
          if (input.path.includes("/git/ref/heads/")) return { ok: true, status: 200, body: { object: { sha: "commit-sha" } } as T };
          if (input.path.includes("/pulls?")) return { ok: true, status: 200, body: [] as T };
          if (input.path.endsWith("/pulls")) return { ok: true, status: 201, body: { number: 3, html_url: "https://example.test/pr/3" } as T };
          return { ok: true, status: 200, body: {} as T };
        },
      },
      thread: { post: async (message: string) => { posts.push(message); } },
    };
    const sandbox = {
      run: async () => {
        sandboxRuns += 1;
        // The first run is the required test; the git diff follows it.
        if (sandboxRuns === 1 && overrides.testExitCode !== undefined) {
          return { stdout: "", stderr: "boom", exitCode: overrides.testExitCode };
        }
        return { stdout: overrides.staged ?? ":000000 100644 1111111 2222222 A\0src/a.txt\0", stderr: "", exitCode: 0 };
      },
      readBinaryFile: async () => Uint8Array.from([1]),
    };
    const receipts: ReceiptSink = { write: async (receipt) => { written.push(receipt); } };
    return { channel, session: { id: "session-1", auth }, receipts, getSandbox: async () => sandbox, posts, written };
  }

  const celldAuth = { current: celldPrincipal, initiator: celldPrincipal };

  test("writes a published receipt carrying the publication", async () => {
    const harnessed = harness(celldAuth);
    await publishTurn(harnessed);
    expect(harnessed.written).toHaveLength(1);
    const receipt = harnessed.written[0];
    expect(receipt?.status).toBe("published");
    expect(receipt?.executionId).toBe(job.executionId);
    expect(receipt?.sessionId).toBe("session-1");
    expect(receipt?.headSha).toBe(job.baseSha);
    expect(receipt?.branch).toBe(deterministicDraftBranch(job.executionId));
    expect(receipt?.commitSha).toBe("commit-sha");
    expect(receipt?.changedPaths).toEqual(["src/a.txt"]);
    expect(receipt?.pullRequestNumber).toBe(3);
    expect(receipt?.pullRequestUrl).toBe("https://example.test/pr/3");
    expect(receipt?.error).toBeUndefined();
    expect(new Date(receipt?.updatedAt ?? "").getTime()).toBeGreaterThan(0);
  });

  test("writes a failed receipt when the SHA check refuses the turn, then rethrows", async () => {
    const harnessed = harness(celldAuth, { headSha: "b".repeat(40) });
    await expect(publishTurn(harnessed)).rejects.toThrow("GitHub base SHA mismatch");
    expect(harnessed.written).toHaveLength(1);
    expect(harnessed.written[0]?.status).toBe("failed");
    expect(harnessed.written[0]?.error).toContain("GitHub base SHA mismatch");
    expect(harnessed.written[0]?.executionId).toBe(job.executionId);
  });

  test("writes a failed receipt when a required test exits non-zero", async () => {
    const harnessed = harness(celldAuth, { testExitCode: 1 });
    await expect(publishTurn(harnessed)).rejects.toThrow("Required test failed");
    expect(harnessed.written).toHaveLength(1);
    expect(harnessed.written[0]?.status).toBe("failed");
    expect(harnessed.written[0]?.error).toContain("Required test failed: unit (exit 1)");
    expect(harnessed.posts).toHaveLength(0);
  });

  /** The no-changes celld case already threw; it becomes a failed receipt too. */
  test("writes a failed receipt when a celld publication produced no changes", async () => {
    const harnessed = harness(celldAuth, { staged: "" });
    await expect(publishTurn(harnessed)).rejects.toThrow("Celld publication produced no changes");
    expect(harnessed.written).toHaveLength(1);
    expect(harnessed.written[0]?.status).toBe("failed");
    expect(harnessed.written[0]?.error).toBe("Celld publication produced no changes");
  });

  test("writes a failed receipt when a forbidden path is refused", async () => {
    const harnessed = harness(celldAuth, { staged: ":000000 100644 1111111 2222222 A\0.github/workflows/ci.yml\0" });
    await expect(publishTurn(harnessed)).rejects.toThrow("Forbidden path");
    expect(harnessed.written[0]?.status).toBe("failed");
    expect(harnessed.written[0]?.error).toContain(".github/workflows/ci.yml");
  });

  /** A human session has no execution id, so there is nothing for a worker to poll. */
  test("writes no receipt for a human session", async () => {
    const harnessed = harness({ current: humanPrincipal, initiator: humanPrincipal });
    await publishTurn(harnessed);
    expect(harnessed.posts).toHaveLength(1);
    expect(harnessed.written).toHaveLength(0);
  });

  test("writes no receipt for a celld turn carrying no execution id", async () => {
    const { executionId: _dropped, ...rest } = celldPrincipal.attributes;
    const harnessed = harness({
      current: { attributes: rest, authenticator: CELLD_AUTHENTICATOR },
      initiator: celldPrincipal,
    });
    await publishTurn(harnessed);
    expect(harnessed.posts).toHaveLength(1);
    expect(harnessed.written).toHaveLength(0);
  });

  /** The receipt is written before the throw escapes, so the delegate still runs. */
  test("stamps the receipt before the wrapper's delegate observes the throw", async () => {
    const order: string[] = [];
    const harnessed = harness(celldAuth, { headSha: "b".repeat(40) });
    const observing = {
      ...harnessed,
      receipts: { write: async (receipt: PublicationReceipt) => { order.push(`receipt:${receipt.status}`); } },
    };
    await expect(publishThenDelegate(() => publishTurn(observing), () => { order.push("delegate"); }))
      .rejects.toThrow("GitHub base SHA mismatch");
    expect(order).toEqual(["receipt:failed", "delegate"]);
  });
});

describe("Eve Celld ingress dispatch attributes", () => {
  test("carries the trusted base ref for the publisher to check", () => {
    expect(authForJob(job).attributes.baseRef).toBe(job.baseRef);
  });

  test("keeps a 202 path unchanged", async () => {
    const to = (() => ({ send: async () => ({ id: "session-accepted" }) })) as Parameters<typeof dispatchCelldJob>[1];
    const response = await routeHandler(request(job), routeArgs(to));
    expect(response.status).toBe(202);
    const payload = await response.json() as unknown;
    expect(payload).toEqual({ sessionId: "session-accepted" });
  });
});
