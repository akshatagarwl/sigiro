---
title: RunWhen / AI SRE Analysis
created: 2026-03-21
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'deployment']
---

# RunWhen / AI SRE Analysis

**Source:** Kyle Forster, CEO RunWhen (Gartner "Cool Vendors in AI SRE")
- LinkedIn: https://www.linkedin.com/in/kyforster/
- Company: https://www.runwhen.com/
- Blog 1: [Do Humans Still Read Logs?](https://www.linkedin.com/pulse/do-humans-read-logs-anymore-kyle-forster-wi4ie/)
- Blog 2: [The Other 70%](https://www.linkedin.com/pulse/context-know-how-ai-sre-what-other-70-kyle-forster-bfq3e/)
- Tools registry: https://registry.runwhen.com/all-tasks

**Analysis date:** 2026-03-21, 5-iteration autoresearch against Sigiro architecture docs

---

## Kyle Forster's Findings

### 1. Traditional o11y data is used `<30%` of the time

AI SRE armed with CLI/API/SQL tools uses traditional observability data (metrics, logs, traces) in less than 30% of LLM tool calls during incident [[architecture/investigation.md|investigation]]. Far less than expected.

**Implication:** Building a "well observed" system when the first-line observer is an AI SRE will be far cheaper, faster, and easier than building o11y for humans.

### 2. The "Other 70%" — what AI SRE actually consumes

Categories (least to most surprising):

1. **Service/resource relationships** — topology, what calls what
2. **Service dependency/call flows** — call chains, upstream/downstream
3. **Events (diverse)** — not just alerts: CI/CD events, gitops reconciliation, K8s events, cloud platform events, SaaS events. Events get winnowed to "happened a lot" or "transient" during reasoning.
4. **Lists of available actions** — most valuable text. Sorts out linguistic mess (synonym/homonym problems across services). More effective for LLM reasoning than knowledge graphs, ontologies, or world models.
5. **Documents (highly summarized)** — runbooks/wiki pages consumed as 3 lines out of 30+ pages. Full docs do more harm than good.
6. **Open source project snippets** — word associations provide logical connections
7. **Application API results** — job [[plans/STATUS.md|status]], transaction info. Carry high-cardinality data that makes later log filtering vastly more efficient.
8. **Pre-canned SQL with docstrings** — resolves linguistic mess + provides cardinality context. Gold for agentic reasoning.

### 3. No universal pattern in the 70%

Different organizations weight different data sources differently without being aware of it. In-house AI SRE implementations sometimes beat vendor PoCs because vendors must generalize.

### 4. Most data sources aren't as good as their authors think

- "22,000 tickets of troubleshooting expertise" → useful info was ~3 pages
- "Hundreds of wiki pages" → one page of useful error codes; rest did more harm than good
- "Multi-million line codebase" → paragraph per service was plenty

**Key insight:** For accurate agent reasoning, shorter is almost always better. Stale/incorrect/probabilistic data is really bad. Precision comes with progressive disclosure in late iterations.

### 5. Context windows need clean, diverse text

Small amounts of good text + very little bad text. As LLM reasoning improved through 2025, sophisticated search/NLP/KG algorithms got scrapped — simple LLM reasoning outperformed them, but only with good text and minimal noise.

### 6. Synonym/homonym problem is endemic

- "store" vs "location" in anti-fraud-db
- "Collection" meaning Qdrant vs Java error vs vector DB
- Humans don't notice; LLMs (and new hires) are driven bananas

### 7. Adding context follows a different pattern than instrumentation

Nobody knows when useful context is missing until they ask a question and get "I don't know." Magic happens when someone building a tool for missing context uses the chat session context as starting point.

### Business Insights

- 10-18% of enterprise cloud bill goes to observability
- Far less than 1% of data sent to o11y tools ever gets looked at by a human
- Storing concise one-sentence "insights" (for RAG) instead of raw data = 4 orders of magnitude less data needed
- 80-85% RCA accuracy on alert triage = "well scoped but mostly useless engineering project"
- 95%+ general question-answering accuracy = high value, far more work
- $100B innovator's dilemma for current o11y vendors

### Architecture Philosophy

- Background agents (not chat-on-top-of-dashboards), 2+ years of development
- Lists of actions > knowledge graphs/ontologies for LLM reasoning
- Progressive disclosure: start summary, drill down only when needed
- MCP-centric approach for adding new context tools

---

## Gap Analysis

### Where we're strongly aligned

| Forster Finding | Sigiro Implementation | Status |
|---|---|---|
| Traditional o11y data is `<30%` of what matters | Investigation block returns structured, dense telemetry | DONE |
| Shorter context = better reasoning | 10-50K tokens per investigation block | DONE |
| Progressive disclosure | Narrow window first, zoom in on demand | DONE |
| Diversity of sources helps | Metrics + spans + logs + profiles + linked traces in one response | DONE |
| 4 orders of magnitude less data needed | $0.005/GB, compute-on-read, no pre-aggregation | DONE |

### Where we're solving the SAME problem differently

| Forster Approach | Sigiro Approach | Assessment |
|---|---|---|
| Thousands of agentic tools in production | Single structured investigation block | **Simpler but narrower.** Sigiro wins on token efficiency but loses on exploration breadth |
| MCP-centric tool addition | OTLP ingest + SQL query | **Different layer.** Neither alone is sufficient |
| "I don't know" triggers tool building | No equivalent | **Missing entirely** |

### Where we're MISSING the thesis entirely

| Forster's "Other 70%" | Sigiro Status | Gap Severity |
|---|---|---|
| **Events (CI/CD, K8s, gitops, SaaS)** | Not ingested, not modeled | CRITICAL |
| **Lists of available actions** | Not provided | CRITICAL |
| **Service/resource relationships** | Plan 20 (dependency signal) — NOT BUILT | HIGH |
| **Pre-canned SQL with docstrings** | Not provided | HIGH |
| **Synonym/homonym resolution** | Not addressed | HIGH |
| **Application API results** | Not ingested | MEDIUM |
| **Summarized runbook/wiki content** | Not available | MEDIUM |
| **Open source project context** | Not available | LOW |

---

## Deep Analysis: 5 Key Gaps

### Gap 1: Events — the missing signal type

Events (CI/CD deploys, K8s events, gitops state, SaaS webhooks) don't naturally flow through [[architecture/write-path.md|OTLP]]. K8s events could arrive as logs, but the rest require a webhook endpoint.

**What to do:** Accept webhook POST at `/v1/events` → `sigiro_events` table. Include in [[architecture/investigation.md|investigation]] block: "deploy v2.3.1 at 14:05, 40 transient errors during rollout." Single highest-ROI addition.

### Gap 2: Synonym/homonym resolution

Zero awareness of this problem. [[architecture/investigation.md|Investigation]] block returns raw service names, metric names, span names. An agent reasoning across multiple services hits this problem constantly.

**What to do:** Service metadata registry mapping service_name → description, team, purpose, known aliases. Include in [[architecture/investigation.md|investigation]] block `context` section. Low effort, massive reasoning quality improvement.

### Gap 3: Available actions — the most valuable missing context

We provide `/investigate` and `/query`. The agent doesn't know what other tools exist, what questions they can answer, what services they cover.

**What to do:** MCP server with rich tool descriptions. Per-service action registry. This is the bridge between "better observability backend" and "AI SRE platform."

### Gap 4: Progressive context addition — "I don't know" feedback

[[architecture/investigation.md|Investigation]] block returns whatever data exists. No guidance on what's missing or how to get it.

**What to do:** Coverage hints: "no metrics found for this service" vs silent empty array. Suggestion engine: "error traces link to payments — [[architecture/investigation.md|investigate]] payments next."

### Gap 5: Document/runbook summarization

**NOT a priority for MVP.** High effort, marginal return. When needed: accept per-service README via API, store as metadata, include 3-line summary. Let the agent's own codebase knowledge handle this.

---

## Strategic Positioning

**RunWhen:** 1000+ tools, CLI wrappers, API integrations. The agent explores broadly.
**Sigiro:** 1 dense [[architecture/investigation.md|investigation]] block from unified telemetry. The agent reasons deeply.

They're complementary, not [[research/competitive-landscape-2026.md|competitive]]. Sigiro is the **data layer**, RunWhen is the **tool layer**.

The risk: staying purely in the data layer = infrastructure — necessary but commoditizable. MCP server is the bridge: Sigiro exposes its [[architecture/investigation.md|investigation]] block as a tool that RunWhen-style agents call alongside their kubectl wrappers and API tools.

### The "other 70%" opportunity

1. Make our `<30%` the best [[architecture/investigation.md|investigation]] block possible (already doing this)
2. Accept events as a first-class signal type (easy)
3. Expose ourselves as an MCP tool with rich context (medium)
4. Signal what we DON'T know so the agent can seek context elsewhere (novel)

Point 4 is the real insight: the agent should say "Sigiro gave me telemetry but I also need deploy history and runbook context" and go get those from other tools.

---

## Action Items

| Priority | Action | Effort | Impact |
|---|---|---|---|
| P0 | Add events section to investigation block (K8s events from logs, deploy markers) | Small | High |
| P1 | Build MCP server exposing `sigiro_investigate` and `sigiro_query` as tools | Medium | High |
| P2 | Service metadata registry (description, team, aliases, dependencies) | Small | High |
| P3 | Coverage/gap signaling in investigation block | Small | Medium |
| P4 | `/v1/events` webhook endpoint for CI/CD, deploy, incident events | Medium | Medium |
| P5 | Investigation block `context` section (metadata + topology + recent events) | Medium | High |

## Key Quotes

> "Lists of actions were simpler and more effective for LLM reasoning than any abstract data structure — knowledge graph, ontology, world model, etc."

> "Getting to 80-85% RCA accuracy on alert triage is a well scoped but mostly useless engineering project. Getting to 95%+ general question-answering accuracy is high value."

> "For accurate agent reasoning, shorter is almost always better. Stale data, incorrect data, and probabilistic data is really really bad."

> "Different tools by different authors with different perspectives make for excellent context windows."

### Community Reaction (Florian Hoeppner)

> "The 'pre-incident context collection' framing is interesting because it's prevention work that doesn't look like prevention work."

**Implication for Sigiro:** Our [[company/plan.md|plan]] 31 auto-derives context (service metadata, deploy events, topology) from [[architecture/write-path.md|OTLP]] data already flowing in. This IS "pre-incident context collection" — it happens automatically, without the team even knowing they're investing in prevention.
