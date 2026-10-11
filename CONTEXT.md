# Sigiro

Sigiro ingests OpenTelemetry signals, stores them as queryable telemetry for agents, and runs an optimizer that turns telemetry evidence into draft pull requests.

## Language

### Deployment

**Hosted**:
The multi-tenant Sigiro service that runs as one Workers application on Cloudflare.
_Avoid_: cloud, SaaS backend

**Self-hosted binary**:
The single native `sigiro` executable that serves ingest, query, CLI, and MCP on a user's machine.
_Avoid_: native server, Rust server

**Cell**:
One named, single-writer Durable Object with its own SQLite storage; the unit of state, placement, and failover.
_Avoid_: actor, shard, instance

**Portable binding subset**:
The Cloudflare bindings that both Cloudflare and celld implement, and the only ones the hosted application may use.
_Avoid_: supported services

**Plan**:
A tenant's fixed-price hosted subscription (Hobby, Pro, Team, or Enterprise) that sets its caps and retention.
_Avoid_: tier, SKU, package

**Cap**:
A plan's hard monthly ingest volume or request-rate limit; reaching it stops ingest rather than adding charges.
_Avoid_: quota, overage, allowance

### Telemetry storage

**Tenant**:
An organization whose telemetry, repository connections, and optimizer runs are isolated from all others.
_Avoid_: project, workspace, account

**Signal**:
One OpenTelemetry data kind: spans, logs, metrics, or profiles.
_Avoid_: telemetry type, stream

**Table**:
One fixed column layout that a signal is stored in; logs and spans have one table each, metrics have one per metric kind.
_Avoid_: stream, dataset, collection

**Ingest cell**:
The cell that accepts one tenant's writes for one signal, groups them, and commits them as segments of that signal's tables.
_Avoid_: writer, buffer, flusher

**Segment**:
One immutable Parquet object holding a committed batch of rows from one tenant's table.
_Avoid_: file, chunk, part

**Manifest**:
The authoritative list of a tenant's live segments per table; a segment that is not in the manifest does not exist for queries.
_Avoid_: catalog, index, file list

**Flush**:
The commit that writes buffered rows as a new segment and records it in the manifest.
_Avoid_: checkpoint, sync

**Compaction**:
Replacing several small segments with one larger segment in a single manifest change.
_Avoid_: merge, vacuum

**Retention**:
Removing segments older than a tenant's retention period from the manifest, then from storage.
_Avoid_: TTL, expiry, cleanup

**Query budget**:
The memory one query may use; exceeding it is a reported error, never a partial answer.
_Avoid_: limit, quota

### Agents

**Agent**:
A tenant-defined configuration of instructions, model, and enabled extensions.
_Avoid_: optimizer, bot, assistant

**Session**:
One durable conversation with an agent, hosted in its own cell, that survives restarts.
_Avoid_: run, job, optimizer run, chat

**Extension**:
A named bundle of tools and prompt sections that an agent may enable.
_Avoid_: plugin, integration, skill

**Codemode script**:
Model-written JavaScript that can do nothing except call the session's enabled tools.
_Avoid_: code tool, eval, snippet

**MCP connection**:
A tenant-registered remote MCP server whose tools a session can call through Sigiro, with the credential held by Sigiro.
_Avoid_: MCP integration, remote tool

**Run sandbox**:
The per-session, credential-free Cloudflare Sandbox where repository files are edited and validated.
_Avoid_: VM, container, workspace

**Repository connection**:
A tenant's link to one GitHub App-installed repository, which may be enabled or disabled.
_Avoid_: repo, integration

**Draft pull request**:
The only way a session publishes repository changes; humans review and merge it.
_Avoid_: PR, auto-merge
