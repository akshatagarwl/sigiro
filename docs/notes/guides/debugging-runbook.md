---
title: Debugging Runbook
created: 2026-05-12
updated: 2026-05-12
type: guide
tags: ['ai-agent', 'investigation', 'deployment', 'duckdb', 'cli']
---

# Sigiro Debugging Runbook

Interactive debugging commands for operators and AI agents. Every code block below is executable via [runme](https://runme.dev/) or copy-paste into a terminal.

---

## Prerequisites

Ensure `sigiro` is in your `$PATH` and a server is running:

```sh {"name":"check-binary"}
which sigiro || echo "sigiro not in PATH. Build: cargo build --release"
sigiro --version
```

Set the local endpoint (Open mode needs no key). For a hosted endpoint, additionally set `SIGIRO_API_KEY=sk_<tenant>_<random>`; the curl Authorization headers below are then required and are ignored by a local Open server.

```sh {"name":"setup-env"}
export SIGIRO_ENDPOINT="${SIGIRO_ENDPOINT:-http://localhost:9999}"
export SIGIRO_DATA_DIR="${SIGIRO_DATA_DIR:-$HOME/.sigiro/data}"
echo "Endpoint: $SIGIRO_ENDPOINT"
echo "Data dir: $SIGIRO_DATA_DIR"
```

---

## 1. Server Health

### Quick health check

```sh {"name":"health-check"}
curl -sf "${SIGIRO_ENDPOINT}/health" && echo "OK" || echo "DOWN"
```

### Detailed status via CLI

```sh {"name":"cli-status"}
sigiro status
```

### Check instance capabilities

```sh {"name":"admin-status"}
curl -sf "${SIGIRO_ENDPOINT}/admin/status" | jq .
```

---

## 2. Service Discovery

### List all services sending telemetry

```sh {"name":"list-services"}
sigiro query "SELECT DISTINCT service_name FROM lake.sigiro_spans ORDER BY service_name"
```

### Service details with first/last seen

```sh {"name":"services-http"}
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  "${SIGIRO_ENDPOINT}/v1/services" | jq '.services[] | {name, firstSeen, lastSeen, spanCount}'
```

### Recently active services (last 5 minutes)

```sh {"name":"recent-services"}
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 300 * 1000000))
sigiro query "SELECT DISTINCT service_name FROM lake.sigiro_spans WHERE timestamp > make_timestamp(${FROM_US}) ORDER BY service_name"
```

---

## 3. Raw SQL Queries

### Count signals by type

```sh {"name":"count-signals"}
echo "=== Spans ==="
sigiro query "SELECT count(*) FROM lake.sigiro_spans"
echo "=== Logs ==="
sigiro query "SELECT count(*) FROM lake.sigiro_logs"
echo "=== Metrics (gauge) ==="
sigiro query "SELECT count(*) FROM lake.sigiro_metrics_gauge"
```

### Top 10 slowest spans in the last hour

```sh {"name":"slow-spans"}
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 3600 * 1000000))
sigiro query "SELECT service_name, span_name, duration / 1e6 AS duration_ms, trace_id FROM lake.sigiro_spans WHERE timestamp > make_timestamp(${FROM_US}) ORDER BY duration DESC LIMIT 10"
```

### Error rate by service (last 15 min)

```sh {"name":"error-rate"}
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 900 * 1000000))
sigiro query "SELECT service_name, count(*) AS total, count(*) FILTER (WHERE status_code = 2) AS errors, round(count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*), 2) AS error_pct FROM lake.sigiro_spans WHERE timestamp > make_timestamp(${FROM_US}) GROUP BY service_name ORDER BY errors DESC"
```

### Log severity distribution

```sh {"name":"log-severity"}
sigiro query "SELECT severity_text, count(*) FROM lake.sigiro_logs GROUP BY severity_text ORDER BY count(*) DESC"
```

### Find traces with errors

```sh {"name":"error-traces"}
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 900 * 1000000))
sigiro query "SELECT DISTINCT trace_id, service_name, span_name, duration / 1e6 AS duration_ms FROM lake.sigiro_spans WHERE status_code = 2 AND timestamp > make_timestamp(${FROM_US}) LIMIT 20"
```

### Recent deployments (CICD pipeline spans)

```sh {"name":"recent-deploys"}
sigiro query "SELECT resource_attributes::JSON->>'service.name' AS service, span_attributes::JSON->>'cicd.pipeline.name' AS pipeline, span_attributes::JSON->>'cicd.pipeline.result' AS result, timestamp::VARCHAR AS ts FROM lake.sigiro_spans WHERE span_name LIKE 'cicd.pipeline%' ORDER BY timestamp DESC LIMIT 10"
```

---

## 4. Investigation

### Investigate a service (last 15 minutes, default)

```sh {"name":"investigate-service"}
# Replace 'frontend' with your service name
SERVICE="frontend"
sigiro diagnose "${SERVICE}"
```

### Investigate with explicit time window

```sh {"name":"investigate-window"}
SERVICE="frontend"
TO_S=$(date +%s)
FROM_S=$((TO_S - 3600))  # last hour
sigiro diagnose "${SERVICE}" --from "${FROM_S}" --to "${TO_S}"
```

### Investigate via HTTP (for agents/scripts)

```sh {"name":"investigate-http"}
SERVICE="frontend"
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 900 * 1000000))
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -H "Content-Type: application/json" \
  -X POST "${SIGIRO_ENDPOINT}/v1/diagnose" \
  -d "{\"service\":\"${SERVICE}\",\"from_ts\":${FROM_US},\"to_ts\":${NOW_US}}" | jq .
```

### Multi-service investigation

```sh {"name":"investigate-multi"}
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 900 * 1000000))
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -H "Content-Type: application/json" \
  -X POST "${SIGIRO_ENDPOINT}/v1/diagnose" \
  -d "{\"services\":[\"frontend\",\"checkout\",\"payment\"],\"from_ts\":${FROM_US},\"to_ts\":${NOW_US}}" | jq 'keys'
```

### Check investigation coverage

```sh {"name":"investigate-coverage"}
SERVICE="frontend"
sigiro diagnose "${SERVICE}" | jq '.coverage'
```

---

## 5. Trace Detail

### Get full trace detail (critical path + service breakdown)

```sh {"name":"trace-detail"}
# Pick a trace_id via HTTP (clean JSON)
TRACE_ID=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -X POST "${SIGIRO_ENDPOINT}/v1/query" \
  -d "SELECT trace_id FROM lake.sigiro_spans ORDER BY timestamp DESC LIMIT 1" | jq -r '.[0].trace_id')
echo "Trace ID: ${TRACE_ID}"
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  "${SIGIRO_ENDPOINT}/v1/traces/${TRACE_ID}" | jq '{trace_id, span_count, total_duration_ms: (.total_duration_ns / 1e6), critical_path, service_breakdown}'
```

---

## 6. Hosted Tenant Keys

Tenant keys are minted and managed by the hosted control plane, not by a local Sigiro server. Configure the operator CLI with `SIGIRO_ENDPOINT` and `SIGIRO_API_KEY=sk_<tenant>_<random>` (or use `--key <token>`).

---

## 7. Federation (Self-Host Edges)

There is no central node or worker registry — each host is a self-contained
`sigiro serve`. Query across hosts with the `sigiro query` CLI coordinator,
which discovers edges over your tailnet (or an explicit override) and folds
their results. See [[guides/self-host-edge-federation.md|Self-Host Edge
Federation]] for the mandatory network perimeter (quack has no TLS) and the
full query grammar.

### List edges the coordinator would use

```sh {"name":"list-edges"}
tailscale status --json | jq '.Peer[], .Self | select(.Tags[]? == "tag:sigiro-edge") | {HostName, TailscaleIPs}'
```

### Cross-edge query

```sh {"name":"federated-query"}
sigiro query "SELECT service_name, count(*) AS n FROM sigiro_spans \
  WHERE timestamp > now() - interval 1 hour GROUP BY service_name ORDER BY n DESC LIMIT 20"
```

---

## 8. Fleet Observability (DuckDB Extensions)

### CPU info (local only — requires sigiro server process permissions)

```sh {"name":"fleet-cpu"}
sigiro query "SELECT * FROM lake.sys_cpu_info()"
```

### Disk usage

```sh {"name":"fleet-disk"}
sigiro query "SELECT path, total, free, round(free * 100.0 / total, 2) AS free_pct FROM lake.sys_disk_info()"
```

### Find large files in data directory

```sh {"name":"fleet-large-files"}
sigiro query "SELECT path, size, mtime FROM lake.fsquery_scan('${SIGIRO_DATA_DIR}') WHERE is_regular_file AND size > 100000000 ORDER BY size DESC LIMIT 10"
```

---

## 9. Troubleshooting

### Server not responding

```sh {"name":"debug-server-down"}
# Check if process is running
ps aux | grep -E '[s]igiro serve' || echo "No sigiro serve process found"

# Check port listeners
lsof -i :9999 2>/dev/null || ss -tlnp | grep 9999 || echo "Nothing on :9999"
lsof -i :4317 2>/dev/null || ss -tlnp | grep 4317 || echo "Nothing on :4317"
lsof -i :4318 2>/dev/null || ss -tlnp | grep 4318 || echo "Nothing on :4318"

# Tail server logs (if running in foreground or logged)
tail -50 "${SIGIRO_DATA_DIR}/../sigiro.log" 2>/dev/null || echo "No log file found"
```

### DuckDB lock errors

If `sigiro query` fails with a lock error, the server holds the catalog lock. Use HTTP instead:

```sh {"name":"duckdb-lock-workaround"}
# Instead of: sigiro query "SELECT ..."
# Use:
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -X POST "${SIGIRO_ENDPOINT}/v1/query" \
  -d "SELECT count(*) FROM lake.sigiro_spans"
```

### Verify data is being ingested

```sh {"name":"verify-ingest"}
BEFORE=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -X POST "${SIGIRO_ENDPOINT}/v1/query" \
  -d "SELECT count(*) FROM lake.sigiro_spans" | jq -r '.[0].count_star()')
echo "Spans before: ${BEFORE}"
echo "Send a test trace..."
curl -sf -X POST "http://localhost:4318/v1/traces" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -d '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"debug-test"}}]},"scopeSpans":[{"scope":{"name":"manual"},"spans":[{"traceId":"aabbccdd11223344aabbccdd11223344","spanId":"1122334455667788","name":"debug-span","kind":1,"startTimeUnixNano":"'"$(python3 -c "import time; print(int(time.time()*1e9))")"'","endTimeUnixNano":"'"$(python3 -c "import time; print(int(time.time()*1e9 + 1e6))")"'"}]}]}]}'
sleep 2
AFTER=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
  -X POST "${SIGIRO_ENDPOINT}/v1/query" \
  -d "SELECT count(*) FROM lake.sigiro_spans" | jq -r '.[0].count_star()')
echo "Spans after: ${AFTER}"
```

---

## Output Formats

The CLI supports two output formats. Default is `toon` (token-efficient for LLMs). Use `--format json` for pretty-printed JSON:

```sh {"name":"output-formats"}
sigiro status --format json | jq .             # JSON
sigiro status --format toon                     # TOON (default)
```

**Note:** `sigiro query --format json` wraps the JSON in a TOON envelope (`[N]{cols}:` prefix). For programmatic parsing, use the HTTP `/v1/query` endpoint which returns bare JSON.

---

## One-Liner Cheat Sheet

```sh {"name":"cheat-sheet"}
cat <<'EOF'
# Health
curl -sf localhost:9999/health

# Services
sigiro query "SELECT DISTINCT service_name FROM lake.sigiro_spans"

# Slow spans (last hour)
sigiro query "SELECT service_name, span_name, duration/1e6 AS ms FROM lake.sigiro_spans WHERE timestamp > make_timestamp($(python3 -c 'import time; print(int(time.time()*1e6)-3600*1e6)')::BIGINT) ORDER BY duration DESC LIMIT 10"

# Errors by service (last 15m)
sigiro query "SELECT service_name, count(*) FILTER(WHERE status_code=2) AS errors FROM lake.sigiro_spans WHERE timestamp > make_timestamp($(python3 -c 'import time; print(int(time.time()*1e6)-900*1e6)')::BIGINT) GROUP BY service_name"

# Investigate
sigiro diagnose frontend

# Trace detail
curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" localhost:9999/v1/traces/<trace_id>
EOF
```
