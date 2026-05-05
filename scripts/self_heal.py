#!/usr/bin/env python3
"""
Self-healing optimizer loop for Sigiro.

Runs ingestion benchmarks periodically, tracks performance trends, and
auto-adjusts configuration parameters to maintain target throughput/latency.

Usage:
    # One-shot benchmark + optimization suggestion
    python scripts/self_heal.py --mode suggest

    # Continuous optimization loop (daemon mode)
    python scripts/self_heal.py --mode loop --interval 300

    # Dry-run: show what would change without applying
    python scripts/self_heal.py --mode suggest --dry-run

Requires: scripts/bench_ingest.py, opentelemetry-proto, requests
"""

import argparse
import json
import os
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Optional


STATE_FILE = Path(".self-loop/sigiro_optimizer_state.json")
CONFIG_HISTORY_FILE = Path(".self-loop/optimizer_config_history.json")

# Tunable parameters and their ranges
TUNABLES = {
    "SIGIRO_FLUSH_INTERVAL_SECS": {
        "type": "int",
        "min": 1,
        "max": 30,
        "unit": "seconds",
        "description": "How often buffered rows are flushed to DuckDB",
        "impact": "latency",
        "direction": "inverse",  # lower = better latency, worse throughput
    },
    "SIGIRO_FLUSH_ROW_THRESHOLD": {
        "type": "int",
        "min": 1000,
        "max": 100_000,
        "unit": "rows",
        "description": "Row count that triggers immediate flush",
        "impact": "throughput",
        "direction": "direct",  # higher = better throughput
    },
    "SIGIRO_BACKPRESSURE_POLICY": {
        "type": "enum",
        "values": ["block", "drop-oldest"],
        "description": "Behavior when per-project channel is full",
        "impact": "reliability",
        "direction": "special",
    },
}

# Performance targets
DEFAULT_TARGETS = {
    "min_throughput": 5000,      # spans/sec minimum
    "max_p99_latency_ms": 100.0,  # p99 latency ceiling
    "max_error_rate": 0.01,      # 1% error rate max
}


@dataclass
class BenchmarkResult:
    total_spans: int
    throughput: float
    p50_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    errors: int
    elapsed_sec: float


@dataclass
class OptimizerState:
    iteration: int
    last_benchmark: Optional[dict]
    trend: list[dict]  # last N benchmark results
    current_config: dict
    best_config: dict
    best_throughput: float
    best_p99: float


@dataclass
class ConfigSuggestion:
    param: str
    current_value: str
    suggested_value: str
    reason: str
    expected_impact: str


def load_state() -> OptimizerState:
    """Load optimizer state from disk."""
    if STATE_FILE.exists():
        with open(STATE_FILE) as f:
            data = json.load(f)
        return OptimizerState(
            iteration=data.get("iteration", 0),
            last_benchmark=data.get("last_benchmark"),
            trend=data.get("trend", []),
            current_config=data.get("current_config", {}),
            best_config=data.get("best_config", {}),
            best_throughput=data.get("best_throughput", 0.0),
            best_p99=data.get("best_p99", float("inf")),
        )
    return OptimizerState(
        iteration=0,
        last_benchmark=None,
        trend=[],
        current_config={},
        best_config={},
        best_throughput=0.0,
        best_p99=float("inf"),
    )


def save_state(state: OptimizerState) -> None:
    """Save optimizer state to disk."""
    STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(STATE_FILE, "w") as f:
        json.dump(asdict(state), f, indent=2)


def get_current_config() -> dict:
    """Read current sigiro configuration from environment."""
    config = {}
    for key in TUNABLES:
        val = os.environ.get(key)
        if val is None:
            # Use defaults
            defaults = {
                "SIGIRO_FLUSH_INTERVAL_SECS": "1",
                "SIGIRO_FLUSH_ROW_THRESHOLD": "10000",
                "SIGIRO_BACKPRESSURE_POLICY": "block",
            }
            val = defaults.get(key, "")
        config[key] = val
    return config


