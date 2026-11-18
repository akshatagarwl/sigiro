---
title: eBPF and GPU Observability — Position
created: 2026-03-30
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'decision', 'design', 'ebpf', 'migration']
---

# eBPF and GPU Observability — Position

## eBPF

Decision (8/8 consensus): Don't add privileged requirement to sigiro.

eBPF needs kernel access (CAP_BPF, CAP_SYS_ADMIN). Baking into Sigiro
means the binary needs root. Breaks the "curl | sh, just works" story.

eBPF tools that emit [[architecture/write-path.md|OTLP]] already exist and just point at Sigiro:

| Tool                        | What                          | OTLP? |
|-----------------------------|-------------------------------|-------|
| Grafana Beyla               | Auto-instrument HTTP/gRPC/SQL | Yes   |
| OpenTelemetry eBPF profiler | Continuous profiling           | Yes   |
| Odigos                      | Auto-instrument any process    | Yes   |
| Coroot                      | Network + process tracing      | Yes   |

Architecture: [eBPF agent] ──[[architecture/write-path.md|OTLP]]──→ [Sigiro :4317]

Sigiro doesn't care how telemetry was generated. eBPF, SDK, Collector — all [[architecture/write-path.md|OTLP]].

Framing: "Works with eBPF auto-instrumentation (Beyla, Odigos) — just point them at Sigiro."
[[research/groundcover-analysis.md|Groundcover]] forces their eBPF agent. Sigiro accepts any. Strength, not gap.

## GPU Observability

No zero-config [[architecture/write-path.md|OTLP]] path exists for GPU data today.

| Tool            | Protocol     | Hops to Sigiro                        |
|-----------------|-------------|---------------------------------------|
| DCGM-exporter   | Prometheus  | Need OTel Collector to convert → OTLP |
| Nsight Systems  | Proprietary | No OTLP. No export path.              |
| nvidia-smi      | CLI stdout  | Would need custom script              |

Sigiro's ICP doesn't have GPUs (Fly/Railway/Vercel). They call GPU-hosted APIs
(OpenAI, Anthropic) — that's GenAI span tracing via gen_ai.* attributes, not GPU profiling.

If a user self-hosts inference (vLLM, llama.cpp), they're sophisticated enough to
set up DCGM + OTel Collector themselves. That's a docs page, not a product feature.

## Action Items
- Document eBPF tool → Sigiro setup (Beyla example) — docs task
- Document GenAI SDK instrumentation setup — docs task
- Don't build eBPF or GPU-specific code into Sigiro
