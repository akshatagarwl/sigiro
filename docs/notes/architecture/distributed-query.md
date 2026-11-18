---
title: Distributed Query Architecture
created: 2026-03-21
updated: 2026-07-11
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'deployment', 'duckdb']
---

# Distributed Query Architecture

> **Operator guide:** for hands-on setup (perimeter, env vars, discovery,
> query grammar, tiering), see
> [[guides/self-host-edge-federation.md|Self-Host Edge Federation]]. This
> page is the architecture/decision record.

## Why single-node first

Sigiro's initial target: 20 services, 10 hosts, ~44 GB/mo of telemetry.

At this scale, a single [[architecture/data-model.md|DuckDB]] instance handles everything. DuckDB's columnar
engine processes analytical queries over millions of rows in milliseconds on a
single core. No sharding, no coordination, no distributed consensus needed.

## Concurrency model (DuckLake)

All access goes through `sigiro serve` HTTP API (`POST /v1/query`, `POST /v1/diagnose`).
There is no ordinary application `.duckdb` database — [[architecture/data-model.md|DuckDB]] runs in-memory and attaches [[architecture/ducklake-integration.md|DuckLake]]. In local mode it uses the DuckDB-native `catalog.ducklake` file catalog plus local [[architecture/data-model.md|Parquet]] files; distributed mode uses PostgreSQL metadata with local or S3/R2 data. The CLI, MCP server, and other clients are all HTTP proxies — they never touch the database directly.

## Self-host edge federation (quack) — shipped

> ⚠️ **Superseded design, deleted from the codebase.** An earlier iteration
> of this feature had collectors self-register with a central Sigiro node
> (`POST /v1/federation/workers`), which then fanned queries out via
> DuckDB's `ATTACH ... (TYPE quack)`. That design was deleted — it was
> broken (`ATTACH` doesn't push predicates down, so it pulled whole tables
> across the network) and insecure (no operator-owned network perimeter was
> ever required). If you find a doc, skill, or old note describing worker
> registration, a central/collector split, `SIGIRO_CENTRAL*`, or a
> `--central`/`--central-url` flag, it's describing that deleted design —
> it doesn't exist in the code anymore.

What actually shipped is a **self-host edge** model with no central node and
no registry:

- Each self-hosted Sigiro node ("edge") ingests OTLP into its **own** local
  DuckLake, same as any single-node deployment. Nothing is forwarded
  anywhere by default.
- Setting `SIGIRO_QUERY_SURFACE=quack` on a node serves that node's DuckLake
  data **read-only**, in-process, over the DuckDB `quack` client-server wire
  protocol (`src/quack.rs`) — no subprocess, no external binary.
- A separate command, **`sigiro query "<SQL>"`**, is the coordinator
  (`src/federate.rs`, `src/main.rs`). It discovers edges (via tailnet peers
  tagged `tag:sigiro-edge`, or an explicit `SIGIRO_EDGES`/`--edge`
  override), pushes a per-edge partial computed at each edge, and folds the
  partials into one answer.
- Every edge keeps its own data. Only query *results* cross the network —
  and only when an operator explicitly runs `sigiro query`.

```
Edge A: sigiro serve (SIGIRO_QUERY_SURFACE=quack)   Edge B: sigiro serve (SIGIRO_QUERY_SURFACE=quack)
    │ OTLP → own local DuckLake                         │ OTLP → own local DuckLake
    │ quack wire server on SIGIRO_QUACK_BIND:PORT        │ quack wire server on SIGIRO_QUACK_BIND:PORT
    └──────────────────────┬──────────────────────────────┘
                           │  private network only (Tailscale/WireGuard) —
                           │  quack has no working TLS of its own
                  sigiro query "<SQL>"   (run by an operator, anywhere on the tailnet)
                           │  1. resolve edges (tailnet tag:sigiro-edge, or --edge/SIGIRO_EDGES)
                           │  2. plan query -> per-edge partial + coordinator fold expression
                           │  3. quack_query() pushes the partial to each edge, pruned there
                           │  4. bridge each edge's Arrow result into one embedded DuckDB conn
                           │  5. fold partials (never naive avg-of-avgs) -> one answer
```

See [[guides/self-host-edge-federation.md|Self-Host Edge Federation]] for
the mandatory network perimeter, env vars, discovery mechanism, and the full
query grammar with examples.

