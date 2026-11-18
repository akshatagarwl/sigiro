---
title: "Competitive Landscape: Self-Improving Investigation Agents (April 2026)"
created: 2026-04-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'duckdb']
---

# Competitive Landscape: Self-Improving Investigation Agents (April 2026)

## The Honest Truth

Yes, this space exists. Multiple tools are building what we described.
But NONE of them do exactly what we designed. Here's the map.

## Direct Competitors (investigation + learning)

### 1. Siclaw (scitix/siclaw) — ⭐86, created March 2026
CLOSEST to our design. Open-source SRE [[architecture/investigation.md|investigation]] copilot.

WHAT IT HAS:
- 4-phase [[architecture/investigation.md|investigation]] workflow (evidence → hypothesis → validation → RCA)
- [[architecture/investigation.md|Investigation]] Memory — learns from past incidents (SQLite + FTS5 + bge-m3 embeddings)
- Reusable Skills — diagnostic playbooks that evolve
- MCP extensible
- Read-only by default
- TUI + web UI + chat channels
- TypeScript/Node.js

WHAT IT DOESN'T HAVE:
- No [[architecture/write-path.md|OTLP]] ingest (connects to existing infra: K8s, Prometheus, etc.)
- No local telemetry storage (queries external systems)
- No "1 env var" install for PaaS devs
- No embedded analytics DB (uses external Qdrant for knowledge, SQLite for meta)
- Targets SRE teams, not indie devs
- No SQL-queryable telemetry (agent runs kubectl, queries APIs)
- Knowledge Retrieval (Qdrant) is "in progress" as of March 2026

OVERLAP WITH SIGIRO: ~40%
  Same: [[architecture/investigation.md|investigation]] memory, skills, MCP, learning loop
  Different: data layer (they query external; we own the data in [[architecture/data-model.md|DuckDB]])

### 2. RunbookAI (Runbook-Agent/RunbookAI) — ⭐8, created Feb 2026
Hypothesis-driven [[architecture/investigation.md|investigation]] agent.

WHAT IT HAS:
- `runbook investigate PD-12345 --learn` — learns from investigations
- Knowledge base: syncs runbooks, postmortems, architecture notes
- Learning artifacts: postmortem drafts, knowledge suggestions, runbook updates
- Shared knowledge server for teams
- Claude Code integration
- MCP server for knowledge search
- Approval-gated remediation

WHAT IT DOESN'T HAVE:
- No [[architecture/write-path.md|OTLP]] ingest, no telemetry storage
- No embedded DB (queries external: AWS, K8s, PagerDuty)
- No automatic self-improvement (learning is opt-in --learn flag)
- No adaptive thresholds
- Very new (8 stars)

OVERLAP WITH SIGIRO: ~25%
  Same: [[architecture/investigation.md|investigation]] + learning, CLI-first, skill/knowledge
  Different: they're a runbook wrapper; we own the data layer

### 3. Aurora (Arvo-AI/aurora) — ⭐117, created Jan 2026
LangGraph-based incident management + RCA.

WHAT IT HAS:
- AI-powered RCA across AWS, Azure, GCP, K8s
- Integrates with PagerDuty, Datadog, [[designs/future/perses-compat.md|Grafana]], Slack
- Infrastructure Knowledge Graph (Memgraph)
- AI-Suggested Code Fixes (generates PRs)
- Python, LangGraph-based

WHAT IT DOESN'T HAVE:
- No self-improvement / learning loop
- No [[architecture/investigation.md|investigation]] memory
- No skills / adaptive behavior
- No [[architecture/write-path.md|OTLP]] ingest, no local data
- Enterprise K8s focus, not indie devs

OVERLAP WITH SIGIRO: ~15%
  Same: AI [[architecture/investigation.md|investigation]] of production issues
  Different: no learning, no local data, enterprise focus

## Agent Memory Layer (general-purpose, not SRE-specific)

### 4. mem0 (mem0ai/mem0) — ⭐51,685
"Universal memory layer for AI Agents"

