# Deployment rearchitecture (2026-09-05)

The deployment target is MinIO, celld v0.4.0, the copied
`@ewhauser/world-celld@0.4.1` `celld-worker`, and Eve 0.51's native Workflow
runtime. The agent supervises exactly celld and Eve. Startup persists separate
world and callback secrets, sets 90-day workflow retention, and deploys the
worker from `/app/world-celld-worker`.

The world-celld package documents `GET /v1/health` and the Eve 0.51 package
documents `GET /eve/v1/health`. The worker documents `WORKFLOW_CALLBACK_SECRET`
as the source of `x-workflow-callback-secret`; Eve 0.51 source does not prove a
callback-secret environment variable that it consumes. The compatibility of
Eve 0.51, the pinned `@workflow/*` packages, and world-celld 0.4.1 still needs
an end-to-end integration proof. A build-only version mismatch is not a
confirmed runtime failure.

The remaining worker-token and coordinator references below are historical
findings from the superseded deployment and are retained as audit evidence.

**Superseded (2026-09-05).** The MinIO claim above no longer holds; see
"MinIO to S3, and the Box sandbox" below.

# Can eve run on a Claude Code subscription? (2026-08-31)

Throwaway experiments, kept only for the answers. Not shippable code.

Result: **works**, with one architectural consequence.

| step | question | result |
|---|---|---|
| A | Does `ai-sdk-provider-claude-code` complete on the subscription, no API key? | PASS. `SUBSCRIPTION_OK`, `ANTHROPIC_API_KEY` absent. |
| B | Does eve's `defineAgent` accept a third-party AI SDK provider? | PASS, after one fix. |

## The fix step B needed

First run failed:

```
Cannot compile agent compaction because the primary compaction trigger model
"claude-code/sonnet" does not have known AI Gateway context window metadata.
```

Any model outside the AI Gateway catalog needs an explicit context window.
`modelContextWindowTokens` is in the TypeScript types but not in the
agent-config guide.

## The consequence: eve's approval hook does not apply on this path

Provider warning, and the README confirms it:

> AI SDK v7 `toolApproval` is **not** bridged to Claude Code's internal tool
> permission system. Use Claude Agent SDK `canUseTool` / `permissionMode` for
> Claude Code built-in and MCP tools.

So permissions move from eve to Claude Code: `allowedTools` / `disallowedTools`,
`canUseTool` (async per-call callback), `permissionMode`, `hooks`. Custom tools
reach the CLI through an in-process MCP server via `createAiSdkMcpServer`.

## Cost note

A three-word prompt cost 24,651 input tokens, because the Agent SDK injects its
own system prompt and tool definitions. Budget per-call overhead accordingly.

---

# Permission boundary (2026-08-31)

Both arms use a Claude Code team subscription, with no API key.

| arm | expectation | result |
|---|---|---|
| denied tool | Bash must be refused | **`DENIED bash`** |
| allowed tool | `sigiro_list_anomalies` must return real data | **"95 anomalies, first service_name worldpol-api"** |

Arm 2 matches a direct `curl` of `/v1/anomalies` exactly, so the tool reached
production rather than being answered from the model's memory. Both arms are
needed: a gate that denied everything would pass arm 1 alone.

## Two of my three "defence layers" were inert

The first version stacked `allowedTools`, `disallowedTools` and `canUseTool`.
Arm 1 passed anyway, which is what made it dangerous. The SDK warns plainly:

```
CLAUDE_SDK_CAN_USE_TOOL_SHADOWED: Bare allowedTools entries auto-approve the
whole tool before the callback is consulted.

Both allowedTools and disallowedTools are specified. Only allowedTools will be used.
```

So naming tools in `allowedTools` silently disabled `canUseTool` for exactly
those tools, and `disallowedTools` was ignored outright. One layer was doing all
the work and it was not the one the code comment credited.

Current config: no `allowedTools`, no `disallowedTools`, one `canUseTool` that
fails closed. Stacking defences is worthless if you cannot say which one answered.

## eve reserves `agent/tools/`

MCP-bridged AI SDK tools cannot live there; eve tries to compile them as its own
tools and fails with "Expected the tool export to match the public eve shape".
They live in `src/tools/` instead.

## Production constraint that shaped the tool design

