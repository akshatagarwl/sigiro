package main

import (
	"fmt"
	"os"
	"strings"

	"github.com/akshatagarwl/sigiro/pkg/schema"
)

func runSchema() {
	fmt.Fprint(os.Stdout, header)
	printColumnTable()
	fmt.Fprint(os.Stdout, nameSemantics)
	fmt.Fprint(os.Stdout, valueSemantics)
	fmt.Fprint(os.Stdout, sortOrder)
	fmt.Fprint(os.Stdout, timestampHandling)
	fmt.Fprint(os.Stdout, cliUsage)
	fmt.Fprint(os.Stdout, investigationPlaybook)
	fmt.Fprint(os.Stdout, sentinelQueries)
	fmt.Fprint(os.Stdout, gotchas)
}

func printColumnTable() {
	fmt.Fprintln(os.Stdout, "## Schema")
	fmt.Fprintln(os.Stdout)
	fmt.Fprintln(os.Stdout, "Every row has these columns:")
	fmt.Fprintln(os.Stdout)
	fmt.Fprintln(os.Stdout, "| column | type | signals | meaning |")
	fmt.Fprintln(os.Stdout, "|---|---|---|---|")

	for _, c := range schema.Columns {
		nullable := ""
		if c.Nullable {
			nullable = "?"
		}

		fmt.Fprintf(os.Stdout, "| %s | %s%s | %s | %s |\n",
			c.Name, c.Type, nullable, strings.Join(c.Signals, ", "), c.Description)
	}

	fmt.Fprintln(os.Stdout)
}

const header = `# Sigiro — Agent Data Guide
#
# Read this file, then query the data with the sigiro CLI.
# All observability data lives in one Parquet table on S3.
# No separate stores, no PromQL, no LogQL — just SQL.

## What This Is

A unified observability table. Metrics, traces, logs, and profiles are
interleaved in the same Parquet files on S3. Every row is one observation.

You query it with the sigiro CLI. One SQL dialect (DuckDB) for everything.

`

const nameSemantics = `### What ` + "`name`" + ` means per signal

- metric: metric name (e.g. ` + "`http.server.duration`" + `, ` + "`process.runtime.go.mem.heap_alloc`" + `)
- span: operation name (e.g. ` + "`POST /checkout`" + `, ` + "`db.query`" + `)
- log: logger/scope name (e.g. ` + "`app.checkout`" + `, ` + "`billing.logger`" + `)
- profile: ` + "`profile.{type}`" + ` (e.g. ` + "`profile.cpu`" + `, ` + "`profile.alloc_space`" + `)

`

const valueSemantics = `### What ` + "`value`" + ` means per signal

- metric: the metric value, in ` + "`unit`" + `
- span: duration in milliseconds
- profile: sample count
- log: null

`

const sortOrder = `## Sort Order

Files are sorted (service, signal, timestamp). This means:

- ALWAYS filter by ` + "`service`" + ` first — row group pruning makes it near-free
- ALWAYS filter by ` + "`signal`" + ` — skips irrelevant row groups
- Time ranges scan sequentially within a signal

Bad: ` + "`WHERE name='http.duration'`" + ` (scans everything)
Good: ` + "`WHERE service='checkout' AND signal='metric' AND name='http.duration'`" + `

`

const timestampHandling = `## Timestamp Handling

Timestamps are Unix nanoseconds, not seconds or milliseconds.

` + "```sql" + `
-- Convert readable time to nanoseconds for WHERE clauses:
SELECT epoch_ns('2026-03-01 14:00:00'::TIMESTAMP);

-- Convert stored nanoseconds to readable time:
SELECT to_timestamp(timestamp / 1e9) FROM ...;

-- Readable bucket for aggregation:
SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M') AS bucket FROM ...;
` + "```" + `

`

const cliUsage = `## CLI Usage

` + "```bash" + `
# Investigate a service (structured block: metrics + spans + logs + profiles + linked traces)
sigiro investigate --service checkout --window 15m

# Investigate a specific trace
sigiro investigate --trace abc123def456 --window 1h

# Raw SQL query (obs view = last 1 hour of all signals)
sigiro query "SELECT count(*) FROM obs WHERE service='checkout' AND signal='span'"

# Pipe to jq
sigiro query "SELECT DISTINCT service FROM obs" | jq '.[].service'

# Pipe investigation to an LLM
sigiro investigate --service checkout --window 15m | llm "what's the root cause?"
` + "```" + `

### Environment variables

` + "```bash" + `
export SIGIRO_BUCKET=sigiro
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_REGION=us-east-1

# For MinIO / local dev:
export SIGIRO_S3_ENDPOINT=localhost:9000
` + "```" + `

`

