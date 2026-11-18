---
title: "ADR-001: Custom WAL Not Needed — DuckDB Handles Durability"
created: 2026-04-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'duckdb', 'investigation']
---

# ADR-001: Custom WAL Not Needed — DuckDB Handles Durability

**Date:** 2026-04-01
**[[plans/STATUS.md|Status]]:** Decided
**PR:** https://github.com/sigiroai/sigiro/pull/1 (closed without merge)

## Context

Sigiro's inserter buffers [[architecture/write-path.md|OTLP]] data (up to 10k rows / 1s) before flushing to
DuckDB. The original [[roadmap.md|roadmap]] included a WAL (write-ahead log) to prevent data
loss on crash, with 7/8 consensus in the architecture review.

We implemented a full WAL (src/wal.rs, ~200 lines) using [[architecture/data-model.md|Arrow]] IPC serialization
with fsync, then conducted a multi-persona architecture review (5 personas, 2
debate rounds) that questioned whether this duplicated [[architecture/data-model.md|DuckDB]]'s built-in durability.

## Decision

**Do not implement a custom WAL.** Rely on [[architecture/data-model.md|DuckDB]]'s built-in WAL instead.

## Rationale

### DuckDB already has a WAL

Once `append_record_batch` returns, data is durable via [[architecture/data-model.md|DuckDB]]'s internal WAL
and checkpoint mechanism. [[architecture/data-model.md|DuckDB]] handles its own crash recovery on next
`Connection::open`.

### Our WAL protected almost nothing

The custom WAL wrote at flush time (in inserter.rs), not at ingest time. This
meant:

```
200 OK → channel → buffer    [UNPROTECTED - up to 1s]
buffer → WAL write → fsync   [PROTECTED by our WAL]
WAL → DuckDB append           [PROTECTED by our WAL]
DuckDB append → commit        [PROTECTED by DuckDB's WAL]
```

Our WAL only protected the milliseconds between WAL fsync and [[architecture/data-model.md|DuckDB]] append —
a window [[architecture/data-model.md|DuckDB]] already covers internally.

### The real data loss window is intentional

The 1-second buffer (between 200 OK and [[architecture/data-model.md|DuckDB]] append) exists for throughput —
batching many small OTLP requests into fewer [[architecture/data-model.md|DuckDB]] appends. Eliminating this
window would require fsync per request (~1-5ms latency added to every ingest).
For a debugging tool where the data source is still running and will re-emit,
this tradeoff is acceptable.

## Options Considered

| Option | Tradeoff | Decision |
|--------|----------|----------|
| A. No custom WAL, rely on DuckDB | Accept 1s buffer window. Simpler code. | **Chosen** |
| B. Custom WAL at flush time | Protects only ms-level window DuckDB covers. ~200 lines for near-zero benefit. | Rejected |
| C. WAL at ingest time (before 200 OK) | Zero data loss. Adds 1-5ms fsync to every request. Overkill for debugging tool. | Rejected |
| D. External WAL crate (durability, raft-wal) | Same fundamental problem as B. Adds dependency. | Rejected |

## What We Shipped Instead

From the WAL PR, we cherry-picked the **inserter refactor** to main:

- `BTreeMap<SignalTable, Vec<RecordBatch>>` replaces 6 hardcoded buffer fields
- Adding new signal types ([[architecture/profiles.md|Profiles]], etc) requires only one enum variant
- Precomputed table names, generic flush loop, `buffer_msg()` helper

## Analysis Artifacts

The multi-persona analysis that led to this decision is archived at:
- `predict/260401-wal-design-review/overview.md` — summary + top findings
- `predict/260401-wal-design-review/findings.md` — 7 ranked findings with evidence
- `predict/260401-wal-design-review/hypothesis-queue.md` — actionable items

These are on the `feat/wal` branch (commit 2ec17aa).

## Consequences

- **Positive:** ~200 fewer lines of code, no WAL file management, no replay logic
- **Positive:** Inserter is cleaner and extensible (map-based buffers)
- **Negative:** Up to 1s of data loss on crash (acceptable for use case)
- **Negative:** If [[architecture/data-model.md|DuckDB]]'s own WAL has bugs, we have no safety net (low risk — DuckDB is mature)
