import { createHash } from "node:crypto";
import { matchesGlob } from "node:path";
import { GitHubApiError } from "eve/channels/github";
import type { GitHubHandle, GitHubJsonObject } from "eve/channels/github";
import type { SandboxSession } from "eve/sandbox";

/** The only blob modes GitHub tree writes may carry here. */
type BlobMode = "100644" | "100755";

interface ChangedFile {
  readonly path: string;
  readonly status: "A" | "M" | "D";
  readonly mode: BlobMode;
  readonly bytes?: Uint8Array;
}

interface CommitResponse {
  readonly sha: string;
}

interface RefResponse {
  readonly object: { readonly sha: string };
}

interface GitCommitResponse {
  readonly parents?: readonly { readonly sha: string }[];
  readonly tree: { readonly sha: string };
}

interface GitTreeResponse {
  readonly sha: string;
}

interface PullRequestResponse {
  readonly base?: { readonly ref?: string };
  readonly body?: string | null;
  readonly draft?: boolean | null;
  readonly head?: { readonly ref?: string; readonly sha?: string };
  readonly html_url?: string;
  readonly number: number;
  readonly title: string;
}

export interface DraftPrPublishState {
  readonly checkoutPath: string | null;
  readonly headSha: string | null;
  readonly baseRef: string | null;
  readonly defaultBranch: string | null;
}

export interface DraftPrPublishInput {
  readonly github: Pick<GitHubHandle, "request" | "repository">;
  readonly sandbox: Pick<SandboxSession, "run" | "readBinaryFile">;
  readonly state: DraftPrPublishState;
  readonly publicationId: string;
  /** Trusted celld job scope. Omitted for direct human GitHub sessions. */
  readonly allowedPaths?: readonly string[];
  readonly tests?: readonly { readonly name: string; readonly command: string; readonly exitCode: number }[];
}

export interface DraftPrPublishResult {
  readonly branch: string;
  readonly commitSha: string;
  readonly changedPaths: readonly string[];
  readonly pullRequest: { readonly number: number; readonly htmlUrl?: string };
}

export const deterministicDraftBranch = (publicationId: string): string =>
  `sigiro/eve-${hash(publicationId).slice(0, 24)}`;

/** Path segments refused anywhere in the tree, matched case-insensitively. */
const forbiddenLeaves = new Set([".gitattributes", ".gitmodules"]);

/**
 * Git and CI machinery is refused whatever the job policy allows. Matching is
 * case-insensitive because case-insensitive checkouts let `.GITHUB/` resolve to
 * the same tree entry GitHub reads for workflow definitions.
 */
const forbidden = (path: string): boolean => {
  const lower = path.toLowerCase();
  const prefixed = (root: string): boolean => lower === root || lower.startsWith(`${root}/`);
  if (prefixed(".github/workflows") || prefixed(".github/actions") || prefixed(".git") ||
      prefixed(".sigiro-test-home") || lower === ".gitignore") return true;
  const leaf = lower.slice(lower.lastIndexOf("/") + 1);
  return forbiddenLeaves.has(leaf);
};

/**
 * This publisher is the only enforcement point for the path policy: the trusted
 * supervisor no longer inspects a local checkout, so traversal and absolute
 * paths must be refused here, before any GitHub write.
 */
const escaping = (path: string): boolean =>
  path.length === 0 || path.startsWith("/") || path === ".." ||
  path.startsWith("../") || path.includes("/../") || path.endsWith("/..");

/**
 * A pattern with no glob metacharacters names a literal path, and a bare
 * directory must cover its subtree: `Bun.Glob("src")` alone does not match
 * `src/a.ts`, which would deny every change under a directory policy.
 */
const matchesPattern = (path: string, pattern: string): boolean => {
  if (!/[*?[\]{}!]/.test(pattern)) {
    const trimmed = pattern.endsWith("/") ? pattern.slice(0, -1) : pattern;
    return path === trimmed || path.startsWith(`${trimmed}/`);
  }
  return matchesGlob(path, pattern);
};

const matchesAllowed = (path: string, allowedPaths: readonly string[]): boolean =>
  allowedPaths.some((pattern) => pattern.length > 0 && matchesPattern(path, pattern));

const disallowed = (path: string, allowedPaths: readonly string[] | undefined): boolean =>
  escaping(path) || forbidden(path) ||
  (allowedPaths !== undefined && !matchesAllowed(path, allowedPaths));

const quote = (value: string): string => `'${value.replaceAll("'", "'\\''")}'`;

const isStatus = (value: string): value is ChangedFile["status"] => value === "A" || value === "M" || value === "D";

/**
 * Maps a git raw-format destination mode onto a publishable blob mode. Symlinks
 * and gitlinks are refused rather than flattened: publishing either as a
 * regular file would silently change what the tree means.
 */
