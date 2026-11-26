#!/usr/bin/env bash
# Soak a sigiro binary under a low open-files limit with Pi-like OTLP load.
#
# Reproduces #228: a launchd-started sigiro has a soft limit of 256 open
# files, flushes many tiny batches, and its DuckLake CHECKPOINT then fails with
# "Too many open files". The script runs the server with `ulimit -Sn` lowered,
# sends many small trace, log and metric requests, forces frequent
# checkpoints, and samples file counts, open descriptors, /health and query
# counts every SAMPLE seconds.
#
# usage: scripts/soak-fd-limit.sh <sigiro-binary> [seconds] [out-dir]
# env:   SOAK_NOFILE (256) SOAK_WORKERS (4) SOAK_SAMPLE (30)
#        SOAK_CHECKPOINT_SECS (30) SOAK_PORT_BASE (39910)
# needs: curl, jq, find
set -euo pipefail

binary=$1
seconds=${2:-600}
out=${3:-$(mktemp -d /tmp/sigiro-soak.XXXXXX)}
nofile=${SOAK_NOFILE:-256}
workers=${SOAK_WORKERS:-4}
sample=${SOAK_SAMPLE:-30}
base=${SOAK_PORT_BASE:-39910}
http=$base otlp=$((base + 1)) grpc=$((base + 2))
api=http://127.0.0.1:$http

mkdir -p "$out/home" "$out/data" "$out/status"
: > "$out/samples.jsonl"

(
  ulimit -Sn "$nofile"
  exec env -i PATH="$PATH" HOME="$out/home" SIGIRO_BIND=127.0.0.1 \
    SIGIRO_CHECKPOINT_INTERVAL_SECS="${SOAK_CHECKPOINT_SECS:-30}" RUST_LOG=info \
    "$binary" serve --data-dir "$out/data" --http-port "$http" \
    --otlp-http-port "$otlp" --otlp-grpc-port "$grpc"
) > "$out/server.log" 2>&1 &
server=$!
trap 'kill $(jobs -p) 2>/dev/null || true; wait 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  curl -sf "$api/health" > /dev/null && break
  kill -0 "$server" 2> /dev/null || { echo "server exited, see $out/server.log" >&2; exit 1; }
  sleep 1
done
echo "soak: pid=$server nofile=$(awk '/Max open files/ {print $4}' "/proc/$server/limits") out=$out"

# One Pi-like turn: a 3-span agent trace, 3 log records and 2 metric points.
# Every request records its HTTP status; 2xx rows count as accepted.
send() {
  local kind=$1 body=$2 code
  code=$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
    --data "$body" "http://127.0.0.1:$otlp/v1/$kind" || echo 000)
  echo "$code" >> "$out/status/$kind.$3"
}

