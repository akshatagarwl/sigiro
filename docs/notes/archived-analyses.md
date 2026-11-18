---
title: Archived Analyses
created: 2026-04-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'deployment', 'duckdb']
---

# Archived Analyses

Historical analysis and [[architecture/investigation.md|investigation]] files preserved in git history, removed from working tree.

## predict/ — Architecture Decision Analysis (2026-03-20)
**Commit:** `0db10c2`
**Retrieve:** `git show 0db10c2:predict/`

Multi-persona AI architecture debates before writing any Rust code:

| Analysis | Personas | Key Outcome |
|----------|----------|-------------|
| sigiro-arch-limits | 5, 2 rounds | 11 findings (3 HIGH) on DuckDB extension approach — !Send/!Sync, schema projection, CMake overhead |
| **optimal-arch** | 8, 3 rounds, unanimous | **Pivotal decision: single Rust binary embedding DuckDB, not an extension** |
| sigiro-post-mvp | 8, 3 rounds | Post-MVP roadmap: cost, zero-config, zero-copy, query perf |
| zero-code | prototype | Docker Compose + SQL views approach — evaluated and rejected |

## investigations/ — Pipeline Validation Logs (2026-03-05/06)
**Commit:** `7a25c63`
**Retrieve:** `git show 7a25c63:investigations/`

Pre-Rust-rewrite validation of OTel collector pipelines on [[roadmap.md|Docker]]/Lima k3s:

- E2E validation with OTel demo v2.2.0 (14 services)
- [[designs/future/ebpf-gpu-position.md|eBPF]] profile pipeline fixes
- OBI + Sock Shop auto-instrumentation proof on Lima k3s
- Lima k3s [[designs/future/ebpf-gpu-position.md|eBPF]] profiling (Apple M4 VZ + Rosetta)
- Service fixes and post-fix validation runs
- Sample attribute service name mapping
