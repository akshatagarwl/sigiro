---
title: Cost Analysis
created: 2026-03-01
updated: 2026-05-12
type: summary
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'deployment']
---

# Cost Analysis

Sigiro is **free to use** — there is no billing system. Your only cost is the infrastructure you run it on.

This doc breaks down what self-hosting actually costs, compares against named competitors with their actual pricing, and gives clear monthly totals at three scales.

---

## 1. Your Cost to Self-Host

**Architecture:** Single Rust binary embedding DuckDB. OTLP received directly on ports 4317 (gRPC) and 4318 (HTTP) — no OTel Collector in the path. Per-signal tables (8 tables: spans, logs, log_templates, metrics_gauge, metrics_sum, metrics_histogram, metrics_exp_histogram, [[architecture/profiles.md|profiles]]). [[architecture/ducklake-integration.md|DuckLake]] storage with SQLite catalog + [[architecture/data-model.md|Parquet]] data files. Zero config — set `SIGIRO_S3_BUCKET` (+ `SIGIRO_S3_ENDPOINT`/credentials) to write data files to S3/R2. Retention is currently a time-based DELETE sweep; automatic compaction and physical storage reclamation (`ducklake_merge_adjacent_files`, `ducklake_cleanup_old_files`, snapshot expiry) are planned via a maintenance role, not yet enabled.

- **Local** (default) — SQLite catalog + local [[architecture/data-model.md|Parquet]]. Zero cost.
- **S3** — Same SQLite catalog, data files on S3. Only pay for S3 storage + requests. No PostgreSQL needed for single-writer setups.

**Compression:** 8-12x with zstd level 7 + delta encoding on timestamps + dictionary encoding on low-cardinality columns. Measured 10.5x on synthetic observability data. We use 10x as the working estimate below. Decompression cost hides behind LLM inference time.

### Storage Provider Comparison

All providers below are S3-compatible — [[architecture/data-model.md|DuckDB]]'s `read_parquet('s3://...')` and the AWS SDK work with all of them. Prices as of early 2026.

| Component | AWS S3 | Cloudflare R2 | Backblaze B2 | Wasabi | Tigris |
|---|---|---|---|---|---|
| **Storage** | $0.023/GB | $0.015/GB | $0.006/GB | $0.007/GB | $0.02/GB |
| **PUT requests** | $5.00/M | $4.50/M | **Free** | **Free** | $5.00/M |
| **GET requests** | $0.40/M | $0.36/M | $4.00/M (2,500/day free) | **Free** | $0.50/M |
| **Egress** | $0.09/GB | **Free** | Free (up to 3× storage) | Free (1:1 ratio) | **Free** |
| **Min storage** | None | None | None | 1 TB min charge | None |
| **Min duration** | None | None | None | 90 days | None |

#### Caveats

- **Cloudflare R2**: Zero egress is the killer feature. S3-compatible, auto-placed across Cloudflare network (no explicit region selection). PUTs still cost money ($4.50/M) — our high PUT volume means this matters.
- **Backblaze B2**: Cheapest storage ($0.006/GB), free PUTs, free egress up to 3× stored amount. Free CDN egress via Cloudflare Bandwidth Alliance. GETs cost more than S3 ($4/M vs $0.40/M) but our GET volume is modest.
- **Wasabi**: Cheapest all-in at scale (flat storage, everything else free). But: 1 TB minimum storage charge ($6.99/mo even at 1 GB), 90-day minimum storage duration (deleting early still bills), and egress limited to 1:1 storage ratio (store 1 TB, download max 1 TB/month free). Exceeding the egress ratio → Wasabi contacts you.
- **Tigris**: S3-compatible, zero egress, global distribution. Same PUT pricing as S3 so no savings there. Storage only slightly cheaper.

### Cost at Each Scale — By Provider

All calculations use 10x compression, 30-day retention, same workload assumptions.

#### Medium — 10 Services, 100 GB/day Ingest (300 GB compressed stored)