const blobMode = (mode: string, path: string): BlobMode => {
  if (mode === "100644" || mode === "100755") return mode;
  if (mode === "120000") throw new Error(`Refusing to publish a symlink: ${path}`);
  if (mode === "160000") throw new Error(`Refusing to publish a submodule gitlink: ${path}`);
  throw new Error(`Unsupported git file mode ${mode}: ${path}`);
};

interface RawChange {
  readonly path: string;
  readonly status: ChangedFile["status"];
  readonly mode: BlobMode;
}

/**
 * Parses `git diff --cached --raw -z --no-renames` records, which take the form
 * `:<srcmode> <dstmode> <srcsha> <dstsha> <status>\0<path>\0`. Deletions carry
 * destination mode `000000`, so the source mode names the blob being removed.
 */
const parseRawDiff = (stdout: string): RawChange[] => {
  const fields = stdout.split("\0").filter((field) => field.length > 0);
  const changes: RawChange[] = [];
  for (let index = 0; index < fields.length; index += 2) {
    const record = fields[index];
    const path = fields[index + 1];
    if (record === undefined || path === undefined) throw new Error("Malformed staged git diff record");
    const parts = record.replace(/^:/, "").split(" ");
    const [sourceMode, destinationMode, , , status] = parts;
    if (sourceMode === undefined || destinationMode === undefined || status === undefined || parts.length !== 5) {
      throw new Error(`Malformed staged git diff record: ${path}`);
    }
    if (status === "T") throw new Error(`Refusing to publish a file type change: ${path}`);
    if (!isStatus(status)) throw new Error(`Unsupported staged git status ${status}: ${path}`);
    changes.push({ path, status, mode: blobMode(status === "D" ? sourceMode : destinationMode, path) });
  }
  return changes;
};

const apiErrorStatus = (error: unknown): number | undefined => error instanceof GitHubApiError ? error.status : undefined;

const hash = (value: string): string => createHash("sha256").update(value).digest("hex");

const deterministicDate = (baseSha: string, publicationId: string): string => {
  const seconds = Number.parseInt(hash(`${baseSha}\0${publicationId}`).slice(0, 8), 16) % 946080000;
  return new Date(Date.UTC(2000, 0, 1) + seconds * 1000).toISOString();
};

const requestBody = async <T>(github: Pick<GitHubHandle, "request">, input: { readonly method: Parameters<GitHubHandle["request"]>[0]["method"]; readonly path: string; readonly body?: GitHubJsonObject }): Promise<T> =>
  (await github.request<T>(input)).body;

const runGit = async (sandbox: Pick<SandboxSession, "run">, command: string) => {
  const result = await sandbox.run({ command });
  if (result.exitCode !== 0) {
    const detail = result.stderr.trim();
    throw new Error(detail.length > 0 ? detail : `Git command failed (exit code ${result.exitCode})`);
  }
  return result;
};

const changedFiles = async (sandbox: Pick<SandboxSession, "run" | "readBinaryFile">, checkoutPath: string, allowedPaths?: readonly string[]): Promise<ChangedFile[]> => {
  const result = await runGit(sandbox, `git -C ${quote(checkoutPath)} add -A -- . && git -C ${quote(checkoutPath)} diff --cached --raw -z --no-renames`);
  const staged = parseRawDiff(result.stdout);
  const rejected = staged.filter(({ path }) => disallowed(path, allowedPaths)).map(({ path }) => path);
  if (rejected.length > 0) {
    const forbiddenPaths = staged.filter(({ path }) => forbidden(path)).map(({ path }) => path);
    const escapingPaths = staged.filter(({ path }) => escaping(path)).map(({ path }) => path);
    const reason = forbiddenPaths.length > 0 ? "Forbidden path"
      : escapingPaths.length > 0 ? "Escaping path"
      : "changed paths outside the job policy";
    throw new Error(`${reason}: ${rejected.slice(0, 20).join(", ")}`);
  }
  const files: ChangedFile[] = [];
  for (const { path, status, mode } of staged) {
    if (status === "D") files.push({ path, status, mode });
    else {
      const bytes = await sandbox.readBinaryFile({ path: `${checkoutPath}/${path}` });
      if (bytes === null) throw new Error(`Missing changed file: ${path}`);
      files.push({ path, status, mode, bytes });
    }
  }
  return files.sort((left, right) => left.path.localeCompare(right.path));
};

const isNotFound = (error: unknown): boolean => apiErrorStatus(error) === 404;

