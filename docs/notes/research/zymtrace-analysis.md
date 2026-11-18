---
title: ZymTrace Competitive Analysis
created: 2026-03-30
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'competitor', 'ebpf', 'investigation', 'market']
---

# ZymTrace Competitive Analysis
> Analyzed: 2026-03-27

## What They Are
eBPF continuous profiler for CPU + GPU. $12.2M raised (Mar 2026).
Self-hosted only. Backend in Rust + React/WASM frontend.
Founded by the team that built and donated the OTel eBPF profiler.

## How It Works
1. DaemonSet (K8s) or standalone binary on each node
2. eBPF unwinders in kernel → collects CPU stack traces
3. CUDA injection library (CUDA_INJECTION64_PATH) → hooks kernel launches, samples GPU SASS instructions + stall reasons
4. Correlates CPU dispatch paths with CUDA kernels ("GPU idle because CPU still computing X")
5. Sends [[architecture/profiles.md|profiles]] via gRPC to their backend (gateway → storage)
6. "Efficiency IQ" — agentic analysis that tells you what to fix
7. [[architecture/write-path.md|OpenTelemetry]] compliant (OTel resource attributes)
8. Has MCP server + AI assistant integration

## Differentiator
Disassembles CUDA SASS and shows stall reasons (memory latency, math pipeline oversubscribed). Correlates back to CPU stack that launched the kernel. Nobody else does this at this granularity with zero instrumentation.

## Relevance to Sigiro
None. Different ICP entirely.
- ZymTrace: GPU/CPU profiler for ML teams with GPU clusters
- Sigiro: App-level observability for indie devs on PaaS (no GPUs)
- Not a competitor, not a plug-in
- Their users run models. Sigiro's users call model APIs.

## What to Steal
- "Efficiency IQ" concept: agentic analysis that tells you what to fix, not just flamegraphs. Same philosophy as [[architecture/investigation.md|InvestigationBlock]].
- MCP server integration pattern for AI assistant tooling.
