---
title: ClickHouse Full-Text Search GA — Relevance to Sigiro
created: 2026-03-11
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'duckdb']
---

# ClickHouse Full-Text Search GA — Relevance to Sigiro

**Date:** 2026-03-11
**Source:** https://clickhouse.com/blog/full-text-search-ga-release
**Verdict:** Not applicable. Sigiro's architecture solves the problem differently.

---

## What ClickHouse Shipped

Production-ready inverted indexes for token-based text search. Key numbers from their 10B+ row GitHub events benchmark:

| Approach | Query Time | Rows Scanned |
|----------|-----------|-------------|
| Full scan | 193s | 10.5B |
| Bloom filter | 143s | 2.16B |
| Inverted index | 0.422s | 334M |

[[architecture/tradeoffs.md|Tradeoffs]]: 50% write throughput reduction, 30x larger index than bloom (215 GiB vs 7 GiB compressed). No relevance scoring, no phrase search — pure token matching via `hasToken()`.

---

## Why It Doesn't Apply to Sigiro

### Architecture mismatch

ClickHouse FTS is a server-side inverted index on persistent tables. Sigiro is [[architecture/data-model.md|Parquet]] on S3 queried with [[architecture/data-model.md|DuckDB]]. There is no persistent server to maintain indexes — files are written once, read with stateless SQL.

### Agent query model already solves this

The [[architecture/investigation.md|investigation]] flow (`investigate.go`) filters to `(service, signal, time_range)` before touching `message`. The temp table is small — scanning 100-1000 log messages in memory is instant. Agents don't grep millions of logs; they read a narrow, pre-filtered window.

ClickHouse FTS solves "search 10B rows for a token." Sigiro's equivalent is "search 100 rows the agent already has." The bottleneck is the S3 [[architecture/data-model.md|Parquet]] scan, not text matching within the result.

### DuckDB FTS extension doesn't help either

[[architecture/data-model.md|DuckDB]] has `match_bm25()` with BM25 scoring, but:
- Index must be recreated per query (no persistence across temp tables)
- Creating an FTS index on a 100-row temp table is slower than scanning it
- BM25 relevance scoring is wasted — agents want exact keyword matches, not ranked results

---

## What IS Actionable: Keyword Filter in InvestigateRequest

The one gap exposed by this analysis: `scanLogs()` in `investigate.go` always returns the last 100 logs with no text filter. The agent guide (`agents.txt:203`) already suggests `AND message LIKE '%timeout%'` but the Go API doesn't expose it.

**Minimal change:** add `Keywords []string` to `InvestigateRequest`, apply as `AND message ILIKE '%keyword%'` in `scanLogs()`. Zero infrastructure change, zero storage [[company/cost.md|cost]], immediately useful for agents investigating specific error patterns.

This is ~10 lines of Go. No indexes, no sidecars, no [[architecture/write-path.md|write-path]] changes needed.

---

## Cross-Reference

- **[[company/plan.md|Plan]] 20** (`docs/plans/20-bloom-message-column.md`) — bloom on `message` evaluated and rejected. Bloom tests exact equality, not substring containment. [[architecture/data-model.md|DuckDB]] doesn't push `LIKE` to bloom.
- **[[company/plan.md|Plan]] 12** (`docs/plans/12-trigram-index.md`) — trigram sidecar for substring search. Correct tool if human-facing log UI is ever built. Not needed for agent queries on narrow windows.

---

## When to Revisit

| Trigger | Action |
|---------|--------|
| Human-facing log search UI (hours/days of data) | Plan 12 trigram sidecar |
| Cross-service keyword scan ("find all services that logged X") | Tokenized bloom or trigram — breaks per-service investigation model |
| Scale beyond 1 TB/day where time-bounded message scans take >30s | Plan 12 or column promotion for high-value terms |

None of these exist today.
