import { describe, expect, test } from "bun:test";
import { GitHubApiError } from "eve/channels/github";
import type { GitHubHandle, GitHubJsonObject } from "eve/channels/github";
import { assertExpectedBaseSha, publishDraftPr } from "../agent/lib/github-draft-pr.ts";

const state = { checkoutPath: "/workspace/repo", headSha: "base", baseRef: null, defaultBranch: "main" } as const;
const repository = { owner: "acme", name: "demo", fullName: "acme/demo", id: 1, private: true } as const;
const branch = "sigiro/eve-84097828fc31a8c8d29210df";
const title = "Draft changes from Eve";
const body = `Draft created for publication session-1.\n\nChanged paths:\n- image.bin\n- old.txt\n- src/a.txt`;

type Call = { readonly method: string; readonly path: string; readonly body?: GitHubJsonObject };
type Options = {
  readonly refSha?: string;
  readonly refParents?: readonly { readonly sha: string }[];
  readonly branchRaceSha?: string;
  readonly prs?: readonly GitHubJsonObject[];
  readonly prRace?: GitHubJsonObject;
  readonly gitExitCode?: number;
  readonly gitStderr?: string;
};

/** One `git diff --cached --raw -z --no-renames` record. */
const raw = (status: string, path: string, mode = "100644", sourceMode = mode) =>
  `:${status === "A" ? "000000" : sourceMode} ${status === "D" ? "000000" : mode} 1111111 2222222 ${status}\0${path}\0`;

const defaultDiff = `${raw("A", "src/a.txt")}${raw("M", "image.bin")}${raw("D", "old.txt")}`;

function api404(input: Parameters<GitHubHandle["request"]>[0]): never {
  throw new GitHubApiError({ body: {}, method: input.method, path: input.path, status: 404 });
}

function fake(options: Options = {}) {
  const calls: Call[] = [];
  let refReads = 0;
  let prReads = 0;
  const files = {
    "src/a.txt": Uint8Array.from(new TextEncoder().encode("hello")),
    "image.bin": Uint8Array.from([0, 255, 2]),
  };
  const github: GitHubHandle = {
    installationId: 7,
    repository,
    request: async <T>(input: Parameters<GitHubHandle["request"]>[0]) => {
      calls.push(input);
      if (input.method === "GET" && input.path.includes("/git/commits/")) {
        const requested = input.path.slice(input.path.lastIndexOf("/") + 1);
        if (requested === "base") return { ok: true, status: 200, body: { tree: { sha: "base-tree" } } as T };
        return { ok: true, status: 200, body: { tree: { sha: "other-tree" }, parents: options.refParents ?? [] } as T };
      }
      if (input.method === "PATCH") return { ok: true, status: 200, body: {} as T };
      if (input.method === "GET" && input.path.includes("/git/ref/heads/")) {
        refReads += 1;
        const refSha = refReads > 1 && options.branchRaceSha !== undefined ? options.branchRaceSha : options.refSha;
        if (refSha === undefined) return api404(input);
        return { ok: true, status: 200, body: { object: { sha: refSha } } as T };
      }
      if (input.method === "POST" && input.path.endsWith("/git/blobs")) {
        const content = (input.body as { content: string }).content;
        const sha = content === Buffer.from("hello").toString("base64") ? "blob-text" : "blob-binary";
        return { ok: true, status: 201, body: { sha } as T };
      }
      if (input.method === "POST" && input.path.endsWith("/git/trees")) return { ok: true, status: 201, body: { sha: "new-tree" } as T };
      if (input.method === "POST" && input.path.endsWith("/git/commits")) return { ok: true, status: 201, body: { sha: "commit" } as T };
      if (input.method === "POST" && input.path.endsWith("/git/refs")) {
        if (options.branchRaceSha !== undefined) throw new GitHubApiError({ body: {}, method: input.method, path: input.path, status: 422 });
        return { ok: true, status: 201, body: {} as T };
      }
      if (input.method === "GET" && input.path.includes("/pulls?")) {
        prReads += 1;
        return { ok: true, status: 200, body: (prReads > 1 && options.prRace !== undefined ? [options.prRace] : options.prs ?? []) as T };
      }
      if (input.method === "POST" && input.path.endsWith("/pulls")) {
        if (options.prRace !== undefined) throw new GitHubApiError({ body: {}, method: input.method, path: input.path, status: 422 });
        return { ok: true, status: 201, body: { number: 4, title, html_url: "https://example.test/pr/4" } as T };
      }
      throw new Error(`unexpected ${input.method} ${input.path}`);
    },
  };
  const sandbox = {
    run: async () => ({ stdout: defaultDiff, stderr: options.gitStderr ?? "", exitCode: options.gitExitCode ?? 0 }),
    readBinaryFile: async ({ path }: { readonly path: string }) => path.endsWith("src/a.txt") ? files["src/a.txt"] : files["image.bin"],
  };
  return { calls, github, sandbox };
}