Measured against `api.sigiro.com`: every `/v1/query` touching
`lake.sigiro_spans` or `lake.sigiro_profiles` fails with `INTERRUPT Error:
Interrupted!`, including a `count(*)` bounded to 5 minutes. The same endpoint
answers `SELECT 1`, `sigiro_anomalies` (95 rows) and `sigiro_logs` (39,045 rows)
in under a second. Cause is the 10 second query timeout on a 0.75 CPU container,
not the codec change: `sigiro_logs` shares the catalog and the mixed codecs.

So the agent gets purpose-built endpoints as its primary surface, not raw SQL.

## Timing

Arm 2 took 11 minutes end to end. Fine for a background investigator, too slow
for anything interactive.

## Denied tools, re-tested from the monorepo path

| tool | result | strength of evidence |
|---|---|---|
| `Bash` | `DENIED - Bash` | strong |
| `Read` | `DENIED. Tool: Read` | strong |
| `Agent` / `Task` | `DENIED - Agent`, plus the model's own reasoning about scope | **weak** |

The `Agent` result is not proof. The model explained that spawning a subagent
"isn't part of that workflow", which is compliance with instructions rather than
a gate refusing a call. Model compliance is not a security boundary. There is
contrary evidence too: in an earlier run the model said it had "launched a
subagent in the background" to read a spilled tool result. Whether `Agent` is
actually gated is **open**.

## A tool whose success case broke the harness

`sigiro_list_anomalies` originally returned all 95 anomalies. That exceeds Claude
Code's inline tool-result token limit, so the CLI spilled the result to a file
and tried to read it back with tools this agent is not allowed to use. The call
stranded, and the run took 150 seconds to produce nothing.

Bounded to a `limit` (default 10, max 50) it returns inline in 30 seconds with
more useful detail than before. Worth generalising: any tool here that can
return an unbounded collection needs a cap, because the failure mode is not a
truncated answer, it is the harness reaching for a denied tool.

---

# Root cause of the query timeouts (2026-08-31, corrected)

**My earlier claim in this file was wrong.** I wrote that `/v1/query` on
`lake.sigiro_spans` and `lake.sigiro_profiles` always fails. It does not.
Re-measured: `count(*)` on spans returns 181,084 rows in 0.94s.

The real cause is a type mismatch that destroys Parquet filter pushdown. The
`timestamp` columns are `TIMESTAMP` (no timezone). Comparing one against a
`TIMESTAMPTZ` makes DuckDB wrap the **column** in a cast, and a cast column
cannot be matched against min/max statistics, so the query opens every file.

Single-variable proof, same table, same window:

| predicate | result |
|---|---|
| `timestamp > TIMESTAMP '2026-08-31 08:00:00'` | 200 in 2.48s |
| `timestamp > TIMESTAMPTZ '2026-08-31 08:00:00+00'` | 400 INTERRUPT at 10.98s |

A constant TIMESTAMPTZ fails too, so this is the type and not `now()` being
volatile. `now()` returns TIMESTAMPTZ, which is how you hit it by accident.

Why my original measurements looked table-shaped: every failing query I wrote
used `now()`, and every passing one did not. The tables were incidental.

---

# The permission gate is a gate, not compliance (2026-09-03)

Measured with `test/provider-spike.ts` (`PROVIDER=claude|codex`), three arms per
provider, no API key in the process either time. Providers:
`ai-sdk-provider-claude-code@4.2.0` (Agent SDK 0.3.251),
`ai-sdk-provider-codex-cli@2.1.2`.

| | claude (team seat) | codex (personal) |
|---|---|---|
| subscription completion | PASS | PASS |
| input tokens, 6-word prompt | 26,668 | 18,436 |
| own tool executed (random sentinel echoed) | PASS | PASS |
| permission callback consulted | PASS: saw `Write`, `Bash` by name | PASS: command + file-change approvals |
| denied write landed on disk | no | no |

The earlier entry recorded the `Agent`/`Task` denial as **weak** because a
polite refusal is indistinguishable from a gate. This run upgrades the
*mechanism*: `canUseTool` was consulted for `Write` and `Bash` by name, both
were denied, and the file the model was told to create does not exist. A
refusal backed by a consulted callback is a boundary. (`Agent`/`Task`
specifically remains untested; the mechanism it would ride through is now
proven.)

Two findings that change how a gate should be written:

