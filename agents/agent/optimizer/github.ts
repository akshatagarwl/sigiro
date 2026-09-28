import { createSign } from "node:crypto";

import { Effect, Schema } from "effect";

export class GitHubFailure extends Schema.TaggedError<GitHubFailure>()(
  "GitHubFailure",
  {
    message: Schema.String,
  }
) {}

const Repository = Schema.Struct({
  id: Schema.Number,
  full_name: Schema.String,
  default_branch: Schema.String,
});
const Ref = Schema.Struct({ object: Schema.Struct({ sha: Schema.String }) });
const Commit = Schema.Struct({
  sha: Schema.String,
  tree: Schema.Struct({ sha: Schema.String }),
  message: Schema.String,
  parents: Schema.Array(Schema.Struct({ sha: Schema.String })),
});
const Tree = Schema.Struct({ sha: Schema.String });
const CheckoutTree = Schema.Struct({
  sha: Schema.String,
  truncated: Schema.Boolean,
  tree: Schema.Array(Schema.Struct({ path: Schema.String, mode: Schema.String, type: Schema.String, sha: Schema.String })),
});
const CheckoutBlob = Schema.Struct({ content: Schema.String, encoding: Schema.String, size: Schema.Number });
const Pull = Schema.Struct({
  id: Schema.Number,
  number: Schema.Number,
  html_url: Schema.String,
  draft: Schema.Boolean,
  state: Schema.String,
  head: Schema.Struct({ ref: Schema.String }),
});
const Pulls = Schema.Array(Pull);
const Token = Schema.Struct({ token: Schema.String });
const Blob = Schema.Struct({ sha: Schema.String });

export type Connection = {
  repositoryId: number;
  owner: string;
  repo: string;
  installationId: number;
};
export type Change = { path: string; content: string | null; executable?: boolean };
export type Publication = {
  pullRequestId: number;
  url: string;
  branch: string;
  commitSha: string;
};

export class TrustedGitHub {
  private readonly config: {
    apiUrl: string;
    appId: string;
    privateKey: string;
  };
  constructor(config: { apiUrl: string; appId: string; privateKey: string }) {
    this.config = config;
  }

  private jwt(): string {
    const now = Math.floor(Date.now() / 1000);
    const encode = (value: object) =>
      Buffer.from(JSON.stringify(value)).toString("base64url");
    const payload = `${encode({ alg: "RS256", typ: "JWT" })}.${encode({ iat: now - 60, exp: now + 540, iss: this.config.appId })}`;
    return `${payload}.${createSign("RSA-SHA256").update(payload).sign(this.config.privateKey, "base64url")}`;
  }

  private request<A>(
    path: string,
    token: string,
    schema: Schema.ConstraintDecoder<A>,
    init: RequestInit = {}
  ) {
    return Effect.tryPromise({
      try: async () => {
        const response = await fetch(new URL(path, this.config.apiUrl), {
          ...init,
          headers: {
            Accept: "application/vnd.github+json",
            Authorization: `Bearer ${token}`,
            "X-GitHub-Api-Version": "2022-11-28",
            ...(init.body ? { "Content-Type": "application/json" } : {}),
          },
        });
        if (!response.ok)
          throw new Error(
            `GitHub ${init.method ?? "GET"} ${path}: ${response.status}`
          );
        return Schema.decodeUnknownSync(schema)(await response.json());
      },
      catch: (error) => new GitHubFailure({ message: String(error) }),
    });
  }