WHAT IT IS:
- General-purpose agent memory: add/search/update memories
- Supports multiple LLM providers, vector stores
- Python + TypeScript SDKs
- Cloud service (mem0.ai) + self-hosted

RELEVANCE TO SIGIRO:
  mem0 is a BUILDING BLOCK, not a competitor. We could USE mem0
  for the skill/insight memory layer instead of building our own.
  But: it's Python, adds a dependency, and doesn't understand
  telemetry/[[architecture/investigation.md|investigation]] context. Our DuckDB tables are simpler
  and more integrated.

VERDICT: Potential dependency, not a competitor.

### 5. Letta (letta-ai/letta) — ⭐21,840
"Platform for building stateful agents with advanced memory"

WHAT IT IS:
- MemGPT successor — OS-inspired memory management for agents
- Agents learn and self-improve over time
- CLI tool (letta-code) + API
- Model-agnostic, supports skills and subagents
- Node.js CLI + Python API

RELEVANCE TO SIGIRO:
  Letta is a FRAMEWORK for building agents with memory.
  You could build a Sigiro-like [[architecture/investigation.md|investigation]] agent ON Letta.
  But Letta doesn't know about telemetry, [[architecture/write-path.md|OTLP]], or SRE patterns.
  It's the memory/agent layer, not the data/observability layer.

VERDICT: Complementary framework, not a competitor.
  We could potentially build ON Letta instead of implementing
  our own memory layer. Worth evaluating.

### 6. LangMem (langchain-ai/langmem) — ⭐1,367
LangChain's memory module for agents.

WHAT IT IS:
- Memory management tools for LangGraph agents
- Background memory manager (auto-extracts, consolidates, updates)
- Core memory API for any storage system
- Tightly coupled to LangGraph ecosystem

RELEVANCE TO SIGIRO:
  Similar to mem0 — a building block for agent memory.
  LangGraph-specific. Not relevant unless we build on LangGraph.

