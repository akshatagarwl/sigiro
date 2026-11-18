---
title: Agent Telemetry Convergence
created: 2026-03-16
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'auth', 'data-model', 'duckdb', 'investigation']
---

# Agent Telemetry Convergence

**[[plans/STATUS.md|Status]]:** Research — not designed yet
**Supersedes:** `agent-observability-gap.md` (which only covered "we can't see what agents do with our CLI")
**Source:** Siddhant Khare's agent observability gap analysis + OTel GenAI semantic conventions

## 1. The Problem

Siddhant Khare identifies four questions agent observability must answer:

| Question | OTel GenAI attribute | Status |
|---|---|---|
| What did the agent do? | `gen_ai.operation.name`, `gen_ai.tool.name`, `gen_ai.tool.call.arguments` | Development-status convention |
| Why did it choose that? | `gen_ai.output.messages` (reasoning), span parent/child structure | Development-status; reasoning capture is lossy |
| What did it cost? | `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, `gen_ai.request.model` | Development-status convention |
| What permissions did it use? | Tool type in `gen_ai.tool.description` | Barely covered — conventions define tool types but not auth policies |

The industry is actively solving this on top of OTel:

- **Standards:** OTel GenAI semantic conventions (Development [[plans/STATUS.md|status]]), agent span conventions
- **Instrumentation:** OpenLLMetry (auto-instruments LangChain, CrewAI, LlamaIndex), OpenLIT (auto-instruments 30+ LLM/vector/agent frameworks)
- **Backends:** Langfuse (prompt management + traces), Arize Phoenix (eval + traces), plus Datadog/[[designs/future/perses-compat.md|Grafana]] adding GenAI dashboards

All emit standard [[architecture/write-path.md|OTLP]]. All use `gen_ai.*` attributes.

## 2. The Greenfield Insight: Agents Are Services

An agent is a process. It has a `service.name`. It receives requests, calls external APIs (LLMs instead of databases), orchestrates tools, and produces outputs. It emits spans, metrics, logs — the same signals any service emits.

The only difference: it calls `api.anthropic.com` instead of `postgres:5432`.

Don't build "agent observability" as a separate feature. Build observability that understands all workload types through the signals they emit. The unified table already handles this: agent spans and service spans share the same schema, same storage, same query engine.

## 3. What Works Today (Zero Changes)

Walk through the actual code path:

1. **OpenLLMetry/OpenLIT emit [[architecture/write-path.md|OTLP]] spans** with `gen_ai.*` attributes
2. **Collector receives** on ports 4317/4318/4319
3. **`mapper.go:mapTraces()`** (line 256-320) converts to UnifiedRow:
   - `attrsToMap(s.Attributes())` at line 297 captures ALL span attributes including `gen_ai.*`
   - `mergeMetadata(r.extra, spanAttrs)` at line 298 puts them in the JSON metadata blob
   - Span events (lines 282-295) capture `gen_ai.client.inference.operation.details` as event attributes
4. **`mapMetrics()`** (line 166-232) handles token usage histograms (`gen_ai.client.token.usage`):
   - Collapsed to avg due to histogram handling (line 213) — known limitation, avg is sufficient for [[company/cost.md|cost]] computation
5. **Query:** `json_extract_string(metadata, '$.gen_ai.operation.name')` works in [[architecture/data-model.md|DuckDB]] against the metadata column

**The data is already there. The presentation is wrong.**

## 4. What's Genuinely Novel (The Convergence)

Langfuse/Phoenix show what the agent did. Datadog/[[designs/future/perses-compat.md|Grafana]] show what the service did. Nobody connects both.

In Sigiro, both are in the same table. One [[architecture/data-model.md|DuckDB]] query correlates:
- Agent's tool-call span ("called sigiro query for checkout")
- With the checkout service's actual spans/metrics/logs in the same time window
- Linked by trace ID propagation or by service name + time overlap

**Self-referential loop:** [[company/plan.md|Plan]] 14's SRE agent instrumented with OpenLLMetry — Sigiro observes the agent that observes the services.

This is not a feature to build. It's a consequence of the unified table design.

## 5. Making the Query Layer Signal-Aware

`investigate.go` currently runs 4 hardcoded queries (metrics, spans, logs, [[architecture/profiles.md|profiles]]) via `runPrimaryQueries()` (line 63-128). The greenfield approach:

**Detect what's present, adapt the response:**
- Probe the data: does this service have `gen_ai.*` in metadata? Include agent sections
- Does it have `http.*`? Include HTTP sections
- Does it have both? Show both (an agent that serves an HTTP API)

**New query patterns (all against the same `_scan` temp table):**

```sql
-- Tool call summary
SELECT json_extract_string(metadata, '$.gen_ai.tool.name') AS tool,
       count(*) AS calls, avg(value) AS avg_ms,
       count(*) FILTER (WHERE status = 'ERROR') AS errors
