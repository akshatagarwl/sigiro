# Spec: Cloudflare-native hosted Sigiro with generic agents

Decisions: [ADR-0001](../../adr/0001-hosted-runs-on-the-portable-binding-subset.md), [ADR-0002](../../adr/0002-segments-and-manifest-replace-ducklake.md), [ADR-0003](../../adr/0003-ingest-cells-keep-rows-out-of-durable-object-storage.md), [ADR-0004](../../adr/0004-agents-are-pi-durable-sessions-in-cells.md), [ADR-0005](../../adr/0005-codemode-runs-in-dynamic-workers.md). Vocabulary: [CONTEXT.md](../../../CONTEXT.md).

## Problem Statement

Hosted Sigiro runs on one Hetzner VM. It stitches together the self-hosted binary, a PostgreSQL DuckLake catalog, a Rivet engine, Eve, a Node ingest relay, a polling optimizer worker, and a separate Cloudflare Sandbox Worker. That host has blocked production repeatedly. The signed DuckLake extension is too old to commit to a PostgreSQL catalog. The VM has no KVM, so Microsandbox is impossible. Codex login, checkpoint encryption, leases, and session reconciliation all exist only to bridge the host to Cloudflare. The optimizer has never completed a verified end-to-end run. Every new capability costs another service and another secret on the host, and the operator pays for a VM that is idle most of the time.

## Solution

Hosted Sigiro becomes one Workers application on Cloudflare that uses only the portable binding subset, so the same application also runs on a self-hosted celld fleet. Telemetry is stored as Parquet segments in R2 and listed by a per-tenant manifest. Ingest cells group writes so that telemetry rows never pass through Durable Object storage. Queries run in DuckDB-wasm. The single-purpose optimizer becomes a generic agent. Tenants define agents (instructions, model, enabled extensions). Each session is a cell hosting a pi-durable Harness. Extensions provide telemetry query, a credential-free Cloudflare Sandbox for repository work, GitHub draft pull requests, and tools from tenant-registered remote MCP servers. The model can compose all of them in codemode scripts that run in network-less Dynamic Workers. The self-hosted `sigiro` binary stays a single executable and adopts the same segment and manifest format through a shared Rust crate, so CLI, MCP, and SDK users see one API and one data format whichever deployment they use. Hetzner is decommissioned.

## User Stories

