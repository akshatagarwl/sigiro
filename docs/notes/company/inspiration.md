---
title: Inspiration
created: 2026-03-01
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'deployment', 'logs', 'market', 'metrics']
---

# Inspiration

- [[[research/sazabi-analysis.md|Sazabi]]](https://www.sazabi.com/) — YC P26, AI-native observability with chat-first UI. "Logs are all you need" + "monitoring is dead" + autonomous alerts. **Closest ideological competitor.** Same founding insight as Sigiro ("if AI can code, it should understand production") but bets on logs-only + LLM inference over OTel-native unified Parquet storage. See [[research/sazabi-analysis.md]].
- [Observability Storage for the Agentic Era](https://www.deductive.ai/blogs/observability-storage-for-the-agentic-era) — Deductive AI
- [What Every Cloud Native Engineer Needs to Know About Kernel Memory](https://edera.dev/stories/what-every-cloud-native-engineer-needs-to-know-about-kernel-memory) — Edera (Jed Salazar, Mar 2026). Companion to "Your AI Agent Has a Kernel Problem" (Cloud Native Rejekts). Key insight: cgroups metrics are shadows — MemAvailable, page faults, PSI are the real signals but invisible per-container on shared kernels. AI agents break the old monitoring model (stateful, long-running, irreplaceable context). Edera's answer: dedicated kernels (microVMs). Sigiro's opportunity: correlate system-level metrics with discrete agent actions (tool calls, context growth) — the insight layer neither Edera nor Datadog provides.
- [[[research/runwhen-analysis.md|RunWhen]] AI SRE Platform](https://www.runwhen.com/) — Kyle Forster. "Any engineer can handle any issue in any part of the stack." Gartner Cool Vendors in AI SRE. Key insight: AI SRE uses traditional o11y data <30% of the time; the "other 70%" is events, service relationships, action lists, pre-canned SQL, runbook summaries. See [thesis](../research/runwhen-forster-thesis.md) and [gap analysis](../research/runwhen-vs-sigiro-analysis.md).
