---
title: Reading List
created: 2026-03-05
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# Reading List

Articles and products that should inform Sigiro's direction. Ordered by importance.

---

## 1. loggingsucks.com — [https://loggingsucks.com/](https://loggingsucks.com/)

**Why important:** Clearest articulation of the problem Sigiro exists to solve.

**Thesis:** Traditional logging is designed for writing, not querying. At scale (130K+ lines/sec in distributed systems), text-search logs become noise, not signal. The solution is **wide events**: one structured JSON event per request per service, with 50+ fields — user context, business metrics, error details, feature flags — so you can ask "all checkout failures for premium users where the new checkout flow was enabled."

**Key practices:**
- Tail sampling: always keep errors and slow requests, randomly sample healthy traffic
- High cardinality + high dimensionality > high volume
- Structured JSON is table stakes; unstructured text is dead

**Relevance to Sigiro:** Our unified table and `metadata string` (JSON attributes) column is the storage backend for this pattern. The `wide event` is exactly a `UnifiedRow` with `signal=span` and rich `metadata`. The gap: our query API doesn't yet let users slice by arbitrary metadata fields.

---

## 2. nominal.dev — [https://nominal.dev/](https://nominal.dev/)

**Why important:** Most direct product vision competitor/complement. Built by Boris Tane (Cloudflare observability).

**Thesis:** "Nobody should be on-call in 2026." AI agents should autonomously resolve production incidents before humans wake up.

**What it does:** AI agents analyze source code + infrastructure state + telemetry to auto-remediate issues. Integrates with AWS, Cloudflare, Vercel, [[company/gtm.md|Fly.io]], Render, Railway, Supabase, PlanetScale, Modal. Private beta, April 2026.

**Relevance to Sigiro:** This is what sits on top of Sigiro. Nominal needs a fast, cheap telemetry query layer — that's us. If nominal (or an equivalent) integrates with Sigiro via `/v1/query`, they get the [[architecture/investigation.md|investigation]] context; we get the data layer. Or: Sigiro builds the agent query layer and nominal is a future competitor in the agent tier.

---

## 3. "The Software Development Lifecycle Is Dead" — [https://boristane.com/blog/the-software-development-lifecycle-is-dead/](https://boristane.com/blog/the-software-development-lifecycle-is-dead/)

**Author:** Boris Tane (same founder as nominal.dev)

**Thesis:** AI agents have collapsed the SDLC from sequential phases into a single tight loop: intent → agent execution → observation → iteration. The new bottleneck is not development speed — it's **context quality** and **production visibility**.

**Key claims:**
- Requirements, design, implementation, testing, review, deployment are merging into one loop
- PR reviews become bottlenecks to eliminate, not ceremonies to maintain
- Observability is no longer retrospective monitoring — it is "the connective tissue driving continuous agent-driven improvements"
- **Context engineering** (giving agents the right production context) is the new critical skill

**Relevance to Sigiro:** Directly validates our agent-first thesis. If observability is connective tissue for AI agents, the query layer must be designed for agents, not humans. `/v1/query` returning a structured `InvestigationBlock` in one call is the right primitive. The doc at `<domain>/agents.txt` is the right interface.

---

## 4. "All You Need Is Wide Events, Not Metrics" — [https://isburmistrov.substack.com/p/all-you-need-is-wide-events-not-metrics](https://isburmistrov.substack.com/p/all-you-need-is-wide-events-not-metrics)

**Author:** Ivan Burmistrov (ex-Meta)

**Thesis:** The three pillars (metrics, logs, traces) are an artificial distinction that makes observability seem harder than it is. Everything is a wide event. Traces are events with TraceId/SpanId. Metrics are periodic snapshot events. Logs are events with a message field.

**Proof point:** Meta's internal Scuba system — one event store, intuitive "slice and dice" exploration, no pre-aggregation, finds unknown unknowns through exploration rather than dashboard-watching.

**What he praises:** Honeycomb, Axiom — both moving toward unified event storage.

