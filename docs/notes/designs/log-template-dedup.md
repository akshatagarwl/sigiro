---
title: Log Template Dedup — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'design', 'duckdb']
---

# Log Template Dedup — Design

## Problem

"User 12345 failed login" and "User 67890 failed login" are the same event.
Without dedup, [[architecture/investigation.md|investigate]] dumps thousands of identical-but-different log lines.
With dedup: "User {id} failed login (10,000 occurrences)" — instant signal.

## Pipeline Placement: Insert Time (inserter.rs)

```
ingest.rs                    inserter.rs                       DuckDB
  receive OTLP ──→ channel ──→ batch ready for flush
                                 ├─ classify each log body
                                 ├─ attach template_id, template_text, values
                                 └─ flush one self-contained sigiro_logs row
```

Why insert time:
- Runs on flush thread, doesn't block ingest hot path
- Sees batches (better clustering for DRAIN)
- diagnosis groups the inline template text — no cross-table join
- DRAIN tree lives in memory per-project alongside inserter

## Option A: Regex Heuristics (~30 lines)

Replace known variable patterns:
- UUIDs, numbers, IPs, hex, timestamps, emails, URL path segments
- `regexp_replace(body, pattern, '{id}')` chained
- GROUP BY replaced string

Pro: simple, no state, works immediately
Con: misses non-obvious variables ("checkout", "us-east-1")

## Option B: DRAIN Algorithm (~200 lines Rust)

- Split log into tokens
- Tree: branch by token count → prefix → token similarity
- Tokens that differ across cluster members → wildcard <*>
- O(1) per log line (fixed depth tree)
- Used by Datadog, [[designs/future/perses-compat.md|Grafana]] Loki, IBM

Pro: learns from data, catches everything
Con: needs in-memory state (tiny — few hundred templates per service)
Con: needs ~50 logs per cluster to converge

## Storage Schema

Add to sigiro_logs:
- `template_id` (UINT64) — first 64 bits of SHA-256(template), in big-endian order
- `template_text` (VARCHAR) — the DRAIN pattern for this immutable log fact
- `template_values` (VARCHAR) — JSON values extracted from wildcard positions

The text deliberately lives on the log row. DuckLake cannot enforce uniqueness
or foreign keys on a separate lookup table, and distributed writers plus
retention can otherwise create duplicate or dangling template references.
Repeated strings are compressed by the columnar storage format.

This is a greenfield storage-format boundary, not a migration surface. Existing
catalogs must be deleted and recreated; Sigiro does not backfill, dual-read, or
preserve the removed lookup schema.

## Query (investigate.rs)

```sql
SELECT template_text, count(*) as occurrences,
       min(timestamp) as first, max(timestamp) as last
FROM sigiro_logs
WHERE service_name = ? AND timestamp > ?
GROUP BY template_text
ORDER BY occurrences DESC
```

## Decision: DRAIN from the start. No regex — misses too much.
