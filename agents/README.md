# sigiro agents

An agent turns a telemetry finding into a tested patch, coordinated by celld
and executed with Effect. The worker pulls assignments outbound, so a
local/customer-hosted pull worker works without inbound worker networking.

## Two processes

```text
       submit / status                 celld fleet
 client ───────────────────▶ coordinator (durable job state)
                              · alarm requeues an expired lease
                              · lease expires when a worker dies
                              · rejects stale generations and attempts
                                         ▲
                              outbound pull│ worker claim, heartbeat, complete
                                         │
                                  worker (Effect, Bun)
                              · claims, persists, and reports attempts
                                         │
                                  dispatch│ /internal/celld/job
                            receipt poll │ /internal/celld/job/result
                                         ▼
                                   Eve (sandboxed agent)
                              · exact base SHA checkout
                              · unmodified coding CLI, no API key
                              · path policy and draft-PR publishing
                              · writes a durable publication receipt
```

Celld holds durable state, retries, and leases. The worker owns claim,
durability, and completion reporting; Eve owns repository execution inside its
sandbox. The worker has no inbound listener, and `WORKER_TOKEN` is used only for
its authenticated worker-facing COORDINATOR endpoints.

## What it enforces

- **One execution per attempt.** `executionId` is `jobId:generation:attempt`.
  A duplicate submission returns the existing receipt and a repeated completion
  does not create a second result.
- **Scope.** Changed paths are checked against the job's `allowedPaths` before
  any GitHub write. Workflow, action, git, and submodule machinery paths are
  always refused, whatever the policy allows, matched case-insensitively.
  Symlinks, gitlinks, and file type changes are refused rather than published
  as regular files.
- **Attribute trust.** The publisher reads a job's `executionId`,
  `expectedBaseSha`, `baseRef`, `allowedPaths`, and `requiredTests` only when
  the *current* turn's auth is the celld worker. Eve's GitHub continuation token
  carries no execution nonce, so a later human `@bot` comment on the same issue
  or PR delivers into the same session with only `auth.current` replaced. A
  celld-initiated session therefore publishes nothing on a turn it did not
  initiate; a human-initiated session publishes under its own session id.
- **Anchor constraint.** An issue-anchored session executes at the repository
  default-branch tip, because Eve's checkout resolves a null head SHA to the
  default branch. A celld job anchored to an issue must set `baseSha` to that
  current tip; a mismatch is refused rather than silently re-checked-out.
- **Credential separation.** Execution happens in Eve's sandbox. The worker
  token is never sent to the model, and the coordinator never sees provider
  credentials.
- **Publication receipts.** An attempt succeeds only when a durable *published*
  receipt exists for its `executionId`, never on an observed `turn.completed`.
  Eve's adapter wraps an authored event handler in `try`/`catch`, logs a throw,
  and stamps the event regardless, so `turn.completed` proves only that the turn
  ended. The GitHub handler writes the receipt after the required tests exit 0
  and the draft PR is published, or a *failed* receipt with the reason, and the
  worker polls `POST /internal/celld/job/result` for it. No receipt inside the
  bound is a failure naming the gap, not a success.
- **Durability.** Celld redelivers work after a lease expires, and the worker
  persists records for recovery and completion retry.

## Model access

Both Codex and Claude use a subscription through the unmodified CLI with no API
key. OAuth state stays in a customer-owned directory mounted into the worker and
is never sent to celld or Sigiro.

## Run it

```sh
bun install
bun run typecheck && bun test
EVE_SANDBOX_PROVIDER=microsandbox bun run eve:build
```

There is no end-to-end proof at present. The previous harness drove a local
clone, provider subprocess, and patch-capture pipeline that no longer exists;
Eve now owns repository execution inside its sandbox. An Eve-backed end-to-end
proof, exercising celld dispatch through to a published draft pull request, is
open work.

## Status and open work

Eve's GitHub channel is the execution surface. `agent/channels/github.ts` wraps
`githubChannel` from `eve/channels/github`: Eve owns webhook verification,
session anchoring, and the `turn.started` checkout that clones the anchor into
the sandbox, and this repository adds one `turn.completed` handler. Because Eve
ships no branch, commit, and draft-PR publisher, `agent/lib/github-draft-pr.ts`
supplies one on top of Eve's authenticated `github.request` handle, and it is
the only enforcement point for the path policy and file modes.

Celld dispatch enters through `agent/channels/celld.ts`, a worker-token-gated
`POST /internal/celld/job` route that validates the job and calls Eve's
`receive()` to anchor a session on the target issue or pull request. The same
channel serves `POST /internal/celld/job/result`, which reads back the
publication receipt `agent/lib/publication-receipts.ts` stored for one
`executionId` and makes no GitHub call of its own.

Real private-repository auth, trigger and dedup integration, hosted tenant
isolation, and production celld fleet failover remain open. Do not claim
any-infrastructure or any-repository support yet.

Deployment: `docs/customer-hosted.md`. Boundary and residue:
`docs/threat-model.md`.
