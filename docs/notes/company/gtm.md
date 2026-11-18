---
title: Go-to-Market
created: 2026-03-01
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'data-model', 'decision']
---

# Go-to-Market

> **Technical [[roadmap.md|roadmap]]:** [docs/roadmap.md](../roadmap.md) — phases 1-4.
> **[[company/plan.md|Company plan]]:** [docs/company/plan.md](./plan.md) — unified timeline.
> This file covers positioning, [[designs/pricing.md|pricing]], distribution, and customer acquisition only.

---

## Positioning

**Primary:** Your engineering team should be shipping product, not managing dashboards.

**Supporting:**
- Cheap enough that you'll never think about the bill
- Easy enough that setup is one afternoon and never again
- Smart enough that your AI agent handles the on-call

**Wedge:** "Observability for vibe coders." Agent-native [[architecture/investigation.md|investigation]] for people who build with AI but can't debug what they build. Nobody owns this yet.

**Not positioned as:** Open source tool / self-hosted alternative / [[designs/future/perses-compat.md|Grafana]] replacement.
**Positioned as:** Managed observability service for lean engineering teams and AI-native operations.

**Do not mention [[architecture/data-model.md|Parquet]], [[architecture/data-model.md|DuckDB]], B2, or [[designs/future/ebpf-gpu-position.md|eBPF]] in marketing copy.** Users care about outcomes.

### Pricing Presentation

$0.005/GB creates credibility risk. Frame it as:

> "We don't charge $0.005/GB because we cut corners. We charge $0.005/GB because we eliminated the complexity that makes observability expensive. One place for everything. No indexing overhead, no four separate backends, no pre-built dashboards nobody reads."

Lead with **simplicity**, then price as proof. Never lead with architecture.

### Pricing

**Free locally forever.** Hosted pricing TBD — free tier will always have a generous local-only option.

Current: install and run on your own machine at no cost. No rate limits, no data limits, no expiration.

Hosted/multi-tenant pricing: undecided. Will be usage-based when launched.

---

## Distribution Channels

Priority order — highest leverage first:

1. **Vercel Marketplace** — 6M+ devs, formal observability category. Code in src/vercel.rs.
2. **Agent setup docs** — Claude Code, OpenCode, Codex, and HTTP examples.
3. **Railway template** — one-click sidecar deploy.
4. **Fly.io template** — fly.toml exists, sigiro already runs there.
5. **npm @sigiro/nextjs** — zero-config Next.js instrumentation.
6. **Netlify Extension** — Extension SDK, build plugin.

### Provider landing pages

Static HTML at `sigiro.com/for/<provider>`. Provider-specific [[company/cost.md|cost]] comparison, deploy button, 3-step setup.

---

## Customer Acquisition

### Target audiences

**Vibe coders** — build apps with AI, can't debug what they built:
- Lovable Discord, Bolt.new community, Replit forums, r/vibecoding
- Message: "Point your app and coding agent at Sigiro. Ask 'is my app healthy?'"

**AI agent builders** — building agents that need observability:
- CrewAI, LangGraph, AutoGen, Claude developer communities
- Position: "the only backend where agent reasoning and service telemetry live in the same table"

**[[company/cost.md|Cost]]-sensitive devs** — small teams drowning in Datadog bills:
- r/devops, r/selfhosted, HN "Datadog too expensive" threads
- YC batch companies (small teams, [[company/cost.md|cost]]-sensitive, AI-native)

### First 10 customers

1. Personal network — anyone running a SaaS side project
2. AI agent builders on X complaining about observability costs
3. YC batch companies
4. r/devops "Datadog is killing us" threads
5. Vibe coder communities

Offer free onboarding for the first 10. 30 min Zoom, first data in sigiro within the call.

---

## Content

| Piece | Purpose | When |
|-------|---------|------|
| "I replaced Datadog for $2/month" | HN-bait, main launch content | After 1 real user |
| "Observability for vibe coders" | Own the narrative | After launch |
| Cost comparison (from cost.md) | Blog post version | After launch |
| "Dashboards All the Way Down" (from pitch.md) | Brand voice | After launch |
| Observability cost calculator | Web page: enter Datadog bill, see savings | After pricing page |
| "How to give your AI agent observability in 5 min" | CLI/HTTP setup guide | After first external user |

---

## Platform Presence

### Done
- [x] Domain: sigiro.com (Cloudflare)
- [x] Landing page: www/ (Cloudflare Pages)
- [x] X: @SigiroAI
- [x] LinkedIn: linkedin.com/company/sigiro
- [x] GitHub: github.com/sigiroai
- [x] npm: npmjs.com/org/sigiro

### TODO (priority order)
- [ ] Discord community server
- [ ] PyPI: reserve `sigiro`
- [ ] [[roadmap.md|Docker]] Hub: `sigiroai`
- [ ] HN account: `sigiroai`
- [ ] Homebrew tap: sigiroai/homebrew-tap

### OSS Showcase Submissions (free backlinks)
- [ ] awesome-duckdb, awesome-rust
- [ ] [[architecture/write-path.md|axum]] ECOSYSTEM.md
- [ ] otlp2records "Used by" in README
- [ ] [[architecture/data-model.md|DuckDB]] Ecosystem Newsletter

---

## Infrastructure

| Component | Platform | Why |
|-----------|----------|-----|
| Landing page | Cloudflare Pages | Static, free, global edge |
| Waitlist | Cloudflare Workers + D1 (FormZero) | Stateless, same account |
| Sigiro binary | Fly.io (DuckLake mode) | Scale-to-zero, stateless compute |
| Visitor analytics | GoatCounter | CLI-queryable, no cookies |

---

## Decisions Locked

| Decision | Date | Choice | Rejected |
|----------|------|--------|----------|
| Analytics | 2026-03-12 | OTel beacon (dogfood) + GoatCounter | Umami, PostHog, Plausible |
| Waitlist | 2026-03-12 | FormZero (Cloudflare Workers) | Tally, GetWaitlist |
| Deployment | 2026-03-12 | Cloudflare Pages | GitHub Pages, Netlify |
| Landing design | 2026-03-12 | Terminal-native, JetBrains Mono, dark bg | — |

---

## Anti-Priorities

- ~~Enterprise features ([[designs/auth-billing.md|RBAC]], SSO, SAML)~~ — only when enterprise deal is on the table
- ~~[[designs/future/perses-compat.md|Grafana]] datasource plugin~~ — antithetical to product thesis
- ~~Heavy analytics SDK~~ — no OTel JS SDK bundle (30-50KB). Lightweight beacon only.