FROM _scan WHERE signal='span'
  AND json_extract_string(metadata, '$.gen_ai.operation.name') = 'execute_tool'
GROUP BY tool ORDER BY calls DESC;

-- Model usage + cost
SELECT json_extract_string(metadata, '$.gen_ai.request.model') AS model,
       count(*) AS calls, avg(value) AS avg_ms,
       sum(CAST(json_extract_string(metadata, '$.gen_ai.usage.input_tokens') AS INT)) AS input_tokens,
       sum(CAST(json_extract_string(metadata, '$.gen_ai.usage.output_tokens') AS INT)) AS output_tokens
FROM _scan WHERE signal='span'
  AND json_extract_string(metadata, '$.gen_ai.operation.name') = 'chat'
GROUP BY model;

-- Reasoning chain (ordered spans within an agent invocation)
SELECT name, json_extract_string(metadata, '$.gen_ai.operation.name') AS op,
       value AS duration_ms, status, trace_id, span_id, parent_span_id
FROM _scan WHERE signal='span'
  AND json_extract_string(metadata, '$.gen_ai.operation.name') IS NOT NULL
ORDER BY timestamp;
```

**Schema additions in `block.go`:**
- `AgentSection` with `ToolCalls []ToolCallSummary`, `ModelUsage []ModelUsageRow`, `TokenCost *CostSummary`
- Added to `InvestigationBlock` as `Agent *AgentSection` (nil when service has no gen_ai data)

## 6. The Trace Correlation (The Killer Query)

When an agent calls Sigiro's query API to [[architecture/investigation.md|investigate]] checkout, two traces exist:
1. **Agent trace:** `invoke_agent -> execute_tool(sigiro_query, service=checkout) -> chat(reasoning)`
2. **Service trace:** checkout's own spans/metrics/logs in the same time window

Both are in the unified table. The join:

```sql
-- Find what service the agent was investigating
SELECT json_extract_string(metadata, '$.gen_ai.tool.call.arguments') AS args
FROM _scan WHERE service='sre-agent' AND signal='span'
  AND json_extract_string(metadata, '$.gen_ai.tool.name') = 'sigiro_query';
-- Parse service name from args -> fetch that service's investigation block
```

This gives you: "Agent investigated checkout at 14:10. Here's what checkout was doing. Here's what the agent concluded." One platform, one query namespace.

## 7. Gaps and Risks (Honest)

| Risk | Severity | Mitigation |
|---|---|---|
| GenAI conventions are Development-status | Medium | Metadata is schemaless JSON — attribute renames don't break storage, only queries |
| `gen_ai.output.messages` can be 100KB+ per span | High | Configurable size limit in mapper, or separate `content` column |
| `json_extract_string` is slow at scale | High | Promote high-value gen_ai attrs to columns when scale demands |
| Histogram collapse loses token distribution | Low | Known limitation, avg is sufficient for cost computation |
| Permission audit trails barely covered by conventions | Medium | Honest gap — conventions define tool types but not auth policies |
| Agent traces shouldn't be sampled | Medium | Tail-based sampling: always keep agent traces |

## 8. Position Shift

- **Old:** "Observability for the agents, by the agents" (of services)
- **New:** "Observability of everything — services, agents, and the reasoning that connects them"
- The differentiator isn't "we also do agent observability" (Langfuse/Phoenix do it better in isolation)
- The differentiator is: **we're the only backend where agent reasoning and service telemetry live in the same table, queryable in the same SQL, correlated by the same trace IDs**

## 9. References

- Siddhant Khare: siddhantkhare.com/writing/agent-observability-gap
- OTel GenAI conventions: opentelemetry.io/docs/specs/semconv/gen-ai/
- OTel agent spans: opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/
- OTel issue #2664: github.com/open-telemetry/semantic-conventions/issues/2664
- OTel blog on agent observability: opentelemetry.io/blog/2025/ai-agent-observability/
- OpenLLMetry: github.com/traceloop/openllmetry
- OpenLIT: github.com/openlit/openlit
- Langfuse: github.com/langfuse/langfuse
- Arize Phoenix: github.com/Arize-ai/phoenix
- Internal: [[company/plan.md|plan]] 14, plan 21, docs/research/agent-observability-gap.md
