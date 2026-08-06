#!/usr/bin/env python3
"""
Run one Sigiro self-improvement measurement iteration.

Stable Sigiro receives self-telemetry from candidate Sigiro. The harness then
benchmarks candidate ingest, asks stable Sigiro to diagnose the candidate,
and writes the evidence Codex should use for the next code change.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from html import escape
from pathlib import Path
from typing import Any


SERVICE_COUNTS_SQL = (
    "SELECT "
    "(SELECT count(*) FROM lake.sigiro_spans WHERE service_name = 'sigiro-candidate') AS spans_all_time, "
    "(SELECT count(*) FROM lake.sigiro_logs WHERE service_name = 'sigiro-candidate') AS logs_all_time, "
    "(SELECT count(*) FROM lake.sigiro_profiles WHERE service_name = 'sigiro-candidate') AS profiles_all_time, "
    "CAST((SELECT max(timestamp) FROM lake.sigiro_logs WHERE service_name = 'sigiro-candidate') AS VARCHAR) AS latest_log_ts, "
    "CAST((SELECT max(timestamp) FROM lake.sigiro_profiles WHERE service_name = 'sigiro-candidate') AS VARCHAR) AS latest_profile_ts"
)


@dataclass
class Ports:
    http: int
    otlp_http: int
    otlp_grpc: int


def run_json(cmd: list[str], env: dict[str, str]) -> Any:
    proc = subprocess.run(cmd, env=env, text=True, capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(
            "command failed:\n"
            f"  {' '.join(cmd)}\n"
            f"exit={proc.returncode}\n"
            f"stdout:\n{proc.stdout}\n"
            f"stderr:\n{proc.stderr}"
        )
    return json.loads(proc.stdout)


def wait_health(base_url: str, timeout_sec: int) -> tuple[float, dict[str, Any]]:
    start = time.monotonic()
    deadline = start + timeout_sec
    last_error = ""
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=2) as resp:
                body = json.loads(resp.read().decode("utf-8"))
                if body.get("status") == "ready":
                    return time.monotonic() - start, body
                last_error = json.dumps(body)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            last_error = str(exc)
        time.sleep(0.25)
    raise RuntimeError(f"{base_url} did not become ready within {timeout_sec}s: {last_error}")


def start_sigiro(
    bin_path: Path,
    data_dir: Path,
    ports: Ports,
    api_key: str,
    extra_env: dict[str, str],
    log_path: Path,
) -> subprocess.Popen[str]:
    env = os.environ.copy()
    env.update(extra_env)
    env["SIGIRO_API_KEY"] = api_key
    data_dir.mkdir(parents=True, exist_ok=True)
    log_file = log_path.open("w")
    return subprocess.Popen(
        [
            str(bin_path),
            "serve",
            "--data-dir",
            str(data_dir),
            "--http-port",
            str(ports.http),
            "--otlp-http-port",
            str(ports.otlp_http),
            "--otlp-grpc-port",
            str(ports.otlp_grpc),
        ],
        stdout=log_file,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
    )


def stop_process(proc: subprocess.Popen[str] | None) -> None:
    if proc is None or proc.poll() is not None:
        return
    proc.send_signal(signal.SIGINT)
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def newest_bench_result(out_dir: Path) -> dict[str, Any]:
    files = sorted(out_dir.glob("bench_result_*.json"), key=lambda p: p.stat().st_mtime)
    if not files:
        raise RuntimeError("bench_ingest.py did not write bench_result_*.json")
    return json.loads(files[-1].read_text())


def query_service_counts(base_url: str, api_key: str) -> list[dict[str, Any]]:
    request = urllib.request.Request(
        f"{base_url}/v1/query",
        data=SERVICE_COUNTS_SQL.encode(),
        headers={"Authorization": f"Bearer {api_key}"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.loads(response.read().decode())


def wait_for_profiles(
    base_url: str,
    api_key: str,
    timeout_sec: int,
    profiles_before: int,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout_sec
    last_counts: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        last_counts = query_service_counts(base_url, api_key)
        profiles_after = int(
            last_counts[0].get("profiles_all_time", 0) if last_counts else 0
        )
        if profiles_after > profiles_before:
            return last_counts
        time.sleep(1)
    profiles_after = int(
        last_counts[0].get("profiles_all_time", 0) if last_counts else 0
    )
    raise RuntimeError(
        "candidate produced no new Pyroscope profiles before timeout "
        f"(before={profiles_before}, after={profiles_after}); "
        "inspect candidate.log and stable.log"
    )


def load_records(records_path: Path) -> list[dict[str, Any]]:
    if not records_path.exists():
        raise RuntimeError(f"no measurement history found at {records_path}")
    records = []
    for line_number, line in enumerate(records_path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            raise RuntimeError(
                f"invalid JSON in {records_path} line {line_number}: {exc}"
            ) from exc
        for field in ("iteration", "commit", "throughput", "p95_latency_ms"):
            if field not in record:
                raise RuntimeError(
                    f"{records_path} line {line_number} is missing required field {field}"
                )
        records.append(record)
    if not records:
        raise RuntimeError(f"measurement history is empty at {records_path}")
    workloads = {
        (record.get("target_rate"), record.get("duration_sec"))
        for record in records
        if record.get("target_rate") is not None
        and record.get("duration_sec") is not None
    }
    if len(workloads) > 1:
        raise RuntimeError(
            f"measurement history in {records_path} mixes workloads: {sorted(workloads)}"
        )
    return records


def write_csv(records_path: Path, csv_path: Path) -> list[dict[str, Any]]:
    records = load_records(records_path)
    fields = [
        "iteration",
        "commit",
        "measured_at",
        "target_rate",
        "duration_sec",
        "candidate_startup_ms",
        "throughput",
        "p50_latency_ms",
        "p95_latency_ms",
        "p99_latency_ms",
        "errors",
        "stable_spans",
        "stable_logs",
        "stable_profiles",
    ]
    with csv_path.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        for record in records:
            writer.writerow({field: record.get(field) for field in fields})
    return records


def axis_bounds(values: list[float]) -> tuple[float, float]:
    low = min(values)
    high = max(values)
    if low == high:
        padding = max(abs(low) * 0.05, 1.0)
    else:
        padding = (high - low) * 0.08
    return max(0.0, low - padding), high + padding


def scaled_points(
    records: list[dict[str, Any]],
    key: str,
    left: float,
    right: float,
    top: float,
    bottom: float,
    bounds: tuple[float, float],
) -> list[tuple[float, float]]:
    values = [float(r.get(key) or 0.0) for r in records]
    low, high = bounds
    if len(values) == 1:
        x_positions = [(left + right) / 2]
    else:
        x_positions = [
            left + (index * (right - left) / (len(values) - 1))
            for index in range(len(values))
        ]
    return [
        (x, bottom - ((value - low) * (bottom - top) / (high - low)))
        for x, value in zip(x_positions, values)
    ]


def metric_label(value: float, bounds: tuple[float, float]) -> str:
    span = bounds[1] - bounds[0]
    if span < 10:
        return f"{value:.2f}"
    if span < 100:
        return f"{value:.1f}"
    if abs(value) >= 100:
        return f"{value:,.0f}"
    return f"{value:.2f}"


def build_svg(records: list[dict[str, Any]], animated: bool = False) -> str:
    width, height = 960, 480
    left, right, top, bottom = 90.0, 870.0, 105.0, 390.0
    throughput_values = [float(record.get("throughput") or 0.0) for record in records]
    latency_values = [float(record.get("p95_latency_ms") or 0.0) for record in records]
    throughput_bounds = axis_bounds(throughput_values)
    latency_bounds = axis_bounds(latency_values)
    throughput = scaled_points(
        records, "throughput", left, right, top, bottom, throughput_bounds
    )
    latency = scaled_points(
        records, "p95_latency_ms", left, right, top, bottom, latency_bounds
    )

    def poly(points: list[tuple[float, float]]) -> str:
        return " ".join(f"{x:.1f},{y:.1f}" for x, y in points)

    elements = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title description">',
        '<title id="title">Sigiro self-improvement trajectory</title>',
        '<desc id="description">Ingest throughput and p95 latency by measured iteration.</desc>',
        '<rect width="100%" height="100%" fill="#101418" rx="12"/>',
        '<text x="48" y="42" fill="#f8fafc" font-family="ui-sans-serif, system-ui" font-size="22" font-weight="600">Sigiro self-improvement trajectory</text>',
        '<text x="48" y="68" fill="#94a3b8" font-family="ui-sans-serif, system-ui" font-size="13">Comparable ingest runs over time. Failed iterations are marked with a red X.</text>',
        '<line x1="510" y1="60" x2="540" y2="60" stroke="#22c55e" stroke-width="3"/>',
        '<text x="548" y="65" fill="#22c55e" font-family="ui-sans-serif, system-ui" font-size="13">throughput (higher is better)</text>',
        '<line x1="735" y1="60" x2="765" y2="60" stroke="#f59e0b" stroke-width="3"/>',
        '<text x="773" y="65" fill="#f59e0b" font-family="ui-sans-serif, system-ui" font-size="13">p95 (lower is better)</text>',
        '<defs><clipPath id="progress-reveal"><rect id="progress-reveal-rect" x="90" y="95" width="%s" height="315"/></clipPath></defs>'
        % ("0" if animated else f"{right - left:.0f}"),
    ]

    tick_count = 4
    for tick in range(tick_count + 1):
        ratio = tick / tick_count
        y = bottom - ratio * (bottom - top)
        throughput_value = throughput_bounds[0] + ratio * (
            throughput_bounds[1] - throughput_bounds[0]
        )
        latency_value = latency_bounds[0] + ratio * (
            latency_bounds[1] - latency_bounds[0]
        )
        elements.extend(
            [
                f'<line x1="{left:.0f}" y1="{y:.1f}" x2="{right:.0f}" y2="{y:.1f}" stroke="#263241"/>',
                f'<text x="{left - 12:.0f}" y="{y + 4:.1f}" text-anchor="end" fill="#22c55e" font-family="ui-monospace, monospace" font-size="11">{metric_label(throughput_value, throughput_bounds)}</text>',
                f'<text x="{right + 12:.0f}" y="{y + 4:.1f}" fill="#f59e0b" font-family="ui-monospace, monospace" font-size="11">{metric_label(latency_value, latency_bounds)}</text>',
            ]
        )

    elements.extend(
        [
            f'<line x1="{left:.0f}" y1="{top:.0f}" x2="{left:.0f}" y2="{bottom:.0f}" stroke="#64748b"/>',
            f'<line x1="{right:.0f}" y1="{top:.0f}" x2="{right:.0f}" y2="{bottom:.0f}" stroke="#64748b"/>',
            f'<line x1="{left:.0f}" y1="{bottom:.0f}" x2="{right:.0f}" y2="{bottom:.0f}" stroke="#64748b"/>',
            '<text x="20" y="248" transform="rotate(-90 20 248)" text-anchor="middle" fill="#22c55e" font-family="ui-sans-serif, system-ui" font-size="12">throughput (spans/sec)</text>',
            '<text x="940" y="248" transform="rotate(90 940 248)" text-anchor="middle" fill="#f59e0b" font-family="ui-sans-serif, system-ui" font-size="12">p95 latency (ms)</text>',
        ]
    )

    label_step = max(1, len(records) // 8)
    for index, (record, (x, _)) in enumerate(zip(records, throughput)):
        if index % label_step != 0 and index != len(records) - 1:
            continue
        label = escape(f"#{record['iteration']} {str(record['commit'])[:7]}")
        elements.append(
            f'<text x="{x:.1f}" y="414" text-anchor="middle" fill="#94a3b8" font-family="ui-monospace, monospace" font-size="10">{label}</text>'
        )

    elements.extend(
        [
            '<g clip-path="url(#progress-reveal)">',
            f'<polyline points="{poly(throughput)}" fill="none" stroke="#22c55e" stroke-width="3" stroke-linejoin="round"/>',
            f'<polyline points="{poly(latency)}" fill="none" stroke="#f59e0b" stroke-width="3" stroke-linejoin="round"/>',
        ]
    )
    for index, (record, throughput_point, latency_point) in enumerate(
        zip(records, throughput, latency)
    ):
        throughput_x, throughput_y = throughput_point
        latency_x, latency_y = latency_point
        tooltip = escape(
            f"iteration {record['iteration']} commit {record['commit']}: "
            f"{float(record['throughput']):.2f} spans/sec, "
            f"p95 {float(record['p95_latency_ms']):.2f} ms, "
            f"errors {record.get('errors', 0)}"
        )
        elements.extend(
            [
                f'<circle data-index="{index}" cx="{throughput_x:.1f}" cy="{throughput_y:.1f}" r="5" fill="#22c55e"><title>{tooltip}</title></circle>',
                f'<circle data-index="{index}" cx="{latency_x:.1f}" cy="{latency_y:.1f}" r="5" fill="#f59e0b"><title>{tooltip}</title></circle>',
            ]
        )
        if int(record.get("errors") or 0) > 0:
            elements.extend(
                [
                    f'<line x1="{throughput_x - 7:.1f}" y1="{top:.1f}" x2="{throughput_x + 7:.1f}" y2="{top + 14:.1f}" stroke="#ef4444" stroke-width="3"/>',
                    f'<line x1="{throughput_x + 7:.1f}" y1="{top:.1f}" x2="{throughput_x - 7:.1f}" y2="{top + 14:.1f}" stroke="#ef4444" stroke-width="3"/>',
                ]
            )
    elements.extend(
        [
            "</g>",
            '<text x="480" y="455" text-anchor="middle" fill="#64748b" font-family="ui-sans-serif, system-ui" font-size="11">iteration and commit</text>',
            "</svg>",
        ]
    )
    return "\n".join(elements) + "\n"


def write_svg(records: list[dict[str, Any]], path: Path) -> None:
    path.write_text(build_svg(records))


def write_animation(records: list[dict[str, Any]], path: Path) -> None:
    payload = json.dumps(records).replace("</", "<\\/")
    svg = build_svg(records, animated=True)
    path.write_text(
        f"""<!doctype html>
