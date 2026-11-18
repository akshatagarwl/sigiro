---
title: Context Integrations — "What changed?" enrichment for sigiro diagnose
created: 2026-03-28
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'data-model', 'deployment', 'ducklake', 'investigation']
---

# Context Integrations — "What changed?" enrichment for sigiro diagnose

## Thesis

Telemetry tells you **what broke**. Context integrations tell you **why** — by correlating
external change events with anomalies. Every integration follows the same pattern:
ingest timestamped change events into `sigiro_events`, surface them in [[architecture/investigation.md|InvestigationBlock]].

## Architecture

All integrations write to a single `sigiro_events` table:

```sql
CREATE TABLE sigiro_events (
    timestamp TIMESTAMPTZ NOT NULL,
    project_id VARCHAR NOT NULL,
    event_type VARCHAR NOT NULL,   -- 'deploy', 'flag_change', 'infra_change', 'command', 'runbook'
    source VARCHAR NOT NULL,       -- 'vercel', 'railway', 'github', 'atuin', 'flipt', ...
    title VARCHAR NOT NULL,
    metadata JSON,                 -- source-specific payload
    service_name VARCHAR           -- nullable, for service-scoped events
);
```

[[architecture/investigation.md|InvestigationBlock]] already has AR-3 (DerivedEvent). These integrations feed real data
into that same pipeline instead of inferring from `service.version` changes.

---

## Tier 1 — Deploy Correlation (highest signal, build first)

### Vercel Deploy Hooks
- **How:** Vercel Log Drains / Deploy Hooks POST to `/v1/events/deploy`
- **Payload:** git SHA, branch, deploy URL, timestamp, [[plans/STATUS.md|status]]
- **Value:** "Deployed commit abc123 → error rate 5x" is the #1 incident pattern
- **Lines:** ~50 (webhook handler + insert)
- **Prerequisite:** already planned in [[plans/context-integrations.md|d5-deploy-corr]].md

### Railway Deploy Webhooks
- **How:** Railway webhooks POST deploy events
- **Payload:** service name, commit SHA, deploy [[plans/STATUS.md|status]], environment
- **Value:** Same as Vercel — Railway is the #2 platform for vibe coders
- **Lines:** ~30 (same handler shape as Vercel)

### GitHub Deployments API
- **How:** GitHub webhook `deployment` + `deployment_status` events
- **Payload:** SHA, environment, description, creator, [[plans/STATUS.md|status]]
- **Value:** Source of truth for git-based deploys regardless of platform
- **Lines:** ~40

---

## Tier 2 — Change Correlation (invisible causes)

### Feature Flag Changes (Flipt / OpenFeature)
- **How:** Webhook on flag evaluation change, or poll Flipt API
- **Payload:** flag key, old/new state, namespace, timestamp
- **Value:** "Flag `new-checkout-flow` enabled 2m before p95 spike" — invisible to deploy detection
- **Lines:** ~60
- **Why Flipt:** OSS, self-hosted, OpenFeature-compatible. LaunchDarkly is SaaS-only.

### Infra Changes (Terraform/Pulumi Cloud)
- **How:** Webhook on `apply` completion
- **Payload:** stack, resources changed, drift detected
- **Value:** "RDS instance class changed from db.t3.medium to db.t3.small" → latency spike
- **Lines:** ~40
- **Audience:** more relevant for teams than solo vibe coders

---

## Tier 3 — Operational Knowledge (runbooks + history)

### Atuin Shell History
- **How:** Atuin export or sync API → periodic import into sigiro_events
- **Payload:** command, exit code, duration, hostname, timestamp
- **Value:** "User ran `npm run deploy` at 14:30" correlates with "errors started 14:32"
- **Lines:** ~50 (import script or CLI command)
- **Privacy:** opt-in only, filter to deploy/restart/migration commands

### Runme Executable Runbooks
- **How:** sigiro [[architecture/investigation.md|diagnose]] generates markdown with executable code blocks
- **Payload:** Runme-compatible `{lang=bash}` fenced blocks
- **Value:** AI finds the problem → generates the fix → user runs it in terminal. Zero ops knowledge needed.
- **Lines:** ~30 (output formatter in [[architecture/investigation.md|investigate]].rs)
- **Direction:** outbound (sigiro produces runbooks), not inbound

### Backstage Service Catalog
- **How:** Read Backstage catalog-info.yaml or API
- **Payload:** service ownership, team, dependencies, docs links, lifecycle stage
- **Value:** Enriches [[architecture/investigation.md|investigate]] context — "owned by team-payments, depends on stripe-api"
- **Lines:** ~60 (catalog reader + context enrichment)

---

## Tier 4 — IDE as Runbook

### Cursor / Copilot / Claude Code Context
- **How:** Agents call `sigiro diagnose` or POST to `/v1/diagnose`
- **Value:** [[architecture/investigation.md|investigate]] returns structured evidence → AI agent in IDE reads it → suggests code fix → user accepts. The runbook IS the diff.
- **Lines:** 0 for CLI/HTTP usage — already implemented
- **Enhancement:** return `suggested_fix` field with code-level hints when error patterns match known fixes

---

## Priority for Vibe Coders

| Priority | Integration | Why |
|----------|------------|-----|
| P0 | Vercel + Railway deploy hooks | where vibe coders deploy, #1 incident cause |
| P0 | Runme output format | zero-ops-knowledge fix path |
| P1 | Feature flag changes (Flipt) | invisible cause of incidents |
| P1 | IDE suggested_fix field | closes the loop: detect → diagnose → fix |
| P2 | GitHub Deployments API | source of truth for all platforms |
| P2 | Atuin shell history | "what did I run?" correlation |
| P3 | Backstage catalog | relevant when teams > 1 person |
| P3 | Terraform/Pulumi | relevant for infra-heavy setups |

## Implementation Order

1. Add `sigiro_events` table to schema.rs
2. Add `POST /v1/events` generic webhook endpoint
3. Vercel deploy hook handler (subsumes [[plans/context-integrations.md|d5-deploy-corr]].md)
4. Railway deploy hook handler
5. Runme-compatible output in [[architecture/investigation.md|InvestigationBlock]]
6. Feature flag webhook handler
7. Everything else as demand appears

## Estimated Total

~300 lines for Tier 1+2. Table + generic webhook + 4 source-specific handlers.
Tier 3-4 are incremental and can ship independently.