def run_benchmark(rate: int = 5000, duration: int = 30) -> Optional[BenchmarkResult]:
    """Run the ingestion benchmark and parse results."""
    bench_script = Path(__file__).parent / "bench_ingest.py"
    if not bench_script.exists():
        print(f"ERROR: Benchmark script not found: {bench_script}")
        return None

    print(f"Running benchmark: {rate} spans/sec for {duration}s...")
    result = subprocess.run(
        [sys.executable, str(bench_script),
         "--rate", str(rate),
         "--duration", str(duration),
         "--endpoint", "http://localhost:4318"],
        capture_output=True,
        text=True,
        timeout=duration + 60,
    )

    if result.returncode != 0:
        print(f"Benchmark failed:\n{result.stderr}")
        return None

    # Parse results from the last JSON file created
    result_files = sorted(
        Path(".").glob("bench_result_*.json"),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    if not result_files:
        print("ERROR: No benchmark result file found")
        return None

    with open(result_files[0]) as f:
        data = json.load(f)

    r = data["results"]
    return BenchmarkResult(
        total_spans=r["total_spans"],
        throughput=r["throughput"],
        p50_latency_ms=r["p50_latency_ms"],
        p95_latency_ms=r["p95_latency_ms"],
        p99_latency_ms=r["p99_latency_ms"],
        errors=r["errors"],
        elapsed_sec=r["elapsed_sec"],
    )


def analyze_trend(trend: list[dict]) -> dict:
    """Analyze performance trend from recent benchmarks."""
    if len(trend) < 2:
        return {"trend": "insufficient_data", "throughput_change": 0, "latency_change": 0}

    recent = trend[-5:]  # last 5 runs
    throughputs = [r["throughput"] for r in recent]
    p99s = [r["p99_latency_ms"] for r in recent]

    throughput_change = (throughputs[-1] - throughputs[0]) / throughputs[0] if throughputs[0] > 0 else 0
    latency_change = (p99s[-1] - p99s[0]) / p99s[0] if p99s[0] > 0 else 0

    trend_dir = "stable"
    if throughput_change < -0.1 or latency_change > 0.2:
        trend_dir = "degrading"
    elif throughput_change > 0.1 or latency_change < -0.2:
        trend_dir = "improving"

    return {
        "trend": trend_dir,
        "throughput_change_pct": round(throughput_change * 100, 1),
        "latency_change_pct": round(latency_change * 100, 1),
        "avg_throughput": round(sum(throughputs) / len(throughputs), 0),
        "avg_p99": round(sum(p99s) / len(p99s), 2),
    }


def generate_suggestions(
    state: OptimizerState,
    targets: dict,
    benchmark: BenchmarkResult,
) -> list[ConfigSuggestion]:
    """Generate configuration suggestions based on benchmark results."""
    suggestions = []
    config = state.current_config

    # Check if we're meeting targets
    throughput_ok = benchmark.throughput >= targets["min_throughput"]
    latency_ok = benchmark.p99_latency_ms <= targets["max_p99_latency_ms"]
    error_ok = (benchmark.errors / max(benchmark.total_spans, 1)) <= targets["max_error_rate"]

    # Trend analysis
    trend = analyze_trend(state.trend)

    # Suggestion 1: If latency is too high, reduce flush interval
    if not latency_ok and trend["trend"] in ("degrading", "stable"):
        current_interval = int(config.get("SIGIRO_FLUSH_INTERVAL_SECS", "1"))
        if current_interval > 1:
            new_interval = max(1, current_interval // 2)
            suggestions.append(ConfigSuggestion(
                param="SIGIRO_FLUSH_INTERVAL_SECS",
                current_value=str(current_interval),
                suggested_value=str(new_interval),
                reason=f"p99 latency {benchmark.p99_latency_ms:.1f}ms exceeds target {targets['max_p99_latency_ms']}ms",
                expected_impact="Lower flush interval reduces buffering latency",
            ))

    # Suggestion 2: If throughput is too low, increase row threshold
    if not throughput_ok:
        current_threshold = int(config.get("SIGIRO_FLUSH_ROW_THRESHOLD", "10000"))
        max_threshold = TUNABLES["SIGIRO_FLUSH_ROW_THRESHOLD"]["max"]
        if current_threshold < max_threshold:
            new_threshold = min(max_threshold, current_threshold * 2)
            suggestions.append(ConfigSuggestion(
                param="SIGIRO_FLUSH_ROW_THRESHOLD",
                current_value=str(current_threshold),
                suggested_value=str(new_threshold),
                reason=f"Throughput {benchmark.throughput:.0f} spans/sec below target {targets['min_throughput']}",
                expected_impact="Higher threshold batches more rows per flush, reducing I/O overhead",
            ))

    # Suggestion 3: If errors are high, switch to drop-oldest backpressure
    if not error_ok:
        current_policy = config.get("SIGIRO_BACKPRESSURE_POLICY", "block")
        if current_policy != "drop-oldest":
            suggestions.append(ConfigSuggestion(
                param="SIGIRO_BACKPRESSURE_POLICY",
                current_value=current_policy,
                suggested_value="drop-oldest",
                reason=f"Error rate too high ({benchmark.errors} errors in {benchmark.total_spans} spans)",
                expected_impact="Drop oldest messages instead of blocking to prevent cascades",
            ))

    # Suggestion 4: If everything is good but we can optimize further
    if throughput_ok and latency_ok and error_ok:
        current_interval = int(config.get("SIGIRO_FLUSH_INTERVAL_SECS", "1"))
        current_threshold = int(config.get("SIGIRO_FLUSH_ROW_THRESHOLD", "10000"))

        # Try reducing threshold if latency headroom exists
        if benchmark.p99_latency_ms < targets["max_p99_latency_ms"] * 0.5 and current_threshold > 1000:
            new_threshold = max(1000, current_threshold // 2)
            suggestions.append(ConfigSuggestion(
                param="SIGIRO_FLUSH_ROW_THRESHOLD",
                current_value=str(current_threshold),
                suggested_value=str(new_threshold),
                reason="Latency headroom available, can reduce batch size for fresher data",
                expected_impact="More frequent smaller flushes, slightly lower throughput",
            ))

    return suggestions


def apply_suggestions(suggestions: list[ConfigSuggestion], dry_run: bool = False) -> None:
    """Apply configuration suggestions to environment."""
    if not suggestions:
        print("No configuration changes needed.")
        return

    for s in suggestions:
        if dry_run:
            print(f"  [DRY-RUN] Would set {s.param}={s.suggested_value}")
            print(f"            Reason: {s.reason}")
            print(f"            Expected: {s.expected_impact}")
        else:
            os.environ[s.param] = s.suggested_value
            print(f"  Applied: {s.param}={s.suggested_value}")
            print(f"           {s.reason}")

    if not dry_run:
        print("\nNOTE: Environment variables set in this process.")
        print("To persist: export these vars before starting sigiro serve")


def print_diagnostics(state: OptimizerState, benchmark: BenchmarkResult, targets: dict) -> None:
    """Print a diagnostic summary."""
    trend = analyze_trend(state.trend)

    print("\n" + "=" * 60)
    print("SIGIRO SELF-HEAL DIAGNOSTICS")
    print("=" * 60)

    print(f"\nIteration: {state.iteration}")
    print(f"Current config:")
    for k, v in state.current_config.items():
        print(f"  {k}={v}")

    print(f"\nPerformance (latest):")
    print(f"  Throughput:     {benchmark.throughput:,.0f} spans/sec")
    print(f"  Latency p50:    {benchmark.p50_latency_ms:.2f}ms")
    print(f"  Latency p95:    {benchmark.p95_latency_ms:.2f}ms")
    print(f"  Latency p99:    {benchmark.p99_latency_ms:.2f}ms")
    print(f"  Errors:         {benchmark.errors}")
    print(f"  Elapsed:        {benchmark.elapsed_sec:.1f}s")

    print(f"\nTargets:")
    print(f"  Min throughput: {targets['min_throughput']:,} spans/sec")
    print(f"  Max p99:        {targets['max_p99_latency_ms']:.1f}ms")
    print(f"  Max errors:     {targets['max_error_rate']*100:.1f}%")

    print(f"\nTrend (last {min(len(state.trend), 5)} runs):")
    print(f"  Direction:      {trend['trend']}")
    print(f"  Throughput:     {trend.get('throughput_change_pct', 0):+.1f}%")
    print(f"  Latency:        {trend.get('latency_change_pct', 0):+.1f}%")

    print("=" * 60)


def run_suggest_mode(args) -> int:
    """Run one-shot benchmark and suggest optimizations."""
    state = load_state()
    state.iteration += 1
    state.current_config = get_current_config()

    benchmark = run_benchmark(rate=args.rate, duration=args.duration)
    if benchmark is None:
        print("ERROR: Benchmark failed")
        return 1

    # Update trend
    state.last_benchmark = {
        "timestamp": time.time(),
        "throughput": benchmark.throughput,
        "p99_latency_ms": benchmark.p99_latency_ms,
        "errors": benchmark.errors,
    }
    state.trend.append(state.last_benchmark)
    state.trend = state.trend[-20:]  # Keep last 20

    # Track best config
    if benchmark.throughput > state.best_throughput:
        state.best_throughput = benchmark.throughput
        state.best_config = dict(state.current_config)
        state.best_p99 = benchmark.p99_latency_ms

    print_diagnostics(state, benchmark, DEFAULT_TARGETS)

    suggestions = generate_suggestions(state, DEFAULT_TARGETS, benchmark)
    if suggestions:
        print("\nSuggestions:")
        for s in suggestions:
            print(f"\n  [{s.param}]")
            print(f"    Current:  {s.current_value}")
            print(f"    Suggest:  {s.suggested_value}")
            print(f"    Reason:   {s.reason}")
            print(f"    Impact:   {s.expected_impact}")

    apply_suggestions(suggestions, dry_run=args.dry_run)

    save_state(state)
    return 0


def run_loop_mode(args) -> int:
    """Run continuous optimization loop."""
    print(f"Starting self-healing loop (interval={args.interval}s, press Ctrl+C to stop)")

    while True:
        try:
            print(f"\n{'='*60}")
            print(f"Iteration at {time.strftime('%Y-%m-%d %H:%M:%S')}")
            print(f"{'='*60}")

            exit_code = run_suggest_mode(args)
            if exit_code != 0:
                print("Benchmark failed, waiting for next iteration...")

            print(f"\nSleeping {args.interval}s...")
            time.sleep(args.interval)

        except KeyboardInterrupt:
            print("\nStopped by user.")
            break
        except Exception as e:
            print(f"Error in loop: {e}")
            time.sleep(args.interval)

    return 0


def main():
    parser = argparse.ArgumentParser(description="Sigiro self-healing optimizer")
    parser.add_argument("--mode", choices=["suggest", "loop"], default="suggest",
                        help="suggest=one-shot, loop=continuous daemon")
    parser.add_argument("--rate", type=int, default=5000,
                        help="Benchmark target spans/sec")
    parser.add_argument("--duration", type=int, default=30,
                        help="Benchmark duration in seconds")
    parser.add_argument("--interval", type=int, default=300,
                        help="Loop interval in seconds (mode=loop)")
    parser.add_argument("--dry-run", action="store_true",
                        help="Show suggestions without applying")
    args = parser.parse_args()

    if args.mode == "suggest":
        sys.exit(run_suggest_mode(args))
    else:
        sys.exit(run_loop_mode(args))


if __name__ == "__main__":
    main()
