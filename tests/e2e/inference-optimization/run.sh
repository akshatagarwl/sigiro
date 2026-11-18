#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
OUT="$ROOT/target/inference-optimization-e2e"
VENV="$OUT/aiperf-venv"
PYTHON=${PYTHON312:-$(command -v python3.12 || true)}
MODEL=meta-llama/Llama-3.2-1B-Instruct
RUNNER="$ROOT/scripts/inference_experiment.py"
AIPERF="$ROOT/tests/e2e/inference-optimization/aiperf_docker.sh"
MOCK_SERVER="$VENV/bin/aiperf-mock-server"

# Clear prior experiments but keep the DuckDB extension cache: it lives under
# results/, and re-fetching it makes every run depend on the network — which is
# how this E2E failed on a transient download error.
if [[ -d "$OUT/results" ]]; then
  find "$OUT/results" -mindepth 1 -maxdepth 1 ! -name '.sigiro-extensions' -exec rm -rf {} +
fi
mkdir -p "$OUT"
printf '%s\n' 'local process E2E; no GPU' > "$OUT/hardware.txt"
[[ -n "$PYTHON" ]] || { echo "Python 3.12 is required (or set PYTHON312)" >&2; exit 1; }

if [[ ! -x "$VENV/bin/aiperf" ]]; then
  uv venv --python "$PYTHON" "$VENV"
  uv pip install --python "$VENV/bin/python" 'aiperf[otel]==0.12.0'
fi
if [[ ! -x "$MOCK_SERVER" ]]; then
  uv pip install --python "$VENV/bin/python" \
    'git+https://github.com/ai-dynamo/aiperf.git@9e96b499d1e942e1b338010fa5d99d9d9c96e074#subdirectory=tests/aiperf_mock_server'
fi
if ! docker image inspect sigiro-aiperf-e2e:0.12.0 >/dev/null 2>&1; then
  docker build -t sigiro-aiperf-e2e:0.12.0 \
    -f "$ROOT/tests/e2e/inference-optimization/Dockerfile.aiperf" "$ROOT"
fi

cargo build --bin sigiro

"$PYTHON" - "$OUT/inputs.json" "$MODEL" <<'PY'
import json, sys
path, model = sys.argv[1:]
data = []
for index in range(20):
    data.append({
        "session_id": f"session-{index:03d}",
        "payloads": [{
            "model": model,
            "messages": [{"role": "user", "content": f"Reply with hello world. Request {index}."}],
            "max_tokens": 8,
            "temperature": 0,
            "stream": True,
            "stream_options": {"include_usage": True},
        }],
    })
with open(path, "w") as handle:
    json.dump({"data": data}, handle, indent=2)
PY

write_manifest() {
  local id=$1 candidate_delay=$2 error_every=$3 http=$4 otlp_http=$5 otlp_grpc=$6
  cat > "$OUT/$id.json" <<JSON
{
  "schema_version": 1,
  "experiment_id": "$id",
  "output_root": "$OUT/results",
  "model": {"id": "$MODEL", "revision": "local-tokenizer-contract"},
  "hardware": {"file": "$OUT/hardware.txt"},
  "sigiro": {
    "binary": "$ROOT/target/debug/sigiro",
    "http_port": $http,
    "otlp_http_port": $otlp_http,
    "otlp_grpc_port": $otlp_grpc,
    "startup_timeout_seconds": 90
  },
  "aiperf": {
    "binary": "$AIPERF",
    "endpoint": "http://host.docker.internal:38080",
    "inputs_file": "$OUT/inputs.json",
    "num_profile_runs": 3,
    "common_args": [
      "--endpoint-type", "chat",
      "--tokenizer", "Qwen/Qwen3-0.6B",
      "--concurrency", "2",
      "--request-count", "200",
      "--warmup-request-count", "1",
      "--goodput", "request_latency:500",
      "--random-seed", "42",
      "--ui-type", "none",
      "--no-server-metrics",
      "--no-gpu-telemetry",
      "--wait-for-model-timeout", "60",
      "--export-level", "summary"
    ],
    "timeout_seconds": 120
  },
  "sidecars": [
    {
      "name": "collector-stub",
      "argv": ["$VENV/bin/python", "-u", "-c", "import itertools, time; [(print('alive', flush=True), time.sleep(1)) for _ in itertools.count()]"],
      "env": {}
    }
  ],
  "topology": {"engine": "aiperf-mock-server", "engine_version": "0.12.0"},
  "control": {
    "argv": ["$MOCK_SERVER", "--port", "38080", "--default-model", "$MODEL", "--ttft", "15", "--itl", "2", "--error-rate", "0"],
    "env": {}
  },
  "candidate": {
    "argv": ["$MOCK_SERVER", "--port", "38080", "--default-model", "$MODEL", "--ttft", "$candidate_delay", "--itl", "2", "--error-rate", "$error_every", "--random-seed", "42"],
    "env": {}
  },
  "telemetry": {"gpu_telemetry": false, "server_metrics": false},
  "telemetry_flush_seconds": 2
}
JSON
}

