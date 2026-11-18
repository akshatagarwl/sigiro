---
title: Taglines & Messaging
created: 2026-03-06
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'duckdb', 'ducklake']
---

# Taglines & Messaging

Raw material for landing pages, marketing, and positioning.

**Rule: Never mention [[architecture/data-model.md|Parquet]], DuckDB, B2, S3, or eBPF in customer-facing copy.
Users care about outcomes: cheap, easy, agent-queryable. Architecture is an implementation detail.**

---

## Top 10 — Use These

Distilled from all categories below. These are the sharpest, most distinctive lines.

1. **"Stop staring at dashboards."** — Hero headline. Confrontational, specific, immediately understood.
2. **"What if you just asked what broke, and got the answer?"** — The core promise in one sentence.
3. **"$0.005/GB. No per-host fees. No per-user fees. No surprises."** — Price as proof of simplicity.
4. **"The margins on your observability bill would make a luxury brand blush."** — Provocation for HN/Reddit.
5. **"Set it up once. Let agents handle the rest."** — The zero-maintenance promise.
6. **"Observability broke when it forgot who's reading."** — Manifesto opener (agents, not humans).
7. **"Dashboards are technical debt with pretty colors."** — Anti-dashboard, shareable standalone.
8. **"Your on-call engineer shouldn't need a PhD in PromQL."** — For the DevOps/SRE audience.
9. **"Every token spent debugging is a token stolen from building."** — For AI-native teams.
10. **"If your monitoring tool needs monitoring, something went wrong."** — Contrarian one-liner.

---

## Core Identity

- Your engineering team should be shipping product, not managing dashboards
- Observability that works so your team doesn't have to
- Built for teams that ship, not teams that stare at graphs

## Hero Headline Candidates

- "Stop staring at dashboards." ← **CHOSEN for landing page**
- "What if you just asked what broke, and got the answer?"
- "Observability your agent handles. [[designs/pricing.md|Pricing]] that doesn't hurt."
- "Your team should be building features — not debugging dashboards."

## Anti-Dashboard / Anti-Noise

- The best dashboard is no dashboard — just answers
- You don't need 47 charts to find a broken pod
- Dashboards are technical debt with pretty colors
- The observability industry sold you complexity and called it visibility

## Agent-First

- Your on-call engineer shouldn't need a PhD in PromQL
- No dashboards. No query language. Just "what broke and why?"
- Set it up once. Let agents handle the rest.
- When something breaks, your agent already knows why

## Tokens for Product, Not Postmortems

- Every token spent debugging is a token stolen from building.
- Your AI budget shouldn't go up in smoke every time production does.
- Spend tokens shipping, not firefighting.

## Cost / Simplicity

- $0.005/GB. No per-host fees. No per-user fees. No surprises
- Your observability costs more than your cloud. That's backwards
- The cheapest [[company/plan.md|plan]] from Datadog costs more than our most expensive one

## Contrarian / Provocative

- The margins on your observability bill would make a luxury brand blush
- If your monitoring tool needs monitoring, something went wrong
- Most observability platforms are optimized for selling seats, not solving incidents

## For the Landing Page (Not Architecture)

DO say:
- "Every signal in one place — logs, metrics, traces, [[architecture/profiles.md|profiles]]"
- "One simple monthly number"
- "Setup in 5 minutes with your existing OTel config"
- "Your agent can [[architecture/investigation.md|investigate]] any incident autonomously"
- "Works with any language, any platform, any cloud"

DON'T say (internal only):
- "[[architecture/data-model.md|Parquet]] table" — users don't care
- "[[architecture/data-model.md|DuckDB]]" — users don't care
- "B2 / Backblaze" — users don't care
- "[[designs/future/ebpf-gpu-position.md|eBPF]]" — users don't care
- "Epoch-hour partitioning" — users REALLY don't care
