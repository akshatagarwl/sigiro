---
title: Company Plan
created: 2026-03-23
updated: 2026-05-12
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'data-model', 'deployment']
---

# Company Plan

Single timeline. Product and distribution aligned.

> **Technical details:** [docs/[[roadmap.md|roadmap]].md](../roadmap.md)
> **GTM details:** [docs/company/gtm.md](./gtm.md)
> **[[company/cost.md|Cost analysis]]:** [docs/company/cost.md](./cost.md)

---

## Now — Get Real Traffic

**Goal:** One real app sending [[architecture/write-path.md|OTLP]] to sigiro, running investigations on real data.

Product:
- [ ] Deploy sigiro to Fly.io ([[architecture/ducklake-integration.md|DuckLake]] mode — stateless, S3 storage)
- [ ] Instrument a real app (even sigiro.com waitlist API) with OTel
- [ ] Run `sigiro diagnose` on real data, validate [[architecture/investigation.md|InvestigationBlock]] quality
- [ ] Fix any ingest/query issues found with real traffic

Distribution:
- [ ] Publish agent setup docs for CLI and HTTP usage
- [ ] Add examples for Claude Code, OpenCode, and Codex workflows
- [ ] Set up Discord community server
- [ ] Reserve PyPI/[[roadmap.md|Docker]] Hub/HN accounts

**Exit criteria:** Sigiro receives real [[architecture/write-path.md|OTLP]], [[architecture/investigation.md|investigation]] returns useful data.

---

## Month 1 — First External User

**Goal:** Someone outside Sigiro sends real data and runs investigations.

Product ([[roadmap.md|Roadmap]] Phase 1):
- [ ] ~~WAL~~ — Decided against (DuckDB has its own WAL). See docs/decisions/001-wal-not-needed.md
- [ ] [[designs/future/anomaly-view.md|Anomaly]] view — SQL view for baseline deviations
- [ ] [[designs/log-template-dedup.md|Log template]] dedup — noise reduction in investigations

GTM:
- [ ] Find first user from personal network / vibe coder communities
- [ ] 30 min onboarding call — first data within the session
- [ ] Document every setup friction → fix immediately
- [ ] Discord DM channel with each user, <4h response

Learn:
- [ ] Watch query logs — what do agents actually ask?
- [ ] What's confusing? What's missing from [[architecture/investigation.md|InvestigationBlock]]?

**Exit criteria:** 1 external user sending real data for 7+ days.

---

## Month 2-3 — Distribution & Polish

**Goal:** 5-10 self-hosted users. Frictionless install.

Product:
- [ ] `sigiro init` CLI scaffolding
- [ ] One-command Fly.io deploy (`fly deploy` with prebuilt image)
- [ ] [[designs/future/web-vitals.md|Web Vitals]] + client errors
- [ ] Deploy event correlation

GTM:
- [ ] OSS showcase submissions (awesome-duckdb, awesome-rust, etc.)
- [ ] r/selfhosted, r/devops cost comparison posts
- [ ] "I replaced Datadog with a free binary" blog post → HN

**Exit criteria:** 5+ users. Real feedback loop.

---

## Future — Hosted Mode (TBD)

No timeline. Revisit only when self-hosted traction justifies the complexity.

- OAuth2 signup (GitHub/Google)
- Hosted OTLP endpoint with per-project isolation
- Stripe billing (usage-based)
- Vercel Marketplace / Railway integrations

Until then: **self-hosted only, everything free.**

---

## Non-Goals (entire timeline)

- Enterprise features (RBAC/SSO/SAML) — only when deal is on the table
- [[designs/future/perses-compat.md|Grafana]] datasource plugin — antithetical to thesis
- Custom OTel Collector pipelines
- PromQL/LogQL/TraceQL compatibility
- Real-time streaming dashboards
