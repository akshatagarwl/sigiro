---
title: Telemetry Cost Optimization Thesis
created: 2026-03-11
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'deployment']
---

# Telemetry Cost Optimization Thesis

Captured from conversation on 2026-03-10. Word-for-word reasoning on positioning, advantages, downsides, and [[company/pitch.md|pitch]].

---

## The Problem We Observed (Redacted Slack Transcript)

A customer (Revelator) hit the OpenSearch/Elasticsearch field mapping limit on their Logs App. The internal debate:

- Support wanted to raise the limit immediately (1.5k fields) so the customer doesn't see an empty app
- Engineering wanted to contact the customer first and teach them to reduce fields
- Ops saw three options: contact customer, silently increase limits, or drop logs in pipeline

Key insight: when the field limit is hit, new fields aren't mapped — data is still ingested and stored, but unmapped fields don't appear as filterable/searchable columns. This makes the Logs App appear "empty" or degraded from the user's perspective, even though data isn't actually lost.

This reveals a common log management tension: customers often send logs with high-cardinality or dynamically-generated field names. Raising limits is a band-aid — it increases index size, slows queries, and the customer will likely hit the new limit too. The real fix is teaching the customer to normalize their log structure.

---

## Why This Problem Doesn't Apply to Sigiro

The field limit issue is fundamentally an OpenSearch/Elasticsearch mapping explosion problem, and Sigiro doesn't use OpenSearch or any index-mapping system.

1. **Fixed schema, not dynamic mappings.** Sigiro has a single unified [[architecture/data-model.md|Parquet]] table with ~18 predefined columns. There's no dynamic field creation — new attributes from a customer don't create new columns.

2. **Attributes go into `metadata` (a JSON blob string).** Arbitrary key-value pairs land in a single `metadata` column, not as individual mapped fields. This is a deliberate tradeoff — you lose per-attribute filtering efficiency but gain zero risk of field explosion.

3. **[[architecture/data-model.md|Parquet]] doesn't have field limits.** Parquet is columnar storage with a compile-time schema. There's no equivalent of OpenSearch's `index.mapping.total_fields.limit`. You can't "run out of fields" because the schema is static.

4. **The Redacted problem is an indexing problem.** OpenSearch builds an inverted index per field — each new field name means new index structures, memory overhead, and eventually cluster instability. Sigiro has no inverted index at all — free-text search is explicitly a full scan.

The closest analogous risk in Sigiro would be the `metadata` JSON blob growing unboundedly large per row if a customer sends thousands of attributes, but that's a row size problem (affecting compression and scan speed), not a schema explosion problem that requires operator intervention to raise limits.

---

## Grep vs Structured Search for AI Agents

The initial instinct was that attribute-based search is strictly better than grep for agents. On closer examination, it's more nuanced: grep's simplicity is better for starting an [[architecture/investigation.md|investigation]], structured queries' output is better for reasoning. Sigiro should support both.

### Where structured search wins

Agents reason structurally. A SQL query returns every result with full context — service, trace_id, [[plans/STATUS.md|status]], timestamp — attached. The agent can immediately reason about patterns, correlate across signals, and follow trace links. Grep returns matching lines with no surrounding structure.

1. **Narrow first:** `WHERE service='checkout' AND status='ERROR' AND timestamp BETWEEN t1 AND t2` — fast in Sigiro (sort order + dict encoding + row group pruning)
2. **Read messages second:** on the narrowed result set (maybe 50-200 rows), the agent reads `message` values directly — no grep needed
3. **Follow links:** grab `trace_id` from the interesting rows, query linked spans/logs across services

- **False positives at scale.** `grep "timeout"` across 10 services returns thousands of hits. An agent can't efficiently triage unstructured text — it burns tokens reading noise.
- **No correlation.** Grep can't answer "show me the span that caused this log line" — that requires joining on `trace_id`, which is an attribute query.

### Where grep genuinely wins

