---
title: Three-Layer Agent Debugging — Design
created: 2026-03-30
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'deployment', 'design', 'ebpf', 'fly-io']
---

# Three-Layer Agent Debugging — Design

## Principle

Sigiro's [[company/pitch.md|pitch]]: "your agent needs the right tools to debug."
The agent needs access to three layers of data, each progressively deeper.

## The Three Layers

### Layer 1: App Telemetry (Sigiro — what we build)
- Traces: span trees, latencies, errors, cross-service correlation
- Logs: template-deduplicated, severity-grouped, trace-correlated
- Metrics: p95, [[designs/future/anomaly-view.md|anomaly]] detection (relative 3σ baselines)
- Profiles: OTLP profiles, flamegraphs (via [[architecture/profiles.md|Pyroscope]] compat)
- [[architecture/investigation.md|InvestigationBlock]]: all of the above, structured JSON for agents

Access: sigiro CLI, HTTP API, MCP server (planned)

### Layer 2: Platform Context (agents.txt — agent reads APIs)
- Deploy events: Vercel, Fly, Railway, GitHub Actions
- Machine state: Fly Machines API, Railway service metrics
- [[designs/future/web-vitals.md|Web vitals]]: Vercel Speed Insights, Cloudflare Analytics
- DB platform insights: PlanetScale Insights, Neon pg_stat_statements, Supabase logs, Turso usage
  - PlanetScale: query-statistics API (slow queries, QPS, latency percentiles)
  - Neon: pg_stat_statements via SQL (mean_exec_time, calls, rows)
  - Supabase: pg_stat_statements + analytics logs API
  - Turso: usage API (query count, rows read)
  - Any Postgres: EXPLAIN ANALYZE, pg_stat_activity, pg_locks

Access: agents.txt teaches agents which APIs to call + how to correlate

### Layer 3: System-Level (MCPtrace / bpftrace — kernel visibility)
- Syscalls: open, read, write, execve
- Network: TCP connect/retransmit/drop, DNS latency, SSL snooping
- I/O: block I/O latency, filesystem cache hits
- CPU: scheduler latency, run queue depth, CPU profiling
- Memory: OOM kills, swap, page faults, memory pressure (PSI)
- Security: capability checks, setuid tracking

Access: MCPtrace MCP server (github.com/eunomia-bpf/MCPtrace)

## When Each Layer Is Used

```
Agent investigating "checkout is slow":

1. Layer 1 (Sigiro): sigiro diagnose checkout
   → "p95 latency spiked at 14:32, 3 error spans, all hitting db.query"
   → Root cause found? → Done.

2. Layer 2 (Platform): Fly Machines API + Vercel deploy API
   → "Deploy v1.2.3 at 14:30, Fly machine restarted at 14:31"
   → Root cause found? → Done.

3. Layer 3 (bpftrace): MCPtrace exec_program("tracepoint:tcp:tcp_retransmit_skb")
   → "TCP retransmits to db.internal:5432 — network issue, not app issue"
   → Root cause found at kernel level.
```

Most issues resolve at Layer 1. Layer 3 is the nuclear option for
"spans look normal but something is wrong."

## MCPtrace Integration

MCPtrace (github.com/eunomia-bpf/MCPtrace) is an MCP server for bpftrace.
It exposes 4 tools to the agent:

| Tool | What |
|------|------|
| list_probes | List available bpftrace probes (syscalls, tcp, block, etc.) |
| bpf_info | System BPF capabilities |
| exec_program | Run a bpftrace program (async, max 60s) |
| get_result | Get output from a previous execution (paginated) |

Requirements: Linux, root/sudo, bpftrace installed.
Does NOT work on PaaS (Vercel, Railway) — only where you have kernel access.
Works on: Fly Machines, K8s nodes, bare metal, self-hosted VMs.

Sigiro does NOT build MCPtrace. It already exists. Sigiro integrates:
1. agents.txt hints: "if Layer 1 doesn't explain it, use bpftrace"
2. `sigiro diagnose` output suggests MCPtrace when anomalies lack obvious cause
3. Blog/docs: "Sigiro + MCPtrace = HTTP spans down to kernel syscalls"

## Optional MCPtrace Configuration

```json
{
  "mcpServers": {
    "bpftrace": {
      "command": "bpftrace-mcp-server",
      "env": { "SUDO_PASSWORD": "..." }
    }
  }
}
```

## Competitive Advantage

No other observability tool offers this combination:
- Datadog: dashboards, no agent access to bpftrace
- [[research/groundcover-analysis.md|Groundcover]]: eBPF but no interactive agent access
- [[research/zymtrace-analysis.md|ZymTrace]]: GPU profiling, different ICP
- New Relic / [[designs/future/perses-compat.md|Grafana]]: no system-level debugging tools for agents

Sigiro + MCPtrace = the only stack where an AI agent can debug
from HTTP request → span tree → SQL query → TCP retransmit → kernel syscall.

## Status
POSITIONING — MCPtrace exists, Sigiro integrates via agents.txt + MCP config.
No code to build for basic integration. Sigiro MCP server is a future item.
