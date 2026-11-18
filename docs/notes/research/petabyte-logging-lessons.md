---
title: Lessons from Netflix and Reddit's Petabyte-Scale Logging
created: 2026-03-10
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'deployment', 'duckdb']
---

# Lessons from Netflix and Reddit's Petabyte-Scale Logging

Sources:
- Netflix: [ClickHouse petabyte-scale logging](https://clickhouse.com/blog/netflix-petabyte-scale-logging) — 5 PB/day, 10.6M events/sec, 40k microservices
- Reddit: Snoolog — millions of log events/sec, 14-day retention, built on ClickHouse

---

## What they confirm we're already doing right

**Flat schema with promoted columns over a normalized star schema.** Reddit separates system attributes (namespace, pod, cluster, log level) from service-specific attributes at ingest time to optimize primary keys and skip indices. Our approach — top-level columns for the high-frequency filter dimensions (`service`, `signal`, `host`, `pod`, `status`, `kind`, `name`) with a JSON blob for the long tail — is the same tradeoff. The promoted columns are the right ones.

**S3 for cold storage is correct.** Reddit uses EBS SSDs for hot + S3 for cold. Netflix uses ClickHouse hot + Iceberg cold. Both validate that S3/object-storage is the right cold tier. The ClickHouse-specific operational pain Reddit describes (no auto-scaling, no compute-storage separation in OSS, expensive replica adds, no read/write separation) is exactly what our [[architecture/data-model.md|Parquet]]-on-S3 approach avoids. We traded their hot tier for lower operational complexity, which is the right call for a greenfield system.

**"Do the least amount of work."** Netflix's core lesson: success required disciplined engineering — choosing the simplest approach that solves the problem rather than clever techniques. Our fail-fast, no-silent-errors, minimal-abstraction philosophy is the same.

---

## Gaps worth knowing about

### 1. No rate limiting at ingestion

Reddit explicitly built client-side rate limits into their Vector agents to protect the system from noisy services. Without it, one misconfigured service can spike ingestion volume and exhaust memory buffers or overwhelm the file_storage queue.

Currently sigiro has no per-service or per-namespace rate limiting. The OTel batch processor provides backpressure but not throttling.

**Options:**
- Add a `memorylimiterprocessor` (OTel contrib) — global memory ceiling, drops data when threshold crossed. Protects the process but doesn't isolate noisy services.
- Add a `filterprocessor` with sampling on high-volume services — coarser but simple.
- Accept the risk for POC; a single-tenant deployment with known services is unlikely to have runaway noise.

**Verdict:** add `memorylimiterprocessor` before shipping to any multi-tenant environment. For single-tenant POC, document and skip.

---

### 2. No hot tier — cold S3 queries are slow for interactive debugging

Both Netflix and Reddit run a hot tier (NVMe/SSD) for recent data with sub-second query latency. Sigiro writes directly to S3 — no hot tier.

For our use case (AI agent hiding latency behind LLM inference), 2-10s query time is acceptable. But for interactive 3am incident debugging where an engineer wants to grep logs directly, cold S3 queries will feel slow.

**This is a known and accepted tradeoff.** The agent model is the mitigation: the agent batches multiple slow queries while reasoning, hiding the latency. Document it explicitly in [[architecture/investigation.md|investigation]].md as a design constraint: "queries are designed for agent consumption, not interactive sub-second response."

If we ever add a direct query interface (not agent-mediated), we need a hot tier — either a ClickHouse instance for recent data or [[architecture/data-model.md|DuckDB]] with a local cache of the last N hours.

---

### 3. Substring search on `message` is expensive

Netflix tried ngram bloom filter indices for substring search (`LIKE '%error%'`) and found they had high false-positive rates — the engine ended up scanning too many row groups. They later moved to token-based (whole-word) search with ClickHouse's inverted index, which is dramatically faster.

Sigiro has a bloom filter on `message` for row-group skipping, but it's a standard [[architecture/data-model.md|Parquet]] column bloom filter (equality lookups, not substring). `WHERE message LIKE '%substring%'` bypasses the bloom filter entirely and scans all rows.

**Implication:** don't support `LIKE '%x%'` in the query layer. Support:
- Equality: `WHERE message = 'exact'` → bloom filter works
- Token-based full-text via [[architecture/data-model.md|DuckDB]]'s `contains(message, 'word')` or FTS extension — faster than LIKE on large scans
- Prefix: `WHERE message LIKE 'prefix%'` → can leverage min/max stats on sorted data

If engineers need substring search, the honest answer is "search by service + time range first, then grep the result set." This is what Reddit's "all-field search" limitation was — they required users to specify the attribute to search rather than searching all fields.

---

### 4. `metadata` JSON blob is unqueryable — promotion path is undocumented

Reddit learned that making arbitrary JSON attributes queryable requires explicit schema decisions. The lesson: don't let the JSON blob grow indefinitely without a promotion checklist.

Sigiro's `metadata` column is documented as "not a filter dimension" (correct), but there's no defined process for deciding when an attribute gets promoted to a top-level column.

**Add a promotion rule:** when a query pattern requires filtering on a `metadata` key more than occasionally, promote it to a top-level column. The signal for promotion is query time — if a [[architecture/data-model.md|DuckDB]] query needs `json_extract(metadata, '$.http.status_code') = '500'` and it's slow, promote `http_status_code` to a real column. Track promoted columns in `data-model.md`.

---

### 5. Query abstraction layer should hide the backend

Reddit built a Lucene-like query language over ClickHouse SQL. The key benefit wasn't UX (though that mattered) — it was that their frontend didn't need to migrate when they tuned or changed the backend. The query translation layer was the isolation boundary.

Sigiro's query layer will eventually need the same. Exposing [[architecture/data-model.md|DuckDB]] SQL directly to the agent or to users creates tight coupling. If we move from DuckDB to a hot ClickHouse tier, every query changes.

**Design principle:** the query API should accept a structured query object (service, time range, signal, filters, limit) and translate to [[architecture/data-model.md|DuckDB]] SQL internally. The agent calls the API, not DuckDB directly. This is already implied by the [[architecture/investigation.md|investigation]].md design — make it explicit.

---

## Kafka: still not needed

Reddit uses Kafka as the durable buffer between ingestion and ClickHouse. Their reason: ClickHouse has no native durable queue — you need something to absorb write spikes and survive ClickHouse downtime.

Sigiro's `file_storage` extension gives equivalent durability (survives S3 outages, replays on restart) without the operational overhead of a Kafka cluster. The difference: Kafka can buffer days of backlog and supports multiple consumers. `file_storage` buffers to local disk and has one consumer.

For a single-destination [[architecture/write-path.md|write path]] (S3 only, no fan-out), `file_storage` is sufficient. If we ever need to simultaneously write to S3 and a hot ClickHouse tier, Kafka becomes attractive as a fan-out layer. Until then, no.

---

## Summary table

| Lesson | Their solution | Sigiro status |
|---|---|---|
| Attribute separation at ingest | Promoted system columns + app attributes map | Done — top-level columns + metadata blob |
| Avoid star schema | Flat schema with promoted columns | Done — unified flat table |
| Cold storage on object store | S3 / Iceberg | Done |
| Hot tier for interactive queries | ClickHouse / EBS SSD | Not needed for agent model; document the latency constraint |
| Rate limiting noisy services | Client-side limits in Vector agents | Not implemented; add memorylimiterprocessor before multi-tenant use |
| Substring search is slow | Token-based search / inverted index | Don't expose LIKE '%x%'; use token-based search only |
| Abstract query layer | Lucene syntax over ClickHouse SQL | Design query API to translate, not expose DuckDB SQL directly |
| Attribute promotion path | Ad hoc at Reddit, painful | Define explicit promotion checklist in data-model.md |
| Durable queue | Kafka | file_storage extension — sufficient for single-destination write path |