write_manifest local-known-slower 120 0 39999 34318 34317
start=$(date +%s)
"$PYTHON" "$RUNNER" "$OUT/local-known-slower.json" > "$OUT/known-slower.stdout"

# Exit 2 from the checker means "host too noisy to judge", not a product
# failure. Keep going: the error-arm test below is fully deterministic and is
# the half that proves fail-closed behaviour. Propagate the verdict at the end.
set +e
"$PYTHON" - "$OUT/results/local-known-slower" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
comparison = json.loads((root / "comparison.json").read_text())

# --- Deterministic contract. These hold on any host and are the hard gate. ---
assert len(comparison["periods"]) == 6
expected = {
    "gen_ai.client.operation.duration",
    "gen_ai.client.operation.time_to_first_chunk",
    "gen_ai.client.operation.time_per_output_chunk",
    "gen_ai.client.token.usage",
}
request = json.loads((root / "comparison-request.json").read_text())
periods = request["calibration_control"] + [request["control_before"]] + request["candidate"] + [request["control_after"]]
previous_to = 0
for evidence, period in zip(comparison["periods"], periods):
    assert period["from_ts"] > previous_to, period
    assert evidence["run_id"] == period["run_id"], evidence
    assert set(evidence["metric_names"]) == expected, evidence
    assert evidence["telemetry_rows"] > 0, evidence
    assert period["from_ts"] <= evidence["observed_from_ts"] <= period["to_ts"]
    assert period["from_ts"] <= evidence["observed_to_ts"] <= period["to_ts"]
    assert f"= '{period['run_id']}'" in evidence["evidence_sql"]
    previous_to = period["to_ts"]

# Every period ran its sidecar, and every period wrote its ledger row: the same
# instrumentation on both arms, and one row per period chained by parent_run_id.
request_fingerprints = {
    "model_sha": request["model_fingerprint"],
    "hardware_sha": request["hardware_fingerprint"],
    "benchmark_sha": request["benchmark_fingerprint"],
}
parent = None
for evidence, period in zip(comparison["periods"], periods):
    run_id = period["run_id"]
    sidecar_log = (root / run_id / "sidecar-collector-stub.log").read_text()
    assert "alive" in sidecar_log, f"{run_id} sidecar produced no output"
    record = json.loads((root / run_id / "run-record.json").read_text())
    assert record["run_id"] == run_id, record
    assert record["experiment_id"] == "local-known-slower", record
    if parent is None:
        assert "parent_run_id" not in record, record  # the first period has no predecessor
    else:
        assert record["parent_run_id"] == parent, record
    assert record["started_at"] == period["from_ts"], record
    assert record["ended_at"] == period["to_ts"], record
    for field, expected_sha in request_fingerprints.items():
        assert record[field] == expected_sha, (field, record)
    assert record["config_sha"] == request[f"{record['arm']}_config_fingerprint"], record
    assert record["config_json"]["argv"], record
    assert abs(record["goodput"] - evidence["goodput_mean"]) < 1e-9, (record, evidence)
    # Declared in the manifest, so it is recorded; never guessed, so nothing the
    # runner cannot see is present.
    assert record["engine"] == "aiperf-mock-server", record
    assert not {"gpu_model", "tp_size", "quantization", "price_per_hour_usd"} & set(record), record
    assert json.loads((root / run_id / "record-run.stdout").read_text())["run_id"] == run_id
    parent = run_id

