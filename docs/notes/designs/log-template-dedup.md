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
                                 ├─ attach template_id column
                                 └─ flush sigiro_logs (body + template_id)
```

Why insert time:
- Runs on flush thread, doesn't block ingest hot path
- Sees batches (better clustering for DRAIN)
- [[architecture/investigation.md|investigate]].rs just does GROUP BY template_id — instant
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
- `template_id` (UINT64) — hash of template string
- Template lookup table: `sigiro_log_templates (template_id, template_text, first_seen)`

## Query (investigate.rs)

```sql
SELECT t.template_text, count(*) as occurrences,
       min(l.timestamp) as first, max(l.timestamp) as last
FROM sigiro_logs l
JOIN sigiro_log_templates t USING (template_id)
WHERE l.service_name = ? AND l.timestamp > ?
GROUP BY t.template_text
ORDER BY occurrences DESC
```

## Decision: DRAIN from the start. No regex — misses too much.