1. As an SDK user, I want to send OTLP traces, logs, metrics, and profiles to the hosted endpoint with a tenant-scoped token, so that my telemetry is stored without running any infrastructure.
2. As an SDK user, I want the hosted OTLP endpoint to accept the same HTTP protobuf and JSON encodings as the self-hosted binary, so that I can switch deployments by changing only the endpoint.
3. As an SDK user, I want a request to be acknowledged only once Sigiro will not lose it, so that exporter retries are the only recovery I need.
4. As an SDK user, I want an oversized or malformed request rejected with an OTLP-conformant error, so that my exporter neither retries forever nor silently drops data.
5. As a tenant, I want my telemetry to be unreadable and unwritable by every other tenant, so that sharing hosted infrastructure is safe.
6. As an agent using the CLI or MCP, I want `/v1/query`, `/v1/services`, and `/v1/traces/{trace_id}` to behave identically on hosted and self-hosted, so that my prompts and tools do not branch on deployment.
7. As an agent, I want data written seconds ago to become queryable within a bounded, documented delay, so that I can investigate live behavior.
8. As an agent, I want a query that exceeds the memory, time, or row bounds to fail with a typed, actionable error, so that I can narrow it instead of timing out.
9. As an agent, I want only read-only SQL over a tenant's own telemetry to be accepted, so that a query can never mutate or escape the tenant.
10. As a tenant, I want segments older than my retention period removed automatically, so that storage cost stays bounded and the retention promise holds.
11. As an operator, I want small segments compacted into larger ones without changing query results, so that queries stay fast and storage request counts stay low.
12. As an operator, I want each tenant's live segments to come from exactly one manifest, so that a crashed flush or compaction never leaves duplicate or missing rows visible.
13. As an operator, I want a measured cost per million ingest requests and per GB stored, so that pricing and limits rest on evidence.
14. As an operator, I want ingest flush window, acknowledgement point, and cell sharding chosen from a formal durability model and real measurements, so that the cheapest safe policy is used.
15. As an operator, I want a hot tenant's ingest to split across more cells without changing the data format, so that one tenant cannot saturate one cell.
16. As an operator, I want the hosted application to deploy from one configuration to Cloudflare or to celld, so that self-hosting the full hosted stack needs no fork.
17. As an operator, I want the deployment to fail if it uses a binding that celld does not implement, so that portability cannot regress silently.
18. As a self-hosted user, I want `sigiro serve` to remain one binary with no external database, so that self-hosting stays trivial.
19. As a self-hosted user, I want the binary to write the same segment and manifest format as hosted, so that I can move data between my bucket and a hosted tenant.
20. As a self-hosted user upgrading from DuckLake, I want a clear statement of what happens to my existing data, so that an upgrade is not a surprise loss.
21. As a tenant admin, I want to define an agent with instructions, a model, and a chosen set of extensions, so that one platform serves many agent use cases.
22. As a tenant admin, I want to register a remote MCP server with a credential that Sigiro stores write-only, so that my agents can use my own tools without exposing the credential.
23. As a tenant admin, I want to create, enable, and disable GitHub repository connections, so that I control which repositories agents may touch.
24. As an authorized user, I want to start a session with an agent using a stable session ID, so that retries never start a second session.
25. As an authorized user, I want to send messages to a session with a request ID, so that a retried send is never answered twice.
26. As an authorized user, I want to follow a session's transcript, tool calls, and usage live and after the fact, so that I can see what the agent did and what it cost.
27. As an authorized user, I want to steer or abort a busy session, so that I can redirect or stop the agent.
28. As an authorized user, I want an interrupted session to continue from its last committed step, so that crashes and deploys cost no model work already done.
29. As an agent with the telemetry extension, I want bounded read-only queries over my tenant's telemetry only, so that I can investigate without reaching other tenants.
30. As an agent with the sandbox extension, I want to edit and validate a connected repository in a sandbox with no credentials and no network, so that untrusted repository code cannot exfiltrate secrets.
31. As an agent with the GitHub extension, I want to open at most one draft pull request per session and branch, even after resuming, so that users never receive duplicates.
32. As an agent, I want to write a codemode script that chains many tool calls and returns only its result, so that large intermediate data never fills my context.
33. As an operator, I want codemode scripts to have no capability except calling the session's enabled tools, with time and memory limits, so that model-written code cannot reach the network or credentials.
34. As an operator, I want the Codex subscription login stored encrypted and refreshable inside the application, and every other secret held as a Worker secret or encrypted in a trusted cell, so that no host file, model, script, or sandbox holds a live credential.
35. As an operator, I want disabling a repository or MCP connection to make every later call through it fail closed, so that I can cut access immediately.
36. As an operator, I want a session's sandbox destroyed when the session goes idle past a limit or ends, so that no sandbox lingers and bills.
37. As an operator, I want existing Auth (Better Auth on D1) and its JWT scopes to keep working unchanged, so that tokens and the CLI keep working through the cutover.
38. As an operator, I want to cut over by pointing exporters at the hosted ingest, then decommission Hetzner after the 7-day retention window, so that no data migration is needed.
39. As an operator, I want the hosted application to send its own traces to a Sigiro tenant, so that hosted Sigiro is observable with itself.
40. As a developer, I want one contract test suite that runs against any Sigiro endpoint URL, so that hosted, celld, and self-hosted parity is proven by the same tests.

## Scenarios