**Relevance to Sigiro:** This is the theoretical foundation for per-signal [[architecture/data-model.md|Parquet]] tables. We store each signal in its own table sorted by `(service, timestamp)`, with metrics split into four per-type tables for zero-copy ingest. The insight still holds at the query layer: `sigiro_investigate` UNION ALLs across tables so the agent sees one cross-signal view — "there is only one data type: events" — even though storage is per-signal for efficiency.

---

## 5. Charity Majors (@mipsytipsy) on Observability 2.0

**Tweet:** [https://x.com/mipsytipsy/[[plans/STATUS.md|status]]/1738048200630792245](https://x.com/mipsytipsy/status/1738048200630792245) (Dec 2023 — requires login to view)

**Background reading:** [charity.wtf/tag/observability-2-0/](https://charity.wtf/tag/observability-2-0/)

**Thesis:** Observability 2.0 has exactly one technical distinction from 1.0: **store your data once, in one format — arbitrarily-wide structured events** — instead of scattering telemetry across separate siloed systems (Prometheus for metrics, Loki for logs, Jaeger for traces).

**Key claims:**
- Multi-pillar architectures force data duplication → unsustainable [[company/cost.md|cost]] multipliers
- Read-time aggregation > write-time decisions (no pre-aggregation)
- "Pillars" is marketing language; "signals" is the correct technical term
- O11y 2.0 = unified storage. O11y 1.0 = multiple databases.

**Relevance to Sigiro:** We are an Observability 2.0 system by this definition. One [[architecture/data-model.md|Parquet]] table, all signals, read-time [[architecture/data-model.md|DuckDB]] aggregation. This is the framing we should use in positioning. The tweet (Dec 2023) likely discusses the cost crisis — multi-pillar duplication is why [[designs/future/perses-compat.md|Grafana]] Cloud costs $1,800/month for mid-size teams.

---

## 6. Axiom — [https://axiom.co/](https://axiom.co/)

**Why important:** Closest SaaS competitor to Sigiro's storage/query model.

**What it does:** Event data platform for logs, traces, metrics at petabyte scale. Schema-less ingestion, no sampling, long retention, usage-based [[designs/pricing.md|pricing]].

**Differentiators (theirs):**
- "Zero sampling" — captures everything
- Long retention without prohibitive [[company/cost.md|cost]]
- Also targeting AI engineering (LLM tracing, prompt eval)
- 40K+ orgs including Vercel, Asana, Netlify, Cal.com

**[[designs/pricing.md|Pricing]]:** Usage-based with free tier. No published per-GB number but positioned as "affordable" vs Datadog.

**Relevance to Sigiro:** Axiom is the SaaS version of what Sigiro does for storage+query. Key differences: (1) Axiom is a managed SaaS, Sigiro is self-hosted with your own S3; (2) Axiom has a UI, Sigiro is API-first/agent-native; (3) Axiom's [[designs/pricing.md|pricing]] is opaque, Sigiro is $0.005/GB transparent. Axiom targets power users who want a UI; Sigiro targets teams going agent-first.

---

## 7. happycontext — [https://github.com/happytoolin/happycontext](https://github.com/happytoolin/happycontext)

**What it is:** Go library implementing the wide-event pattern for HTTP services. Middleware captures one structured JSON event per request, merging all `hc.Add()` fields accumulated during the request lifecycle. Smart sampling: always keep errors, randomly sample healthy traffic.

**Framework support:** net/http, Gin, Echo, Fiber. Logger support: slog, Zap, Zerolog.

**Relevance to Sigiro:** This is the client-side complement to Sigiro. If someone uses happycontext + OTel SDK on their Go service, their wide events land in our collector as spans with rich metadata. Could reference this in setup guides as the recommended logging pattern for Go services sending to Sigiro.

---

## 8. ClickHouse Full-Text Search GA — [https://clickhouse.com/blog/full-text-search-ga-release](https://clickhouse.com/blog/full-text-search-ga-release)

**Why evaluated:** Inverted indexes on text columns achieved 0.4s vs 193s full scan on 10B rows. Could this accelerate agent log search?

**Verdict:** Not applicable. Sigiro's [[architecture/data-model.md|Parquet]]/[[architecture/data-model.md|DuckDB]] architecture has no persistent server to maintain inverted indexes, and agent queries already filter to narrow windows where message scanning is instant. The actionable takeaway: add a `Keywords` filter to `InvestigateRequest` (~10 lines of Go, zero infrastructure change).

**Full analysis:** [[[research/clickhouse-fts-evaluation.md|clickhouse-fts-evaluation]].md](clickhouse-fts-evaluation.md)

---

## 9. Siddhant Khare: The Agent Observability Gap — [siddhantkhare.com/writing/agent-observability-gap](https://siddhantkhare.com/writing/agent-observability-gap)

**Why important:** Identifies four gaps in agent observability: what agents do, why they chose it, what it [[company/cost.md|cost]], and what permissions they used. Maps directly to OTel GenAI semantic conventions.

**Relevance to Sigiro:** Prompted the convergence research (`agent-telemetry-convergence.md`). The key insight: these gaps are already being solved by OpenLLMetry/OpenLIT emitting `gen_ai.*` spans as standard [[architecture/write-path.md|OTLP]]. That telemetry flows through our `mapper.go:mapTraces()` into the unified table with zero code changes. Sigiro's differentiator isn't "we also do agent observability" — it's that agent reasoning and service telemetry live in the same table, queryable in the same SQL.

---

## 10. OTel GenAI Semantic Conventions — [opentelemetry.io/docs/specs/semconv/gen-ai/](https://opentelemetry.io/docs/specs/semconv/gen-ai/)

**[[plans/STATUS.md|Status]]:** Development (not stable yet)

**What it defines:** Standardized span/metric attributes for LLM and agent operations: `gen_ai.operation.name`, `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.tool.name`, `gen_ai.output.messages`.

**Agent spans extension:** [opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/)

**Relevance to Sigiro:** These attributes already land in our metadata JSON blob via `attrsToMap()` + `mergeMetadata()`. The query layer just needs to know how to surface them. Development-[[plans/STATUS.md|status]] means attribute names may change, but our schemaless JSON metadata absorbs renames — only queries break, not storage.

---

## 11. Edera: "What Every Cloud Native Engineer Needs to Know About Kernel Memory" — [https://edera.dev/stories/what-every-cloud-native-engineer-needs-to-know-about-kernel-memory](https://edera.dev/stories/what-every-cloud-native-engineer-needs-to-know-about-kernel-memory)

**Author:** Jed Salazar (Edera, Mar 2026). Companion to Cloud Native Rejekts talk "Your AI Agent Has a Kernel Problem."

**Thesis:** Container memory metrics (cgroups `memory.current`) are Plato's cave shadows. The kernel tracks MemAvailable (real headroom), page faults (major = disk thrash), PSI (pressure stall), context switches — none of which exist per-cgroup. On shared kernels, `/proc/meminfo` shows node-wide totals useless for per-workload insight. AI agents break the old model: they're stateful, long-running, accumulate irreplaceable context, and their memory patterns (discrete growth per tool call) look pathological to cgroups but healthy to the kernel. An OOMKill costs 35 minutes of compute, not one HTTP request.

**Edera's solution:** Dedicated per-workload kernels (microVMs/zones) so `/proc` becomes per-workload signal. Security + observability converge on same architecture.

**Key demo insight:** They correlated MemAvailable decline with discrete agent tool calls — could distinguish "healthy stepwise context growth" from "smooth decline indicating a leak." Also showed page faults (major) near zero = everything fits in RAM, context switches ~400/s during file reads vs ~330/s during report writing. These are per-workload signals impossible on shared kernels.

**[[designs/future/ebpf-gpu-position.md|eBPF]] caveat:** eBPF reads kernel state but can't isolate it per-pod — MemAvailable, PSI, reclaim are global counters. eBPF complements dedicated kernels, doesn't replace them.

**Relevance to Sigiro:**
1. **Market validation.** Edera (KubeCon-tier company) saying "existing observability doesn't work for AI agents" is the exact thesis Sigiro is built on.
2. **Different layer, same problem.** Edera solves via infra (microVMs). Sigiro solves via the observability layer — works regardless of infra. Most agent users won't rearchitect to microVMs.
3. **The correlation insight is the steal.** Correlating system metrics (MemAvailable, page faults) with agent actions (tool calls, context growth) is the killer feature. Edera exposes the kernel metrics; Sigiro should own the correlation layer that maps "MemAvailable dropped 40MB" → "tool call #17 read src/ (12K tokens added to context)" → "3 more calls like this will OOM."
4. **Complementary.** If someone runs agents on Edera zones, Sigiro is still needed to tell them WHY the agent behaved a certain way. Edera = kernel health. Sigiro = agent health.
5. **On [[company/gtm.md|Fly.io]] (Sigiro's deploy target), single-purpose VMs already give you per-workload /proc.** The Edera problem (shared kernel noise) doesn't apply — Sigiro can read real kernel metrics today on Fly Machines.

---

## 12. Vivian Voss: "The Observability Tax" — [https://vivianvoss.net/blog/the-observability-tax](https://vivianvoss.net/blog/the-observability-tax)

**Author:** Vivian Voss (Performance-Fresser series, Episode 18, 22 Mar 2026)

**Thesis:** The observability industry turned `tail -f` into a seven-figure problem. A mid-sized company (100 engineers) spends $708K–$1.08M/year on observability. Vendors charge $0.10–$3.00/GB ingested vs $0.02/GB cloud storage — a 5x to 150x markup. Some orgs spend 70% of their infra budget just watching their infra. The monitoring stack itself needs monitoring (Prometheus watching → needs another Prometheus — "quis custodiet ipsos custodes, but with YAML").

**Key data points:**
- Prometheus at 1M time series: 3 GB RAM; scraping increases Kubelet CPU by 4x
- OTel Collector baseline: 2 CPU cores + 2 GB RAM for 20K spans/sec
- Sentry JS SDK: 53 KB per page load to observe page load speed (observer slows the observed)
- 97% of companies report unexpected observability [[company/cost.md|cost]] surprises; 67% regularly
- Evereve left Datadog → 90% [[company/cost.md|cost]] reduction
- Discover Financial migrated 2,200 services away from vendor
- Deductive AI had access revoked overnight, rebuilt on open-source in 48 hours

**Root cause argument:** Bootcamps teach [[designs/future/perses-compat.md|Grafana]] dashboards, not `vmstat`. Universities accept vendor sponsorships. "The customer base was not discovered. It was cultivated." Engineers who know pipes don't need dashboard subscriptions.

**Relevance to Sigiro:**
1. **Market narrative ammunition.** This is the most quotable articulation of the [[company/cost.md|cost]] crisis. The "watchtower costs more than the castle" framing is perfect for Sigiro's [[company/pitch.md|pitch]]. The specific dollar figures ($708K–$1.08M for 100 engineers) give Sigiro a concrete anchor: "what if that number was $500/year on B2 storage?"
2. **The overhead paradox validates our architecture.** Prometheus (3 GB), OTel Collector (2 cores + 2 GB), Sentry SDK (53 KB/page) — the observability stack itself is a performance problem. Sigiro's approach (lightweight collector → [[architecture/data-model.md|Parquet]] → [[architecture/data-model.md|DuckDB]] on query) has near-zero steady-state overhead because there's no always-on Prometheus scraping or heavy collector process.
3. **The vendor lock-in stories (Deductive AI especially) validate self-hosted.** When your vendor can unilaterally revoke your visibility, you don't own your observability. Sigiro stores data in your S3 bucket. Nobody can switch it off.
4. **The "cultivated customer base" insight maps to Sigiro's agent-first bet.** Voss says vendors created dependency by replacing Unix fundamentals with dashboards. Sigiro bets the next shift is agents replacing dashboards. The question: will the agent-native generation be cultivated by a new set of vendors, or will tools like Sigiro keep the data layer open?

---

## 13. OneUptime: "The Observability Tax: What Monitoring Actually Costs" — [https://oneuptime.com/blog/post/2026-03-01-the-observability-tax-what-monitoring-actually-costs/view](https://oneuptime.com/blog/post/2026-03-01-the-observability-tax-what-monitoring-actually-costs/view)

**Author:** Jamie Mallers ([[research/oneuptime-analysis.md|OneUptime]]), 1 Mar 2026. Linked from Voss's "Observability Tax" (#12).

**Thesis:** The fragmented "best-of-breed" monitoring approach is the most expensive possible way to monitor systems. Most teams don't know their true observability spend because costs are buried across dozens of line items.

**Key data (100-engineer team, ~200 services):**
- Datadog (APM, infra, logs): $45K–$65K/mo
- PagerDuty: $5K–$8K/mo; Sentry: $3K–$5K/mo; StatusPage: $1.5K–$3K/mo
- Total tool [[company/cost.md|cost]]: $59K–$90K/month ($708K–$1.08M/year)
- Hidden costs: seat-based multipliers ($8K–$25K/mo), log indexing ($15K–$50K/mo), 1–2 FTEs on glue ($200K–$400K/year)
- Real total: $900K–$1.5M/year
- Consolidated platform alternative: $100K–$250K/year (70–90% savings)

**Relevance to Sigiro:** Source of the headline "$708K–$1.08M" figure Voss cites. The hidden costs (context switching at 23 min/switch, 2–3 week onboarding just for the monitoring stack) strengthen the argument that dashboards themselves are a [[company/cost.md|cost]] center. Sigiro's single-query API eliminates multi-tool context switching entirely.

---

## 14. OneUptime: "Observability Tax: Monitoring Costs vs Cloud Spend" — [https://oneuptime.com/blog/post/2026-03-10-observability-tax-monitoring-costs-vs-cloud-spend/view](https://oneuptime.com/blog/post/2026-03-10-observability-tax-monitoring-costs-vs-cloud-spend/view)

**Author:** Jamie Mallers ([[research/oneuptime-analysis.md|OneUptime]]), 10 Mar 2026. Linked from Voss's "Observability Tax" (#12).

**Thesis:** Engineering teams are spending more on monitoring than on the infrastructure being monitored.

**Key data (50-engineer B2B SaaS, 150 hosts):**
- Itemized monitoring stack: $16,893/month ($202K/year) — Datadog infra $3.4K, APM $5.9K, logs $4.5K, PagerDuty $2K, StatusPage $399, Sentry $320, Pingdom $249
- Actual infrastructure being monitored: $10K–$25K/month
- Result: ~70% of infrastructure budget on observability alone
- Vendor markup: $0.10–$3.00/GB ingested vs $0.02/GB cloud storage = 5x–150x markup
- Self-hosted savings: 70–90% ($200K/year → $20K/year)

**Relevance to Sigiro:** Source of the "$0.10–$3.00/GB" and "70% of infra budget" figures. The specific Datadog per-host [[designs/pricing.md|pricing]] breakdown ($23/host infra, $39.50/host APM) is useful [[research/competitive-landscape-2026.md|competitive]] ammunition — Sigiro has zero per-host charges.

---

## 15. OpenObserve: "Datadog vs OpenObserve Part 9: Cost" — [https://openobserve.ai/blog/datadog-vs-openobserve-part-9-cost/](https://openobserve.ai/blog/datadog-vs-openobserve-part-9-cost/)

**Linked from Voss's "Observability Tax" (#12). Contains the Evereve case study.**

**Test methodology:** Side-by-side comparison using OTel Demo (16 microservices with Kafka, PostgreSQL, Redis, LLM service). Same data sent to both platforms for 30 days.

**Headline [[company/cost.md|cost]] comparison:**
- Datadog: ~$174/day; [[research/observe-vs-openobserve.md|OpenObserve]]: ~$3/day — 58x difference / 98% savings
- Datadog's LLM Observability alone: $120/day (auto-activates when LLM spans detected, no opt-out)

**Evereve case study:**
- Fashion retail, 130+ stores. Contact: Aaron Bell, Principal Systems Engineer
- Problem: Datadog per-metric charges forced [[company/cost.md|cost]]-driven monitoring decisions. Stood up parallel systems (Prometheus/[[designs/future/perses-compat.md|Grafana]]/Loki, New Relic, AppSignal) creating silos
- After 1-year eval, selected [[research/observe-vs-openobserve.md|OpenObserve]]. Full UI + API ingesting within 15 minutes. 3 TB audit logs compressed to 0.5 TB
- Result: >90% [[company/cost.md|cost]] reduction, platform consolidation, unlimited monitoring

**Relevance to Sigiro:** The Datadog LLM Observability auto-charge ($120/day, no opt-out) is a gift for Sigiro's positioning: "agent telemetry should [[company/cost.md|cost]] $0.005/GB, not $120/day." The Evereve story shows real companies going through the consolidation pain — Sigiro should be even simpler than [[research/observe-vs-openobserve.md|OpenObserve]] (no UI to learn, just send [[architecture/write-path.md|OTLP]]).

---

## 16. Deductive AI: "Datadog, Thank You for Blocking Us" — [https://www.deductive.ai/blogs/datadog-thank-you-for-blocking-us](https://www.deductive.ai/blogs/datadog-thank-you-for-blocking-us)

**Authors:** Andrew Gross and Scott Rabin (Deductive AI), 13 Mar 2025. Linked from Voss's "Observability Tax" (#12).

**What happened:** Deductive monitors thousands of vendor [[plans/STATUS.md|status]] pages to detect incidents faster than vendor self-reporting. They polled Datadog's public status page every 5 minutes with a clearly identified user-agent. Datadog blocked them.

**Thesis:** Vendor [[plans/STATUS.md|status]] pages are "narrative control tools rather than objective incident communication channels." Common patterns: delayed acknowledgment (30 min to hours), vague language, premature resolution, scope minimization, and outright omission. If a vendor were confident their status page reflected reality, they wouldn't care who reads it.

**Relevance to Sigiro:** This is the strongest vendor lock-in story. The company selling you visibility can unilaterally remove it. Deductive adapted in 48 hours using open-source tooling. For Sigiro: your data lives in your S3 bucket. The data layer can't be blocked, revoked, or price-hiked. This story belongs in Sigiro's [[company/pitch.md|pitch]] deck.

---

## 17. SigNoz: "Why Does Prometheus Consume So Much Memory?" — [https://signoz.io/guides/why-does-prometheus-consume-so-much-memory/](https://signoz.io/guides/why-does-prometheus-consume-so-much-memory/)

**Published:** Nov 7, 2024. Linked from Voss's "Observability Tax" (#12).

**Key thesis:** Prometheus's memory consumption is inherent to its architecture — recent time-series data stored in RAM ("head block") for fast access. Scales linearly with cardinality.

**Specific numbers:**
- ~3 kB RAM per time series (timestamp+value pairs, label metadata, indexing structures)
- 1M time series ≈ 3 GB RAM (storage alone, before query processing and OS overhead)
- Causes: in-memory head block, label cardinality explosions, complex PromQL query spikes, short scrape intervals

**Relevance to Sigiro:** Confirms the 3 GB @ 1M series figure Voss cites. Sigiro's [[architecture/data-model.md|Parquet]]/[[architecture/data-model.md|DuckDB]] architecture has no equivalent — there's no in-memory head block. Telemetry is written to Parquet files and queried on-demand. Steady-state memory is bounded by the collector buffer, not the data volume.

---

## 18. Red Hat: "Prometheus Performance and Scraping Intervals" — [https://www.redhat.com/en/blog/openshift-monitoring-stack-playing-with-prometheus-performance-and-scraping-intervals](https://www.redhat.com/en/blog/openshift-monitoring-stack-playing-with-prometheus-performance-and-scraping-intervals)

**Linked from Voss's "Observability Tax" (#12). Source of the "4x Kubelet CPU" claim.**

**Benchmark results (OpenShift):**
- Clean cluster (few pods), 30s scrape: Prometheus ~17% CPU each, Kubelet 0.10s CPU
- Loaded cluster (239 pods, 268 containers), 30s scrape: Prometheus ~30% CPU, Kubelet 0.37s CPU — **3.7x increase (≈4x)**
- Doubling scrape intervals to 60s: Prometheus CPU halved, but Kubelet only modest improvement
- Must specifically target kubelet ServiceMonitor scrape jobs for meaningful Kubelet relief

**Key insight:** The CPU explosion comes from Kubelet/cAdvisor having to gather and return metrics about all pods/containers/memory/CPU on every scrape. More pods = more work per scrape = more CPU.

**Relevance to Sigiro:** Confirms pull-based scraping is fundamentally expensive at scale. Sigiro uses push-based [[architecture/write-path.md|OTLP]] (the app sends data when it has it), avoiding the N×M scrape multiplication. No Kubelet overhead, no cAdvisor tax.

---

## 19. OneUptime: "Right-Size CPU and Memory for the OTel Collector" — [https://oneuptime.com/blog/post/2026-02-06-right-size-cpu-memory-opentelemetry-collector/view](https://oneuptime.com/blog/post/2026-02-06-right-size-cpu-memory-opentelemetry-collector/view)

**Author:** Nawaz Dhandala ([[research/oneuptime-analysis.md|OneUptime]]), 6 Feb 2026. Linked from Voss's "Observability Tax" (#12).

**Production sizing tiers:**
- Small (1–10 services, ~1K spans/s): 1 core, 512 MB, 2 replicas
- Medium (10–50 services, ~10K spans/s): 2 cores, 2 GB, 3 replicas
- Large (50–200 services, ~50K spans/s): 4 cores, 4 GB, 5 replicas
- Very Large (200+ services, 100K+ spans/s): 8+ cores, 8+ GB, 10+ replicas

**Memory formula:** `base_overhead (100 MB) + (queue_size × batch_size × avg_item_size × exporter_count)`. Example: 1000 queue × 1024 batch × 2 KB × 2 exporters = ~4 GB.

**Relevance to Sigiro:** The OTel Collector baseline (2 cores + 2 GB for ~10K spans/s) is a tax that every [[architecture/write-path.md|OTLP]] user pays regardless of backend. Sigiro can't eliminate this, but can document minimal collector configs that reduce the footprint. Also useful context: at 100K spans/s you need 8+ cores + 8+ GB just for the collector — the pipeline overhead Voss criticizes.

---

## 20. Nick Khami: Amazon S3 Files — Agent Swarm Implications — [https://x.com/skeptrune/status/2041657917708103915](https://x.com/skeptrune/status/2041657917708103915)

**Author:** Nick Khami (@skeptrune) — Mintlify, prev. Trieve (acq. YCW24)
**Date:** 7 Apr 2026 · 315K views · 1.5K likes · 1.3K bookmarks

**Quoted:** AWS announcement of Amazon S3 Files — "The first and only cloud object store with fully-featured, high-performance file system access." [go.aws/4tw17Zg](https://go.aws/4tw17Zg)

**Thesis:** S3 Files eliminates the need to spin up sandbox VMs just to give agents POSIX filesystem access. You can now point arbitrarily large amounts of compute at S3 to run massively parallel agent swarms on the same filesystem.

**Key insight:** This changes the infrastructure model for tool-using agents:
- **Before:** Agent needs sandbox VM → EBS volume → isolated filesystem → limited parallelism
- **After:** Agent reads/writes directly to S3 with POSIX semantics → shared filesystem → massively parallel swarms
- Same filesystem means multiple agents can collaborate on shared state without coordination layers

**Relevance to Sigiro:**
1. **Storage layer validation.** Sigiro already stores telemetry in S3/[[architecture/data-model.md|Parquet]]. S3 Files could let Sigiro agents write [[architecture/investigation.md|investigation]] results directly to the data lake without API calls.
2. **Agent swarms for incident response.** Parallel agents analyzing the same telemetry data on S3 — one reads traces, another reads logs, a third reads metrics, all on the same filesystem. Correlation happens naturally through shared file access, not through a query API.
3. **Infrastructure convergence.** S3 becomes the universal substrate: store your telemetry (Sigiro's [[architecture/data-model.md|Parquet]] files) AND your agent workspace (S3 Files POSIX layer) in the same bucket. One data gravity point.
4. **The viral signal matters.** 315K views + 1.3K bookmarks on this specific framing suggests the market is primed for "S3 as agent infrastructure" — a narrative Sigiro can ride.

---

## Synthesis: What These Say About Sigiro's Direction

All sources converge on three themes (plus a fourth from the agent telemetry research):

**1. Wide events beat pillars.** Store one thing (events), not three separate databases. Sigiro already does this. Lean into it harder in positioning.

**2. Agents are the interface.** The query layer should be designed for AI agents, not humans. Dashboards are the wrong abstraction. Sigiro's `/v1/query` → `InvestigationBlock` is the right primitive. Nominal and the Tane blog validate this as the direction the market is moving.

**3. [[company/cost.md|Cost]] is the forcing function.** Multi-pillar architectures create 3-10x data duplication costs. That's why [[designs/future/perses-compat.md|Grafana]] Cloud costs $1,800/month. Sigiro's $0.005/GB on B2 is the answer. Axiom competes on this but with opaque [[designs/pricing.md|pricing]] and a SaaS model. Voss's "Observability Tax" (#12) puts hard numbers on this: $708K–$1.08M/year for 100 engineers, 5x–150x vendor markup over raw storage, and the monitoring stack itself consuming significant compute (Prometheus 3 GB RAM, OTel Collector 2 cores + 2 GB). The watchtower costing more than the castle is the most compelling framing of the problem Sigiro exists to solve.

**4. Agent telemetry is just more [[architecture/write-path.md|OTLP]].** OpenLLMetry/OpenLIT emit standard `gen_ai.*` spans. They land in the unified table with zero code changes. The opportunity isn't "build agent observability" — it's "be the only backend where agent reasoning and service telemetry are in the same table, queryable in the same SQL." See `agent-telemetry-convergence.md`.

**5. Correlate system metrics with agent actions.** (From Edera article.) The killer insight for agent observability isn't just collecting kernel metrics or agent spans separately — it's correlating them. "MemAvailable dropped 40MB when tool call #17 read src/" is the diagnostic that prevents the next OOMKill. On single-purpose VMs (Fly Machines), Sigiro can read real /proc metrics and join them with gen_ai.* spans in the same [[architecture/data-model.md|DuckDB]] table. This is the insight layer neither infra vendors (Edera) nor traditional APM (Datadog) provide.

**Open questions for product direction:**
- Should Sigiro support high-cardinality metadata queries (slice by any field in the `metadata` JSON)? This is the core Honeycomb/Axiom capability — and what "wide events" queries actually need.
- Should Sigiro build a browser UI (even minimal) to compete with Axiom for teams not yet agent-native?
- Nominal is building the agent layer on top. Should Sigiro integrate with nominal, compete in the agent tier, or stay purely a data layer?
- Should the [[architecture/investigation.md|investigation]] block become signal-attribute-aware (detect `gen_ai.*` in metadata and surface agent sections automatically)?
- Should Sigiro read /proc metrics (MemAvailable, page faults, PSI) directly on Fly Machines and correlate with agent tool call spans? This is trivial on single-purpose VMs but meaningless on shared-kernel K8s. Is Fly-first enough?