| Component | AWS S3 | Cloudflare R2 | Backblaze B2 | Wasabi |
|---|---|---|---|---|
| Storage | 300 GB × $0.023 = **$6.90** | 300 GB × $0.015 = **$4.50** | 300 GB × $0.006 = **$1.80** | 300 GB × $0.007 = **$2.10** |
| PUTs | 1.73M × $5/M = **$8.64** | 1.73M × $4.50/M = **$7.78** | **$0.00** | **$0.00** |
| GETs | 300K × $0.40/M = **$0.12** | 300K × $0.36/M = **$0.11** | 225K × $4/M = **$0.90** | **$0.00** |
| Egress | 30 GB × $0.09 = **$2.70** | **$0.00** | **$0.00** (within 3× limit) | **$0.00** |
| **Total** | **$18.36** | **$12.39** | **$2.70** | **$2.10** |
| **Effective $/GB** | **$0.006** | **$0.004** | **$0.0009** | **$0.0007** |

#### Large — 50 Services, 1 TB/day Ingest (3 TB compressed stored)

| Component | AWS S3 | Cloudflare R2 | Backblaze B2 | Wasabi |
|---|---|---|---|---|
| Storage | 3,000 GB × $0.023 = **$69.00** | 3,000 GB × $0.015 = **$45.00** | 3,000 GB × $0.006 = **$18.00** | 3,000 GB × $0.007 = **$21.00** |
| PUTs | 8.64M × $5/M = **$43.20** | 8.64M × $4.50/M = **$38.88** | **$0.00** | **$0.00** |
| GETs | 3M × $0.40/M = **$1.20** | 3M × $0.36/M = **$1.08** | 2.93M × $4/M = **$11.70** | **$0.00** |
| Egress | 300 GB × $0.09 = **$27.00** | **$0.00** | **$0.00** (within 3× limit) | **$0.00** |
| **Total** | **$140.40** | **$84.96** | **$29.70** | **$21.00** |
| **Effective $/GB** | **$0.005** | **$0.003** | **$0.001** | **$0.0007** |

### Recommendation: Backblaze B2 default, customer-choice S3

**B2 as the default:**
- 85% cheaper than S3 thanks to free PUTs (our largest non-storage cost)
- Cheapest storage at $0.006/GB
- Free egress via Cloudflare Bandwidth Alliance
- S3-compatible — [[architecture/data-model.md|DuckDB]], AWS SDK, all existing code works unchanged
- Best for write-heavy workloads (observability is write-heavy by nature)
- B2 GET pricing ($4/M) is higher but our query volume is modest (agent makes 1-2 SQL round trips per incident)

**Cloudflare R2 as alternative:**
- Zero egress, no minimum storage or duration
- 33-40% cheaper than S3 but more expensive than B2 due to PUT costs ($4.50/M)

**AWS S3 as opt-in for customers who require it:**
- Data residency requirements (specific AWS regions)
- Existing AWS commitments / Enterprise Discount Programs
- VPC endpoints for private connectivity

**Why not Wasabi:** Cheapest on paper, but the 1 TB minimum charge ($6.99/mo even at 1 GB stored), 90-day minimum storage duration, and 1:1 egress ratio make it operationally risky. The egress ratio means a spike in agent queries could trigger manual intervention from Wasabi.

### Cost Components (AWS S3 baseline)

For reference, the AWS S3 component pricing used in competitor comparisons:

| Component | Unit Price | Notes |
|---|---|---|
| S3 storage | $0.023/GB/month | Standard tier, first 50 TB |
| S3 PUT requests | $0.005/1,000 requests | One PUT per flush per node |
| S3 GET requests | $0.0004/1,000 requests | DuckDB range reads for queries |
| S3 data transfer in | $0.00/GB | Ingest is free |
| S3 data transfer out | $0.09/GB | Query egress (minimized by range reads) |
| Compute (collector) | $0.00 incremental | DaemonSet on existing K8s nodes, ~100-200 MB RAM per node |
| Compute (query server) | $0.00 incremental | DuckDB runs in a single pod, minimal CPU at POC scale |

### Key Insight: PUTs Dominate at Scale

At medium scale (100 GB/day) on S3, PUTs cost **$2.16/mo** (10 nodes × 1 PUT/min × 4 signals = 1.73M PUTs/month at 60s flush). Lever to reduce further:

