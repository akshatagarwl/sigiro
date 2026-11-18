---
title: OneUptime Competitor Analysis
created: 2026-03-28
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# OneUptime Competitor Analysis
> Analyzed: 2026-03-23

## What They Are
Open-source observability platform. 6.7k GitHub stars, 37K commits, 341 forks. Founded by Nawaz Dhandala. TypeScript monorepo.

Positioning: "The open-source Datadog/PagerDuty/Statuspage replacement" — 10+ products in one platform. Classic SRE toolchain consolidation play.

## Tech Stack
- Language: TypeScript (Node.js)
- Telemetry: ClickHouse (logs/metrics/traces)
- App DB: PostgreSQL
- Ingestion: [[architecture/write-path.md|OpenTelemetry]] Collector, FluentBit, Fluentd
- Infra: [[roadmap.md|Docker]] Compose, Helm, Nginx
- Mobile: React Native
- AI: Separate AIAgent service, MCP directory
- Dev tools: AGENTS.md, CLAUDE.md (they use AI coding agents too)

## Product Surface (10+ products)
1. Monitoring (uptime, synthetic, API)
2. [[plans/STATUS.md|Status]] Pages (branded, custom domain)
3. Incident Management
4. On-Call & Alerts (rotation, escalation)
5. Logs Management
6. Metrics
7. Traces (distributed tracing)
8. Exceptions / Error Tracking
9. Workflows (drag-and-drop automation, 5000+ integrations)
10. Dashboards
11. AI Agent (root cause analysis, self-healing, runbooks)

## Pricing
- Free: $0/mo — 1 [[plans/STATUS.md|status]] page, basic monitoring, logs/traces/metrics
- Growth: $20/mo per user — on-call, integrations, API
- Scale: $84/mo per user — SSO, [[designs/auth-billing.md|RBAC]], multi-cloud
- Enterprise: Custom
- Pay-as-you-go:
  - Active monitors: $1/monitor/month
  - AI Agent: $0.02/1K tokens ($20/1M tokens) OR bring your own LLM ($0)

## What They Do Better
1. **Breadth / "one platform" story** — replace Datadog + PagerDuty + Statuspage + Sentry in one bill. Fortune 500 logos.
2. **Self-hosting / open-source trust** — full self-host via [[roadmap.md|Docker]] Compose or [[roadmap.md|Helm]]. MIT license. 6.7k stars.
3. **[[plans/STATUS.md|Status]] pages as wedge** — high-visibility, low-effort. Free tier includes one. Smart growth loop.
4. **Bring your own LLM** — plug in your own OpenAI/Anthropic key, pay $0 to OneUptime for AI features.
5. **MCP integration** — MCP directory in repo. AI agents can interact with OneUptime natively.
6. **Workflows engine** — drag-and-drop automation with 5000+ integrations and custom code.

## What They Do Worse
1. **Kitchen sink complexity** — 10+ products, massive surface area. Latest CI build was FAILING. 188 open issues.
2. **AI is bolted on, not native** — AI Agent is a separate service. Core product built for humans staring at dashboards.
3. **Heavy tech stack** — TypeScript + ClickHouse + Postgres = many moving parts. Compare to Sigiro's Rust + [[architecture/data-model.md|DuckDB]] single binary.
4. **No focus on AI coding agents** — target is traditional SRE/DevOps. No concept of "your agent needs tools to debug."
5. **Per-user [[designs/pricing.md|pricing]] kills agent adoption** — $20-84/mo per user doesn't make sense when "users" are AI agents.
6. **Heavyweight self-hosting** — requires ClickHouse + Postgres + Nginx + OTel Collector + multiple Node services.

## Ideas Worth Stealing

### 1. Bring Your Own LLM [HIGH VALUE]
Let users plug in their own API keys for any AI features. Pay $0 markup. Builds trust, removes [[designs/pricing.md|pricing]] objection. Make BYOLLM the default for any AI-powered root cause or log analysis.

### 2. Status Page as Wedge [MEDIUM VALUE]
Dead-simple [[plans/STATUS.md|status]] page (your-app.sigiro.dev/status) that auto-updates from monitoring data. Low effort to build, high visibility, easy to share. "Your agent's work has a public health page."

### 3. MCP Server [HIGH VALUE]
Build a Sigiro MCP server so Claude Code / Cursor / Codex can natively query Sigiro data during debugging. "sigiro, show me the last 5 errors" from inside the agent. THE differentiator for agent-native observability.

### 4. Incident Timeline / Postmortem [MEDIUM VALUE]
Auto-generated incident timelines from telemetry data. When something breaks, auto-create a "what happened" narrative. Useful for both agents and humans reviewing agent work.

### 5. "Replaces N Tools" Positioning [LOW EFFORT, HIGH IMPACT]
"Your agent doesn't need 5 tabs open — one tool, one interface, one context window." Frame consolidation for agents, not humans.

### 6. Pay-as-you-go Telemetry Pricing [MEDIUM VALUE]
Usage-based [[designs/pricing.md|pricing]] per event or per GB makes way more sense than per-user for agent-native workflows.

## The Biggest Insight
OneUptime = open-source Datadog for HUMANS. Sigiro = observability for AI AGENTS.

Fundamentally different products even with similar data. OneUptime optimizes for dashboards, on-call rotations, [[plans/STATUS.md|status]] pages — human workflows. Their AI Agent is a thin LLM wrapper on top.

Sigiro's opportunity: be the observability platform AI agents can actually USE — not just generate data for humans to look at. The MCP server is the clearest embodiment. An agent doesn't need a pretty dashboard. It needs structured, queryable context it can reason over.

**Don't compete on breadth. Compete on depth for the agent workflow.**