export async function publishDraftPr(input: DraftPrPublishInput): Promise<DraftPrPublishResult | null> {
  const { state } = input;
  if (state.checkoutPath === null || state.headSha === null || (state.baseRef === null && state.defaultBranch === null)) return null;
  const files = await changedFiles(input.sandbox, state.checkoutPath, input.allowedPaths);
  if (files.length === 0) return null;

  const repository = input.github.repository;
  const prefix = `/repos/${repository.owner}/${repository.name}`;
  const baseRef = state.baseRef ?? state.defaultBranch;
  if (baseRef === null) return null;
  const branch = deterministicDraftBranch(input.publicationId);
  const title = "Draft changes from Eve";
  const testLines = input.tests === undefined ? [] : ["", "Tests:", ...input.tests.map((test) => `- ${test.name}: ${test.command} (exit ${test.exitCode})`)];
  const body = `Draft created for publication ${input.publicationId}.\n\nChanged paths:\n${files.map((file) => `- ${file.path}`).join("\n")}${testLines.join("\n")}`;

  const baseCommit = await requestBody<GitCommitResponse>(input.github, { method: "GET", path: `${prefix}/git/commits/${state.headSha}` });
  const treeEntries: GitHubJsonObject[] = [];
  for (const file of files) {
    if (file.status === "D") {
      treeEntries.push({ path: file.path, mode: file.mode, type: "blob", sha: null });
      continue;
    }
    const blob = await requestBody<{ readonly sha: string }>(input.github, {
      method: "POST", path: `${prefix}/git/blobs`, body: { content: Buffer.from(file.bytes ?? new Uint8Array()).toString("base64"), encoding: "base64" },
    });
    treeEntries.push({ path: file.path, mode: file.mode, type: "blob", sha: blob.sha });
  }
  const tree = await requestBody<GitTreeResponse>(input.github, {
    method: "POST", path: `${prefix}/git/trees`, body: { base_tree: baseCommit.tree.sha, tree: treeEntries },
  });
  const date = deterministicDate(state.headSha, input.publicationId);
  const commit = await requestBody<CommitResponse>(input.github, {
    method: "POST", path: `${prefix}/git/commits`, body: {
      message: `${title}\n\n${files.map((file) => file.path).join("\n")}`,
      parents: [state.headSha], tree: tree.sha,
      author: { name: "Eve", email: "eve@users.noreply.github.com", date },
      committer: { name: "Eve", email: "eve@users.noreply.github.com", date },
    },
  });

  /**
   * A replay that produced a different tree is still the same job on the same
   * base, so the deterministic branch is force-updated to the new commit. A
   * branch built on any other base is left alone: that is a genuine collision.
   */
  const reconcileRef = async (existing: RefResponse): Promise<void> => {
    if (existing.object.sha === commit.sha) return;
    const parents = await (async () => {
      try {
        return (await requestBody<GitCommitResponse>(input.github, { method: "GET", path: `${prefix}/git/commits/${existing.object.sha}` })).parents ?? [];
      } catch (error) {
        if (isNotFound(error)) return [];
        throw error;
      }
    })();
    if (parents.length !== 1 || parents[0]?.sha !== state.headSha) throw new Error(`Conflicting branch: ${branch}`);
    await requestBody(input.github, { method: "PATCH", path: `${prefix}/git/refs/heads/${branch}`, body: { sha: commit.sha, force: true } });
  };

  let ref: RefResponse | null = null;
  try { ref = await requestBody<RefResponse>(input.github, { method: "GET", path: `${prefix}/git/ref/heads/${branch}` }); } catch (error) { if (!isNotFound(error)) throw error; }
  if (ref !== null) await reconcileRef(ref);
  if (ref === null) {
    try {
      await requestBody(input.github, { method: "POST", path: `${prefix}/git/refs`, body: { ref: `refs/heads/${branch}`, sha: commit.sha } });
    } catch (error) {
      if (apiErrorStatus(error) !== 422) throw error;
      await reconcileRef(await requestBody<RefResponse>(input.github, { method: "GET", path: `${prefix}/git/ref/heads/${branch}` }));
    }
  }

  /** Identity is title, head, and base; a replay's changed body is updated, not duplicated. */
  const openPr = async (): Promise<PullRequestResponse | undefined> => {
    const prs = await requestBody<readonly PullRequestResponse[]>(input.github, { method: "GET", path: `${prefix}/pulls?head=${encodeURIComponent(`${repository.owner}:${branch}`)}&base=${encodeURIComponent(baseRef)}&state=open` });
    return prs.find((pr) => pr.title === title && pr.head?.ref === branch && pr.base?.ref === baseRef);
  };
  const matching = await openPr();
  const pullRequest = matching ?? await (async () => {
    try {
      return await requestBody<PullRequestResponse>(input.github, { method: "POST", path: `${prefix}/pulls`, body: { title, body, head: branch, base: baseRef, draft: true } });
    } catch (error) {
      if (apiErrorStatus(error) !== 422) throw error;
      const found = await openPr();
      if (found === undefined) throw error;
      return found;
    }
  })();
  if (matching !== undefined && matching.body !== body) {
    await requestBody(input.github, { method: "PATCH", path: `${prefix}/pulls/${matching.number}`, body: { body } });
  }
  const pullRequestResult = pullRequest.html_url === undefined
    ? { number: pullRequest.number }
    : { number: pullRequest.number, htmlUrl: pullRequest.html_url };
  return { branch, commitSha: commit.sha, changedPaths: files.map((file) => file.path), pullRequest: pullRequestResult };
}