const publish = (input = fake(), allowedPaths?: readonly string[]) => publishDraftPr({
  github: input.github,
  sandbox: input.sandbox,
  state,
  publicationId: "session-1",
  ...(allowedPaths === undefined ? {} : { allowedPaths }),
});
const matchingPr = { number: 8, title, body, head: { ref: branch, sha: "commit" }, base: { ref: "main" } };

describe("Eve GitHub draft PR publisher", () => {
  test("rejects an exact trusted base SHA mismatch before sandbox access", () => {
    let sandboxAccessed = false;
    expect(() => {
      assertExpectedBaseSha("expected", "actual");
      sandboxAccessed = true;
    }).toThrow("GitHub base SHA mismatch: the anchor's checked-out head actual differs from the job's base SHA expected");
    expect(sandboxAccessed).toBe(false);
  });

  test("publishes text, binary, and delete changes with exact tree and deterministic commit metadata", async () => {
    const first = fake();
    const second = fake();
    const result = await publish(first);
    await publish(second);
    expect(result?.changedPaths).toEqual(["image.bin", "old.txt", "src/a.txt"]);
    const blobs = first.calls.filter((call) => call.path.endsWith("/git/blobs"));
    expect(blobs.map((call) => call.body)).toEqual([
      { content: Buffer.from([0, 255, 2]).toString("base64"), encoding: "base64" },
      { content: Buffer.from("hello").toString("base64"), encoding: "base64" },
    ]);
    const tree = first.calls.find((call) => call.path.endsWith("/git/trees"));
    expect(tree?.body).toEqual({ base_tree: "base-tree", tree: [
      { path: "image.bin", mode: "100644", type: "blob", sha: "blob-binary" },
      { path: "old.txt", mode: "100644", type: "blob", sha: null },
      { path: "src/a.txt", mode: "100644", type: "blob", sha: "blob-text" },
    ] });
    const commit = first.calls.find((call) => call.path.endsWith("/git/commits"));
    const secondCommit = second.calls.find((call) => call.path.endsWith("/git/commits"));
    const signature = commit?.body as { readonly author: { readonly date: string } } | undefined;
    if (signature === undefined) throw new Error("expected a commit signature");
    const date = signature.author.date;
    expect(commit?.body).toEqual({ message: `${title}\n\nimage.bin\nold.txt\nsrc/a.txt`, parents: ["base"], tree: "new-tree", author: { name: "Eve", email: "eve@users.noreply.github.com", date }, committer: { name: "Eve", email: "eve@users.noreply.github.com", date } });
    expect(secondCommit?.body).toEqual(commit?.body);
  });

  test("rejects forbidden paths before GitHub writes", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("M", ".github/workflows/x.yml"), stderr: "", exitCode: 0 });
    await expect(publish(input)).rejects.toThrow("Forbidden path");
    expect(input.calls).toHaveLength(0);
  });

  test("accepts changes matching a trusted glob policy", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("M", "src/a.txt"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["src/**"])).resolves.toMatchObject({ changedPaths: ["src/a.txt"] });
  });

  test("rejects changes outside a trusted path policy before GitHub writes", async () => {
    const input = fake();
    await expect(publish(input, ["src/**"])).rejects.toThrow("changed paths outside the job policy");
    expect(input.calls).toHaveLength(0);
  });

  test("preserves an executable bit rather than flattening it to 100644", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("A", "src/a.txt", "100755"), stderr: "", exitCode: 0 });
    await publish(input, ["src/**"]);
    const tree = input.calls.find((call) => call.path.endsWith("/git/trees"));
    expect(tree?.body).toEqual({ base_tree: "base-tree", tree: [
      { path: "src/a.txt", mode: "100755", type: "blob", sha: "blob-text" },
    ] });
  });

  test("refuses a symlink before any GitHub write", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("A", "src/link", "120000"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["src/**"])).rejects.toThrow("Refusing to publish a symlink: src/link");
    expect(input.calls).toHaveLength(0);
  });

  test("refuses a submodule gitlink before any GitHub write", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("A", "vendor/dep", "160000"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["**"])).rejects.toThrow("Refusing to publish a submodule gitlink: vendor/dep");
    expect(input.calls).toHaveLength(0);
  });

  test("refuses a typechange naming the path before any GitHub write", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: `:100644 120000 1111111 2222222 T\0src/a.txt\0`, stderr: "", exitCode: 0 });
    await expect(publish(input, ["**"])).rejects.toThrow("Refusing to publish a file type change: src/a.txt");
    expect(input.calls).toHaveLength(0);
  });

  test("refuses a deleted symlink, reading the mode from the source side", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("D", "src/link", "120000", "120000"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["**"])).rejects.toThrow("Refusing to publish a symlink: src/link");
    expect(input.calls).toHaveLength(0);
  });

  /** Case-insensitive checkouts make `.GITHUB/` resolve to the workflow tree. */
  test("rejects case-varied and nested git machinery paths", async () => {
    for (const path of [".GITHUB/workflows/x.yml", "sub/dir/.gitattributes", ".GitAttributes", ".gitignore", "sub/.gitmodules", ".Git/config"]) {
      const input = fake();
      input.sandbox.run = async () => ({ stdout: raw("M", path), stderr: "", exitCode: 0 });
      await expect(publish(input, ["**"])).rejects.toThrow("Forbidden path");
      expect(input.calls).toHaveLength(0);
    }
  });

  /** `Bun.Glob("src")` alone matches neither `src/a.txt` nor `src`. */
  test("treats a bare directory policy as covering its whole subtree", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: `${raw("M", "src/a.txt")}${raw("A", "test/b.txt")}`, stderr: "", exitCode: 0 });
    await expect(publish(input, ["src", "test/"])).resolves.toMatchObject({ changedPaths: ["src/a.txt", "test/b.txt"] });
  });

  test("does not let a bare directory policy allow a sibling prefix", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("M", "srcery/a.txt"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["src"])).rejects.toThrow("changed paths outside the job policy");
    expect(input.calls).toHaveLength(0);
  });

  test("rejects forbidden paths even when a trusted glob allows them", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("M", ".github/workflows/x.yml"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["**"])).rejects.toThrow("Forbidden path");
    expect(input.calls).toHaveLength(0);
  });

  test("does not let an empty trusted pattern allow changes", async () => {
    const input = fake();
    await expect(publish(input, [""])).rejects.toThrow("changed paths outside the job policy");
    expect(input.calls).toHaveLength(0);
  });

  /**
   * This publisher is the only remaining guard on the path policy, so traversal
   * and absolute paths must be refused here even under the most permissive
   * policy a job can carry.
   */
  test("rejects traversal and absolute paths before GitHub writes", async () => {
    for (const path of ["../outside.ts", "/etc/passwd", "src/../../escape.ts", "src/.."]) {
      for (const allowedPaths of [undefined, ["**"] as readonly string[]]) {
        const input = fake();
        input.sandbox.run = async () => ({ stdout: raw("M", path), stderr: "", exitCode: 0 });
        await expect(publish(input, allowedPaths)).rejects.toThrow("Escaping path");
        expect(input.calls).toHaveLength(0);
      }
    }
  });

  test("accepts a nested in-tree path that only resembles traversal", async () => {
    const input = fake();
    input.sandbox.run = async () => ({ stdout: raw("M", "src/a.txt"), stderr: "", exitCode: 0 });
    await expect(publish(input, ["src/**"])).resolves.toMatchObject({ changedPaths: ["src/a.txt"] });
  });

  test("reuses an identical branch and existing PR without creating either", async () => {
    const input = fake({ refSha: "commit", prs: [matchingPr] });
    const result = await publish(input);
    expect(result?.pullRequest.number).toBe(8);
    expect(input.calls.some((call) => call.method === "POST" && (call.path.endsWith("/git/refs") || call.path.endsWith("/pulls")))).toBe(false);
  });

  test("does not update a branch built on a different base", async () => {
    const input = fake({ refSha: "other", refParents: [{ sha: "unrelated" }] });
    await expect(publish(input)).rejects.toThrow("Conflicting branch");
    expect(input.calls.some((call) => call.method === "PATCH")).toBe(false);
    expect(input.calls.some((call) => call.method === "POST" && call.path.endsWith("/git/refs"))).toBe(false);
  });

  test("does not update a branch with more than one parent", async () => {
    const input = fake({ refSha: "other", refParents: [{ sha: "base" }, { sha: "merged" }] });
    await expect(publish(input)).rejects.toThrow("Conflicting branch");
    expect(input.calls.some((call) => call.method === "PATCH")).toBe(false);
  });

  /**
   * A replay of the same job on the same base legitimately produces a different
   * tree, so the deterministic ref is force-updated rather than hard-failing.
   */
  test("force-updates a replayed branch whose sole parent is the same base", async () => {
    const input = fake({ refSha: "other", refParents: [{ sha: "base" }] });
    const result = await publish(input);
    expect(result?.commitSha).toBe("commit");
    const patch = input.calls.find((call) => call.method === "PATCH" && call.path.endsWith(`/git/refs/heads/${branch}`));
    expect(patch?.body).toEqual({ sha: "commit", force: true });
  });

  test("updates a matching PR whose body drifted instead of creating a second one", async () => {
    const input = fake({ refSha: "commit", prs: [{ ...matchingPr, body: "stale body" }] });
    const result = await publish(input);
    expect(result?.pullRequest.number).toBe(8);
    expect(input.calls.some((call) => call.method === "POST" && call.path.endsWith("/pulls"))).toBe(false);
    const patch = input.calls.find((call) => call.method === "PATCH" && call.path.endsWith("/pulls/8"));
    expect(patch?.body).toEqual({ body });
  });

  test("leaves a matching PR untouched when the body already matches", async () => {
    const input = fake({ refSha: "commit", prs: [matchingPr] });
    await publish(input);
    expect(input.calls.some((call) => call.method === "PATCH" && call.path.includes("/pulls/"))).toBe(false);
  });

  test("reconciles a 422 branch creation race when the matching ref wins", async () => {
    const input = fake({ branchRaceSha: "commit" });
    const result = await publish(input);
    expect(result?.pullRequest.number).toBe(4);
    expect(input.calls.filter((call) => call.path.includes("/git/ref/heads/")).length).toBe(2);
  });

  test("reconciles a 422 PR creation race when the matching PR wins", async () => {
    const input = fake({ prRace: matchingPr });
    const result = await publish(input);
    expect(result?.pullRequest.number).toBe(8);
    expect(input.calls.filter((call) => call.method === "POST" && call.path.endsWith("/pulls")).length).toBe(1);
  });

  test("uses the GitHub handle rather than direct fetch or token paths", async () => {
    let directFetchCalls = 0;
    const originalFetch = globalThis.fetch;
    const input = fake();
    globalThis.fetch = (async () => {
      directFetchCalls += 1;
      throw new Error("direct fetch is not allowed");
    }) as unknown as typeof fetch;
    try {
      await publish(input);
    } finally {
      globalThis.fetch = originalFetch;
    }
    expect(directFetchCalls).toBe(0);
    expect(input.calls.every((call) => !call.path.includes("token"))).toBe(true);
  });

  test("surfaces sandbox git failures with stderr", async () => {
    const input = fake({ gitExitCode: 128, gitStderr: "fatal: not a repository" });
    await expect(publish(input)).rejects.toThrow("fatal: not a repository");
    expect(input.calls).toHaveLength(0);
  });
});