- **Codex gates MCP tools per server, not per tool.** The elicitation carries
  `serverName` and `codex_approval_kind: "mcp_tool_call"`, no tool name. So the
  fail-closed shape there is "accept our server, decline everything else". On
  Claude the gate sees full tool names (`mcp__sigiro__readDeployMarker`).
- **Codex declines MCP tool calls by default** when `autoApprove` is false and
  no `onMcpElicitation` handler exists. Two spike runs failed as "the tool did
  not run" before this was the diagnosis; the bridge was fine both times.

Also corrected in passing: `sandboxPolicy`/`approvalPolicy`/`serverRequests`
belong to `codexAppServer`, not `codexExec`. `codexExec` has no approval
surface at all.

The billing boundary stands: the Claude login uses a team subscription, while
the Codex login is personal. Same conclusion as before, now with the measurement
attached.

## GitHub publishing audit (corrected 2026-09-04)

**Superseded.** This section said Eve's GitHub integration "is not used by the
deployment". It is now the execution surface: `agent/channels/github.ts` wraps
`githubChannel` from `eve/channels/github` and adds one `turn.completed`
handler. The audit's narrow finding survives: Eve ships no reusable branch,
commit, and draft-PR publisher, which is why `agent/lib/github-draft-pr.ts`
supplies one over Eve's authenticated `github.request` handle.

Real private-repository auth remains open, so the project must not claim
any-infrastructure or any-repository support yet.

---

# Eve local microsandbox proof (2026-09-03)

Eve 0.51 peers with `microsandbox` `^0.5`. The first isolated run used a path that was too long; `/tmp/eve-msb-051` worked. A later environment picked up microsandbox 0.6.16, which completed the run but failed cleanup with `configure is not a function`. Microsandbox 0.5.10 therefore needs a separate state database after the 0.6 migrations; do not reuse the migrated state.

Direct microsandbox proof:

- **1 pass**: the intentionally failing fixture failed before Eve edited it and passed after Eve's edit.
- Only `bug.js` was modified, with an exact one-line diff.
- Eve stopped the sandbox and reported no cleanup errors.

The proof used Eve's sandbox tools and left the source fix uncommitted. No `OPENAI_API_KEY` was present. `eve invoke` exits 0 even when the model reports that tools failed, so process exit status alone is invalid evidence; inspect the invocation outcome and the resulting artifact (test result, git diff, and git status).

`checkoutGitHubRepository` is not exported from Eve's public `eve/channels/github` entrypoint. Its shipped type marks it channel-internal and says it requires a firewall-capable backend. No GitHub credential was supplied and no public checkout was attempted through a non-public import. This is a blocked public-API proof, not a claim of support.

## Adapter completion ordering (2026-09-03, corrected 2026-09-04)

**The earlier claim in this file was wrong.** I wrote that a throwing GitHub
`turn.completed` handler prevents `turn.completed` from reaching the worker, and
that the worker may therefore treat an observed `turn.completed` as proof the
handler completed. It cannot.

Two corrections, both measured against eve 0.51.0 in `node_modules/eve/dist`:

- The file was wrong. There is no `execution/agent-workflow.js`. The emit path
  lives in `execution/workflow-steps.js`, whose `turnStep` defines
  `emit = async t => { let n = await callAdapterEventHandler(u, t, b); ... await
  A.write(encodeMessageStreamEvent(r)) }`. The ordering claim holds: the handler
  is awaited before the event is written.
- The consequence was wrong, and this is the part that matters.
  `callAdapterEventHandler` in `channel/adapter.js` wraps the handler call in
  `try { await i(...) } catch (n) { log.error("adapter event handler threw —
  event swallowed", ...) }`. A throwing handler is logged and **swallowed**, so
  `emit` proceeds to write the event normally. `turn.completed` reaches the
  worker whether the handler succeeded or threw.

So handler-throw suppression does not hold for `turn.completed` either. An
observed `turn.completed` is proof only that the turn ended, never that the
draft PR was published. A worker that needs publication status must observe the
published artifact (the branch, the commit, or the pull request) rather than
infer it from the event.

Separately, `execution/terminal-session-event.js` writes terminal
`session.completed`/`session.failed` events even when the adapter handler
throws: `handleEvent` there catches the `callAdapterEventHandler` failure with
its own `log.error("adapter failed to handle terminal ... event")` and then
writes the event to the durable stream in a following `try`. Terminal session
events are therefore doubly unsuppressible.

