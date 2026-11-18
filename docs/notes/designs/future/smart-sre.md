---
title: Smart SRE — Sigiro as the Agent's Full Debugging Toolkit
created: 2026-03-31
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'deployment', 'design']
---

# Smart SRE — Sigiro as the Agent's Full Debugging Toolkit

## Principle

Sigiro is not just a telemetry store. It's everything an AI agent needs
to debug production incidents autonomously — from HTTP spans down to
kernel syscalls. One install, one agent, full-stack debugging.

The agent IS the SRE. Sigiro is its toolkit.

## The Toolkit

| Capability | Tool | What the agent gets | Interface |
|------------|------|---------------------|-----------|
| App telemetry | Sigiro core | Traces, logs, metrics, InvestigationBlock | MCP / CLI / HTTP |
| OS state | osquery | 200+ SQL tables: processes, network, files, docker, packages, users | SQL via MCP |
| Live kernel events | bpfquery | Streaming kernel probes as SQL: TCP, I/O, scheduler, syscalls | SQL → bpftrace |
| Raw kernel probes | MCPtrace | Custom bpftrace programs for edge cases | MCP tools |
| Platform context | Platform APIs | Deploys, machine events, web vitals, infra metrics | HTTP (agents.txt) |
| DB insights | DB platform APIs | Query stats, slow queries, locks, connections | HTTP / SQL (agents.txt) |
| Profiles | Sigiro (OTLP Profiles) | CPU/memory flamegraphs, hot functions | MCP / HTTP |

## Agent Investigation Flow

```
Incident: "checkout is slow"

Agent autonomously:

1. sigiro diagnose checkout
   → InvestigationBlock: p95 spiked at 14:32, 3 error spans on db.query
   → Root cause obvious? → Done.

2. If not: check platform context
   → Fly API: any recent deploys? machine restarts? memory spikes?
   → PlanetScale: query insights, slow queries, QPS change?
   → Root cause found? → Done.

3. If not: check OS state
   → osquery: SELECT * FROM processes WHERE name = 'sigiro'
   → osquery: SELECT * FROM listening_ports WHERE port = 5432
   → osquery: SELECT used, free FROM memory_info
   → Something abnormal? → Done.

4. If not: live kernel tracing
   → bpfquery: SELECT comm, latency FROM tracepoint.tcp.tcp_retransmit_skb
   → bpfquery: SELECT pid, latency FROM tracepoint.block.block_rq_complete
   → MCPtrace: custom probe for edge case
   → "TCP retransmits to DB every 200ms — network congestion"
   → Root cause at kernel level. Done.

5. Agent reports: "Checkout latency caused by TCP retransmits between
   app (10.0.0.3) and database (10.0.0.5). Not a code issue. Likely
   internal network congestion. Recommend: check Fly internal routing,
   try direct IP instead of DNS."
```

## Install

```bash
curl -sSf https://sigiro.com/install | sh
# Installs:
# - sigiro (telemetry store + query + investigate)
# - Adds agent-facing CLI/HTTP examples
# - Optional: osquery, bpftrace-mcp-server (Linux only, prompted)
# - agents.txt with full debugging playbook
```

## Agent Configuration

```bash
sigiro diagnose checkout
sigiro query "SELECT * FROM sigiro_spans WHERE service_name = 'checkout' LIMIT 20"
```

## What Makes This Unique

No other observability tool gives an AI agent this full stack:

| Tool | App telemetry | OS state | Kernel tracing | Agent-native |
|------|:---:|:---:|:---:|:---:|
| Datadog | ✓ | ✗ | ✗ | ✗ (dashboard) |
| Groundcover | ✓ | ✗ | eBPF (passive) | ✗ (dashboard) |
| New Relic | ✓ | partial | ✗ | ✗ (dashboard) |
| Grafana | ✓ | ✗ | ✗ | ✗ (dashboard) |
| **Sigiro** | **✓** | **✓ (osquery)** | **✓ (bpfquery/MCPtrace)** | **✓ (CLI/HTTP-native)** |