# The candidate is 8x slower by construction, so the *ranking* is robust to host
# noise even when the confidence intervals are not.
means = {p["run_id"]: p["goodput_mean"] for p in comparison["periods"]}
assert max(means["B1"], means["B2"]) < min(means["A1"], means["A2"]), means

# --- Statistical verdict. Host-sensitive: the A/A guard compares control
# periods against EACH OTHER, so a laptop whose load wanders during the ~7min
# run makes `drift_detected` a correct refusal rather than a regression. Exit 2
# so a caller can tell "unusable measurement environment" from "product broke".
if comparison["outcome"] != "rejected":
    controls = {k: round(v, 2) for k, v in means.items() if k.startswith("A")}
    spread = (max(controls.values()) - min(controls.values())) / min(controls.values())
    print(
        f"\nENVIRONMENT, not a product failure. The contract above all passed.\n"
        f"  outcome: {comparison['outcome']} - {comparison['reason']}\n"
        f"  control goodput: {controls}  (spread {spread:.0%})\n"
        f"  The A/A calibration correctly declined to compare. Re-run on an idle\n"
        f"  host; a spread above ~10% will keep tripping it.\n", file=sys.stderr)
    raise SystemExit(2)
assert comparison["winner"] == "control", comparison
assert json.loads((root / "status.json").read_text())["status"] == "completed"
PY
verdict=$?
set -e
if [ "$verdict" -ne 0 ] && [ "$verdict" -ne 2 ]; then exit "$verdict"; fi

# The rows must be IN the catalog, not merely written to a JSON file next to it.
# `record-run` refuses a run it already holds, so a second attempt failing with
# "already has run A1" is proof the first insert committed. This also checks the
# ledger is its own catalog: the observer's is deleted with the experiment, and
# DuckDB's single-writer lock means nothing can insert into it while it serves.
export SIGIRO_DATA_DIR="$OUT/results/ledger" SIGIRO_EXTENSION_DIR="$OUT/results/.sigiro-extensions"
if "$ROOT/target/debug/sigiro" record-run \
  --file "$OUT/results/local-known-slower/A1/run-record.json" \
  > "$OUT/re-record.stdout" 2> "$OUT/re-record.stderr"; then
  echo "re-recording A1 must be refused; the ledger accepted a duplicate row" >&2
  exit 1
fi
grep -q 'already has run A1' "$OUT/re-record.stderr" || {
  echo "record-run failed for the wrong reason:" >&2; cat "$OUT/re-record.stderr" >&2; exit 1; }
unset SIGIRO_DATA_DIR SIGIRO_EXTENSION_DIR

write_manifest local-faster-with-errors 1 30 40999 35318 35317
if "$PYTHON" "$RUNNER" "$OUT/local-faster-with-errors.json" > "$OUT/error.stdout" 2> "$OUT/error.stderr"; then
  echo "error arm unexpectedly completed" >&2
  exit 1
fi