### The consequence for the worker's success criterion

The worker treated `turn.completed` followed by `session.waiting` as success.
Given the swallow, that is unsound: the publisher could throw at the base SHA
check, on a failing required test, on a rejected path, or anywhere inside the
GitHub writes, and the worker would still report the attempt succeeded and map
every required test to exit 0. The bug is not a missing log line, it is a false
success reported to celld for an attempt that published nothing.

## Publication receipts (2026-09-04)

The fix is to make the handler leave evidence the swallow cannot erase, and to
make the worker read that evidence rather than infer it.

`agent/lib/publication-receipts.ts` is a file-backed store, one JSON file per
`executionId` under `CELLD_RECEIPTS_DIR` (default `.celld-receipts` in the app
working directory), written temp-plus-rename so a reader never sees a partial
record. A receipt carries `{ executionId, status: "published" | "failed",
error?, branch?, commitSha?, changedPaths?, pullRequestNumber?, pullRequestUrl?,
headSha, sessionId, updatedAt }`. Reads validate the shape and return
`undefined` for anything else, so a truncated file reads as "not yet written"
rather than deciding an execution's fate. The file name is a SHA-256 of the
execution id, because a worker-supplied id is bounded but is not a safe file
name.

Three rules the store enforces:

- A `published` receipt is immutable evidence: the branch, the commit, and the
  pull request exist on GitHub whatever happens next in this process. A later
  `failed` write is ignored rather than allowed to retract it.
- A replay that republishes may overwrite `published` with `published`, because
  the branch is deterministic in `executionId` and the second write describes
  the same artifact.
- A `failed` receipt is freely replaceable, including by a later `published`.

`publishTurn` wraps the SHA check, the required-test loop, and the publish in
one `try`/`catch`, but only for a celld-trusted turn carrying an `executionId`.
A human turn has nothing for a worker to poll and stamps nothing. The
no-changes celld case already threw; it now becomes a failed receipt too. The
receipt is written before the throw is re-raised, so the wrapper's delegate and
Eve's swallow both still see the throw.

`executor/eve.ts` polls `POST /internal/celld/job/result` through the injected
transport after the stream reaches `turn.completed` plus `waiting`/`completed`:
first read immediate, then every 2 s to a 30 s bound. Only a `published`
receipt succeeds, and its `branch`, `commitSha`, `changedPaths`,
`pullRequestNumber`, and `pullRequestUrl` are carried into the run result while
the summary stays the model's message. A `failed` receipt fails with its
recorded reason. Still pending at the bound is a failure that names the gap,
because "the handler may or may not have published" is not a success.

The result route holds no credential beyond the existing worker bearer check,
makes no GitHub call, and answers 404 `{ status: "pending" }` while no receipt
exists.

This also re-justifies the worker's tests-exit-0 mapping, which previously
rested on the unsound event inference. A published receipt is written only
after the test loop finished with every exit code 0 and the publish returned,
so the receipt, not the event, is what proves those tests passed.

What this does not do: the receipt proves the handler reached its end in the
Eve host's filesystem. If that filesystem is lost between the write and the
worker's read, the run fails as pending rather than succeeding, which is the
safe direction. Nothing here re-derives publication from GitHub itself, so a
receipt whose store was tampered with is trusted; it sits behind the same
worker token as dispatch, which is the boundary noted below.

## The GitHub session is not an execution boundary (2026-09-04)

Two structural facts about eve 0.51.0's GitHub channel, both of which the
publisher now has to defend against.

`continuationTokenFromState` in `public/channels/github/state.js` derives the
session key from `repositoryId`, `conversationKind`, `issueNumber`,
`pullRequestNumber`, and `reviewThreadRootCommentId`. There is no execution
nonce. A celld job dispatched through `receive()` and a later human `@bot`
comment on the same issue therefore land in the *same* session.

Across those turns `execution/workflow-steps.js` sets only `AuthKey`
(`s.input.auth !== void 0 && l.set(AuthKey, ...)`), while `InitiatorAuthKey` is
written once at session creation (`i.set(InitiatorAuthKey, r.initiatorAuth ?? a)`
in the runtime context builder) and never replaced. So `ctx.session.auth.initiator`
still names the celld worker on a human turn. Reading trusted attributes from
`initiator` let a human comment inherit the job's `executionId`,
`expectedBaseSha`, `allowedPaths`, and `requiredTests`. The handler now reads
them only when `auth.current.authenticator` is the celld worker, and a
celld-initiated session publishes nothing on a turn it did not initiate.