For the "I have no idea what's wrong" phase — grep is better. The agent doesn't know which attribute to filter on, doesn't know the error message, doesn't know the service. It just needs to scan and find something interesting.

The one case where literal grep wins over structured queries: **searching across fields you didn't anticipate.** The timeout string might be in `metadata`, in `name`, in `message` — you don't know. A raw grep hits everything. A SQL query requires you to pick a column.

### Resolution: grep as entry point, structured as follow-up

Even structured queries can simulate grep — but with context:

```sql
-- "grep" but with context
SELECT service, status, message, trace_id, timestamp
FROM unified
WHERE signal = 'log'
  AND timestamp BETWEEN t1 AND t2
  AND message LIKE '%timeout%'
ORDER BY timestamp DESC
LIMIT 50
```

That's grep — but every result comes with service, trace_id, [[plans/STATUS.md|status]] attached. Real grep gives you the matching line and nothing else. The agent would need a second step to figure out where that line came from.

The cross-field problem is a query layer problem, not an architecture problem. You can add `WHERE message LIKE '%x%' OR metadata LIKE '%x%' OR name LIKE '%x%'` and it's still structured with full context on every hit.

**Sigiro should support both:** grep-style full-text scan as the entry point for exploration, structured attribute queries as the follow-up for reasoning. The architecture already supports this — it's a query API surface decision, not a storage change.

---

## Query-Time JSON Extraction (No Schema Change Needed)

[[architecture/data-model.md|DuckDB]] can extract JSON fields at query time:

```sql
WHERE json_extract_string(metadata, '$.http.method') = 'GET'
```

No schema change needed. The [[company/cost.md|cost]] is JSON deserialization on every row in the scan — but if you've already narrowed by `service + signal + timestamp`, you're deserializing maybe a few thousand rows, not millions. That's milliseconds.

Promoting to a column only matters when the attribute is used as a primary filter on a wide scan (e.g., "find all 500s across all services in the last hour"). For agent-driven [[architecture/investigation.md|investigation]] on a narrow window for one service, query-time JSON extraction is fine.

Promote only when you measure it being slow, not preemptively.

---

## Log Message Deduplication in Columnar Format

### What Parquet gives you for free

Dictionary encoding on the `message` column handles exact duplicates perfectly. If `"connection pool exhausted"` appears 50,000 times in a flush, it's stored once in the dictionary, referenced by integer index. Zstd on top compresses the index array further. Storage [[company/cost.md|cost]] is near-zero for repeated messages.

This covers a lot of real-world logs — health checks, periodic [[plans/STATUS.md|status]] lines, identical error messages from retry loops.

### What it doesn't handle

Log messages with embedded variables — these are string-unique but semantically identical:

```
Connection to 10.0.0.5:3306 timed out after 30s (req=abc123)
Connection to 10.0.0.7:3306 timed out after 31s (req=def456)
```

Dictionary encoding sees two distinct strings. Zstd catches some shared substrings across nearby rows, but it's compression, not deduplication.

### Does this matter for an agent?

Not really. The agent already handles this at query time:

```sql
SELECT message, count(*) as occurrences
FROM unified
WHERE service='checkout' AND status='ERROR'
  AND timestamp BETWEEN t1 AND t2
GROUP BY message
ORDER BY occurrences DESC
LIMIT 10
```

For the near-duplicate case, the agent could regex-normalize at reasoning time ("these 3 messages are the same error with different IPs"). That's what agents are good at — pattern recognition over a small structured result set.

### Where it would actually hurt

Storage [[company/cost.md|cost]], not query quality. If a service emits 100k log lines/sec and 90% are near-duplicate, you're storing 90% redundant data. Columnar format + zstd gets you maybe 5-8x compression on similar strings, but true template-based dedup (extract pattern + store parameters separately) could get 50-100x.

