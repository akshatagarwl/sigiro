# Sigiro assistant prototype

Eve owns the coding-agent loop and session API; Rivet World runs the durable workflow, while repository execution takes place in the credential-free Cloudflare Sandbox. The current model uses Eve's experimental ChatGPT-subscription adapter (`experimental_chatgpt()`, default `gpt-5.6-sol`). It reads a writable `auth.json` from `CODEX_HOME`, populated by `codex login`; no AI Gateway API key is needed. The subscription adapter is experimental and token refresh requires the login file to remain writable in the trusted Eve container. For the authenticated optimizer API, containerized Eve and worker, Alchemy deployment, and remaining live prerequisites, see [the optimizer deployment guide](../../docs/notes/optimizer-api.md).

## Local use

Use Node.js 24 or newer and Aube 2.2.16. From the repository root, run `aube -C agents/agent install --frozen-lockfile`, `aube -C agents/agent run build`, and `aube -C agents/agent run start`. Run `codex login` in the same `CODEX_HOME` as Eve for a model-driven run; do not copy the OAuth file into source or a sandbox. Set `SIGIRO_OTLP_TRACES_URL` to Sigiro's OTLP HTTP traces endpoint (for example, `http://127.0.0.1:4318/v1/traces`) and `SIGIRO_API_KEY` to a scoped ingest bearer token **on the controller or in a secret manager**, not in source, the PR, or chat. Both are required by this agent at startup. A local self-hosted Sigiro in Open auth mode ignores the supplied bearer token and maps data to `default`; it is protected only by its network perimeter, not by that token. Hosted Sigiro requires a valid `telemetry:ingest` token. `aube -C agents/agent run dev` runs Eve headlessly for development. The built server binds `127.0.0.1:3000`; development binds `127.0.0.1:2000`. Do not expose Eve's unauthenticated HTTP session API to untrusted networks. No operator CLI, approval step, or UI is included. `optimizer/github.ts` is the trusted GitHub App publication step, wired to the tenant-authorized API and Eve worker via the Cloudflare Sandbox boundary. It uses GitHub's Git database API to write blobs, a tree, and a commit without ever passing an installation token to a Git process. It verifies installation repository identity and the target SHA, uses `sigiro/optimizer-<run-id>` as its stable branch, refuses to replace a foreign branch, reconciles an existing draft PR, and recovers after a lost PR-create response. Callers must supply evidence-redacted title/body and changes from a verified credential-free workspace. Run `aube -C agents/agent run test:optimizer:github` to exercise it against a resettable, seeded emulate GitHub App with a generated test private key. The emulator does not emulate git clone/push and this test does not establish live GitHub compatibility. `optimizer/leases.ts` and `optimizer/worker.ts` provide a fenced PostgreSQL claim/resume boundary, driven by `optimizer/main.ts` in the separately supervised worker container. `aube -C agents/agent run test:optimizer:leases` checks leases, recovery, and worker publication against disposable PostgreSQL and the GitHub emulator. It does not establish Microsandbox isolation or Rivet session resumption.

Call the running server programmatically with `Client` from `eve/client`: create a session, send a task, then consume `response.result()` for status and text. Eve manages follow-up turns and session state. The former Rivet `tasks` queue and `readResult` action are removed, not aliased.

## Agent-run monitoring

Sigiro stores Eve's `ai.eve.turn` parent span, Workflow child spans, and (when a model call succeeds) AI SDK model/tool spans under the Eve service name. Copy the session ID returned by `eve/client` and use Sigiro's `POST /v1/query` endpoint (HTTP API port, not OTLP port). Hosted deployments require a separate `query:read` credential; local Open mode has no per-request authentication. Bound each query to the run's time window:

```sql
SELECT timestamp, trace_id, span_name, status_code
FROM lake.sigiro_spans
WHERE service_name = 'agent'
  AND timestamp >= TIMESTAMP '<start-utc>'
  AND timestamp < TIMESTAMP '<end-utc>'
  AND json_extract_string(span_attributes, '$."eve.session.id"') = '<session-id>'
ORDER BY timestamp DESC
```

The `ai.eve.turn` span carries the session ID; use its `trace_id` to inspect related Workflow spans, which need not carry `eve.session.id` themselves:

```sql
SELECT timestamp, span_name, status_code
FROM lake.sigiro_spans
WHERE trace_id = '<trace-id>'
  AND timestamp >= TIMESTAMP '<start-utc>'
  AND timestamp < TIMESTAMP '<end-utc>'
ORDER BY timestamp
```

The local Rivet engine has a separate native OTLP/gRPC exporter: launch Eve with `RIVET_OTEL_ENABLED=1` and `RIVET_OTEL_GRPC_ENDPOINT` set to Sigiro's gRPC endpoint (for example, `http://127.0.0.1:4317`). Set `RIVET_OTEL_SAMPLER_RATIO` explicitly for the expected volume; a ratio of `1` produced over 50,000 spans during startup and one failed local turn. `RIVET_EXPERIMENTAL_OTEL=1` enables RivetKit actor tracing but is not an exporter. The engine exporter did reach an isolated local Sigiro in Open auth mode. Engine span `actor_key` included the Eve session ID (as `Some("<session-id>")`), allowing a time-bounded cross-service lookup even though engine traces did **not** share the Eve turn's OTel trace ID:

```sql
SELECT timestamp, trace_id, span_name, status_code
FROM lake.sigiro_spans
WHERE service_name = 'rivet'
  AND timestamp >= TIMESTAMP '<start-utc>'
  AND timestamp < TIMESTAMP '<end-utc>'
  AND json_extract_string(span_attributes, '$.actor_key') LIKE '%<session-id>%'
ORDER BY timestamp DESC
```

This is an observed local join, not a guaranteed trace-context link. Engine spans include raw actor keys and may include other attributes such as `opts`; review/redact these before exporting production traffic. The engine's gRPC exporter has not been verified with hosted Sigiro authentication; do not direct it to an unauthenticated public endpoint. Engine metrics export was not observed. An exported span proves that part of the run reached Sigiro; it does not prove completion or clean actor lifecycle. **agentOS VM/sidecar, component logs, and metrics are not yet ingested or correlated.** Do not treat missing telemetry as a successful component or a clean run. Eve's own Vercel Workflow `$eve.*` tags are separate from these OTel traces.

## Source

- `agent/agent.ts`: Eve agent, AI Gateway model, and Rivet World configuration.
- `agent/instrumentation.ts`: exports Eve/Workflow and AI SDK traces to Sigiro without prompt, model-output, or default fetch-instrumentation capture.
- `agent/sandbox.ts`: Cloudflare Sandbox backend for Eve's workspace.
- `agent/instructions.md`: keep durable work under `/workspace`.
- `actors.ts`: agentOS VM and Rivet World actor registry.
- `world.ts`: Rivet World factory.

Earlier local Eve and agentOS actor tests proved only a workspace write, not repository edits, validation, or a Sigiro verdict. The former in-process VM is not the production containment boundary; Cloudflare Sandbox execution is documented in the optimizer guide. The authenticated bridge exposes `GET /v1/operator/assignments` to inspect retained sandbox IDs and intercepts `DELETE /v1/sandbox/{id}` to stop a known assignment without allocating a container slot. The stock bridge allocates before DELETE, which prevents cleanup once the account's two slots are full; reclaim only after verifying no optimizer run is active. Eve records a private sandbox ID immediately after workspace creation so completed runs can release their slot even if no checkpoint was needed. A fully authenticated, model-driven Rivet run with a persisted real draft PR has not yet been verified. Keep the pull request draft.