### Why quack, and why `quack_query` not `ATTACH`

DuckDB's `ATTACH ... (TYPE quack)` was the first thing tried, and it's
**not** what the coordinator uses: plain `ATTACH` does not push predicates
to the remote side — it pulls whole tables across the network before
filtering locally, which is exactly the "no coordination between storage
tiers" problem this feature exists to avoid. `quack_query('quack://host:port',
'<sub-sql>')` does push the sub-SQL to the edge and run it there, with full
DuckLake partition/min-max pruning — that's the primitive the coordinator
actually uses (`src/federate.rs::wrap_quack_query`).

Neither [[architecture/ducklake-integration.md|DuckLake]] nor duckherder
provide this on their own: DuckLake is a storage format, duckherder is
distributed execution — neither merges across storage tiers or handles
time-based routing by itself.

## The Correctness Problem with Naive Distribution

In Kubernetes, a service like "checkout" has N replica pods across M nodes.
Each pod's [[architecture/write-path.md|OTLP]] data goes to the local sigiro node. One service's telemetry
data is split across multiple sigiro nodes. This is the **steady state**, not
a transitional problem.

### What breaks with naive scatter-gather

If each peer runs `investigate()` locally and the coordinator merges
InvestigationBlocks, these aggregates are **mathematically wrong**:

| Aggregate | Why it breaks |
|-----------|--------------|
| `approx_quantile(duration, 0.95)` | p95 of partials != p95 of whole. No merge strategy works. |
| `error_pct` | avg of ratios != ratio of sums (e.g., tiny node with 100% errors dominates) |
| `count(DISTINCT pod)` | sum of distinct counts != distinct of union |
| `avg(value)` | avg of avgs != avg of whole (measured ~13% off in this codebase) |

The federated query coordinator (`sigiro query`) handles the foldable subset
of this correctly: `count`/`sum`/`min`/`max` are naturally additive, and
`avg` is computed as `sum(per-edge sums) / sum(per-edge counts)` at the
coordinator — never as an average of per-edge averages. Anything not in that
foldable set (`approx_quantile`, `count(DISTINCT ...)`, and anything else
outside the supported grammar) is refused outright rather than silently
approximated — see the query grammar in
[[guides/self-host-edge-federation.md|the operator guide]].

### Solution history

**Phase 1 (completed):** Single-node [[architecture/data-model.md|DuckDB]] — no distribution needed at small scale.

**Phase 2 (deleted):** A central node + self-registering worker collectors,
fanning queries out via `ATTACH ... (TYPE quack)`. Replaced — see the
warning above.

**Phase 3 (current, shipped):** Self-host edge federation. No central node,
no registry. Each edge serves its own data read-only over `quack`; the
`sigiro query` CLI is the coordinator, using `quack_query()` push-down (not
`ATTACH`) and an additive-partial fold for correctness.

**Earlier approach (replaced before Phase 2):** Rust-native Apache
DataSketches with a custom peer-to-peer sketch merge protocol. The
correctness analysis above (why naive aggregate merging is wrong) remains
valid regardless of which federation mechanism implements the fix.

## Deployment Events

Sigiro ingests deployment data as OTel spans using standard CICD semantic
conventions (`cicd.pipeline.*`, `vcs.change.*`). No custom table, no custom
signal type.

**Sources that already emit OTel deployment data:**
- Jenkins OTel Plugin — pipeline runs as trace spans, build metrics, stage durations
- GitHub Actions OTel — workflow runs as spans
- Keptn — pre/post-deployment evaluations as OTel traces + metrics (K8s native)

**For non-OTel tools** (ArgoCD, Flux): thin webhook adapter (~50 lines) translates
webhook payloads to [[architecture/write-path.md|OTLP]] spans with `cicd.*` attributes via `POST /v1/events`.

```sql
-- Recent deployments
SELECT resource_attributes['service.name'], attributes['cicd.pipeline.result'], timestamp
FROM sigiro_spans WHERE span_name LIKE 'cicd.pipeline%'
  AND timestamp > now() - interval '24 hours' ORDER BY timestamp DESC;