Everyone else builds dashboards for humans. Sigiro builds a toolkit for agents.

## bpfquery vs MCPtrace

Both give the agent kernel access. Different interfaces:

| | bpfquery | MCPtrace |
|--|----------|----------|
| Interface | SQL | MCP tools (4) |
| LLM friendliness | HIGH — LLMs write SQL natively | MEDIUM — LLM writes bpftrace scripts |
| Probe access | SQL → compiled to bpftrace | Raw bpftrace programs |
| Streaming | Yes (live events as rows) | Poll-based (exec → get_result) |
| Maturity | Experimental (~40 stars) | Early (~65 stars) |
| MCP integration | None (needs wrapper) | Native MCP server |

Ideal: bpfquery for common patterns (LLM writes SQL), MCPtrace for
escape hatch (custom probes when SQL can't express it).

Future: build bpfquery MCP wrapper so agent uses SQL for kernel queries.

## Fleet-Wide Debugging (multi-machine)

### The problem
Parent agent (Claude Code) wants to run osquery or [[designs/future/ebpf-gpu-position.md|eBPF]] probes across
ALL machines in a fleet from one MCP call.

### Fan-out (on-demand queries)
Central sigiro acts as fleet controller. Agent calls one MCP tool,
central fans out to all edge sigiros, aggregates results.

```
Agent: sigiro_fleet_osquery(sql="SELECT * FROM processes WHERE cpu > 80")
Central: fans out to Machine A, B, C
Returns: [Machine B: node at 91% CPU]
```

MCP tools:
- `sigiro_fleet_osquery(sql)` — run osquery on all machines, return aggregated
- `sigiro_fleet_trace(probe, duration)` — run [[designs/future/ebpf-gpu-position.md|eBPF]] probe on all machines
- `sigiro_fleet_status()` — which machines are connected

Uses osquery's built-in distributed query protocol (TLS API) for osquery.
Uses sigiro edge → central gRPC for [[designs/future/ebpf-gpu-position.md|eBPF]] probe fan-out.

### Why NOT background sync as primary model
Pre-syncing osquery tables to central requires knowing what the agent
will ask ahead of time. osquery has 200+ tables. The agent might ask
"what Ruby gems are on Machine C?" or "show crontab entries fleet-wide."
We can't predict this. Syncing all 200+ tables every 30s is absurd.

The fan-out IS the product. Central sigiro is a pure relay — it doesn't
interpret the query, just routes to edge(s) and aggregates results.
The agent sends arbitrary SQL or bpftrace programs. Zero pre-knowledge.

### Background vitals (secondary, optional)
Small set of vitals synced every 30s for "what happened 5 min ago" context:
- CPU, memory, disk usage, load average
- Shipped as [[architecture/write-path.md|OTLP]] metrics (already works, no new protocol)
- NOT osquery tables — just basic procfs/sysinfo gauges

Two channels:
- App telemetry: [[architecture/write-path.md|OTLP]] (streaming, standard, already works)
- System vitals: [[architecture/write-path.md|OTLP]] metrics from edge sigiro (same protocol)
- On-demand queries: gRPC fan-out (arbitrary osquery/[[designs/future/ebpf-gpu-position.md|eBPF]])

### Edge mode vs central mode (same binary)
```bash
# Edge (collect + forward)
sigiro serve --forward=https://central:4317 --osquery --ebpf

# Central (store + investigate + fleet controller)
sigiro serve
```

### When to build
- Single machine (Fly indie): Phase 3-4 (osquery + [[designs/future/ebpf-gpu-position.md|eBPF]] in binary)
- Fleet fan-out: Phase 5+ (when K8s / multi-machine users arrive)
- Background fleet sync: Phase 5+ ([[architecture/data-model.md|Parquet]] shipping)

## Roadmap Impact

| What | When | Effort |
|------|------|--------|
| agents.txt: osquery + bpfquery hints | Now | 0 lines (docs) |
| bpfquery MCP wrapper | Phase 3-4 | ~100 lines (thin wrapper) |
| osquery OTLP exporter (system state as metrics) | Phase 5+ | ~200 lines |
| ElectricSQL multi-machine sync | Phase 5+ | ~500 lines |

## The Pitch

"sigiro: your agent needs the right tools to debug.
One install. App traces to kernel syscalls.
Not a dashboard. A toolkit for AI agents."

## Single Binary Architecture

One download. One binary. Full debugging toolkit.

### Compiled into sigiro (Rust-native):
- **OTLP ingest + [[architecture/data-model.md|DuckDB]]** — existing core
- **[[architecture/investigation.md|InvestigationBlock]]** — existing query engine
- **aya [[designs/future/ebpf-gpu-position.md|eBPF]] runtime** — load pre-built probes at runtime, pure Rust, no C deps
  - 30+ pre-built probes: tcp_retransmit, block_io_latency, dns_latency,
    oom_kill, syscall_count, exec_snoop, file_open_snoop, ssl_latency, etc.
- **procfs/sysinfo** — basic OS state for platforms without osquery
- **Subprocess manager** — manages osqueryd + bpftrace lifecycle

### Embedded binaries (extracted on `sigiro setup`):
- **osqueryd** (~8MB compressed) — full 200+ tables, industry standard
  - sigiro communicates via osquery extension API (unix socket)
  - Agent queries any table: `sigiro_osquery(sql="SELECT * FROM processes")`
- **bpftrace** (~3MB compressed) — escape hatch for arbitrary kernel scripts
  - Only used when pre-built aya probes aren't enough
  - Agent writes bpftrace scripts or bpfquery SQL

### What the agent sees (via CLI/HTTP):
```bash
# App telemetry
sigiro diagnose checkout
sigiro query "SELECT ..."

# OS state (full osquery, 200+ tables)
sigiro_osquery(sql="SELECT * FROM processes WHERE cpu_time > 1000")
sigiro_osquery(sql="SELECT * FROM listening_ports")
sigiro_osquery(sql="SELECT * FROM docker_containers")

# Kernel tracing (pre-built eBPF, fast)
sigiro_trace(probe="tcp_retransmit", duration=10)
sigiro_trace(probe="block_io_latency", duration=10)
sigiro_trace_list()

# Kernel tracing (arbitrary bpftrace, escape hatch)
sigiro_bpftrace(program="tracepoint:syscalls:sys_enter_* { ... }")
```

### Graceful degradation:
| Platform | Telemetry | OS state | Pre-built eBPF | bpftrace |
|----------|:---------:|:--------:|:--------------:|:--------:|
| Linux + root | ✓ | ✓ osquery + procfs | ✓ aya | ✓ |
| Linux no root | ✓ | ✓ osquery (partial) | ✗ | ✗ |
| macOS (dev) | ✓ | ✓ osquery | ✗ (no eBPF) | ✗ |
| Docker --cap-add | ✓ | ✓ osquery | ✓ aya | ✓ |
| Vercel/Railway | ✓ | ✗ | ✗ | ✗ |

### Crates:
```toml
aya = "0.13"           # pure Rust eBPF loader (pre-built probes)
aya-log = "0.2"        # eBPF log relay to userspace
procfs = "0.17"        # /proc fallback when osquery unavailable
sysinfo = "0.33"       # cross-platform system info fallback
```

### Build pipeline:
```
docker buildx bake (multi-arch):
├─ Compile sigiro + aya eBPF probes (Rust)
├─ Download osqueryd static binary (from osquery GitHub releases)
├─ Download bpftrace static binary (from bpftrace releases)
├─ Compress with zstd, embed via include_bytes!()
└─ Output: single sigiro binary (~35MB)
```

## Status
VISION — immediate action is agents.txt + MCP config. Code comes in Phase 3+.
