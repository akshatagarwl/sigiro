#!/usr/bin/env bash
# Compares two Sigiro self-diagnose JSON files to detect regressions.
#
# Usage:
#   scripts/ci-compare.sh CURRENT_JSON BASELINE_JSON
#
# Exit codes:
#   0 — no regression detected
#   1 — regression detected
#   2 — usage error
#
# Environment:
#   THRESHOLD_P99   — p99 latency regression tolerance as multiplier (default: 1.20)
#   THRESHOLD_ERROR — error rate absolute regression tolerance (default: 0.05)

set -euo pipefail

THRESHOLD_P99="${THRESHOLD_P99:-1.20}"
THRESHOLD_ERROR="${THRESHOLD_ERROR:-0.05}"

CURRENT="${1:-}"
BASELINE="${2:-}"

usage() {
    echo "Usage: $0 <current_diagnose.json> <baseline_diagnose.json>"
    echo "Compares two Sigiro self-diagnose JSON files for regressions."
    echo "Returns exit 0 (pass) or 1 (regression detected)."
}

if [ -z "$CURRENT" ] || [ -z "$BASELINE" ]; then
    usage
    exit 2
fi

if [ ! -f "$CURRENT" ]; then
    echo "ERROR: Current file not found: $CURRENT"
    exit 2
fi

if [ ! -f "$BASELINE" ]; then
    echo "ERROR: Baseline file not found: $BASELINE"
    exit 2
fi

# Extract key metrics using Python (available in all CI environments)
python3 - <<'PYEOF'
import sys
import json

def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (json.JSONDecodeError, FileNotFoundError) as e:
        print(f"ERROR: failed to load {path}: {e}", file=sys.stderr)
        sys.exit(2)

def get(d, *keys, default=0):
    for k in keys:
        if isinstance(d, dict):
            d = d.get(k, default)
        else:
            return default
    return d if d is not None else default

def pct_change(new, old):
    if old == 0:
        return None
    return (new - old) / old

def check(name, curr_val, base_val, thresh_mult=None, thresh_abs=None):
    if curr_val is None or base_val is None:
        return "SKIP", f"{name}: missing data (current={curr_val}, base={base_val})"

    delta = pct_change(curr_val, base_val)

    # Latency: regression if new > base * thresh_mult
    if thresh_mult is not None:
        threshold = base_val * thresh_mult
        if curr_val > threshold:
            pct = f"{delta*100:.1f}%" if delta is not None else "N/A"
            return "FAIL", f"{name}: {curr_val:.2f}ms > {threshold:.2f}ms (base {base_val:.2f}ms, +{pct})"
        else:
            pct = f"{delta*100:.1f}%" if delta is not None else "N/A"
            return "PASS", f"{name}: {curr_val:.2f}ms vs base {base_val:.2f}ms ({pct})"

    # Error rate: regression if delta > thresh_abs
    if thresh_abs is not None:
        delta_abs = curr_val - base_val
        if delta_abs > thresh_abs:
            return "FAIL", f"{name}: {curr_val:.4f} (base {base_val:.4f}, +{delta_abs:.4f} > {thresh_abs})"
        else:
            return "PASS", f"{name}: {curr_val:.4f} vs base {base_val:.4f}"

    return "SKIP", f"{name}: no threshold configured"

current = load(sys.argv[1])
baseline = load(sys.argv[2])

t_p99 = float(sys.argv[3]) if len(sys.argv) > 3 else 1.20
t_err = float(sys.argv[4]) if len(sys.argv) > 4 else 0.05

results = []

# p99 latency — derived from span latency percentiles
# Look in span_latencies or fall back to computing from spans
curr_p99 = get(current, "span_latencies", "p99", default=None)
base_p99 = get(baseline, "span_latencies", "p99", default=None)

if curr_p99 is None:
    # Try computing from spans array
    def compute_p99(diagnosis):
        spans = diagnosis.get("spans", [])
        if not spans:
            return None
        durations = []
        for s in spans:
            d = s.get("duration_ms") or s.get("duration_nanos", 0) / 1_000_000
            if d:
                durations.append(d)
        if not durations:
            return None
        durations.sort()
        idx = int(len(durations) * 0.99)
        return durations[min(idx, len(durations)-1)]

    curr_p99 = compute_p99(current)
    base_p99 = compute_p99(baseline)

# Total spans (proxy for throughput)
curr_spans = get(current, "span_count", default=0)
base_spans = get(baseline, "span_count", default=0)

# Error count
curr_errors = get(current, "error_count", "count", default=0)
base_errors = get(baseline, "error_count", "count", default=0)

# Error rate
curr_total = get(current, "total_spans", default=curr_spans)
base_total = get(baseline, "total_spans", default=base_spans)
curr_err_rate = (curr_errors / curr_total) if curr_total > 0 else 0.0
base_err_rate = (base_errors / base_total) if base_total > 0 else 0.0

# Check p99 latency
if curr_p99 is not None and base_p99 is not None:
    status, msg = check("p99 latency", curr_p99, base_p99, thresh_mult=t_p99)
    results.append((status, msg))
elif curr_p99 is None and base_p99 is None:
    results.append(("SKIP", "p99 latency: no data"))
elif curr_p99 is None:
    results.append(("SKIP", "p99 latency: current has no data"))
else:
    results.append(("SKIP", "p99 latency: baseline has no data"))

# Check span count (throughput) — regression if spans dropped significantly
# Only fail if it dropped by >30% (not a latency regression)
throughput_thresh = 0.70
if curr_spans > 0 and base_spans > 0:
    if curr_spans < base_spans * throughput_thresh:
        pct = pct_change(curr_spans, base_spans)
        results.append(("FAIL", f"throughput: {curr_spans} spans < {base_spans * throughput_thresh:.0f} (base {base_spans}, {pct*100:.1f}%)"))
    else:
        results.append(("PASS", f"throughput: {curr_spans} spans vs base {base_spans}"))

# Check error rate
status, msg = check("error rate", curr_err_rate, base_err_rate, thresh_abs=t_err)
results.append((status, msg))

# Print results
fail_count = sum(1 for s, _ in results if s == "FAIL")
pass_count = sum(1 for s, _ in results if s == "PASS")
skip_count = sum(1 for s, _ in results if s == "SKIP")

print("=== Sigiro Self-Monitoring Regression Report ===")
print(f"Current:   {sys.argv[1]}")
print(f"Baseline:  {sys.argv[2]}")
print()
for status, msg in results:
    symbol = {"PASS": "✓", "FAIL": "✗", "SKIP": "-"}[status]
    print(f"  [{symbol}] {msg}")
print()
print(f"Results: {pass_count} pass, {fail_count} fail, {skip_count} skip")

if fail_count > 0:
    print()
    print("REGRESSION DETECTED — failing metrics above threshold")
    sys.exit(1)
else:
    print()
    print("PASS — no regression detected")
    sys.exit(0)
PYEOF

EXIT_CODE=$?

if [ $EXIT_CODE -eq 0 ]; then
    :
elif [ $EXIT_CODE -eq 1 ]; then
    :
else
    echo "ERROR: comparison script failed with exit code $EXIT_CODE"
fi

exit $EXIT_CODE
