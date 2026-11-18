---
title: "How Sigiro Improves Itself With Sigiro"
description: Stable Sigiro watches candidate Sigiro while Codex makes one measured improvement at a time.
---

# How Sigiro Improves Itself With Sigiro

Sigiro should be able to improve itself without pretending that a benchmark is a production incident. The loop is simple: keep one stable Sigiro as the observer, run a candidate Sigiro as the system under test, and make every code change answer to telemetry from the stable observer.

## The Loop

1. Build or choose a stable Sigiro binary.
2. Build a candidate Sigiro binary from the current branch.
3. Start stable on isolated HTTP, OTLP HTTP, and OTLP gRPC ports.
4. Start candidate on different ports with `OTEL_EXPORTER_OTLP_ENDPOINT` and `SIGIRO_PYROSCOPE_ENDPOINT` pointing to stable.
5. Run real ingest traffic against candidate with `scripts/bench_ingest.py`.
6. Wait for stable to persist at least one candidate Pyroscope profile, then ask stable to investigate `sigiro-candidate`.
7. Save the benchmark result, stable investigation output, logs, CSV, SVG plot, animated HTML trajectory, and a Codex next-step prompt.
8. Codex reads the evidence, chooses one optimization, changes the code, validates it, and runs the loop again.
9. Promote the candidate only when errors stay at zero and guardrail metrics do not regress.

## Running One Iteration

```bash
cargo build --release --bin sigiro
cargo build --bin sigiro
python scripts/self_improve_loop.py \
  --stable-bin target/release/sigiro \
  --candidate-bin target/debug/sigiro \
  --out-dir target/self-improve \
  --rate 500 \
  --duration 10
```

The harness assigns the next iteration number from `iterations.jsonl`. Use
`--iteration` only when importing or repairing an explicit sequence; duplicate
and non-positive iteration numbers fail before Sigiro starts. When measuring a
binary built from another checkout, pass `--candidate-commit` so the plot keeps
the correct provenance.

The harness writes:

- `iterations.jsonl` and `iterations.csv`
- `investigate-sigiro-candidate.json`
- `stable.log` and `candidate.log`
- `progress.svg`
- `progress.html`
- `codex-next-step.md`
- `blog-post.md`

Each iteration records per-run `stable_spans`, `stable_logs`, and
`stable_profiles` deltas plus `latest_profile_ts`. The loop snapshots stable's
profile count before candidate startup and fails unless that count increases
before `--profile-timeout-sec`; historical profiles cannot silently pass a later
candidate. Stable and candidate reuse `~/.duckdb/extensions` by default so
isolated data directories do not redownload DuckDB extensions on every run.
Each iteration gets a fresh candidate data directory so later measurements do
not inherit an initialized DuckLake project from earlier candidates.

Regenerate the CSV and plots without starting either Sigiro instance:

```bash
python scripts/self_improve_loop.py \
  --out-dir target/self-improve \
  --plot-only
```

## Why Two Sigiro Instances

A candidate under load should not be the source of truth about its own health.
The candidate is configured to export spans, logs, and Pyroscope CPU profiles
while it handles ingest. Stable records per-run deltas for each signal, so a
missing export path remains visible instead of being hidden by older data. If
candidate stalls, crashes, or slows down, stable still has the trail it received.

That separation is also the guardrail against overfitting. Codex does not optimize against a hand-rolled timer hidden in the candidate. It optimizes against the observer's view plus the same ingest benchmark used across iterations.

## Promotion Rule

The first promotion rule is deliberately boring:

- benchmark errors must be `0`;
- startup time must not regress materially;
- p95 ingest latency must not regress materially;
- throughput must improve or stay within noise;
- release tests still pass.

That rule keeps the loop honest. Faster but flaky is not a promotion. Cleaner but slower needs an explicit reason. A large rewrite without a metric win waits.

## What Changes Over Time

Each run appends one row to `iterations.csv`, including its UTC measurement time
and benchmark rate and duration. A history cannot mix workloads because that
would make its trajectory misleading; use another output directory when the
rate or duration changes. `progress.svg` plots labeled throughput and p95 axes,
and `progress.html` animates those same points. The generated `blog-post.md` is a
per-run report; this page is the durable explanation of the loop.

The loop is intentionally not fully autonomous code mutation. Codex remains the decision maker. Sigiro supplies evidence. The human can inspect every commit. That is the boring shape that can become reliable.

## Current Trajectory

![Sigiro self-improvement throughput and p95 latency trajectory](/self-improvement/progress.svg)

| Iteration | Commit | Throughput | p95 latency | Fresh profiles | Errors |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | `c114ffd` | 509.46 spans/sec | 2.72 ms | 19 | 0 |
| 2 | `02cbdfa` | 509.47 spans/sec | 3.74 ms | 11 | 0 |
| 3 | `2b800f7` | 509.43 spans/sec | 3.13 ms | 20 | 0 |

The workload is rate-limited to 500 spans/sec, so the flat throughput line is
expected and should not be presented as an optimization. Iteration 3 improves
p95 latency by 16% relative to iteration 2 and, more importantly, removes the
cancelled final checkpoint and post-shutdown telemetry warnings found by stable
Sigiro. The first iteration still has the lowest p95, so the performance result
is a recovery rather than a new global best. The raw measurements are available
as [CSV](/self-improvement/iterations.csv).
