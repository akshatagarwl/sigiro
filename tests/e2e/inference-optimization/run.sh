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

rm -rf "$OUT/results"
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
    "num_profile_runs": 2,
    "common_args": [
      "--endpoint-type", "chat",
      "--tokenizer", "Qwen/Qwen3-0.6B",
      "--concurrency", "2",
      "--request-count", "40",
      "--warmup-request-count", "1",
      "--goodput", "request_latency:500",
      "--random-seed", "42",
      "--ui-type", "none",
      "--no-server-metrics",
      "--no-gpu-telemetry",
      "--wait-for-model-timeout", "15",
      "--export-level", "summary"
    ],
    "timeout_seconds": 120
  },
  "control": {
    "argv": ["$MOCK_SERVER", "--port", "38080", "--default-model", "$MODEL", "--ttft", "15", "--itl", "2", "--error-rate", "0"],
    "env": {}
  },
  "candidate": {
    "argv": ["$MOCK_SERVER", "--port", "38080", "--default-model", "$MODEL", "--ttft", "$candidate_delay", "--itl", "2", "--error-rate", "$error_every", "--random-seed", "42"],
    "env": {}
  },
  "telemetry_flush_seconds": 2
}
JSON
}

write_manifest local-known-slower 120 0 39999 34318 34317
start=$(date +%s)
"$PYTHON" "$RUNNER" "$OUT/local-known-slower.json" > "$OUT/known-slower.stdout"

"$PYTHON" - "$OUT/results/local-known-slower" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
comparison = json.loads((root / "comparison.json").read_text())
assert comparison["outcome"] == "rejected", comparison
assert comparison["winner"] == "control", comparison
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
assert json.loads((root / "status.json").read_text())["status"] == "completed"
PY

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
ledger = [json.loads(line) for line in (root / "experiments.jsonl").read_text().splitlines() if line]
assert [row["experiment_id"] for row in ledger] == ["local-known-slower"], ledger
PY

elapsed=$(( $(date +%s) - start ))
printf 'local inference optimization E2E passed in %ss\n' "$elapsed"
printf 'artifacts: %s\n' "$OUT/results"