  checkout(connection: Connection, targetCommitSha: string) {
    return Effect.gen(function* (this: TrustedGitHub) {
      if (!/^[a-f0-9]{40}$/.test(targetCommitSha)) return yield* new GitHubFailure({ message: "Invalid target SHA" });
      const token = yield* this.installationToken(connection.installationId);
      const root = `/repos/${encodeURIComponent(connection.owner)}/${encodeURIComponent(connection.repo)}`;
      const repository = yield* this.request(root, token, Repository);
      if (repository.id !== connection.repositoryId || repository.full_name !== `${connection.owner}/${connection.repo}`) {
        return yield* new GitHubFailure({ message: "Repository identity mismatch" });
      }
      const commit = yield* this.request(`${root}/git/commits/${targetCommitSha}`, token, Commit);
      const tree = yield* this.request(`${root}/git/trees/${commit.tree.sha}?recursive=1`, token, CheckoutTree);
      if (tree.truncated || tree.tree.length > 1024 || tree.sha !== commit.tree.sha) {
        return yield* new GitHubFailure({ message: "Repository tree is truncated, too large or inconsistent" });
      }
      const entries = tree.tree.filter((entry) => entry.type !== "tree");
      for (const entry of entries) {
        if (entry.type !== "blob" || !["100644", "100755", "120000"].includes(entry.mode) ||
          entry.path.startsWith("/") || entry.path.split("/").some((part) => !part || part === "." || part === ".." || part === ".git")) {
          return yield* new GitHubFailure({ message: "Unsupported or unsafe repository entry" });
        }
      }
      const github = this;
      let total = 0;
      return yield* Effect.forEach(entries, (entry) => Effect.gen(function* () {
        const blob = yield* github.request(`${root}/git/blobs/${entry.sha}`, token, CheckoutBlob);
        if (blob.encoding !== "base64" || blob.size > 1024 * 1024) {
          return yield* new GitHubFailure({ message: "Repository blob is too large or unsupported" });
        }
        const content = Buffer.from(blob.content.replaceAll("\n", ""), "base64");
        if (content.length !== blob.size) return yield* new GitHubFailure({ message: "Repository blob length mismatch" });
        total += content.length;
        if (total > 20 * 1024 * 1024) return yield* new GitHubFailure({ message: "Repository checkout exceeds 20 MiB" });
        return { path: entry.path, content, executable: entry.mode === "100755", ...(entry.mode === "120000" ? { symlink: true } : {}) };
      }), { concurrency: 6 });
    }.bind(this));
  }

  resolveTarget(connection: Connection) {
    return Effect.gen(
      function* (this: TrustedGitHub) {
        const token = yield* this.installationToken(connection.installationId);
        const root = `/repos/${encodeURIComponent(connection.owner)}/${encodeURIComponent(connection.repo)}`;
        const repo = yield* this.request(root, token, Repository);
        if (
          repo.id !== connection.repositoryId ||
          repo.full_name !== `${connection.owner}/${connection.repo}`
        ) {
          return yield* new GitHubFailure({
            message: "Repository identity mismatch",
          });
        }
        const baseRef = repo.default_branch;
        const ref = yield* this.request(
          `${root}/git/ref/heads/${baseRef}`,
          token,
          Ref
        );
        return { baseRef, targetCommitSha: ref.object.sha };
      }.bind(this)
    );
  }

  reconcile(input: {
    connection: Connection;
    runId: string;
    baseRef: string;
    targetCommitSha: string;
  }) {
    return Effect.gen(
      function* (this: TrustedGitHub) {
        const { connection, runId, baseRef, targetCommitSha } = input;
        if (
          !/^[a-f0-9-]{36}$/.test(runId) ||
          !/^[a-f0-9]{40}$/.test(targetCommitSha)
        ) {
          return yield* new GitHubFailure({ message: "Invalid run identity" });
        }
        const token = yield* this.installationToken(connection.installationId);
        const root = `/repos/${encodeURIComponent(connection.owner)}/${encodeURIComponent(connection.repo)}`;
        const repo = yield* this.request(root, token, Repository);
        if (
          repo.id !== connection.repositoryId ||
          repo.full_name !== `${connection.owner}/${connection.repo}`
        ) {
          return yield* new GitHubFailure({
            message: "Repository identity mismatch",
          });
        }
        const branch = `sigiro/optimizer-${runId}`;
        const pulls = yield* this.request(
          `${root}/pulls?state=all&head=${encodeURIComponent(`${connection.owner}:${branch}`)}&base=${encodeURIComponent(baseRef)}`,
          token,
          Pulls
        );
        if (pulls.length > 1)
          return yield* new GitHubFailure({
            message: "Ambiguous existing PRs",
          });
        const ref = yield* this.request(
          `${root}/git/ref/heads/${branch}`,
          token,
          Ref
        ).pipe(
          Effect.catch((error) =>
            error.message.endsWith(": 404")
              ? Effect.succeed(null)
              : Effect.fail(error)
          )
        );
        if (!ref) {
          if (pulls.length)
            return yield* new GitHubFailure({
              message: "PR branch is missing",
            });
          return null;
        }
        const commit = yield* this.request(
          `${root}/git/commits/${ref.object.sha}`,
          token,
          Commit
        );
        if (
          commit.message !== `optimizer: ${runId}` ||
          commit.parents.length !== 1 ||
          commit.parents[0].sha !== targetCommitSha
        ) {
          return yield* new GitHubFailure({
            message: "Existing branch is not this run's commit",
          });
        }
        if (!pulls.length) return null;
        if (!pulls[0].draft || pulls[0].state !== "open")
          return yield* new GitHubFailure({
            message: "Existing PR is not an open draft",
          });
        return {
          pullRequestId: pulls[0].id,
          url: pulls[0].html_url,
          branch,
          commitSha: ref.object.sha,
        } satisfies Publication;
      }.bind(this)
    );
  }

