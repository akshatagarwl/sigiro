---
title: Groundcover Competitive Analysis
created: 2026-03-30
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'competitor', 'ebpf', 'investigation', 'migration']
---

# Groundcover Competitive Analysis
> Analyzed: 2026-03-27

## What They Ship (AI Observability, Mar 2026)
1. Trace every LLM call — prompts, responses, tokens, [[company/cost.md|cost]], latency
2. Trace drill-down — full execution chain of LLM calls, tool executions, agent workflows
3. [[company/cost.md|Cost]] governance — track spend by model, service, or agent
4. Zero-instrumentation — OpenAI, Anthropic, Bedrock captured via eBPF
5. OTel-native — works with any [[architecture/write-path.md|OpenTelemetry]] GenAI SDK
6. BYOC — prompts & sensitive data never leave your cloud

## Architecture
- K8s-only. eBPF requires kernel access = cluster agents + DaemonSets.
- "Zero instrumentation" = intercepting HTTP calls at kernel level to api.openai.com, api.anthropic.com, etc.
- [[company/cost.md|Cost]] tracking = aggregating gen_ai.usage.input_tokens * price. A SQL query.
- BYOC = they run in your K8s cluster. Not self-hosted single binary.

## Sigiro Overlap (already have or trivial)
- OTel-native GenAI tracing: gen_ai.* semantic conventions land in sigiro_spans today
- Trace drill-down: [[architecture/investigation.md|InvestigationBlock]] assembles span trees
- [[company/cost.md|Cost]] by model: SQL query over gen_ai.* span attributes
- BYOC: Sigiro IS your infrastructure. Single binary, no DaemonSets.

## Where They Win
- Zero-instrumentation via eBPF: useful for teams that won't add OTel SDK. But K8s-only.
- Polished UI for [[company/cost.md|cost]] dashboards: Sigiro is agent-first, not dashboard-first.

## Verdict
Different ICP. Groundcover = K8s enterprise. Sigiro = PaaS indie devs.
Not a direct competitor. Steal the FRAMING: "trace every LLM call" is a great headline.
GenAI span data already flows into Sigiro via standard OTel.
