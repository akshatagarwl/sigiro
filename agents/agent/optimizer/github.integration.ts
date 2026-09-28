import assert from "node:assert/strict";
import { createSign } from "node:crypto";

import { Effect } from "effect";
import { createEmulator } from "emulate";

import { TrustedGitHub } from "./github.ts";

const github = await createEmulator({
  service: "github",
  port: 0,
  seed: {
    github: {
      users: [{ login: "octocat" }],
      repos: [{ owner: "octocat", name: "sample", auto_init: true }],
      apps: [
        {
          app_id: 12345,
          slug: "sigiro",
          name: "Sigiro",
          permissions: {
            contents: "write",
            pull_requests: "write",
            metadata: "read",
          },
          installations: [
            {
              installation_id: 100,
              account: "octocat",
              repository_selection: "all",
            },
          ],
        },
      ],
    },
  },
});
try {
  const key = github.generatedSecrets.find(
    (secret) =>
      secret.kind === "github.app_private_key" && secret.id === "12345"
  )?.value;
  assert.ok(key);
  const now = Math.floor(Date.now() / 1000);
  const encode = (data: object) =>
    Buffer.from(JSON.stringify(data)).toString("base64url");
  const data = `${encode({ alg: "RS256", typ: "JWT" })}.${encode({ iss: "12345", iat: now - 60, exp: now + 540 })}`;
  const jwt = `${data}.${createSign("RSA-SHA256").update(data).sign(key, "base64url")}`;
  const tokenResponse = await fetch(
    `${github.url}/app/installations/100/access_tokens`,
    { method: "POST", headers: { Authorization: `Bearer ${jwt}` } }
  );
  assert.equal(tokenResponse.status, 201);
  const { token } = (await tokenResponse.json()) as { token: string };
  const get = async (path: string) => {
    const response = await fetch(`${github.url}${path}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    assert.equal(response.status, 200);
    return response.json();
  };
  const repo = await get("/repos/octocat/sample");
  const base = await get(
    `/repos/octocat/sample/git/ref/heads/${repo.default_branch}`
  );
  const service = new TrustedGitHub({
    apiUrl: github.url,
    appId: "12345",
    privateKey: key,
  });
  const input = {
    connection: {
      repositoryId: repo.id as number,
      owner: "octocat",
      repo: "sample",
      installationId: 100,
    },
    runId: "12b9c846-1890-4e1e-8b1b-1f0e19f45a45",
    baseRef: repo.default_branch as string,
    targetCommitSha: base.object.sha as string,
    changes: [{ path: "optimization.txt", content: "measured improvement\n" }],
    title: "optimizer: reduce latency",
    body: "Validation: not run. Draft only.",
  };
  assert.deepEqual(
    await Effect.runPromise(service.resolveTarget(input.connection)),
    {
      baseRef: input.baseRef,
      targetCommitSha: input.targetCommitSha,
    }
  );
  const unmodifiedFetch = globalThis.fetch;
  let activeBlobs = 0;
  let peakBlobs = 0;
  globalThis.fetch = async (url, options) => {
    const route = String(url);
    if (route.includes("/git/trees/") && route.includes("recursive=1")) {
      const response = await unmodifiedFetch(url, options);
      const tree = await response.json() as { tree: Array<{ path: string; mode: string; type: string; sha: string }> };
      const fixture = tree.tree.find((entry) => entry.type === "blob" && entry.mode === "100644");
      assert.ok(fixture);
      tree.tree.push(...Array.from({ length: 12 }, (_, i) => ({ ...fixture, path: `parallel/${i}.txt` })));
      return new Response(JSON.stringify(tree), { status: response.status, headers: response.headers });
    }
    if (route.includes("/git/blobs/")) {
      activeBlobs++;
      peakBlobs = Math.max(peakBlobs, activeBlobs);
      try {
        await new Promise((resolve) => setTimeout(resolve, 25));
        return await unmodifiedFetch(url, options);
      } finally { activeBlobs--; }
    }
    return unmodifiedFetch(url, options);
  };
  let checkout: readonly { path: string; content: Uint8Array }[] = [];
  try { checkout = await Effect.runPromise(service.checkout(input.connection, input.targetCommitSha)); }
  finally { globalThis.fetch = unmodifiedFetch; }
  assert.equal(peakBlobs, 6, "GitHub checkout must cap concurrent blob fetches");
  assert.ok(checkout.some((file) => file.path === "README.md"));
  assert.ok(checkout.every((file) => !Buffer.from(file.content).toString("utf8").includes(token)));
  assert.equal(await Effect.runPromise(service.reconcile(input)), null);
  const first = await Effect.runPromise(service.publish(input));
  assert.deepEqual(await Effect.runPromise(service.reconcile(input)), first);
  const second = await Effect.runPromise(service.publish(input));
  assert.deepEqual(second, first);
  const pulls = await get("/repos/octocat/sample/pulls?state=all");
  assert.equal(pulls.length, 1);
  assert.equal(pulls[0].draft, true);
  const interrupted = {
    ...input,
    runId: "32b9c846-1890-4e1e-8b1b-1f0e19f45a45",
  };
  const originalFetch = globalThis.fetch;
  let lostResponse = false;
  globalThis.fetch = async (url, options) => {
    const response = await originalFetch(url, options);
    if (
      !lostResponse &&
      options?.method === "POST" &&
      String(url).endsWith("/repos/octocat/sample/pulls")
    ) {
      lostResponse = true;
      throw new Error("connection lost after PR creation");
    }
    return response;
  };
  try {
    await assert.rejects(Effect.runPromise(service.publish(interrupted)));
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.equal(lostResponse, true);
  const recovered = await Effect.runPromise(service.publish(interrupted));
  assert.equal(recovered.branch, `sigiro/optimizer-${interrupted.runId}`);
  assert.equal((await get("/repos/octocat/sample/pulls?state=all")).length, 2);
  await assert.rejects(
    Effect.runPromise(
      service.publish({
        ...input,
        connection: { ...input.connection, repositoryId: -1 },
        runId: "22b9c846-1890-4e1e-8b1b-1f0e19f45a45",
      })
    )
  );
  await assert.rejects(
    Effect.runPromise(
      service.publish({
        ...input,
        changes: [{ path: "../bad", content: "no" }],
      })
    )
  );
  assert.equal((await get("/repos/octocat/sample/pulls?state=all")).length, 2);
  const removed = await Effect.runPromise(service.publish({
    ...input, runId: "42b9c846-1890-4e1e-8b1b-1f0e19f45a45",
    changes: [{ path: "README.md", content: null }], title: "optimizer: remove obsolete documentation",
  }));
  assert.ok(removed.pullRequestId > 0);
  const closed = await fetch(
    `${github.url}/repos/octocat/sample/pulls/${pulls[0].number}`,
    {
      method: "PATCH",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ state: "closed" }),
    }
  );
  assert.equal(closed.status, 200);
  await assert.rejects(Effect.runPromise(service.reconcile(input)));
  await assert.rejects(Effect.runPromise(service.publish(input)));
  assert.equal((await get("/repos/octocat/sample/pulls?state=all")).length, 3);
  console.log(
    "GitHub App draft PR, recovery, ownership and unsafe path checks passed"
  );
} finally {
  await github.close();
}