But that's a [[architecture/write-path.md|write-path]] optimization ([[designs/log-template-dedup.md|log template]] extraction in the collector's processor chain) — not a schema problem. OTel collectors already have processors that can do this. It's worth adding when storage [[company/cost.md|cost]] becomes a concern, not before.

---

## The Agent Can Generate OTel Processor Configs

If the agent can see a sample of the customer's logs, it can generate the right OTel processor config to fix the problem — template extraction, field promotion, noise reduction, whatever's needed.

This is a strong product angle: the agent doesn't just diagnose runtime issues, it diagnoses telemetry quality issues. "Your checkout service is emitting 50k near-duplicate log lines/sec — here's a `transform` processor config that extracts the template and drops the noise":

```yaml
processors:
  transform:
    log_statements:
      - context: log
        statements:
          - replace_pattern(body, "Connection to [\\d.]+:\\d+ timed out after \\d+s \\(req=\\w+\\)", "Connection to {host}:{port} timed out after {duration}s (req={id})")
```

That's the inverse of the Redacted transcript — instead of ops manually raising field limits or scheduling a call to "teach the customer," the agent sees the problem, generates the fix, and offers it. Self-service.

---

## Structured Logging as the Root Fix

Structured logging solves it at the root:

```go
// before: string interpolation, every message unique
log.Printf("Connection to %s:%d timed out after %ds (req=%s)", host, port, dur, reqID)

// after: structured, message is a static template
logger.Error("connection timeout",
    "host", host,
    "port", port,
    "duration", dur,
    "request_id", reqID,
)
```

With structured logs, the `message` column becomes a static string ("connection timeout") — dictionary encoding deduplicates it perfectly. The variables land in `metadata` as queryable attributes. Storage, search, dedup — all solved.

The agent could detect this and nudge:

> "Your checkout service emits 47k log lines/hour with 12k unique message strings but only 8 distinct patterns. Switching to structured logging would reduce storage by ~60% and make all parameters searchable. Here's the code change for the top 5 offenders."

Three levels of intervention, from quick fix to root cause:

1. **Query time** — `GROUP BY message` to dedup for the agent (free, already works)
2. **Collector config** — `transform` processor to extract templates (minutes to deploy, no code change)
3. **Engineering nudge** — switch to structured logging (the real fix, requires code change)

The agent can offer all three ranked by effort. The Redacted conversation was humans debating option 2 vs "schedule a call to teach them." The agent automates that entire loop.

---

## Concrete Dollar Impact at Scale

The agent has everything it needs to calculate storage waste:

```sql
-- find the worst offenders
SELECT
    service,
    count(DISTINCT message) as unique_messages,
    count(*) as total_lines,
    sum(length(message)) as total_bytes
FROM unified
WHERE signal = 'log' AND timestamp BETWEEN t1 AND t2
GROUP BY service
ORDER BY total_bytes DESC
```

Then for the top offender:

```sql
-- cluster near-duplicate messages
SELECT
    left(message, 50) as pattern_prefix,
    count(*) as occurrences,
    count(DISTINCT message) as unique_variants,
    sum(length(message)) as bytes_wasted
FROM unified
WHERE service = 'checkout' AND signal = 'log'
GROUP BY pattern_prefix
HAVING unique_variants > 100
ORDER BY bytes_wasted DESC
```

The agent can then say:

> "Your `checkout` service emits 2.3M log lines/day. 1.8M are variants of `Connection to {ip}:{port} timed out...` — 12 bytes of useful data embedded in a 140-byte string, repeated 1.8M times. That's 241 MB/day of redundant text.
>
> Switch to structured logging and this becomes a 19-byte dictionary entry + 12 bytes of attributes x 1.8M = 22 MB/day. 90% reduction.
>
> Here's the diff for `checkout/internal/db/pool.go:47`."

### The math at scale

Take a mid-size company: 50 services, 10TB/day of logs. If 70% is unstructured noise (typical — most teams `log.Printf` everything), that's 7TB/day of bloated messages.

