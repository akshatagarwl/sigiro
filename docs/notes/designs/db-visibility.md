---
title: DB Visibility — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['architecture', 'data-model', 'design', 'investigation', 'metrics', 'migration']
---

# DB Visibility — Design

Low effort, high value. Data already exists in sigiro_spans.span_attributes.

## How It Works

OTel database instrumentation libraries (sqlalchemy, prisma, diesel, etc.) add
these semantic convention attributes to spans:

  db.system     = "postgresql" / "mysql" / "redis" / "mongodb"
  db.statement  = "SELECT * FROM users WHERE id = $1"
  db.name       = "myapp_production"
  db.operation  = "SELECT" / "INSERT" / "findOne"
  db.sql.table  = "users"

These land in sigiro_spans.span_attributes (JSON). Sigiro just needs to
query them — zero ingest changes.

## Queries to Add (investigate.rs)

### 1. Slow Queries
```sql
SELECT span_attributes::JSON->>'db.statement' AS query,
       span_attributes::JSON->>'db.system' AS db,
       count(*) AS calls,
       approx_quantile(duration, 0.95) AS p95_ns,
       avg(duration) AS avg_ns
FROM sigiro_spans
WHERE service_name = ?
  AND span_attributes::JSON->>'db.system' IS NOT NULL
  AND timestamp BETWEEN ? AND ?
GROUP BY query, db
ORDER BY p95_ns DESC
LIMIT 20
```

### 2. N+1 Detection
```sql
-- Same db.statement executed many times in a single trace = N+1
SELECT trace_id,
       span_attributes::JSON->>'db.statement' AS query,
       count(*) AS repeat_count,
       sum(duration) AS total_ns
FROM sigiro_spans
WHERE service_name = ?
  AND span_attributes::JSON->>'db.system' IS NOT NULL
  AND timestamp BETWEEN ? AND ?
GROUP BY trace_id, query
HAVING count(*) > 5
ORDER BY repeat_count DESC
LIMIT 20
```

### 3. DB Summary per Service
```sql
SELECT span_attributes::JSON->>'db.system' AS db,
       span_attributes::JSON->>'db.name' AS database,
       count(*) AS total_calls,
       count(*) FILTER (WHERE status_code = 2) AS errors,
       approx_quantile(duration, 0.5) AS p50_ns,
       approx_quantile(duration, 0.95) AS p95_ns
FROM sigiro_spans
WHERE service_name = ?
  AND span_attributes::JSON->>'db.system' IS NOT NULL
  AND timestamp BETWEEN ? AND ?
GROUP BY db, database
```

## Integration into InvestigationBlock

Add a `database` section to the [[architecture/investigation.md|investigate]] JSON output:
```json
{
  "database": {
    "summary": [{ "db": "postgresql", "total_calls": 1420, "p95_ns": 52000000 }],
    "slow_queries": [{ "query": "SELECT ...", "p95_ns": 890000000, "calls": 47 }],
    "n_plus_one": [{ "trace_id": "abc...", "query": "SELECT ...", "repeat_count": 87 }]
  }
}
```

## Anomaly View Integration

Add to sigiro_anomalies v2:
- slow_query: p95 > baseline + 3σ for that query
- n_plus_one: traces with >10 identical DB calls

## Files to Change

- `src/investigate.rs` — add database section to [[architecture/investigation.md|InvestigationBlock]]
- `src/schema.rs` — optionally add db anomalies to sigiro_anomalies view

## Estimate: ~100 lines (SQL queries + JSON assembly in investigate.rs)
## No schema changes. No ingest changes. Just read what's already stored.
