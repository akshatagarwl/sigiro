#!/usr/bin/env python3
"""
Run one Sigiro self-improvement measurement iteration.

Stable Sigiro receives self-telemetry from candidate Sigiro. The harness then
benchmarks candidate ingest, asks stable Sigiro to investigate the candidate,
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
from pathlib import Path
from typing import Any


SERVICE_COUNTS_SQL = (
    "SELECT "
    "(SELECT count(*) FROM lake.sigiro_spans WHERE service_name = 'sigiro-candidate') AS spans_all_time, "
    "(SELECT count(*) FROM lake.sigiro_logs WHERE service_name = 'sigiro-candidate') AS logs_all_time, "
    "(SELECT count(*) FROM lake.sigiro_profiles WHERE service_name = 'sigiro-candidate') AS profiles_all_time, "
    "CAST((SELECT min(timestamp) FROM lake.sigiro_logs WHERE service_name = 'sigiro-candidate') AS VARCHAR) AS first_log_ts, "
    "CAST((SELECT min(timestamp) FROM lake.sigiro_profiles WHERE service_name = 'sigiro-candidate') AS VARCHAR) AS first_profile_ts"
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


def query_service_counts(stable_bin: Path, env: dict[str, str]) -> list[dict[str, Any]]:
    return run_json(
        [str(stable_bin), "--output", "json", "query", SERVICE_COUNTS_SQL],
        env,
    )


def wait_for_profiles(
    stable_bin: Path,
    env: dict[str, str],
    timeout_sec: int,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout_sec
    last_counts: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        last_counts = query_service_counts(stable_bin, env)
        if last_counts and last_counts[0].get("profiles_all_time", 0) > 0:
            return last_counts
        time.sleep(1)
    raise RuntimeError(
        "candidate produced no Pyroscope profiles before timeout; "
        "inspect candidate.log and stable.log"
    )


def write_csv(records_path: Path, csv_path: Path) -> list[dict[str, Any]]:
    records = [json.loads(line) for line in records_path.read_text().splitlines() if line.strip()]
    fields = [
        "iteration",
        "commit",
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


def scaled_points(records: list[dict[str, Any]], key: str, width: int, height: int) -> list[tuple[float, float]]:
    values = [float(r.get(key) or 0.0) for r in records]
    if not values:
        return []
    lo = min(values)
    hi = max(values)
    span = hi - lo or 1.0
    if len(values) == 1:
        return [(width / 2, height / 2)]
    return [
        (20 + (i * (width - 40) / (len(values) - 1)), height - 20 - ((v - lo) * (height - 40) / span))
        for i, v in enumerate(values)
    ]


def write_svg(records: list[dict[str, Any]], path: Path) -> None:
    width, height = 720, 320
    throughput = scaled_points(records, "throughput", width, height)
    latency = scaled_points(records, "p95_latency_ms", width, height)
    def poly(points: list[tuple[float, float]]) -> str:
        return " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
    path.write_text(
        "\n".join(
            [
                f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
                '<rect width="100%" height="100%" fill="#101418"/>',
                '<text x="20" y="32" fill="#f8fafc" font-family="ui-sans-serif, system-ui" font-size="20">Sigiro self-improvement trajectory</text>',
                '<text x="20" y="58" fill="#94a3b8" font-family="ui-sans-serif, system-ui" font-size="13">green = throughput, amber = p95 latency</text>',
                '<line x1="20" y1="300" x2="700" y2="300" stroke="#334155"/>',
                '<line x1="20" y1="20" x2="20" y2="300" stroke="#334155"/>',
                f'<polyline points="{poly(throughput)}" fill="none" stroke="#22c55e" stroke-width="3"/>',
                f'<polyline points="{poly(latency)}" fill="none" stroke="#f59e0b" stroke-width="3"/>',
                *[
                    f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4" fill="#22c55e"/>'
                    for x, y in throughput
                ],
                *[
                    f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4" fill="#f59e0b"/>'
                    for x, y in latency
                ],
                "</svg>",
            ]
        )
        + "\n"
    )


def write_animation(records: list[dict[str, Any]], path: Path) -> None:
    payload = json.dumps(records, indent=2)
    path.write_text(
        f"""<!doctype html>