Second, an issue anchor executes at the default-branch tip, not at the job's
base SHA. `resolveCheckoutRef` in `public/channels/github/checkout.js` falls
through `ref`, `headSha`, `refs/pull/N/head`, `headRef`, to `defaultBranch`; for
an issue-anchored session `stateFromReceiveTarget` leaves `headSha` null, so the
default branch is fetched and the channel's `turn.started` handler assigns
`state.headSha = checkout.sha`. A celld job anchored to an issue must set
`baseSha` to the current default-branch tip. The mismatch is refused, not
silently re-checked-out.

The same call also returns `baseRef: descriptor.baseRef`, which is null without
a pull request, so `state.baseRef` stays null for issue anchors and the trusted
`baseRef` attribute can only be enforced when the anchor actually resolved one.

## One worker token, two boundaries (2026-09-04)

The coordinator and the Eve host currently share one worker token. The same
secret authenticates the worker to the coordinator's claim, heartbeat, and
completion endpoints and authenticates the worker's dispatch to Eve's
`/internal/celld/job` route. Compromising either side yields both.

---

# MinIO to S3, and the Box sandbox (2026-09-05)

## MinIO to S3

Cloned `world-celld` to audit its own documentation on why MinIO was in this
deployment at all. Its README states plainly: "MinIO Community is **not** a
supported celld production store. It does not implement the conditional
writes celld needs for ownership fencing." The package's own required CI
smoke test disables celld's storage probe for exactly this reason and runs
exactly one celld process at a time, so it proves single-process restart
behavior only, never multi-node ownership or failover correctness. This was a
correctness gap in the deployment, not a preference between two equivalent
object stores.

Removed the `minio` service, the `minio-data` volume, `MINIO_ROOT_USER`, and
`MINIO_ROOT_PASSWORD` from `deploy/docker-compose.yml` and `deploy/env.example`,
and removed the `mc alias`/`mc mb` bucket-bootstrap calls from
`deploy/start-agent.sh`. The deployment now points celld at an operator-owned
Amazon S3 bucket that the operator must provision and create themselves;
celld does not create the bucket, whether against MinIO or S3.

**Whether `--endpoint` is still required against real S3.** Checked rather
than assumed. Both the top-level `world-celld` README and
`celld-worker/README.md` show the canonical production deploy command as
`celld deploy ./celld-worker --bucket s3://my-cells-bucket`, with no
`--endpoint` flag, in every example that is not exercising MinIO. The only
places `--endpoint` appears in the audited repository are the CI smoke test
and the opt-in MinIO performance harness, and in both, the value passed is
the loopback URL of the MinIO process the test itself just started
(`this.endpoint` / `minioUrl`, built from a locally allocated port). Nothing
in the audited source or docs documents `--endpoint` as a general production
requirement; it exists to redirect celld's S3 client away from real AWS S3
and toward a local MinIO server. Conclusion: `--endpoint` is a MinIO-only (or
more generally, non-AWS-store-only) override, not a flag real S3 deployments
need. Removed it from `deploy/start-agent.sh` and `deploy/supervisord.conf`,
so celld now deploys and runs with only `--bucket` and `--region`, letting
its S3 client resolve the real AWS endpoint from the region.