const investigationPlaybook = `## Investigation Playbook

Given: a service name and an approximate time of incident.
Goal: determine root cause.

### Quick start

` + "```bash" + `
sigiro investigate --service checkout --window 15m
` + "```" + `

This runs 4 queries in parallel (metrics, spans, logs, profiles) plus a
linked trace follow-up, and returns a structured JSON block.

### Manual drill-down with raw SQL

#### Metrics — bucketed time series
` + "```bash" + `
sigiro query "
  SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M') AS bucket,
         name, approx_quantile(value, 0.95) AS p95, avg(value) AS avg, count(*) AS n
  FROM obs
  WHERE service='checkout' AND signal='metric'
  GROUP BY bucket, name
  ORDER BY bucket, name
"
` + "```" + `

#### Spans — errors and slow operations
` + "```bash" + `
sigiro query "
  SELECT trace_id, span_id, name, value AS duration_ms, status
  FROM obs
  WHERE service='checkout' AND signal='span' AND (status='ERROR' OR value > 1000)
  ORDER BY CASE WHEN status='ERROR' THEN 0 ELSE 1 END, value DESC
  LIMIT 50
"
` + "```" + `

#### Logs — recent errors
` + "```bash" + `
sigiro query "
  SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t,
         status, message, trace_id
  FROM obs
  WHERE service='checkout' AND signal='log'
  ORDER BY timestamp DESC
  LIMIT 100
"
` + "```" + `

#### Profiles — hot functions
` + "```bash" + `
sigiro query "
  SELECT stacktrace[len(stacktrace)] AS leaf_function, sum(value) AS samples
  FROM obs
  WHERE service='checkout' AND signal='profile'
        AND stacktrace IS NOT NULL AND len(stacktrace) > 0
  GROUP BY leaf_function
  ORDER BY samples DESC
  LIMIT 10
"
` + "```" + `

#### Follow cross-service traces
` + "```bash" + `
sigiro query "
  SELECT trace_id, service, name, span_id, parent_span_id,
         value AS duration_ms, status
  FROM obs
  WHERE trace_id IN ('abc123...', 'def456...') AND signal IN ('span', 'log')
  ORDER BY trace_id, timestamp
"
` + "```" + `

`

const sentinelQueries = `## Sentinel Queries (Health Checks)

Quick checks to determine if a service needs investigation.

### Error rate (last 5 minutes)
` + "```bash" + `
sigiro query "
  SELECT count(*) FILTER (WHERE status = 'ERROR') * 100.0 / NULLIF(count(*), 0) AS error_pct
  FROM obs
  WHERE service = 'checkout' AND signal = 'span'
        AND timestamp > epoch_ns(now() - INTERVAL '5 minutes')
"
` + "```" + `
Anomalous if: > 5%

### Latency spike (5-min p95 vs baseline)
` + "```bash" + `
sigiro query "
  WITH recent AS (
    SELECT approx_quantile(value, 0.95) AS p95 FROM obs
    WHERE service='checkout' AND signal='span'
          AND timestamp > epoch_ns(now() - INTERVAL '5 minutes')
  ),
  baseline AS (
    SELECT approx_quantile(value, 0.95) AS p95 FROM obs
    WHERE service='checkout' AND signal='span'
          AND timestamp BETWEEN epoch_ns(now() - INTERVAL '1 hour')
                            AND epoch_ns(now() - INTERVAL '5 minutes')
  )
  SELECT CASE WHEN baseline.p95 > 0
    THEN (recent.p95 - baseline.p95) / baseline.p95 * 100
    ELSE 0 END AS pct_change
  FROM recent, baseline
"
` + "```" + `
Anomalous if: > 50% increase

### Service silence
` + "```bash" + `
sigiro query "
  SELECT count(*) AS row_count FROM obs
  WHERE service = 'checkout'
        AND timestamp > epoch_ns(now() - INTERVAL '5 minutes')
"
` + "```" + `
Anomalous if: 0

`

const gotchas = `## Gotchas

1. timestamp is nanoseconds. Not seconds. Not milliseconds.
2. stacktrace is a list, not a string. Leaf: stacktrace[len(stacktrace)]. Root: stacktrace[1].
3. value is null for logs. Use count(*), not sum(value), for log queries.
4. Linked trace queries (WHERE trace_id IN (...)) are the slowest query type. Limit to 10-20 trace_ids.
5. Always include service AND signal in WHERE. Row group pruning from the sort order makes this fast.
6. metadata is for reading context, not for filtering. Filter on promoted columns first.
7. The obs view in ` + "`sigiro query`" + ` covers the last 1 hour. For wider ranges, use read_parquet() directly.
`