```gherkin
Feature: Hosted telemetry ingest

  Scenario: An acknowledged span becomes queryable
    Given a tenant with an ingest-scoped token
    When the SDK exports a span for service "checkout" and receives success
    Then within the documented freshness bound a tenant query returns that span

  Scenario: Acknowledged data survives an ingest cell restart
    Given an exporter request has been acknowledged
    When the ingest cell is evicted before its next flush
    Then a later tenant query still returns every acknowledged row exactly once

  Scenario: Tenants are isolated
    Given tenant A has exported spans for service "checkout"
    When tenant B queries spans for service "checkout"
    Then tenant B receives no rows from tenant A

  Scenario: An oversized request is rejected
    When the SDK exports a request larger than the ingest limit
    Then it receives an OTLP-conformant error that is not retryable

Feature: Query parity

  Scenario Outline: The same query gives the same result on every deployment
    Given the shared fixture telemetry is ingested into <deployment>
    When the agent runs each contract query
    Then each result equals the recorded expected result

    Examples:
      | deployment            |
      | the self-hosted binary |
      | hosted on Cloudflare  |
      | hosted on celld       |

  Scenario: A query beyond its bounds fails clearly
    When the agent runs a query that would exceed the memory bound
    Then it receives a typed error naming the bound and how to narrow the query

  Scenario: A mutating query is refused
    When the agent submits a statement that writes or attaches storage
    Then it is rejected before execution

Feature: Storage lifecycle

  Scenario: Compaction does not change results
    Given a tenant with many small segments
    When compaction runs
    Then the segment count falls and every contract query returns the same result

  Scenario: Retention removes old data
    Given segments older than the tenant's retention period
    When retention runs
    Then those rows are no longer queryable and their objects are deleted

  Scenario: A crashed flush leaves no partial data visible
    Given a flush wrote a segment object but did not record it in the manifest
    Then queries do not return that segment's rows
    And the orphaned object is later deleted

  Scenario: Self-hosted and hosted formats are interchangeable
    Given segments and a manifest written by the self-hosted binary
    When the hosted query Worker reads them
    Then contract queries return the same results

Feature: Agent sessions

  Scenario: A retried session start returns the same session
    Given an agent "investigator" in tenant A
    When a user starts session S with that agent twice
    Then both calls return session S and only one session exists

  Scenario: A retried message is answered once
    Given an idle session
    When the user sends a message with request ID M twice
    Then the transcript contains that message and its answer once

  Scenario: A session resumes after its cell restarts
    Given a session that has committed a tool result mid-run
    When the session cell restarts
    Then the run continues from that tool result and the final answer arrives

  Scenario: A codemode script composes tools and returns only its result
    Given a session with the telemetry extension enabled
    When the model runs a codemode script that issues three telemetry queries and returns a summary
    Then the transcript contains the summary and not the raw query results

  Scenario: A codemode script cannot reach the network
    When a codemode script tries to fetch an external URL
    Then the script fails and no request leaves the Dynamic Worker

  Scenario: Telemetry is tenant-fixed
    Given a session in tenant A
    When the agent queries spans
    Then it receives only tenant A's spans

  Scenario: A tenant MCP tool is called without exposing its credential
    Given tenant A registered an MCP server with a credential
    When the agent calls one of its tools
    Then the call succeeds and the credential appears nowhere in the transcript, script, or sandbox

  Scenario: A disabled MCP connection fails closed
    Given tenant A disabled its MCP connection
    When the agent calls one of its tools
    Then the call fails with a "connection disabled" error

  Scenario: The sandbox holds no credentials
    Given a session with the sandbox extension and a connected repository
    When the agent inspects the sandbox environment, files, and network
    Then it finds no GitHub, Sigiro, MCP, or model credential and cannot reach the network

  Scenario: A resumed session does not open a duplicate draft pull request
    Given a draft pull request was created for the session's branch but not yet recorded
    When the session cell restarts and the agent submits again
    Then the existing draft pull request is found and recorded
    And no second pull request is created
```

## Implementation Decisions

### Runtime boundary

- Hosted Sigiro is one Workers application deployed from one Wrangler configuration. Allowed bindings are Workers, Durable Objects with SQLite, R2, Queues, Workflows, Cron Triggers, D1, Dynamic Workers, the Sandbox SDK, and service bindings. A deployment check fails on any other binding or on a top-level configuration key that celld rejects.
- Every Worker and Durable Object isolate is limited to 128 MB on both platforms; designs assume that limit.
- The existing Auth Worker (Better Auth on D1) and its JWT scopes are reused unchanged. Hosted routes verify the same signed tenant JWTs the binary verifies today. There is no static-key fallback.

### Shared Rust core

- A new crate, used by the binary natively and by ingest cells as wasm (workers-rs), owns OTLP HTTP decoding (protobuf and JSON), the per-signal table schema, the Parquet segment writer, and the manifest format and its operations: add segments, replace segments (compaction), drop segments (retention), and list live segments for a time range.
- The manifest is versioned. Each change is atomic and stamps a monotonically increasing generation. A segment object is written before the manifest records it, and is deleted only after the manifest stops referencing it and a grace period has passed, so readers holding an older generation never lose data mid-query.
- Segment object keys are tenant-scoped and content-addressed or generation-scoped so a retried flush cannot overwrite a committed segment.
- The binary replaces DuckLake with the shared format: local directory or S3-compatible bucket for segments, SQLite for the manifest. DuckLake, its PostgreSQL catalog, the signed-extension cache repair, and the unsigned-extension opt-in are removed.

### Ingest