<meta charset="utf-8">
<title>Sigiro self-improvement trajectory</title>
<style>
body {{ margin: 0; background: #101418; color: #f8fafc; font: 14px system-ui, sans-serif; }}
main {{ max-width: 880px; margin: 32px auto; padding: 0 20px; }}
canvas {{ width: 100%; height: 360px; background: #0b1117; border: 1px solid #263241; }}
pre {{ white-space: pre-wrap; color: #cbd5e1; }}
</style>
<main>
<h1>Sigiro self-improvement trajectory</h1>
<canvas id="chart" width="880" height="360"></canvas>
<pre id="caption"></pre>
</main>
<script>
const records = {payload};
const canvas = document.getElementById("chart");
const ctx = canvas.getContext("2d");
const caption = document.getElementById("caption");
function value(key, i) {{ return Number(records[i]?.[key] || 0); }}
function y(key, i) {{
  const vals = records.map((_, n) => value(key, n));
  const min = Math.min(...vals), max = Math.max(...vals), span = max - min || 1;
  return 320 - ((value(key, i) - min) / span) * 260;
}}
function x(i) {{ return records.length <= 1 ? 440 : 50 + i * 780 / (records.length - 1); }}
let frame = 0;
function draw() {{
  const upto = Math.min(records.length - 1, Math.floor(frame / 20));
  ctx.clearRect(0, 0, 880, 360);
  ctx.strokeStyle = "#334155"; ctx.beginPath(); ctx.moveTo(50, 20); ctx.lineTo(50, 320); ctx.lineTo(840, 320); ctx.stroke();
  for (const [key, color] of [["throughput", "#22c55e"], ["p95_latency_ms", "#f59e0b"]]) {{
    ctx.strokeStyle = color; ctx.lineWidth = 3; ctx.beginPath();
    for (let i = 0; i <= upto; i++) {{ i === 0 ? ctx.moveTo(x(i), y(key, i)) : ctx.lineTo(x(i), y(key, i)); }}
    ctx.stroke();
  }}
  const r = records[upto] || {{}};
  caption.textContent = `iteration=${{r.iteration}} commit=${{r.commit}} throughput=${{r.throughput}} spans/s p95=${{r.p95_latency_ms}}ms errors=${{r.errors}}`;
  frame = (frame + 1) % Math.max(1, records.length * 20);
  requestAnimationFrame(draw);
}}
draw();
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

Sigiro's self-improvement loop uses two binaries. A stable Sigiro instance is the observer. A candidate Sigiro instance is the system under test. Candidate exports its own OpenTelemetry traces, logs, and Pyroscope CPU profiles to stable while the harness drives real ingest traffic through candidate.

The point is not a synthetic score. The point is a repeatable evidence loop:

1. Start stable Sigiro on isolated ports.
2. Start candidate Sigiro with `OTEL_EXPORTER_OTLP_ENDPOINT` and `SIGIRO_PYROSCOPE_ENDPOINT` pointing at stable.
3. Run the existing `scripts/bench_ingest.py` workload against candidate's OTLP HTTP endpoint.
4. Query stable with `sigiro investigate sigiro-candidate`.
5. Save the benchmark, investigate output, logs, CSV, SVG plot, animated HTML trajectory, and a Codex next-step prompt.
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
    parser.add_argument("--out-dir", type=Path, default=Path("target/self-improve"))
    parser.add_argument("--iteration", type=int, default=1)
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

        candidate_proc = start_sigiro(
            args.candidate_bin,
            args.out_dir / "candidate-data",
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

        env = os.environ.copy()
        env["SIGIRO_ENDPOINT"] = f"http://127.0.0.1:{stable_ports.http}"
        env["SIGIRO_API_KEY"] = args.api_key
        wait_for_profiles(args.stable_bin, env, args.profile_timeout_sec)

        stop_process(candidate_proc)
        candidate_proc = None
        time.sleep(2)
        investigate = run_json([str(args.stable_bin), "--output", "json", "investigate", "sigiro-candidate"], env)
        (args.out_dir / "investigate-sigiro-candidate.json").write_text(json.dumps(investigate, indent=2) + "\n")
        service_counts = query_service_counts(args.stable_bin, env)
        (args.out_dir / "service-counts.json").write_text(json.dumps(service_counts, indent=2) + "\n")
        counts = service_counts[0] if service_counts else {}

        results = bench_result["results"]
        record = {
            "iteration": args.iteration,
            "commit": git_commit(),
            "stable_startup_ms": round(stable_startup * 1000, 2),
            "candidate_startup_ms": round(candidate_startup * 1000, 2),
            "throughput": round(results["throughput"], 2),
            "p50_latency_ms": round(results["p50_latency_ms"], 2),
            "p95_latency_ms": round(results["p95_latency_ms"], 2),
            "p99_latency_ms": round(results["p99_latency_ms"], 2),
            "errors": results["errors"],
            "stable_spans": counts.get("spans_all_time", 0),
            "stable_logs": counts.get("logs_all_time", 0),
            "stable_profiles": counts.get("profiles_all_time", 0),
            "first_log_ts": counts.get("first_log_ts"),
            "first_profile_ts": counts.get("first_profile_ts"),
        }
        records_path = args.out_dir / "iterations.jsonl"
        with records_path.open("a") as f:
            f.write(json.dumps(record, sort_keys=True) + "\n")
        records = write_csv(records_path, args.out_dir / "iterations.csv")
        write_svg(records, args.out_dir / "progress.svg")
        write_animation(records, args.out_dir / "progress.html")
        write_blog(record, args.out_dir / "blog-post.md")
        (args.out_dir / "codex-next-step.md").write_text(
            "Use the stable Sigiro evidence in investigate-sigiro-candidate.json and iterations.csv. "
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