"$PYTHON" - "$OUT/results" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
failed = root / "local-faster-with-errors"
status = json.loads((failed / "status.json").read_text())
assert status == {
    "status": "failed",
    "error": "B1 AIPerf artifact reports nonzero request_error_rate_avg",
}, status
aggregate = json.loads((failed / "B1/aiperf/aggregate/profile_export_aiperf_aggregate.json").read_text())
assert aggregate["metrics"]["request_error_rate_avg"]["mean"] > 0, aggregate
assert (failed / "B1/aiperf.stdout").exists()  # AIPerf misleadingly exited zero.
assert not (failed / "B2").exists()
assert not (failed / "A2").exists()
# The periods that did finish keep their ledger rows; the one that failed has
# none, because the row is written only once the period is complete.
for run_id in ("A0-1", "A0-2", "A1"):
    record = json.loads((failed / run_id / "run-record.json").read_text())
    assert record["experiment_id"] == "local-faster-with-errors", record
assert not (failed / "B1/run-record.json").exists()
ledger = [json.loads(line) for line in (root / "experiments.jsonl").read_text().splitlines() if line]
assert [row["experiment_id"] for row in ledger] == ["local-known-slower"], ledger
PY

# --- The two documented ledger queries, run against a real /v1/query. ---
# Byte-identical to the two SQL blocks in README.md ("Two queries over the
# ledger"); a documented query nobody runs is a query that has already rotted.
# They read the ledger this run just wrote: six finalized rows for
# local-known-slower and three un-finalized ones for the experiment that failed.
LINEAGE_SQL=$(cat <<'SQL'
SELECT child.run_id, child.arm, child.parent_run_id, parent.arm AS parent_arm,
       substr(child.config_sha, 1, 12) AS config_sha,
       substr(parent.config_sha, 1, 12) AS parent_config_sha,
       child.config_json <> parent.config_json AS config_changed,
       round(child.goodput - parent.goodput, 3) AS goodput_delta,
       child.outcome, child.drift_detected
FROM lake.sigiro_experiment_runs AS child
JOIN lake.sigiro_experiment_runs AS parent
  ON parent.experiment_id = child.experiment_id
 AND parent.run_id = child.parent_run_id
WHERE child.experiment_id = 'local-known-slower'
ORDER BY child.started_at
SQL
)
OP_CLASS_SQL=$(cat <<'SQL'
SELECT kernel,
       sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END) AS control_ns,
       sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END) AS candidate_ns,
       sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END)
         - sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END) AS delta_ns,
       sum(executions) AS executions,
       any_value(bucket) AS bucket
FROM lake.sigiro_gpu_kernels
WHERE experiment_id = 'local-known-slower' AND run_id IN ('A1', 'B1')
GROUP BY kernel
ORDER BY abs(sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END)
             - sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END)) DESC, kernel
SQL
)

# This host has no GPU, so the kernel rows are a fixture — written to Parquet and
# ingested through the real `sigiro ingest-kernels`, because what the op-class
# query has to be proven against is a real DuckLake catalog, not a real GPU. The
# candidate swaps flash attention for paged attention, which is the "moved
# between arms" case the query exists to show.
"$VENV/bin/python" - "$OUT/kernels.parquet" local-known-slower <<'PY'
import datetime, sys
import pyarrow as pa
import pyarrow.parquet as pq