- An OTLP ingest Worker authenticates, enforces the body limit, decodes via the shared crate, splits rows by signal, and forwards them to the tenant's ingest cell or cells.
- Ingest cells keep telemetry rows out of Durable Object storage (ADR-0003). They group rows in memory and flush each group as one segment plus one manifest change.
- **Open until the spike decides:** the flush window and size; whether a request is acknowledged after the segment is in R2 (hold-open, bounded by the default 10 s exporter timeout) or after a per-request compressed blob is written to cell storage as a write-ahead record; the cell key (tenant, tenant×signal, or with shard suffixes); and the sharding trigger. The decision must come from (a) a TLA+ or PlusCal model of acknowledge → write-ahead → flush → manifest under eviction, retry, and concurrent compaction, checked for "every acknowledged row is queryable exactly once", and (b) measured cost and latency for each candidate on Cloudflare and on celld at representative request sizes and rates. The chosen policy and its evidence are recorded as a new ADR before the ingest cell is built.
- Where the manifest lives (inside each ingest cell, or a per-tenant catalog cell fed by ingest cells) follows from the chosen cell key.

### Query

- A query Worker serves `/v1/query`, `/v1/services`, `/v1/traces/{trace_id}`, and `/v1/auth/validate` with the same request and response contract as the binary's OpenAPI document. CLI and MCP therefore need only an endpoint change.
- It reads the live segment list for the requested tenant and time range from the manifest, then runs the bounded, read-only SQL in DuckDB-wasm (`@ducklings/workers`) over those R2 segments, using the binary's existing SQL allowlist rules.
- DuckDB-wasm is about 9.7 MB gzipped, close to the compressed Worker limit, so the query Worker is its own Worker reached by a service binding. A spike must prove it fits, initializes, and reads R2 Parquet inside 128 MB on both Cloudflare and celld before the query Worker is built.
- Before DuckDB runs, the query Worker fetches the needed segment byte ranges, or whole small segments, from R2 in parallel. Measured on Cloudflare, each ranged read takes about 150 ms and DuckDB-wasm issues them one at a time, so an uncached query that needs 41 reads took 7 to 20 s.
- Queries carry hard time, memory, and row bounds and fail with typed errors. Fan-out across several isolates for large scans is not in v1.

### Storage lifecycle

- A Cron Trigger starts per-tenant compaction and retention, each a Workflow whose steps write the replacement segment, change the manifest atomically, and delete superseded objects after the grace period. Orphaned objects that no manifest generation references are deleted by the same sweep.

### Agents

- The optimizer API (connections, mappings, runs, the internal optimizer query route), the Jev assessment gate, and service mappings are removed without backwards compatibility, as the anomaly APIs were.
- Control API (tenant-scoped, JWT-authenticated): manage agents, repository connections, and MCP connections (`tenant:admin`); start sessions, send messages with a request ID (follow-up, steer, or reject when busy), abort, and read or stream a session's view and events (`agent:write`). The `optimizer:write` scope is replaced by `agent:write` through a guarded Auth D1 resource-scope migration. Agents and connections live in a per-tenant agent-registry cell; MCP credentials are encrypted there and are write-only through the API.
- An agent stores names, not code: instructions, a model choice, and enabled extensions with their per-agent configuration (for example the repository connection for the sandbox and GitHub extensions, or the MCP connections to expose).
- Each session is one cell (ADR-0004) hosting a pi-durable Harness on its SQLite storage. Session IDs are caller-supplied and stable; messages use pi-durable request IDs for idempotence. Its view and agent events are exposed over the control API for UIs, the CLI, and MCP clients.
- Built-in extensions in v1:
  - **telemetry:** bounded, read-only, tenant-fixed queries through the query Worker, with the same SQL allowlist.
  - **sandbox:** pi-durable read, write, edit, and bash through an execution environment backed by the Sandbox binding. The trusted side produces a pinned, size-bounded checkout of the configured repository with a GitHub App installation token that never enters the sandbox. Commands run with `enableInternet = false` plus a fresh network namespace, with paths confined to the workspace. Today's checkout bounds and symlink rules carry over.
  - **github:** submit-draft collects the patch from the sandbox, then on the trusted side verifies the repository, base, and stable session branch, checks for an existing branch and pull request, pushes, opens a draft pull request, and records it. On resume it reconciles against GitHub before any mutation.
  - **mcp:** for each enabled MCP connection, the session cell lists the server's tools and exposes them as tools. Calls are made from the trusted side with the stored credential.
  - **codemode:** one `codemode` tool whose script runs in a Dynamic Worker with no outbound network (ADR-0005). The script sees TypeScript declarations of the session's other enabled tools, rendered by pi-codemode's declaration renderer, and calls each through an RPC binding back to the session cell. The cell applies the same authorization, hooks, and usage accounting as a direct call. Scripts have time, memory, and output limits. Only the script's output and return value enter the transcript.
