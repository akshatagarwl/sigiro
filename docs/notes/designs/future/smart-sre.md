---
title: Smart SRE — Sigiro as an Agent Debugging Toolkit
created: 2026-03-31
updated: 2026-09-13
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'deployment', 'design']
---

# Smart SRE — Sigiro as an Agent Debugging Toolkit

## Principle

Sigiro gives an AI agent application telemetry and queryable operating-system
state. Platform APIs supply deployment and infrastructure context. Each surface
uses an existing protocol instead of adding product-specific collectors.

## The Toolkit

| Capability | Tool | Agent interface |
|---|---|---|
| Application telemetry | Sigiro core | MCP, CLI, HTTP |
| OS state | osquery | SQL |
| Platform context | Platform APIs | HTTP |
| Database insights | Database APIs | HTTP or SQL |
| Profiles | Sigiro OTLP Profiles | MCP or HTTP |

## Investigation Flow

1. Query Sigiro for traces, logs, metrics, profiles, and anomalies.
2. Query deployment and infrastructure APIs for changes near the incident.
3. Query osquery for current process, network, package, and host state.
4. Report the evidence and the narrowest supported remediation.

Most incidents should resolve from Sigiro telemetry. OS state is a deeper,
on-demand surface, not a background copy of every host table.

## Fleet Queries

A parent agent can run the same osquery SQL across selected machines and combine
the rows with an explicit host identity. Do not pre-sync all osquery tables: the
agent decides what state it needs during an investigation.

Background CPU, memory, disk, and load history belongs in standard OTLP metrics.
That preserves one telemetry protocol while keeping live host inspection
on-demand.

## Deployment Shape

- Sigiro continues to receive and query OTLP telemetry.
- osquery remains a separate maintained binary and protocol.
- Fleet fan-out is deferred until multi-machine users need it.
- Platform credentials stay with the investigating agent, not Sigiro.

## Status

VISION. Build only after a concrete incident workflow proves that current
telemetry and platform APIs are insufficient.
