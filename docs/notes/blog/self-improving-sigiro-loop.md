---
title: "Sigiro self-improvement experiment"
description: Historical measurements from the retired Python ingest and self-improvement harness.
---

# Sigiro self-improvement experiment

In June 2026, a prototype ran a candidate Sigiro under ingest traffic while a separate Sigiro observed its telemetry. The custom Python harness and HTTP ingest benchmark have since been retired. This page preserves the recorded results, not a runnable workflow or a production regression gate.

## Recorded trajectory

![Historical Sigiro ingest throughput and p95 latency](/self-improvement/progress.svg)

| Iteration | Commit | Reported throughput | Reported p95 latency | Observer profiles | Reported errors |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | `c114ffd` | 509.46 spans/sec | 2.72 ms | 19 | 0 |
| 2 | `02cbdfa` | 509.47 spans/sec | 3.74 ms | 11 | 0 |
| 3 | `2b800f7` | 509.43 spans/sec | 3.13 ms | 20 | 0 |

The run was rate-limited to 500 spans/sec, so the flat throughput line does not demonstrate increased capacity. All three rows recorded **zero observer spans**. The retired harness counted attempted spans in throughput even when HTTP requests failed; treat these numbers as historical observations, not verified end-to-end ingest performance. The original measurements remain available as [CSV](/self-improvement/iterations.csv).

For current **in-process** diagnosis measurements, run `mise run bench`. This Criterion benchmark does not measure HTTP/OTLP ingest or reproduce the historical workload; a new external load benchmark needs its own validated workload and success criteria.