- Model access uses `openai-codex` through `@earendil-works/pi-ai`. Its refreshable OAuth state lives encrypted in a single credentials cell, which refreshes it and lends short-lived access to session cells. An operator completes the device login once through an authenticated admin route.
- Disabling a repository or MCP connection makes every later call through it fail closed. A session's sandbox is destroyed after an idle limit or when the session is deleted.
- Removed: Eve, Rivet and its PostgreSQL role, the polling lease worker, host checkpoint archives and their key, the Node ingest relay, the host Codex login helper, and the separate sandbox bridge Worker with its API key.

### Cutover

- Exporters are repointed at the hosted ingest. Hetzner services stay read-only for 7 days so old telemetry ages out, then the VM and its Alchemy stacks are retired. Existing optimizer run records are not migrated; their historical audit is exported before decommissioning.

## Testing Decisions

- A good test drives Sigiro only through its public contract: OTLP export, the `/v1` HTTP API, and the agent control API. It asserts on what an SDK user, agent, or operator observes, not on cell internals, segment layout, or manifest rows.
- **Primary seam (one suite, three targets):** a contract suite parameterized by endpoint URL ingests a shared fixture through OTLP and asserts query, service, trace, isolation, bounds, and error results. It runs against the self-hosted binary, the Workers application under celld dev (no Cloudflare account needed in CI), and a disposable Cloudflare deployment before release. The Gherkin scenarios above are stored as `.feature` files and drive it.
- **Durability seam:** the eviction, crashed-flush, compaction, and retention scenarios run against celld dev, which can kill a cell owner and restart it, and are cross-checked against the TLA+ model's invariants.
- **Agent seam:** the agent control API, with the model replaced by pi-ai's faux provider for deterministic turns, GitHub replaced by the `emulate` GitHub App emulator (prior art: the existing emulated draft-PR tests), a fake MCP server, and a real Sandbox and Dynamic Worker under celld dev or Cloudflare. Restart scenarios kill and restart the session cell. One smoke test with a real GitHub App installation, a disposable repository, and a real Codex login is required before the GitHub extension is described as verified.
- **Format seam:** the shared crate's round-trip and cross-reader tests (binary writes, Workers read, and vice versa) live at the crate's public API.
- Prior art: the existing optimizer integration tests (emulated GitHub, live Cloudflare Sandbox, composed flow), the OpenAPI parity assertions in the server tests, and `tests/mcp_composition_test.rs`.

## Out of Scope

- Scheduled or telemetry-triggered sessions, a Jev assessment extension, policies, budgets, PR lifecycle tracking, and post-merge outcome measurement.
- Tenant-authored extension code; tenant-registered stdio MCP servers. Only remote MCP servers are supported.
- Cross-isolate distributed query fan-out, and query features beyond the binary's current SQL surface.
- Migrating existing hosted telemetry; it ages out instead.
- Migrating existing self-hosted DuckLake data automatically; the upgrade note documents the boundary.
- Cloudflare-only products: Pipelines, R2 Data Catalog, Hyperdrive, Workers AI, AI Gateway, Analytics Engine.
- Quack edge federation in the hosted application.
- Billing and pricing changes beyond producing the measured cost model.

## Further Notes

- First work, in order: (1) the ingest durability and cost spike producing an ADR; (2) the DuckDB-wasm-in-Workers spike on Cloudflare and celld; (3) the shared crate's segment and manifest format. Ingest, query, and agent slices follow; the agent slices can start in parallel once the codemode-in-Dynamic-Workers spike passes on Cloudflare and celld.
- `@earendil-works/pi-durable`, `@earendil-works/pi-codemode`, and `@ducklings/workers` are experimental or young; pin exact versions and keep each behind one module so it can be replaced.
- celld 0.x is alpha and runs a single-tenant trust model; hosted multi-tenancy is validated on Cloudflare, and celld is the self-hosting target.
- This spec supersedes the former automatic optimizer design and the host deployment sections of `docs/notes/optimizer-api.md`.