  private installationToken(installationId: number) {
    return this.request(
      `/app/installations/${installationId}/access_tokens`,
      this.jwt(),
      Token,
      { method: "POST" }
    ).pipe(Effect.map((result) => result.token));
  }

  publish(input: {
    connection: Connection;
    runId: string;
    baseRef: string;
    targetCommitSha: string;
    changes: readonly Change[];
    title: string;
    body: string;
  }) {
    return Effect.gen(
      function* (this: TrustedGitHub) {
        const {
          connection,
          runId,
          baseRef,
          targetCommitSha,
          changes,
          title,
          body,
        } = input;
        if (
          !/^[a-f0-9-]{36}$/.test(runId) ||
          !/^[a-f0-9]{40}$/.test(targetCommitSha) ||
          !/^[a-zA-Z0-9._/-]+$/.test(baseRef) ||
          baseRef.startsWith("/") ||
          baseRef.includes("..")
        ) {
          return yield* new GitHubFailure({
            message: "Invalid run ID, base ref or target SHA",
          });
        }
        if (
          !changes.length ||
          changes.length > 100 ||
          changes.some(
            ({ path, content }) =>
              path.startsWith("/") ||
              path
                .split("/")
                .some(
                  (part) =>
                    !part || part === "." || part === ".." || part === ".git"
                ) ||
              (content !== null && Buffer.byteLength(content) > 1024 * 1024)
          )
        )
          return yield* new GitHubFailure({
            message: "Invalid or empty patch",
          });
        if (
          new Set(changes.map((change) => change.path)).size !== changes.length
        )
          return yield* new GitHubFailure({ message: "Duplicate paths" });
        const branch = `sigiro/optimizer-${runId}`;
        const token = yield* this.installationToken(connection.installationId);
        const root = `/repos/${encodeURIComponent(connection.owner)}/${encodeURIComponent(connection.repo)}`;
        const repo = yield* this.request(root, token, Repository);
        if (
          repo.id !== connection.repositoryId ||
          repo.full_name !== `${connection.owner}/${connection.repo}`
        ) {
          return yield* new GitHubFailure({
            message: "Repository identity mismatch",
          });
        }
        const pullList = () =>
          this.request(
            `${root}/pulls?state=all&head=${encodeURIComponent(`${connection.owner}:${branch}`)}&base=${encodeURIComponent(baseRef)}`,
            token,
            Pulls
          );
        const found = yield* pullList();
        if (found.length > 1)
          return yield* new GitHubFailure({
            message: "Ambiguous existing PRs",
          });
        const refPath = `${root}/git/ref/heads/${branch}`;
        const existingRef = yield* this.request(refPath, token, Ref).pipe(
          Effect.catch((error) =>
            error.message.endsWith(": 404")
              ? Effect.succeed(null)
              : Effect.fail(error)
          )
        );
        if (existingRef) {
          const existingCommit = yield* this.request(
            `${root}/git/commits/${existingRef.object.sha}`,
            token,
            Commit
          );
          if (
            existingCommit.message !== `optimizer: ${runId}` ||
            existingCommit.parents.length !== 1 ||
            existingCommit.parents[0].sha !== targetCommitSha
          ) {
            return yield* new GitHubFailure({
              message: "Existing branch is not this run's commit",
            });
          }
          if (found.length === 1) {
            if (!found[0].draft || found[0].state !== "open")
              return yield* new GitHubFailure({
                message: "Existing PR is not draft",
              });
            return {
              pullRequestId: found[0].id,
              url: found[0].html_url,
              branch,
              commitSha: existingRef.object.sha,
            } satisfies Publication;
          }
          const pull = yield* this.createPull(
            root,
            token,
            branch,
            baseRef,
            title,
            body,
            pullList
          );
          return {
            pullRequestId: pull.id,
            url: pull.html_url,
            branch,
            commitSha: existingRef.object.sha,
          } satisfies Publication;
        }
        if (found.length === 1)
          return yield* new GitHubFailure({ message: "PR branch is missing" });
        const base = yield* this.request(
          `${root}/git/ref/heads/${baseRef}`,
          token,
          Ref
        );
        if (base.object.sha !== targetCommitSha)
          return yield* new GitHubFailure({
            message: "Target ref moved; run must be reviewed",
          });
        const parent = yield* this.request(
          `${root}/git/commits/${targetCommitSha}`,
          token,
          Commit
        );
        const entries: Array<{path: string;mode: string;type: string;sha: string | null}> = [];
        for (const change of changes) {
          if (change.content === null) {
            entries.push({ path: change.path, mode: "100644", type: "blob", sha: null });
            continue;
          }
          const blob = yield* this.request(`${root}/git/blobs`, token, Blob, {
            method: "POST",
            body: JSON.stringify({ content: change.content, encoding: "utf-8" }),
          });
          entries.push({ path: change.path, mode: change.executable ? "100755" : "100644", type: "blob", sha: blob.sha });
        }
        const tree = yield* this.request(`${root}/git/trees`, token, Tree, {
          method: "POST",
          body: JSON.stringify({ base_tree: parent.tree.sha, tree: entries }),
        });
        if (tree.sha === parent.tree.sha)
          return yield* new GitHubFailure({ message: "No changes to publish" });
        const commit = yield* this.request(
          `${root}/git/commits`,
          token,
          Commit,
          {
            method: "POST",
            body: JSON.stringify({
              message: `optimizer: ${runId}`,
              tree: tree.sha,
              parents: [targetCommitSha],
            }),
          }
        );
        yield* this.request(`${root}/git/refs`, token, Ref, {
          method: "POST",
          body: JSON.stringify({
            ref: `refs/heads/${branch}`,
            sha: commit.sha,
          }),
        }).pipe(
          Effect.catch((error) =>
            error.message.endsWith(": 422")
              ? this.request(refPath, token, Ref).pipe(
                  Effect.flatMap((ref) =>
                    ref.object.sha === commit.sha
                      ? Effect.succeed(ref)
                      : Effect.fail(
                          new GitHubFailure({
                            message:
                              "Branch creation raced with another commit",
                          })
                        )
                  )
                )
              : Effect.fail(error)
          )
        );
        const pull = yield* this.createPull(
          root,
          token,
          branch,
          baseRef,
          title,
          body,
          pullList
        );
        return {
          pullRequestId: pull.id,
          url: pull.html_url,
          branch,
          commitSha: commit.sha,
        } satisfies Publication;
      }.bind(this)
    );
  }

  private createPull(
    root: string,
    token: string,
    branch: string,
    base: string,
    title: string,
    body: string,
    list: () => ReturnType<TrustedGitHub["request"]>
  ) {
    return Effect.gen(
      function* (this: TrustedGitHub) {
        const existing = yield* list();
        if (!Array.isArray(existing) || existing.length > 1)
          return yield* new GitHubFailure({ message: "Ambiguous PRs" });
        const pull =
          existing[0] ??
          (yield* this.request(`${root}/pulls`, token, Pull, {
            method: "POST",
            body: JSON.stringify({
              title,
              body,
              head: branch,
              base,
              draft: true,
            }),
          }).pipe(
            Effect.catch((error) =>
              error.message.endsWith(": 422")
                ? list().pipe(
                    Effect.flatMap((pulls) =>
                      Array.isArray(pulls) && pulls.length === 1
                        ? Effect.succeed(pulls[0])
                        : Effect.fail(error)
                    )
                  )
                : Effect.fail(error)
            )
          ));
        if (!pull.draft || pull.state !== "open")
          return yield* new GitHubFailure({
            message: "Existing PR is not draft",
          });
        return pull;
      }.bind(this)
    );
  }
}