Structured logging gets 80-90% reduction on that portion:

| | before | after |
|---|---|---|
| daily log volume | 10 TB | 3.8 TB |
| monthly storage (S3) | 300 TB | 114 TB |
| monthly S3 cost ($0.023/GB) | $6,900 | $2,622 |
| annual savings | | $51,336 |

That's just S3 storage. Add query costs — [[architecture/data-model.md|DuckDB]] scans less data, fewer bytes over the network, faster queries. And compression ratios improve dramatically on structured data (dictionary encoding on static messages vs zstd struggling with unique strings).

At Netflix/Reddit scale (petabytes/day), the same 70% reduction is millions per year.

The real leverage: the agent doesn't just identify the savings once. It watches continuously. New team ships a service with `fmt.Sprintf` logging — the agent catches it in the first hour, before it accumulates into a [[company/cost.md|cost]] problem. That's the difference between a one-time audit and a continuous cost optimization loop.

---

## Competitive Landscape: What Others Do (and Don't)

### Usage-based vendors (Datadog, Splunk, Elastic)

They charge per GB ingested. Telling you to send less data cuts their revenue. They offer some tools:

- Datadog has "Log Patterns" (clusters similar messages) and ingestion controls
- Splunk has Data Stream Processor for pre-ingest filtering
- Elastic has ingest pipelines with grok/dissect

But these are buried in settings, not proactively surfaced. Nobody at Datadog is going to email you saying "hey, you could cut your $80k/month bill to $30k."

### Pipeline companies (Cribl, Edge Delta, Calyptia)

Their entire business is "reduce your Datadog/Splunk bill." Cribl sits between your agents and your backend, letting you drop, sample, and reshape before ingestion. This is a billion-dollar company built on the fact that observability vendors won't cannibalize themselves.

### Cost control platforms (Chronosphere)

Specifically for metrics cardinality reduction. Same thesis — vendors won't optimize against their own [[designs/pricing.md|pricing]].

### What nobody does

The full loop:

1. **Detect** the waste pattern in actual stored data
2. **Calculate** the dollar impact
3. **Generate** the code change or collector config
4. **Deliver** it to the engineering team as a PR or bot message

Cribl gets you to step 1-2 at the pipeline level. But it can't look at your Go source code and say "line 47 of pool.go is costing you $4k/month — here's the fix." That requires understanding both the telemetry data AND the source code.

---

## Sigiro's Position

Sigiro doesn't have the conflict of interest — storage is the customer's S3 bucket, so reducing their volume doesn't cut Sigiro's revenue. And an AI agent can close the full loop because it can read both the log patterns AND the source code.

The [[company/pitch.md|pitch]] isn't "we're cheaper than Datadog." It's "we make your entire observability spend cheaper, including ours."

---

## Summary: Sigiro's Telemetry Cost Thesis

**Advantages:**
- No field mapping explosion (fixed [[architecture/data-model.md|Parquet]] schema, metadata JSON blob)
- Agent query model supports both grep-style exploration and structured attribute follow-up
- Query-time JSON extraction means no premature schema optimization
- Dictionary encoding handles exact log deduplication for free
- No conflict of interest — customer's S3 bucket, reducing volume doesn't cut our revenue
- AI agent closes the full detect-calculate-generate-deliver loop

**Downsides:**
- Free-text log search is a full scan (no inverted index)
- Near-duplicate log messages don't deduplicate at storage level (only exact matches)
- `metadata` JSON extraction is slower than promoted columns for wide scans
- No pre-aggregation — every query re-scans raw data

**[[company/pitch.md|Pitch]]:**
"We make your entire observability spend cheaper, including ours."

**Positioning:**
The only observability platform where the AI agent proactively detects telemetry waste, calculates the dollar impact, generates the fix (collector config or source code change), and delivers it to the engineering team. No conflict of interest. No human ops debate about whether to raise limits or schedule a call.