<meta charset="utf-8">
<title>Sigiro self-improvement trajectory</title>
<style>
body {{ margin: 0; background: #101418; color: #f8fafc; font: 14px system-ui, sans-serif; }}
main {{ max-width: 960px; margin: 32px auto; padding: 0 20px; }}
svg {{ width: 100%; height: auto; }}
pre {{ white-space: pre-wrap; color: #cbd5e1; }}
</style>
<main>
{svg}
<pre id="caption"></pre>
</main>
<script>
const records = {payload};
const caption = document.getElementById("caption");
const reveal = document.getElementById("progress-reveal-rect");
function pointX(index) {{ return records.length <= 1 ? 480 : 90 + index * 780 / (records.length - 1); }}
let index = 0;
function draw() {{
  const record = records[index];
  reveal.setAttribute("width", Math.max(0, pointX(index) - 90 + 6));
  caption.textContent = `iteration=${{record.iteration}} commit=${{record.commit}} throughput=${{record.throughput}} spans/s p95=${{record.p95_latency_ms}}ms errors=${{record.errors}}`;
  index = (index + 1) % records.length;
}}
draw();
if (records.length > 1) setInterval(draw, 1200);
</script>
"""
    )


def write_blog(record: dict[str, Any], path: Path) -> None:
    path.write_text(
        f"""---
title: "How Sigiro Improves Itself With Sigiro"
description: Stable Sigiro watches candidate Sigiro, Codex reads the evidence, and each iteration leaves a measurable trail.
---

# How Sigiro Improves Itself With Sigiro

Sigiro's self-improvement loop uses two binaries. A stable Sigiro instance is the observer. A candidate Sigiro instance is the system under test. Candidate is configured to export its own OpenTelemetry traces and logs plus Pyroscope CPU profiles to stable while the harness drives real ingest traffic through candidate. Per-run signal deltas show which exports actually arrived.

The point is not a synthetic score. The point is a repeatable evidence loop:

1. Start stable Sigiro on isolated ports.
2. Start candidate Sigiro with `OTEL_EXPORTER_OTLP_ENDPOINT` and `SIGIRO_PYROSCOPE_ENDPOINT` pointing at stable.
3. Run the existing `scripts/bench_ingest.py` workload against candidate's OTLP HTTP endpoint.
4. Query stable with `sigiro diagnose sigiro-candidate`.
5. Save the benchmark, diagnose output, logs, CSV, SVG plot, animated HTML trajectory, and a Codex next-step prompt.
6. Codex reads the evidence, makes one small code change, validates it, and reruns the loop.
7. A candidate is promoted only when it has zero benchmark errors and improves or preserves the chosen guardrail metrics.

Latest measured iteration:

- Iteration: `{record.get("iteration")}`
- Commit: `{record.get("commit")}`
- Candidate startup: `{record.get("candidate_startup_ms")} ms`
- Throughput: `{record.get("throughput")} spans/sec`
- p95 ingest latency: `{record.get("p95_latency_ms")} ms`
- Benchmark errors: `{record.get("errors")}`
- Stable observed spans: `{record.get("stable_spans")}`
- Stable observed log templates: `{record.get("stable_logs")}`
- Stable observed profile samples: `{record.get("stable_profiles")}`

The loop deliberately keeps one obvious path: measure, inspect, change, validate, promote. No hidden controller rewrites code on its own. Codex remains the decision point, and Sigiro remains the source of operational truth.
"""
    )


def git_commit() -> str:
    return subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], text=True).strip()


def main() -> int:
    parser = argparse.ArgumentParser(description="Run one Sigiro self-improvement loop iteration")
    parser.add_argument("--stable-bin", type=Path, default=Path("target/release/sigiro"))
    parser.add_argument("--candidate-bin", type=Path, default=Path("target/debug/sigiro"))
    parser.add_argument(
        "--candidate-commit",
        help="Commit represented by candidate-bin (defaults to the current checkout)",
    )
    parser.add_argument("--out-dir", type=Path, default=Path("target/self-improve"))
    parser.add_argument("--iteration", type=int, help="Iteration number (defaults to the next recorded value)")
    parser.add_argument(
        "--plot-only",
        action="store_true",
        help="Regenerate CSV, SVG, and HTML from iterations.jsonl without starting Sigiro",
    )
    parser.add_argument("--api-key", default="sigiro_dev_self_improve")
    parser.add_argument("--timeout-sec", type=int, default=90)
    parser.add_argument("--rate", type=int, default=500)
    parser.add_argument("--duration", type=int, default=10)
    parser.add_argument("--profile-timeout-sec", type=int, default=25)
    parser.add_argument(
        "--extension-dir",
        type=Path,
        default=Path.home() / ".duckdb" / "extensions",
    )
    parser.add_argument("--stable-http", type=int, default=19999)
    parser.add_argument("--stable-otlp-http", type=int, default=14318)
    parser.add_argument("--stable-otlp-grpc", type=int, default=14317)
    parser.add_argument("--candidate-http", type=int, default=29999)
    parser.add_argument("--candidate-otlp-http", type=int, default=24318)
    parser.add_argument("--candidate-otlp-grpc", type=int, default=24317)
    parser.add_argument("--promote-to", type=Path, help="Copy candidate binary here when the run has zero benchmark errors")
    args = parser.parse_args()

    stable_ports = Ports(args.stable_http, args.stable_otlp_http, args.stable_otlp_grpc)
    candidate_ports = Ports(args.candidate_http, args.candidate_otlp_http, args.candidate_otlp_grpc)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    records_path = args.out_dir / "iterations.jsonl"
    if args.plot_only:
        records = write_csv(records_path, args.out_dir / "iterations.csv")
        write_svg(records, args.out_dir / "progress.svg")
        write_animation(records, args.out_dir / "progress.html")
        print(
            json.dumps(
                {
                    "iterations": len(records),
                    "csv": str(args.out_dir / "iterations.csv"),
                    "svg": str(args.out_dir / "progress.svg"),
                    "html": str(args.out_dir / "progress.html"),
                },
                indent=2,
            )
        )
        return 0

    existing_records = load_records(records_path) if records_path.exists() else []
    iteration = (
        args.iteration
        if args.iteration is not None
        else max((int(record["iteration"]) for record in existing_records), default=0) + 1
    )
    if iteration < 1:
        raise RuntimeError("iteration must be greater than zero")
    if any(int(record["iteration"]) == iteration for record in existing_records):
        raise RuntimeError(f"iteration {iteration} already exists in {records_path}")
    existing_workloads = {
        (record.get("target_rate"), record.get("duration_sec"))
        for record in existing_records
        if record.get("target_rate") is not None
        and record.get("duration_sec") is not None
    }
    if existing_workloads and existing_workloads != {(args.rate, args.duration)}:
        raise RuntimeError(
            f"existing history uses workload {sorted(existing_workloads)}; "
            f"requested rate={args.rate}, duration={args.duration}"
        )

    extension_dir = args.extension_dir.expanduser().resolve()
    extension_dir.mkdir(parents=True, exist_ok=True)

    stable_proc: subprocess.Popen[str] | None = None
    candidate_proc: subprocess.Popen[str] | None = None
    try:
        stable_proc = start_sigiro(
            args.stable_bin,
            args.out_dir / "stable-data",
            stable_ports,
            args.api_key,
            {"SIGIRO_EXTENSION_DIR": str(extension_dir)},
            args.out_dir / "stable.log",
        )
        stable_startup, _ = wait_health(f"http://127.0.0.1:{stable_ports.http}", args.timeout_sec)
        env = os.environ.copy()
        env["SIGIRO_ENDPOINT"] = f"http://127.0.0.1:{stable_ports.http}"
        env["SIGIRO_API_KEY"] = args.api_key
        stable_url = f"http://127.0.0.1:{stable_ports.http}"
        counts_before_result = query_service_counts(stable_url, args.api_key)
        counts_before = counts_before_result[0] if counts_before_result else {}

        candidate_proc = start_sigiro(
            args.candidate_bin,
            args.out_dir / f"candidate-data-{iteration}",
            candidate_ports,
            args.api_key,
            {
                "OTEL_SERVICE_NAME": "sigiro-candidate",
                "OTEL_EXPORTER_OTLP_ENDPOINT": f"http://127.0.0.1:{stable_ports.otlp_http}",
                "SIGIRO_PYROSCOPE_ENDPOINT": f"http://127.0.0.1:{stable_ports.otlp_http}",
                "SIGIRO_OTEL_AUTH_KEY": args.api_key,
                "SIGIRO_EXTENSION_DIR": str(extension_dir),
            },
            args.out_dir / "candidate.log",
        )
        candidate_startup, _ = wait_health(f"http://127.0.0.1:{candidate_ports.http}", args.timeout_sec)

        bench_cmd = [
            sys.executable,
            str(Path(__file__).with_name("bench_ingest.py")),
            "--endpoint",
            f"http://127.0.0.1:{candidate_ports.otlp_http}",
            "--auth-key",
            args.api_key,
            "--rate",
            str(args.rate),
            "--duration",
            str(args.duration),
        ]
        bench = subprocess.run(bench_cmd, cwd=args.out_dir, text=True, capture_output=True)
        (args.out_dir / "bench.stdout").write_text(bench.stdout)
        (args.out_dir / "bench.stderr").write_text(bench.stderr)
        if bench.returncode != 0:
            raise RuntimeError(f"bench failed with exit {bench.returncode}; see {args.out_dir / 'bench.stderr'}")
        bench_result = newest_bench_result(args.out_dir)

        wait_for_profiles(
            stable_url,
            args.api_key,
            args.profile_timeout_sec,
            int(counts_before.get("profiles_all_time", 0) or 0),
        )

        stop_process(candidate_proc)
        candidate_proc = None
        time.sleep(2)
        diagnose = run_json([str(args.stable_bin), "--output", "json", "diagnose", "sigiro-candidate"], env)
        (args.out_dir / "diagnose-sigiro-candidate.json").write_text(json.dumps(diagnose, indent=2) + "\n")
        service_counts = query_service_counts(stable_url, args.api_key)
        counts = service_counts[0] if service_counts else {}
        (args.out_dir / "service-counts.json").write_text(
            json.dumps({"before": counts_before, "after": counts}, indent=2) + "\n"
        )

        def signal_delta(field: str) -> int:
            before = int(counts_before.get(field, 0) or 0)
            after = int(counts.get(field, 0) or 0)
            if after < before:
                raise RuntimeError(
                    f"stable observer count for {field} decreased from {before} to {after}"
                )
            return after - before

        results = bench_result["results"]
        benchmark_config = bench_result["config"]
        record = {
            "iteration": iteration,
            "commit": args.candidate_commit or git_commit(),
            "measured_at": datetime.now(timezone.utc).isoformat(),
            "target_rate": benchmark_config["rate"],
            "duration_sec": benchmark_config["duration"],
            "stable_startup_ms": round(stable_startup * 1000, 2),
            "candidate_startup_ms": round(candidate_startup * 1000, 2),
            "throughput": round(results["throughput"], 2),
            "p50_latency_ms": round(results["p50_latency_ms"], 2),
            "p95_latency_ms": round(results["p95_latency_ms"], 2),
            "p99_latency_ms": round(results["p99_latency_ms"], 2),
            "errors": results["errors"],
            "stable_spans": signal_delta("spans_all_time"),
            "stable_logs": signal_delta("logs_all_time"),
            "stable_profiles": signal_delta("profiles_all_time"),
            "latest_log_ts": counts.get("latest_log_ts"),
            "latest_profile_ts": counts.get("latest_profile_ts"),
        }
        with records_path.open("a") as f:
            f.write(json.dumps(record, sort_keys=True) + "\n")
        records = write_csv(records_path, args.out_dir / "iterations.csv")
        write_svg(records, args.out_dir / "progress.svg")
        write_animation(records, args.out_dir / "progress.html")
        write_blog(record, args.out_dir / "blog-post.md")
        (args.out_dir / "codex-next-step.md").write_text(
            "Use the stable Sigiro evidence in diagnose-sigiro-candidate.json and iterations.csv. "
            "Choose one optimization, make the smallest code change, run the same harness again, "
            "and promote only when errors remain zero and guardrail metrics do not regress.\n"
        )

        if args.promote_to and record["errors"] == 0:
            args.promote_to.parent.mkdir(parents=True, exist_ok=True)
            if args.promote_to.exists():
                backup = args.promote_to.with_suffix(args.promote_to.suffix + ".bak")
                shutil.copy2(args.promote_to, backup)
            shutil.copy2(args.candidate_bin, args.promote_to)

        print(json.dumps(record, indent=2))
        return 0
    finally:
        stop_process(candidate_proc)
        stop_process(stable_proc)


if __name__ == "__main__":
    raise SystemExit(main())