VERDICT: Not relevant (we're Rust + MCP, not Python + LangGraph).

### 7. Cognee (topoteretes/cognee) — ⭐14,847
"Knowledge Engine for AI Agent Memory in 6 lines of code"

WHAT IT IS:
- Graph-based knowledge engine for agents
- Auto-extracts knowledge from documents/data
- Builds knowledge graphs for retrieval

RELEVANCE TO SIGIRO:
  Another building block. Could be used for knowledge extraction
  from [[architecture/investigation.md|investigation]] trajectories. But adds Python dependency.

## Observability + AI (but no self-improvement)

### 8. Coroot — ⭐7,536
eBPF-based observability with AI-powered RCA.

WHAT IT HAS:
- eBPF zero-instrumentation (the collection layer)
- AI-powered root cause analysis
- Predefined dashboards and inspections
- SLO-based alerting

WHAT IT DOESN'T HAVE:
- No self-improvement, no learning
- No skill/memory system
- Not agent-based (it's a dashboard + alerting tool)

### 9. Keep (keephq/keep) — ⭐11,589
Open-source AIOps and alert management platform.

WHAT IT HAS:
- Alert aggregation and correlation
- Workflow automation
- Integrations with monitoring tools

WHAT IT DOESN'T HAVE:
- No [[architecture/investigation.md|investigation]] agent, no learning, no self-improvement

## Commercial (Closed Source)

### 10. Rootly — "AI-native incident management platform, including AI SRE agents"
- Most well-funded direct competitor in the commercial space
- AI SRE agents that learn from incidents
- Unknown how much self-improvement they actually do
- Enterprise [[designs/pricing.md|pricing]], Slack-native

### 11. Shoreline.io — "Automated incident remediation"
- Learns from past incidents, builds automated runbooks
- Progressive automation: manual → semi-auto → fully automated
- Acquired/integrated into Cisco (2023)

### 12. BigPanda — RAG over incident databases

### 13. PagerDuty AI — Incident similarity matching

---

## THE MAP

```
                    SELF-IMPROVING
                    ↑
                    │
        Siclaw ●    │    ● (what Sigiro would be)
                    │        
    RunbookAI ●     │    
                    │
     Rootly ●       │    ● Letta (general-purpose)
                    │
                    │    ● mem0 (general-purpose)
    ────────────────┼──────────────────────────→
    EXTERNAL DATA   │         OWNS THE DATA
    (queries K8s,   │         (ingests OTLP,
     AWS, Datadog)  │          stores in DuckDB)
                    │
      Aurora ●      │    ● Coroot  
                    │
      Keep ●        │
                    │
                    ↓
                NOT LEARNING
```

## WHAT'S ACTUALLY NOVEL ABOUT SIGIRO'S DESIGN

1. **OWNS THE DATA** — Every competitor queries external systems (kubectl, 
   Prometheus API, Datadog API, AWS API). Sigiro INGESTS and STORES telemetry 
   in [[architecture/data-model.md|DuckDB]]. This means:
   - Agent queries SQL, not APIs (faster, cheaper, structured)
   - Self-improvement operates on DATA you control
   - No dependency on external tool availability
   - Historical analysis without external retention limits

2. **1 ENV VAR INSTALL** — Every competitor needs: API keys for monitoring 
   tools + kubectl access + cloud provider credentials + the tool itself.
   Sigiro: `OTEL_EXPORTER_OTLP_ENDPOINT=...` — done. Data flows. 
   [[architecture/investigation.md|Investigation]] works. No kubectl, no cloud API keys, no monitoring vendor.

3. **AGENT-AGNOSTIC VIA MCP** — Siclaw IS the agent. RunbookAI IS the agent.
   Aurora IS the agent. They all bundle their own LLM orchestration.
   Sigiro is a DATA TOOL that any agent can use. Claude Code, Hermes, 
   OpenCode, Cursor — they all benefit from Sigiro's improving data layer.

4. **ZERO NEW COMPUTE FOR SELF-IMPROVEMENT** — Siclaw needs Qdrant (vector DB).
   RunbookAI needs external knowledge sync. Aurora needs Memgraph.
   Sigiro: everything in [[architecture/data-model.md|DuckDB]] + a 30MB local embedding model. Zero servers.

5. **INDIE DEV ICP** — Every competitor targets enterprise SRE teams (K8s, 
   multi-cloud, PagerDuty). Nobody targets the indie dev on [[company/gtm.md|Fly.io]] who 
   wants "why is my app slow?" answered in 3 seconds.

## WHAT'S NOT NOVEL (we should acknowledge)

1. [[architecture/investigation.md|Investigation]] memory / learning from past incidents — Siclaw, RunbookAI 
   both do this (March 2026). We are NOT first.

2. Skill/runbook extraction — RunbookAI does `--learn`, Siclaw has skills.
   Both predate our design.

3. Hypothesis-driven [[architecture/investigation.md|investigation]] — RunbookAI's core pattern. Well-established.

4. MCP extensibility — Siclaw supports MCP too.

## RECOMMENDATION

Don't build a general-purpose self-improving [[architecture/investigation.md|investigation]] agent.
That's what Siclaw is building (and they have a head start on the agent side).

Instead, double down on what NOBODY ELSE has:

**Sigiro = the self-improving DATA LAYER for [[architecture/investigation.md|investigation]] agents.**

- Sigiro ingests OTLP, stores in [[architecture/data-model.md|DuckDB]], computes anomalies
- Sigiro's [[architecture/investigation.md|InvestigationBlock]] gets better over time (skills, synthesis, strategies)
- ANY agent (Siclaw, RunbookAI, Claude Code, Hermes) calls Sigiro via MCP
- The agent does the reasoning; Sigiro provides progressively better data

This is the moat:
- Siclaw could USE Sigiro as its data source (better than kubectl scraping)
- RunbookAI could USE Sigiro for evidence gathering (better than AWS API calls)
- Claude Code already uses Sigiro via MCP

**Don't compete with the agent layer. Own the data layer that agents depend on.**