path, experiment_id = sys.argv[1:]
# run_id, arm, kernel, layer, op, bucket, total_ns, executions
ROWS = [
    ("A1", "control", "sm90_gemm", "model.layers.0.mlp", "matmul", "attributed", 4000, 4),
    ("A1", "control", "flash_attn_fwd", "model.layers.0.attn", "attention", "attributed", 6000, 2),
    ("A1", "control", "elementwise_add", "", "", "residual_no_provenance", 1000, 1),
    ("B1", "candidate", "sm90_gemm", "model.layers.0.mlp", "matmul", "attributed", 5000, 4),
    ("B1", "candidate", "paged_attn_fwd", "model.layers.0.attn", "attention", "attributed", 9000, 2),
    ("B1", "candidate", "elementwise_add", "", "", "residual_no_provenance", 1000, 1),
]
count = len(ROWS)
observed = datetime.datetime(2026, 1, 1, 0, 0, 0)
pq.write_table(
    pa.table(
        {
            "run_id": pa.array([row[0] for row in ROWS], pa.string()),
            "experiment_id": pa.array([experiment_id] * count, pa.string()),
            "arm": pa.array([row[1] for row in ROWS], pa.string()),
            "timestamp": pa.array([observed] * count, pa.timestamp("us")),
            "service_name": pa.array(["aiperf-mock-server"] * count, pa.string()),
            "device": pa.array(["cuda:0"] * count, pa.string()),
            "rank": pa.array([0] * count, pa.int32()),
            "graph_id": pa.array([1] * count, pa.int64()),
            "node_index": pa.array(list(range(count)), pa.int64()),
            "kernel": pa.array([row[2] for row in ROWS], pa.string()),
            "layer": pa.array([row[3] for row in ROWS], pa.string()),
            "op": pa.array([row[4] for row in ROWS], pa.string()),
            "attribution_method": pa.array(["graph-node"] * count, pa.string()),
            "bucket": pa.array([row[5] for row in ROWS], pa.string()),
            "executions": pa.array([row[7] for row in ROWS], pa.int64()),
            "total_ns": pa.array([row[6] for row in ROWS], pa.int64()),
            "mean_ns": pa.array([row[6] // row[7] for row in ROWS], pa.int64()),
            "max_ns": pa.array([row[6] for row in ROWS], pa.int64()),
            "occupancy_pct": pa.array([62.5] * count, pa.float64()),
            "registers_per_thread": pa.array([128] * count, pa.int32()),
            "shared_mem_bytes": pa.array([49152] * count, pa.int64()),
            "limiting_factor": pa.array(["memory"] * count, pa.string()),
            "source_nodes": pa.array(["node_1"] * count, pa.string()),
        }
    ),
    path,
)
PY
SIGIRO_DATA_DIR="$OUT/results/ledger" SIGIRO_EXTENSION_DIR="$OUT/results/.sigiro-extensions" \
  "$ROOT/target/debug/sigiro" ingest-kernels --file "$OUT/kernels.parquet" \
  > "$OUT/ingest-kernels.stdout" 2> "$OUT/ingest-kernels.stderr"

# Serve the LEDGER catalog, not the observer's: the observer's is deleted with
# its experiment, and lineage across experiments is the only thing this table is
# for. Nothing else holds the catalog now, both runners having exited.
LEDGER_URL=http://127.0.0.1:41999
SIGIRO_EXTENSION_DIR="$OUT/results/.sigiro-extensions" "$ROOT/target/debug/sigiro" serve \
  --data-dir "$OUT/results/ledger" --http-port 41999 \
  --otlp-http-port 36318 --otlp-grpc-port 36317 > "$OUT/ledger-serve.log" 2>&1 &
ledger_pid=$!
trap 'kill "$ledger_pid" 2>/dev/null || true' EXIT
for _ in $(seq 90); do
  if curl -sf "$LEDGER_URL/health" 2>/dev/null | grep -q ready; then break; fi
  sleep 1
done
if ! curl -sf "$LEDGER_URL/health" | grep -q ready; then
  echo "the ledger server never became ready" >&2; cat "$OUT/ledger-serve.log" >&2; exit 1
fi

# Raw SQL as the request body — no JSON envelope. --fail-with-body so a rejected
# query is a failure here rather than an error object the asserts read as data.
query() { curl -sS --fail-with-body -X POST "$LEDGER_URL/v1/query" --data-binary @-; }
printf '%s' "$LINEAGE_SQL" | query > "$OUT/lineage.json"
printf '%s' "$OP_CLASS_SQL" | query > "$OUT/op-class.json"
# The same lineage query against the experiment that failed in B1: its rows were
# never finalized, so the verdict must be absent there. That is what proves
# finalize-experiment wrote a verdict per experiment and not per table.
printf '%s' "${LINEAGE_SQL/local-known-slower/local-faster-with-errors}" | query \
  > "$OUT/lineage-unfinalized.json"
kill "$ledger_pid"

"$PYTHON" - "$OUT" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
lineage = json.loads((out / "lineage.json").read_text())
unfinalized = json.loads((out / "lineage-unfinalized.json").read_text())
op_class = json.loads((out / "op-class.json").read_text())
comparison = json.loads((out / "results/local-known-slower/comparison.json").read_text())

# --- LINEAGE. Six periods, five parent edges: A0-1 has no parent to join to. ---
assert [row["run_id"] for row in lineage] == ["A0-2", "A1", "B1", "B2", "A2"], lineage
assert [row["parent_run_id"] for row in lineage] == ["A0-1", "A0-2", "A1", "B1", "B2"], lineage
# config_json differs exactly at the two arm switches, and nowhere else: that is
# the query answering "what changed" without anyone reading argv by hand.
assert [row["config_changed"] for row in lineage] == [False, False, True, False, True], lineage
by_run = {row["run_id"]: row for row in lineage}
assert by_run["B1"]["config_sha"] != by_run["B1"]["parent_config_sha"], by_run["B1"]
assert by_run["B2"]["config_sha"] == by_run["B2"]["parent_config_sha"], by_run["B2"]
# And "did it help", from this one table with no join: the verdict is on every
# row, and drift_detected agrees with it.
outcome = comparison["outcome"]
assert {row["outcome"] for row in lineage} == {outcome}, lineage
assert {row["drift_detected"] for row in lineage} == {outcome == "drift_detected"}, lineage
# The candidate is 8x slower by construction, so its step down and the step back
# up to A2 are the two big movements on any host.
assert by_run["B1"]["goodput_delta"] < 0, by_run["B1"]
assert by_run["A2"]["goodput_delta"] > 0, by_run["A2"]
# The experiment that died in B1 was never compared, so it has no verdict — the
# finalized rows above did not spill onto it.
assert [row["run_id"] for row in unfinalized] == ["A0-2", "A1"], unfinalized
assert {row["outcome"] for row in unfinalized} == {None}, unfinalized
assert {row["drift_detected"] for row in unfinalized} == {None}, unfinalized

# --- OP-CLASS DIFF. Biggest mover first; a kernel in one arm only reads zero
# on the other side, which is how a swapped implementation shows up at all. ---
assert [row["kernel"] for row in op_class] == [
    "paged_attn_fwd", "flash_attn_fwd", "sm90_gemm", "elementwise_add"], op_class
by_kernel = {row["kernel"]: row for row in op_class}
assert by_kernel["paged_attn_fwd"]["control_ns"] == 0, by_kernel["paged_attn_fwd"]
assert by_kernel["paged_attn_fwd"]["delta_ns"] == 9000, by_kernel["paged_attn_fwd"]
assert by_kernel["flash_attn_fwd"]["candidate_ns"] == 0, by_kernel["flash_attn_fwd"]
assert by_kernel["flash_attn_fwd"]["delta_ns"] == -6000, by_kernel["flash_attn_fwd"]
assert by_kernel["sm90_gemm"]["delta_ns"] == 1000, by_kernel["sm90_gemm"]
assert by_kernel["sm90_gemm"]["executions"] == 8, by_kernel["sm90_gemm"]
# `bucket` is never null, so unattributed GPU time is a row in the diff and the
# columns account for every nanosecond of both runs rather than part of them.
assert by_kernel["elementwise_add"]["bucket"] == "residual_no_provenance", by_kernel
assert sum(row["control_ns"] for row in op_class) == 11000, op_class
assert sum(row["candidate_ns"] for row in op_class) == 15000, op_class
PY

elapsed=$(( $(date +%s) - start ))
if [ "$verdict" -eq 2 ]; then
  printf 'local inference optimization E2E: contract PASSED, verdict SKIPPED (noisy host) in %ss\n' "$elapsed"
  exit 2
fi
printf 'local inference optimization E2E passed in %ss\n' "$elapsed"
printf 'artifacts: %s\n' "$OUT/results"