Credentials still come from the standard AWS environment variables,
`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, plus `AWS_REGION`; no new
credential mechanism was invented. `deploy/env.example` now states exactly
which four values the operator supplies: `CELLD_BUCKET`, `AWS_REGION`,
`AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY`.

Checked every MinIO-specific guard before deleting it, rather than assuming
its reason was gone:

- The storage-probe disable and single-celld-process constraint live only in
  `world-celld`'s own smoke test, gated on MinIO's missing conditional
  writes. That reason does not apply to this deployment's `deploy/` files at
  all; there was never a copy of that guard here to remove, and the
  celld process count in `supervisord.conf` (one) is unrelated and unchanged.
- `docs/threat-model.md`'s single-node warning was about the deployment's own
  MinIO container: "MinIO is a single node in this Compose deployment. It has
  no redundancy or failover and should be backed up consistently." Its reason
  was the specific container this deployment ran, and that container is gone,
  so the warning is gone. It is replaced with a note that this deployment
  provisions no redundancy of its own and relies on the durability and backup
  properties of whatever S3 bucket the operator provisions.
- Nothing else fenced on MinIO's conditional-write gap was found in
  `deploy/`, `docs/`, `README.md`, or `FINDINGS.md`.

The compose file shrank: the `minio` service, its healthcheck, and the
`minio-data` volume are gone, and the `agent` service no longer depends on a
`minio` healthcheck. `deploy/agent.Dockerfile` no longer copies `mc`. The
operator must now provision, themselves: an S3 bucket with conditional-write
support (real Amazon S3 has this; MinIO Community does not), in a chosen
region, and an IAM identity with read and write access to it.

While auditing the Dockerfile I also found `deploy/agent.Dockerfile` still
had a `docker:27-cli` stage and a `COPY --from=docker` step, left over from
when the sandbox ran as a Docker container the trusted host controlled
through the Docker socket. Nothing in the codebase invokes the `docker`
binary anymore now that `agent/sandbox/sandbox.ts` uses the Box backend
instead, so that stage and the socket mount in `docker-compose.yml` are dead
weight from the same era as MinIO, and removing them is not part of the
MinIO-to-S3 correctness fix itself but is bundled here since both are
`deploy/` cleanup driven by the same code change already in this tree.

Not fixed, and out of scope for this pass: `deploy/agent.Dockerfile` still
runs `COPY executor ./executor`, but the `agents/executor/` directory no
longer exists on disk (its files were deleted in the celld migration). That
`COPY` will fail a real build. It predates this pass, lives partly outside
this pass's assigned scope, and touching it risks colliding with in-flight
work on `agents/agent`, `agents/src`, and `agents/test` that is explicitly
someone else's. Flagging it here rather than silently fixing it.

## The Box sandbox

`agents/agent/lib/box-sandbox.ts` wraps `@asciidev/eve-box` because the
published adapter targets Eve 0.11 and its type declarations claim
conformance with a shape it does not actually implement at runtime: Eve 0.51
calls `delete()`, `stop()`, and `shutdown()` on a sandbox handle, and the
shipped class only ever defined `captureState()` and `dispose()`. The gap is
invisible at the type level and only surfaces when the runtime calls a
method that does not exist. The wrapper fixes `noEnv` on unconditionally, and
maps both `stop` and `shutdown` to a Box stop, and `delete` to the Box delete
endpoint. `agent/sandbox/sandbox.ts` now calls `boxSandbox()` instead of the
old Docker backend. `EVE_SANDBOX_IMAGE` is dead and is removed from
`deploy/env.example`, `deploy/docker-compose.yml`, and `deploy/start-agent.sh`;
nothing in the deployment reads it anymore.

This is a real change to the sandbox trust boundary, documented in full in
`docs/threat-model.md`. In short: a box is a full Linux VM with sudo, Box's
own docs never name a hypervisor, and this project does not invent one to
name either. Boxes run only in Box's EU regions (Germany, Finland, France),
so job data and snapshots stay there. Box has no compliance certification
yet; SOC 2 is in progress. Machine types are `small`, `default`, and `large`,
all `x86_64`. `noEnv` withholds the operator's Box account secrets, secret
files, GitHub credentials, SSH identity, and selected repositories from every
box; that withholding is the isolation the design now depends on, because Box
cannot enforce Eve's fine-grained network policies. The previous deploy's
deny-all egress rule is gone, and sandbox code can now reach the internet.
Stop snapshots the box's disk and is free; resume takes seconds; fork copies
the whole filesystem. Egress is included up to 2 TB per box per month, and
billing is per second of machine time.

Known gap, not solved here: the adapter does not fork prepared template
boxes, so per-session setup replays on every new box rather than cloning a
snapshot. Upstream issue
[ariana-dot-dev/eve-box#1](https://github.com/ariana-dot-dev/eve-box/issues/1)
tracks it. Because `EVE_SANDBOX_IMAGE` is gone, nothing now pre-installs a
target repository's build dependencies, so a job's required tests need an
install step on a cold box every time. No workaround is implemented for
either gap; both are recorded as open in `docs/threat-model.md` and
`docs/customer-hosted.md`.