1. **Switch to R2/B2** — R2 PUTs are 10% cheaper, B2 PUTs are free

---

## 2. Competitor Pricing — What They Charge

We don't consider any of the existing observability vendors as direct competitors. What we're building — an agent-native, unified [[architecture/data-model.md|Parquet]] table with no dashboards, no indexing, and no pre-aggregation — is so fundamentally different from the incumbent model that it occupies a different category entirely. Every vendor below, from Datadog to the cheapest open-source alternative, is building some variation of "store telemetry, render dashboards, let humans query." We're building infrastructure for AI agents to [[architecture/investigation.md|investigate]] incidents autonomously.

That said, customers still compare on cost, and the cost story is worth telling. Here's how the incumbents price.

Prices sourced from public pricing pages as of early 2026.

### Enterprise Tier ($15-30/GB effective)

#### Datadog

| Component | Price |
|---|---|
| Infrastructure monitoring | $15/host/month (Pro), $23/host/month (Enterprise) |
| APM | $31/host/month + $0.10/GB span ingest |
| APM indexed spans | $1.70/GB |
| Log management ingest | $0.10/GB |
| Log management indexed | $1.70/million events (15-day retention) |
| Custom metrics | $0.05 per custom metric/month |
| Log retention beyond 15 days | $0.06/GB/month (archive) |

**Typical all-in cost:** $15-30/GB effective. Per-host fees dominate at low volumes; per-GB ingest fees dominate at high volumes.

### Mid-Tier ($0.50-3/GB effective)

#### Grafana Cloud

| Component | Price |
|---|---|
| Logs | $0.50/GB ingest |
| Traces | $0.50/GB ingest |
| Profiles | $0.50/GB ingest |
| Metrics | $8/1,000 active series/month |
| Retention | 30 days included, additional at extra cost |

**Typical all-in cost:** $0.50/GB for logs/traces/[[architecture/profiles.md|profiles]]. Metrics add $80/month per 10K active series. Blended: $1-3/GB.

#### New Relic

| Component | Price |
|---|---|
| Data ingest | $0.30/GB (Standard), $0.50/GB (Pro/Enterprise) |
| Full platform users | $49/user/month (Standard), $349/user/month (Pro) |
| Core users | $49/user/month |
| Free tier | 100 GB/month included |

**Typical all-in cost:** $0.50-2.00/GB for data + per-user tax ($1,745/month for 5 Pro users before any data).

### Mid-Budget Tier ($0.15-0.25/GB)

#### Better Stack

| Component | Price |
|---|---|
| Logs ingest | $0.15/GB |
| Traces ingest | $0.15/GB |
| Logs/Traces retention | $0.08/GB/month |
| Metrics retention | $0.75/GB/month |
| Telemetry bundles | $30-500/month (Nano 40 GB each to Tera 700 GB each) |
| Error tracking | 100K/month free, then $0.000075/exception |
| Session replay | 5,000 free, then $0.00225/replay |
| Responder license | $34/month (for incident management) |
| Per-user | $0 for telemetry access |