worker() {
  local w=$1 i=0 now tid res
  while :; do
    i=$((i + 1))
    now=$(date +%s%N)
    tid=$(printf '%08x%08x%016x' "$w" "$i" "$now")
    res="{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"pi-soak\"}},{\"key\":\"host.name\",\"value\":{\"stringValue\":\"w$w\"}}]}"
    send traces "{\"resourceSpans\":[{\"resource\":$res,\"scopeSpans\":[{\"scope\":{\"name\":\"pi\"},\"spans\":[
{\"traceId\":\"$tid\",\"spanId\":\"$(printf '%08x%08x' "$w" "$i")\",\"name\":\"invoke_agent pi\",\"kind\":1,\"startTimeUnixNano\":\"$now\",\"endTimeUnixNano\":\"$((now + 900000000))\",\"attributes\":[{\"key\":\"gen_ai.operation.name\",\"value\":{\"stringValue\":\"invoke_agent\"}}]},
{\"traceId\":\"$tid\",\"spanId\":\"$(printf '%08x%08x' "$w" $((i + 1000000000)))\",\"parentSpanId\":\"$(printf '%08x%08x' "$w" "$i")\",\"name\":\"chat claude\",\"kind\":3,\"startTimeUnixNano\":\"$now\",\"endTimeUnixNano\":\"$((now + 700000000))\",\"attributes\":[{\"key\":\"gen_ai.usage.input_tokens\",\"value\":{\"intValue\":\"$((i % 4000))\"}},{\"key\":\"gen_ai.request.model\",\"value\":{\"stringValue\":\"claude\"}}]},
{\"traceId\":\"$tid\",\"spanId\":\"$(printf '%08x%08x' "$w" $((i + 2000000000)))\",\"parentSpanId\":\"$(printf '%08x%08x' "$w" "$i")\",\"name\":\"execute_tool bash\",\"kind\":1,\"startTimeUnixNano\":\"$((now + 700000000))\",\"endTimeUnixNano\":\"$((now + 850000000))\",\"attributes\":[{\"key\":\"gen_ai.tool.name\",\"value\":{\"stringValue\":\"bash\"}}]}]}]}]}" "$w"
    send logs "{\"resourceLogs\":[{\"resource\":$res,\"scopeLogs\":[{\"logRecords\":[
{\"timeUnixNano\":\"$now\",\"severityText\":\"INFO\",\"body\":{\"stringValue\":\"turn $i started\"}},
{\"timeUnixNano\":\"$now\",\"severityText\":\"DEBUG\",\"body\":{\"stringValue\":\"tool bash exit 0\"}},
{\"timeUnixNano\":\"$now\",\"severityText\":\"INFO\",\"body\":{\"stringValue\":\"turn $i done\"}}]}]}]}" "$w"
    send metrics "{\"resourceMetrics\":[{\"resource\":$res,\"scopeMetrics\":[{\"metrics\":[
{\"name\":\"pi.context.tokens\",\"gauge\":{\"dataPoints\":[{\"timeUnixNano\":\"$now\",\"asInt\":\"$((i % 200000))\"}]}},
{\"name\":\"pi.turns\",\"sum\":{\"aggregationTemporality\":2,\"isMonotonic\":true,\"dataPoints\":[{\"startTimeUnixNano\":\"1\",\"timeUnixNano\":\"$now\",\"asInt\":\"$i\"}]}}]}]}]}" "$w"
    sleep 0.2
  done
}

query() {
  curl -s -H 'Content-Type: text/plain' --data "$1" "$api/v1/query"
}

counts() {
  query "SELECT (SELECT count(*) FROM sigiro_spans WHERE service_name = 'pi-soak') AS spans,
                (SELECT count(*) FROM sigiro_logs WHERE service_name = 'pi-soak') AS logs,
                (SELECT count(*) FROM sigiro_metrics_gauge WHERE service_name = 'pi-soak')
              + (SELECT count(*) FROM sigiro_metrics_sum WHERE service_name = 'pi-soak') AS points"
}

statuses() {
  cat "$out"/status/* 2> /dev/null | sort | uniq -c | awk '{printf "%s\"%s\":%s", (NR > 1 ? "," : "{"), $2, $1} END {print (NR ? "}" : "{}")}'
}

accepted() {
  local t l m
  t=$(cat "$out"/status/traces.* 2> /dev/null | grep -c '^2' || true)
  l=$(cat "$out"/status/logs.* 2> /dev/null | grep -c '^2' || true)
  m=$(cat "$out"/status/metrics.* 2> /dev/null | grep -c '^2' || true)
  printf '{"spans":%d,"logs":%d,"points":%d}' $((t * 3)) $((l * 3)) $((m * 2))
}

snapshot() {
  local q h
  q=$(counts || echo null)
  h=$(curl -s "$api/health" || echo null)
  jq -cn --argjson t "$1" --argjson files "$(find "$out/data" -name '*.parquet' | wc -l)" \
    --argjson fds "$(find "/proc/$server/fd" -mindepth 1 -maxdepth 1 2> /dev/null | wc -l)" \
    --argjson status "$(statuses)" --argjson accepted "$(accepted)" \
    --arg q "$q" --arg h "$h" \
    '{t: $t, parquet_files: $files, open_fds: $fds, http: $status, accepted: $accepted,
      queried: ($q | try fromjson catch $q), health: ($h | try fromjson catch $h)}' \
    | tee -a "$out/samples.jsonl" \
    | jq -c '{t, parquet_files, open_fds, http, accepted, queried, health: (.health | if type == "object" then {status, reason, storage: (.storage // {} | map_values({ok: .last_checkpoint_ok, failures: .consecutive_checkpoint_failures, data_files, inlined_rows}))} else . end)}'
}

for w in $(seq 1 "$workers"); do worker "$w" & done
start=$(date +%s)
while [ $(($(date +%s) - start)) -lt "$seconds" ]; do
  sleep "$sample"
  kill -0 "$server" 2> /dev/null || { echo "server died" >&2; break; }
  snapshot $(($(date +%s) - start))
done
for w in $(jobs -p); do [ "$w" = "$server" ] || kill "$w" 2> /dev/null || true; done
sleep 5
echo "final (after 5s drain):"
snapshot "$seconds"
plain() { sed 's/\x1b\[[0-9;]*m//g' "$out/server.log"; }
echo "fd_exhaustion_log_lines=$(plain | grep -c -i -E 'too many open files|no file descriptors available' || true)"
echo "checkpoint log lines:"
plain | grep -E 'CHECKPOINT' | sed -E 's/^[^ ]+ +//; s/(failed|completed)(:| ).*/\1/' | sort | uniq -c | sort -rn | head -8 || true
echo "evidence: $out"
