# sigiro agents

An agent turns a telemetry finding into a tested draft PR, coordinated by celld
and executed by Eve, with an Effect pull worker. The worker pulls assignments outbound, so a
local/customer-hosted pull worker works without inbound worker networking.

The host-side anomaly bridge connects detection to this execution path:
`anomaly → diagnosis → immutable evidence → job`. Run one pass or watch with
`bun run bridge --config policy.json [--watch]`; configuration, recovery, and
current limits are in [Anomaly bridge](docs/anomaly-bridge.md).

## Execution flow

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
                              · subscription model, no API key
                              · path policy and draft-PR publishing
                              · writes a durable publication receipt
```

Celld holds durable state, retries, and leases. The worker owns claim,
durability, and completion reporting; Eve owns repository execution inside its
sandbox. The worker has no inbound listener. The coordinator's `WORKER_TOKEN`
authenticates bridge submission/status requests and worker claim, heartbeat,
and completion requests.

## What it enforces

- **One execution per attempt.** `executionId` is `jobId:generation:attempt`.
  A duplicate submission returns the existing receipt and a repeated completion
  does not create a second result.
- **Scope.** Changed paths are checked against the job's `allowedPaths` before
  any GitHub write. Workflow, action, git, and submodule machinery paths are
  always refused, whatever the policy allows, matched case-insensitively.
  Symlinks, gitlinks, and file type changes are refused rather than published
  as regular files.
- **Attribute trust.** Publication requires the current celld worker's authenticated
  execution ID, pinned base SHA, base ref, tests, and allowed paths to match the job.
  Git HEAD is checked before and after the required tests.
- **Credential separation.** Execution happens in Eve's sandbox. The worker
  token is never sent to the model, and the coordinator never sees provider
  credentials.
- **Publication receipts.** An attempt succeeds only when a durable *published*
  receipt exists for its `executionId`, never on an observed `turn.completed`.
  Eve's adapter wraps an authored event handler in `try`/`catch`, logs a throw,
  and stamps the event regardless, so `turn.completed` proves only that the turn
  ended. The trusted publisher writes the receipt after the required tests exit 0
  and the draft PR is published, or a *failed* receipt with the reason, and the
  worker polls `POST /internal/celld/job/result` for it. No receipt inside the
  bound is a failure naming the gap, not a success.
- **Durability.** Celld redelivers work after a lease expires, and the worker
  resumes persisted sessions after validating their lease, preserves the original
  deadline, and retries completion reporting.

## Model access

Claude uses the bundled Claude SDK with MCP tools restricted to Eve's sandbox.
ChatGPT uses Eve's built-in adapter: the bundled Codex runtime handles subscription
login/refresh and Eve sends model requests. No global client install or model API
key is required. Credentials stay on the customer's trusted host.

## Run it

For the three-service Docker Compose installation, follow
[the customer-hosted setup](docs/customer-hosted.md): Sigiro, MinIO, and an agent
container bundling celld, Eve, and the worker. GitHub and Claude sign-in use native
clients with persistent authentication. Investigations create temporary sandboxes.

For local development:

```sh
bun install
bun run build
bun run eve:build
```

Validation uses actual celld/Eve HTTP requests, Sigiro ingestion and both
subscription-to-draft-PR loops. Local demo files and E2E evidence are ignored in
`demo/`; they are not shipped with the agent.

Configure `SIGIRO_REPOSITORY`, `SIGIRO_EVIDENCE_DIR`, and `EVE_SANDBOX_IMAGE` on the trusted Eve host.
The image must include the repository’s offline build dependencies. Celld jobs carry an incident ID and SHA-256 evidence digest. Eve verifies a
bounded local artifact inside that directory before staging the exact Git commit
into its sandbox. GitHub publishing uses the host's authenticated `gh` account.

## Status and open work

Celld dispatch enters through the worker-token-gated
`POST /internal/celld/job` route in `agent/channels/celld.ts`. Eve prepares the
pinned checkout and runs the agent. `agent/lib/publication.ts` runs required tests
and calls the path-safe GitHub publisher directly.

The worker polls `POST /internal/celld/job/result` for a typed publication receipt
with the actual test results, commit, and PR. Eve owns its event stream; the worker
only reads cancellation events to confirm a requested stop. Separate atomic
success and failure receipt files prevent a late failure from overwriting a PR.

The bridge persists candidates and exact job submissions, remembers overlapping
incident members, and reserves its configured repository while work or a draft
PR remains outstanding. It supports one source/service/repository per state
directory on one host; the Compose installation does not start the bridge.
A durable source incident feed, distributed repository admission, publication
recovery across attempts, hosted tenant isolation, and production celld fleet
failover remain open. Private-repository access and draft-PR publishing use the
trusted host’s authenticated GitHub CLI. Do not claim
any-infrastructure or any-repository support yet.

Deployment: [customer-hosted setup](docs/customer-hosted.md). Boundary and residue:
[threat model](docs/threat-model.md).

## Agent observability

Eve exports native model/channel/tool spans; Claude MCP execution adds a span
through the same tracer. The worker uses Effect OTLP tracing/logging and native
HTTP trace propagation. Agent telemetry accepts the local Sigiro receiver at
`http://127.0.0.1:14318` or the Compose receiver at `http://sigiro:4318`;
agent payloads are excluded and safe diagnostic codes identify evidence and
receipt failures. Effect services/layers are the DI seam.

Verified fixes: receipt failures (#34), lease recovery (#35), Claude tool spans
(#36), early-deadline cancellation (#40), worker restart recovery (#41), and
publication receipt races (#42), and timeout error normalization (#43). Receipt polling removes stream cursor tracking (#39). Dogfooding found unresolved limitations: Sigiro's case-sensitive log
severity filters (#37) and celld0.4 dev reloads on file-open events in Docker
(#38). The anomaly bridge now provides optional polling; durable incident
identity across detector reinterpretation and measured4GB cloud capacity remain
future work.