**Source:** [betterstack.com/pricing](https://betterstack.com/pricing)

**Typical all-in cost:** $0.15-0.25/GB effective. Bundles reduce per-GB cost at committed volumes. The Tera bundle ($500/mo) includes 700 GB each of logs, traces, and metrics — effective $0.24/GB at 2,100 GB. Overage at $0.15/GB ingest + $0.08/GB retention. Metrics retention at $0.75/GB is a gotcha for metrics-heavy workloads. At 100 GB/day (mixed signals): ~$730/month using Tera bundle + overage.

**Architecture:** ClickHouse-backed SaaS. $18.6M Series A. Positions as "AI SRE observability stack" — 30x cheaper than Datadog. Full product suite: logs, metrics, traces, uptime monitoring, incident management, [[plans/STATUS.md|status]] pages, error tracking, session replay, AI root cause analysis. OTel-native, PromQL support, Sentry SDK compatible. No profiling.

### Budget Tier ($0.30-0.50/GB)

These are the "cheap Datadog alternatives" — the closest price-point comparison, even though they're solving a different problem.

#### SigNoz Cloud

| Component | Price |
|---|---|
| Logs | $0.30/GB ingest |
| Traces | $0.30/GB ingest |
| Metrics | $0.10/million samples |
| Base fee | $49/month (includes $49 usage credit) |
| Retention | 15 days (logs/traces), 30 days (metrics) |
| Per-host / per-user | $0 |

**Source:** [signoz.io/pricing](https://signoz.io/pricing/)

**Typical all-in cost:** $0.30/GB. At 100 GB/day: ~$900/month. ClickHouse-backed, self-hosted option available free.

#### OpenObserve Cloud

| Component | Price |
|---|---|
| Logs/Metrics/Traces ingest | $0.30-0.50/GB (pricing updated June 2025) |
| Query | $0.01/GB scanned |
| Pipelines | $0.20/GB processed |
| Retention | 15 days included, +$0.10 per 30 days |
| Per-host / per-user | $0 |

**Source:** [openobserve.ai/pricing](https://openobserve.ai/pricing/)

**Typical all-in cost:** $0.30/GB ingest + query costs. Claims 90% savings vs Datadog. S3-backed, self-hosted option available free (200 GB/day).

#### Parseable Cloud

| Component | Price |
|---|---|
| Logs/Metrics/Traces ingest | $0.37-0.39/GB |
| Query scans | Included up to 10x ingested volume, then $0.02/GB |
| Base fee | $29/month minimum |
| Retention | 21-30 days (Standard), 12 months (Pro) |
| Per-host / per-user | $0 |

**Source:** [parseable.com/pricing](https://www.parseable.com/pricing)

**Typical all-in cost:** $0.37/GB. [[architecture/data-model.md|Parquet]]-on-S3 architecture (closest to ours). Self-hosted cost is raw S3 rate (~$0.023/GB).

#### HyperDX Cloud

| Component | Price |
|---|---|
| Base plan | $20/month (includes 50 GB) |
| Overage | $0.40/GB |
| Retention | 30 days |
| Per-host / per-user | $0 (unlimited users) |

**Source:** [hyperdx.io/pricing](https://www.hyperdx.io/pricing)

**Typical all-in cost:** $0.40/GB after base allowance. ClickHouse-backed.

#### Middleware

| Component | Price |
|---|---|
| Metrics/Logs/Traces ingest | $0.30/GB |
| OpsAI (root cause) | $1/error solved (detection free) |
| RUM sessions | $1/1K sessions |
| Synthetic checks | $1/5K checks |
| Free tier | 100 GB/month, 14-day retention |
| Retention | 30 days (Pay As You Go) |
| Per-host / per-user | $0 (unlimited users) |

**Source:** [middleware.io/pricing](https://middleware.io/pricing/)

**Typical all-in cost:** $0.30/GB for telemetry data. AI-powered root cause analysis adds $1/error on top. Free tier generous at 100 GB/month but only 14-day retention.

#### Gigapipe (qryn)

| Component | Price |
|---|---|
| Startup tier | EUR 275/mo (600 GB NVMe) |
| Scale Up tier | EUR 550/mo (~1 TB NVMe) |
| Enterprise tier | EUR 1,100/mo (~1.5 TB NVMe) |
| Extreme tier | EUR 2,150/mo (~2.5 TB NVMe) |
| Supreme tier | EUR 3,250/mo (3.5 TB NVMe) |
| Self-hosted | Free (AGPLv3) |
| Per-GB / per-host / per-user | $0 (flat-rate tiers) |

**Source:** [gigapipe.com](https://gigapipe.com/)

**Typical all-in cost:** Flat-rate tiers, no per-GB charge — but NVMe storage is fixed. At 100 GB/day ingest, the Scale Up tier (1 TB NVMe) fills in ~10 days, triggering automatic data rotation. Effective per-GB cost: ~$0.18-0.37/GB depending on tier and ingest rate. Retention is implicitly limited by NVMe capacity — ingest faster than storage allows and old data gets rotated out.

**Architecture:** ClickHouse-backed (primary), with [[architecture/data-model.md|DuckDB]]/S3 alternative. AGPLv3. Drop-in API compatibility with Loki (LogQL), Prometheus (PromQL), Tempo (TraceQL), [[architecture/profiles.md|Pyroscope]]. Also ingests Datadog, InfluxDB, Elastic, Zipkin, OTLP. Native [[designs/future/perses-compat.md|Grafana]] datasource — existing dashboards work unchanged.

#### ClickStack (Managed ClickHouse Cloud)

| Component | Price |
|---|---|
| Storage retention | < $0.03/GB/month (compressed) |
| Ingest compute | < $0.01/GB/month |
| Query compute | Pay-per-use (independently scalable) |
| Per-host / per-user | $0 |

**Source:** [clickhouse.com/blog/introducing-managed-clickstack-beta](https://clickhouse.com/blog/introducing-managed-clickstack-beta)

**Typical all-in cost:** Three-axis pricing (storage + ingest + query compute). With 10-15x compression: 100 GB/day × 30 days = 3 TB raw → ~200-300 GB compressed × $0.03 storage = $6-9/mo + $30/mo ingest + variable query. Estimated $120-180/mo at 100 GB/day. This is ClickHouse Inc. going direct — same engine behind SigNoz, HyperDX, Gigapipe, now offered managed with HyperDX as the UI layer.

**Architecture:** ClickHouse Cloud with separation of storage and compute. Object storage primary with distributed caching. OTel-native ingest. Logs, metrics, traces, session replays (no [[architecture/profiles.md|profiles]]). Beta as of Feb 2026.

### Budget Tier Summary

| Competitor | Per-GB | Base Fee | Retention | Backend |
|---|---|---|---|---|
| ClickStack | ~$0.04-0.06 (est.) | $0 | Unlimited | ClickHouse Cloud |
| Better Stack | $0.15-0.25 | $0 (free tier) | 30 days | ClickHouse |
| Gigapipe | ~$0.18-0.37 | EUR 275/mo min | NVMe-bound (days-weeks) | ClickHouse |
| SigNoz | $0.30 | $49/mo | 15 days | ClickHouse |
| OpenObserve | $0.30-0.50 | Free tier | 15 days | S3 + custom |
| Middleware | $0.30 | Free tier (100 GB) | 30 days | — |
| Parseable | $0.37-0.39 | $29/mo | 21-30 days | Parquet on S3 |
| HyperDX | $0.40 | $20/mo (50 GB incl) | 30 days | ClickHouse |
| **sigiro (self-hosted)** | **$0** | **$0** | **30 days** | **Parquet on B2** |

sigiro self-hosted infrastructure cost at 100 GB/day: ~$3/month (B2 storage only). That's **300-400x cheaper** than the budget tier and **600-1,400x cheaper** than enterprise.

---

## 3. Head-to-Head Comparison

Monthly cost at each scale. Competitors assume equivalent signal coverage (metrics + logs + traces + APM).

### Full Comparison — All Tiers

| Scale | sigiro (self-hosted) | ClickStack | Better Stack | Gigapipe | SigNoz | OpenObserve | Middleware | Parseable | HyperDX | Grafana Cloud | New Relic | Datadog |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **10 GB/day** | **~$0.30** | **~$15** | **~$71** | **$300** | **$139** | **$139** | **$90** | **$140** | **$140** | **$230** | **$1,840** | **$620** |
| **100 GB/day** | **~$3** | **~$150** | **~$730** | **$600** | **$949** | **$949** | **$900** | **$1,139** | **$1,020** | **$1,800** | **$3,245** | **$4,300** |
| **1 TB/day** | **~$30** | **~$1,500** | **~$6,900** | **$3,550** | **$9,049** | **$9,049** | **$9,000** | **$11,129** | **$12,020** | **$16,000** | **$11,745** | **$30,500** |

sigiro self-hosted price = your S3 storage only. At 100 GB/day with B2: ~$3/month.

### Budget Tier Breakdown (Medium — 100 GB/day)

**Better Stack:** Tera bundle $500 (700 GB each signal) + ~800 GB log overage × $0.23 + ~200 GB trace overage × $0.23 = ~$730/month
**SigNoz:** 100 GB/day × 30 × $0.30 = $900 + $49 base = $949/month
**[[research/observe-vs-openobserve.md|OpenObserve]]:** 100 GB/day × 30 × $0.30 = $900 + query costs ~$49 = $949/month
**Middleware:** 100 GB/day × 30 × $0.30 = $900/month (100 GB free tier absorbed)
**Parseable:** 100 GB/day × 30 × $0.37 = $1,110 + $29 base = $1,139/month
**HyperDX:** 50 GB included in $20, then 2,950 GB × $0.40 overage = $1,000 + $20 = $1,020/month
**sigiro self-hosted:** B2 storage only = **~$3/month**

### Enterprise Tier Breakdown (Medium — 10 hosts, 100 GB/day)

**Datadog:**
- Infra: 10 hosts × $23 = $230
- APM: 10 hosts × $31 = $310 + 30 GB spans × $0.10 = $3 + indexed $1.70 × 5 GB = $8.50
- Logs: 70 GB × $0.10 = $7 ingest + 70 GB indexed events = ~$3,500 (at $1.70/M events, ~2M events/GB)
- Custom metrics: ~2,000 metrics × $0.05 = $100
- Total: ~$4,300/month

**[[designs/future/perses-compat.md|Grafana]] Cloud:**
- Logs: 50 GB/day × 30 × $0.50 = $750
- Traces: 30 GB/day × 30 × $0.50 = $450
- [[architecture/profiles.md|Profiles]]: 5 GB/day × 30 × $0.50 = $75
- Metrics: 50K active series × $8/1K = $400
- Retention upgrades: ~$125
- Total: ~$1,800/month

**New Relic:**
- Data: 100 GB/day × 30 = 3,000 GB - 100 GB free = 2,900 GB × $0.50 = $1,450
- Users: 5 Pro users × $349 = $1,745
- Total: ~$3,245/month

---

## 4. Why the Gap Exists

**No indexing.** Traditional backends build inverted indices on ingest — Elasticsearch, Loki's chunk index, Tempo's tag index. We use [[architecture/data-model.md|Parquet]] column statistics and bloom filters (on trace_id and message). Index storage and compute is zero.

**No pre-aggregation.** Pre-aggregation is the most expensive operation in observability pipelines. Prometheus compaction, Datadog's rollups, [[designs/future/perses-compat.md|Grafana]] Mimir's compactor — all burn CPU to downsample data on write. We store raw data and compute on read. The query cost is higher per-query, but queries are rare (agent makes 1-2 SQL round trips per incident) while ingest is continuous.

**No multi-backend.** A typical observability stack runs 4 separate backends: Prometheus (metrics), Loki (logs), Tempo (traces), [[architecture/profiles.md|Pyroscope]] (profiles). Each has its own storage, replication, and retention config. We write per-signal Parquet tables to one S3 bucket via [[architecture/ducklake-integration.md|DuckLake]]. One [[architecture/write-path.md|write path]] (direct OTLP → DuckDB, no collector), one storage tier, one catalog.

**Compression economics flip.** Traditional backends compress 2-3x because humans need sub-100ms dashboard refresh, so decompression must be fast. Our primary reader is an LLM agent with 500ms-1.3s acceptable latency. zstd level 7 + delta encoding on sorted timestamps + dictionary encoding gives us 8-12x compression (measured 10.5x) because decompression hides behind inference time.

---

## 5. What You Give Up

Honest [[architecture/tradeoffs.md|tradeoffs]] — this architecture is not free lunch:

- **No dashboards.** There is no [[designs/future/perses-compat.md|Grafana]], no Datadog dashboard builder. The agent reads and interprets data. Humans get structured [[architecture/investigation.md|investigation]] blocks (metrics time series, span trees, top [[architecture/profiles.md|profiles]], linked traces), not drag-and-drop charts.

- **Higher query latency.** 500ms-1.3s per query with bloom filters, vs 40ms dashboard refresh from pre-aggregated data. Acceptable for agent-driven [[architecture/investigation.md|investigation]]. Not acceptable for a wall-mounted NOC screen.

- **No managed alerting UI.** No PagerDuty-style rule builder. Edge alerts via webhook, configured in code. The agent can react to alerts but there's no point-and-click alert management.

- **You run it.** Self-hosted means you provision the machine, manage the binary, and handle upgrades. We provide the binary and docs; you operate it.