```

## instance_status Table

Where a node stores its [[plans/STATUS.md|status]] depends on its mode.

Hosted/distributed nodes (`SIGIRO_CATALOG_URL` set) write to the PostgreSQL
table `sigiro_admin.instance_status`:

```sql
CREATE SCHEMA IF NOT EXISTS sigiro_admin;
CREATE TABLE IF NOT EXISTS sigiro_admin.instance_status (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

Local self-hosted nodes (no `SIGIRO_CATALOG_URL`) keep the same logical table in
`admin.sqlite`:

```sql
CREATE TABLE IF NOT EXISTS instance_status (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT DEFAULT (datetime('now'))
);
```

Keys include version, mode, platform_os, platform_arch, platform_kernel, and
pyroscope_status.

AI agents (or an operator) read this via HTTP to understand a node. There is no
central node that aggregates this across a fleet — read it per-node.

## S3 Cold Tier

This section describes DuckLake's native S3-writing capability as used in
Sigiro's **distributed/hosted multi-tenant mode** (`SIGIRO_CATALOG_URL`
set) — a separate concept from self-host edge federation above. **A
`SIGIRO_QUERY_SURFACE=quack`-servable edge does not support this today**
(the server refuses to start quack when `SIGIRO_CATALOG_URL` is set — see
[[guides/self-host-edge-federation.md|the operator guide's tiering
section]]); a self-host edge's data lives on local disk only.

### Write path: DuckDB COPY TO (not OpenDAL)

[[architecture/data-model.md|DuckDB]] natively writes [[architecture/data-model.md|Parquet]] to S3 with Hive partitioning in one SQL statement:

```sql
BEGIN;
COPY (SELECT *, year(timestamp) AS year, month(timestamp) AS month, day(timestamp) AS day
      FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY')
TO 's3://bucket/prefix/sigiro_spans/'
(FORMAT PARQUET, PARTITION_BY (year, month, day), COMPRESSION ZSTD, OVERWRITE_OR_IGNORE);
DELETE FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY';
COMMIT;
CHECKPOINT;
```

### Read path: httpfs + cache_httpfs

```sql
SELECT * FROM read_parquet('s3://bucket/prefix/sigiro_spans/**/*.parquet',
                           hive_partitioning=true)
WHERE timestamp BETWEEN ... AND ...;  -- DuckDB auto-prunes partitions
```

### Unified views

```sql
CREATE OR REPLACE VIEW sigiro_spans_all AS
    SELECT * FROM sigiro_spans
    UNION ALL
    SELECT * EXCLUDE (year, month, day)
    FROM read_parquet('s3://bucket/prefix/sigiro_spans/**/*.parquet',
                      hive_partitioning=true);
```

Zero overhead when S3 not configured (view aliases hot table directly).

## Alternatives evaluated and rejected

| Alternative | Why rejected |
|-------------|-------------|
| Custom peer-to-peer sketch merge (SIGIRO_PEERS) | Replaced by quack — proper distributed query execution |
| Central node + self-registering worker collectors, `ATTACH (TYPE quack)` fan-out | Deleted: `ATTACH` doesn't push predicates (pulls whole tables), and the design had no required network perimeter. Replaced by the self-host edge model above. |
| DataFusion (replace DuckDB) | Massive rewrite, lose table management |
| InfluxDB IOx pattern | Multi-year rebuild from scratch |
| DuckHerder (distributed DuckDB) | AGGREGATE_MERGE doesn't fix quantile correctness; quack provides native DuckDB federation |
| OpenDAL for S3 writes | DuckDB COPY TO does it in one SQL statement (-200 LOC, -2 deps) |
| DuckDB datasketches extension | SQL merge requires ~100 mutex acquisitions; Rust crate is zero-contention |
| `tdigest` Rust crate | Unmaintained since 2021; Apache `datasketches` is actively maintained |
| ElectricSQL | Syncs Postgres → SQLite for offline-first apps; wrong direction for telemetry |
| Spice.ai subprocess | External binary, complex build story; quack uses DuckDB's built-in HTTP support |

## Token budget for agent consumption

| Component | Tokens (typical) |
|-----------|-----------------|
| Metrics (1h, 1m buckets) | 2-5K |
| Spans (top 50) | 3-8K |
| Logs (30 templates) | 2-5K |
| Linked traces (5 traces, 200 spans) | 5-15K |
| **Total InvestigationBlock** | **10-50K** |

That's 1-5% of a 1M context window. The [[architecture/investigation.md|investigation]] fits.
