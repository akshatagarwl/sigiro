---
title: Roadmap
created: 2026-03-16
updated: 2026-09-05
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'data-model', 'decision']
---

# Roadmap

Single Rust binary embedding [[architecture/data-model.md|DuckDB]]. Simplest possible architecture.

---

## Priority Order (reviewed 2026-04-11)

### Build Now (Phase 1 — unblocked, high value)
- **#88 — Pyroscope profile ingest is shipped and advertised nowhere, and cannot reach the API reference at all.** High priority: the code is already paid for, so the only thing between it and users is prose plus one routing decision.
- **#89 — Small Parquet files: two levers that work, and two that don't. BLOCKED on upstream DuckLake.** Quiet projects emit ~11 KiB Parquet files. Two attempts to fix it are recorded here so neither gets retried.

  **Lever 1, flush interval: tried, disproved, reverted.** `SIGIRO_FLUSH_INTERVAL_SECS` went 1 to 60 and shipped on 2026-08-24. Production over a 15-hour window: file rate fell from 612-910/day to **407/day** and average file size moved **11 KiB to 12 KiB**. The premise was wrong. At ~0.5 rows/s telemetry arrives in bursts every few minutes and the flusher skips when the buffer is empty, so file count tracks the arrival pattern, not the timer. Reverted the same day. Measure file *size*, not count, before touching this again.

  Two methodology notes, both worth keeping. A synthetic ladder that generates one file per simulated second does **not** model a bursty 0.5 rows/s workload, and it produced a projection 140x off. And counting `ducklake_data_file` joined to `ducklake_snapshot` silently drops every file whose creating snapshot retention has expired: 522 of 1844 rows here, 28%. Count on `begin_snapshot` id ranges instead.

  **Lever 2, DuckLake data inlining: right mechanism, wrong trigger, and blocked.** `data_inlining_row_limit` defaults to 10 against a measured 77-112 row flush, so nothing ever inlines. Raising it puts small appends in the catalog and keeps them queryable there. Measured locally at limit 200, 60 inserts of 112 rows: 3 Parquet files averaging 204 KiB versus 61 averaging 10.8 KiB, identical row counts, and rows queryable while still inlined (112 in, 112 visible, 0 files on disk). The limit also self-selects, a 112-row insert inlines and a 10,000-row insert writes Parquet directly.

  That local result **does not transfer**, because production checkpoints every 30 **seconds** (`inserter.rs:839`) while the test checkpointed every 30 **inserts**. At 0.45 rows/s, 3,360 rows takes ~2.1 hours to accumulate, so each ~4-minute burst would sit inline for under 30s and then become its own ~12 KiB file anyway. The materializer must trigger on accumulated rows/bytes with a max-age bound, not on a fixed 30s timer.

  **Upstream blockers, verified against our exact artifact** (`Dockerfile` pins `DUCKDB_VERSION=1.5.3`, crate `=1.10503.1`):
  - [ducklake#1335](https://github.com/duckdb/ducklake/issues/1335) (**open**) — an inlined commit stamps a stale `next_file_id`, so subsequent non-inlined writes collide on `ducklake_data_file_pkey`. Reproduces on DuckDB v1.5.3 with extension `e6a3bd0a`. Our proposed mixed 112-inline/10,000-direct shape is exactly the trigger.
  - [ducklake#1215](https://github.com/duckdb/ducklake/issues/1215) (**open**) — concurrent deletes straddling `data_inlining_row_limit` both commit, so deleted rows can reappear. Touches our retention `DELETE` path.
  - [ducklake#1067](https://github.com/duckdb/ducklake/issues/1067) (closed) — inline cleanup; confirm whether `expire_snapshots` + `cleanup_old_files` now reclaims dead inline rows.
  - [ducklake#1401](https://github.com/duckdb/ducklake/discussions/1401) — predicates are not pushed into the inline catalog table, so the hot set is transferred and filtered in DuckDB. Fine for 30s of inline data, not for hours.

  So: do not enable inlining until those gates pass on a proven artifact. Pick the limit from per-table append p95/p99 **rows and bytes**, not an average, because the decision is per-table and row-count-only while a sub-200-row log or profile batch can approach the 8 MiB body cap.

  **Interim lever that is not blocked:** scheduled [`ducklake_merge_adjacent_files`](https://ducklake.select/docs/stable/duckdb/maintenance/merge_adjacent_files) with a 1s flush. Costs rewrite traffic and temporary R2 objects, but it does not turn the constrained catalog Postgres into a telemetry staging store.
- **#91 — A failing `CHECKPOINT` only warns while ingest keeps accepting.** `inserter.rs:1008` logs `warn!` and continues. In distributed mode that means an R2 outage can have Postgres absorbing telemetry while materialization fails on every cycle, and `SIGIRO_MAX_DISK_BYTES` measures only the local footprint, never the external catalog volume. Independent of #89 and worse if inlining is ever enabled. Needs a consecutive-failure counter that either sheds load or surfaces itself the way `DiskCapacityState` does.
- **#92 — Every documented hosted OTLP endpoint was wrong, and the failure is silent. Docs fixed 2026-08-25; three product gaps stay open.** Found by driving a memoryless Claude Code session through `install-with-ai` against a third-party repo (`distroinfinity/ccwarriors`, PR 122) while `agent-telemetry` captured the session itself. `agent-telemetry` sent hosted readers to gRPC `:4317` and `hosted-onboarding` offered `:4317` and `:4318`; on `api.sigiro.com` both ports are closed (`nc`), the gRPC service path returns an nginx 404, and kamal-proxy forwards a path allowlist with no gRPC entry. An SDK that cannot connect logs in the caller's own process, so the reader sees an empty tenant and no error. The docs now say the hosted endpoint carries no port. What is still open: (1) **no log signal for unnamed loggers** — the bundled bridges are pino/winston/bunyan, so a `console.*` service gets traces and metrics only, while `install-with-ai` promises all three; either ship guidance or stop promising logs. (2) **the DB column of the coverage table overstates reach** — `instrumentation-pg` patches `node-postgres` only, and `drizzle-orm/tracing` can never fire because its `await import('@opentelemetry/api')` is commented out in the published package, still true in 0.45.2; a maintained third-party wrapper is currently the only route to query spans on `postgres.js`. (3) **tenant onboarding needs a shell on the box** — `POST /admin/tenants` and `/admin/keys` are loopback-only by design, which is right, but it makes every new tenant a manual ssh. Also worth a decision: Claude Code's enhanced telemetry stamps `user.email`, `user.id`, `user.account_id` and `organization.id` on every span and metric point, so anyone following `agent-telemetry` puts operator identity in their tenant. 9 localized copies of the three pages are stale until `blume translate` reruns.
- **#90 — Replica freshness by conditional GET, instead of coordination.** Filed, not to build: there are no read replicas today for it to help. Prior art is Cursor's Continuity ([cursor.com/blog/git-at-any-scale](https://cursor.com/blog/git-at-any-scale), Vicent Martí, 2026-08-18). Each replica remembers the ETag of the last index version it caught up to; a read issues a conditional GET, where 304 with no body is a metadata-only round trip under 10 ms and serves immediately, and 200 carries the new index to catch up from. Gossip is best-effort UDP that only warms this, so a dropped packet costs nothing. That buys fully consistent reads across arbitrarily many replicas with no consensus, no routing table and no database, measured at 100 replicas with linear read scaling. The DuckLake analogue is cheaper still, since snapshots are already numbered: a replica caches the snapshot it has served through and does one cheap freshness check before answering. Relevant when hosted read load outgrows one process.

### Build Now — distribution and go-to-market (added 2026-09-05)

Full entries are #93–#106 in the section **Go-to-market plan: distribution
channels and the incident campaign** further down. Every channel leads to the
same five steps: install → run a prepared incident → ask the agent what
happened → inspect the evidence → connect your own application. The order below
is by dependency first, then by the plan's own channel ranking.

- **P0, week 1 — the pieces every channel consumes.** #93 public integration
  repo (this repo is private with no LICENSE, and Cursor requires a public one)
  · #94 the one skill, "Debug an application with Sigiro" (#48's first
  candidate, finally written) · #95 prepared incident #1 plus the shared example
  app (nothing like it exists in the repo today) · #96 funnel tracking from the
  first visit.
- **P1, week 1–2 — first submissions.** #97 Cursor plugin submission plus the
  Cursor MCP install button on the site · #98 skills.sh via `npx skills add` ·
  #99 tutorial #1 and its "Try this incident" landing page.
- **P2, week 2–4 — incremental channels off the same repo.** #100 Claude Code
  plugin · #101 Smithery bundle, then the official MCP Registry (needs
  checksummed release artifacts, so #42/#52 first) · #102 tutorial #2 · #103 the
  weekly incident-to-content cycle · #104 AlternativeTo.
- **P3 — only after #96 and #99 exist.** #105 the capped $200 Reddit
  experiment.
- **Blocker track, code.** #106 automatic hosted signup, key issuance and
  checkout. Until it ships the funnel ends at a call (#26) and hosted tenants
  are cut by hand (#92).
- **Deferred, reasons in the section.** Railway (Open auth), OpenTelemetry
  directory lists (frozen), Coolify (1,000-star gate).

### Build Next (Phase 2 — reach)
5. **S3 storage** — [[architecture/ducklake-integration.md|DuckLake]] config + test + docs, 0 new lines
6. **Docker + PaaS deploy** — Dockerfile, Railway/Render/Fly templates, ~50 config
7. **Vercel Marketplace** — wire existing code, ~50 lines
8. ~~**[[architecture/profiles.md|Pyroscope]] SDK profiles**~~ — **SHIPPED, and nobody was told.** Four routes are live and handled by `otlp_http_pyroscope` (`src/ingest.rs:896`). Correcting this entry is part of **#88**, which covers why the capability reaches no reader.

### Build Later (Phase 3 — Smart SRE + optimize)
9. **sysquery module** — procfs/sysinfo Rust-native OS state (processes, memory, network, disk). ~500 lines. Fallback when [[designs/future/smart-sre.md|osquery]] unavailable (macOS, containers).
10. **Embedded [[designs/future/smart-sre.md|osquery]]** — Bundle osqueryd static binary (~8MB compressed) via include_bytes!(). Extract on `sigiro setup`. Full 200+ tables. ~100 lines (subprocess manager + osquery extension API).
11. **[[designs/future/ebpf-gpu-position.md|eBPF]] probes (aya)** — Pre-built probes compiled into sigiro: tcp_retransmit, block_io_latency, dns_latency, oom_kill, exec_snoop, ssl_latency, etc. ~800 lines. Pure Rust, no bpftrace dep.
12. **Embedded bpftrace** — Bundle static binary (~3MB) for escape hatch. Agent writes arbitrary bpftrace/bpfquery SQL when pre-built probes aren't enough. ~50 lines.
13. **Perses/[[designs/future/perses-compat.md|Grafana]] protocol compat** — Prometheus + Tempo + Loki + [[architecture/profiles.md|Pyroscope]] HTTP APIs, ~600 lines. See docs/designs/[[designs/future/perses-compat.md|perses-compat]].md
14. **Platform marketplace integrations** — Netlify/Railway/Render webhooks, ~50/platform
15. **Usage-based [[designs/pricing.md|pricing]] + RBAC** — Stripe Meters + [[designs/auth-billing.md|Zitadel]], ~100 lines. See docs/designs/pricing.md + docs/designs/[[designs/auth-billing.md|auth-billing]].md
16. **Helm chart** — K8s packaging, ~50 config
17. **Blog post + launch** — "Your agent debugs like a senior SRE" → HN, X, Reddit

### Build Eventually (Phase 5 — fleet)

18. **Edge mode — SHIPPED, different shape than planned.** No `--central-url`/`--central` flags (never built). Instead: any `sigiro serve` node becomes queryable by setting `SIGIRO_QUERY_SURFACE=quack` — same binary, no self-registration, no central node. See [[guides/self-host-edge-federation.md|Self-Host Edge Federation]].
19. **Federation — SHIPPED, not via Spice.ai.** The Spice.ai-subprocess design below (and the central node + `POST /v1/federation/workers` self-registration it depended on) was never built and is superseded. What shipped: `SIGIRO_QUERY_SURFACE=quack` serves each edge's DuckLake read-only in-process; `sigiro query "<SQL>"` is a CLI coordinator that discovers edges over the tailnet, pushes a per-edge partial via `quack_query()` (not `ATTACH` — doesn't push predicates), and folds correctly (never avg-of-avgs). See [[architecture/distributed-query.md|distributed-query]].md and [[guides/self-host-edge-federation.md|the operator guide]].
20. **Fleet observability** — [[architecture/data-model.md|DuckDB]] extensions (system_stats, fsquery, hostfs) loaded by default on every node. Still **per-node only** — the `sigiro query` coordinator's grammar is one `sigiro_*` table per query, so it does not yet fan `sys_cpu_info()`-style functions out across edges. See docs/architecture/[[architecture/distributed-query.md|distributed-query]].md.
21. **Fleet query API — superseded by the CLI coordinator.** The originally-planned server-side fan-out (`/v1/query` on a central node fanning out to workers) was rejected as a design (see distributed-query.md's "deleted" note) in favor of `sigiro query`, a client-side coordinator with no server-side central node at all.
22. **Background vitals** — Edge sigiro ships CPU/mem/disk as [[architecture/write-path.md|OTLP]] metrics every 30s for historical context. ~50 lines.

### Docs / GTM (no code)
23. **fly.toml one-liner** — Sigiro sidecar deploy docs
24. **sigiro.com/for/fly** — landing page. Subsumed by #29: one page per platform, of which Fly is one.
25. **GenAI SDK setup guide** — opentelemetry-instrumentation-openai → Sigiro

#### Landing-page surfaces (added 2026-08-02)

Four pages the site does not have. They are listed together because they answer
the same objection in different registers: sigiro has no users yet, so a visitor
has no way to tell whether it is alive, whether it is going anywhere, and
whether it fits what they run. Every one of these is a page a competitor
already publishes.

26. ~~**Waitlist**~~ — **rejected 2026-08-04 in favour of a call, and that
    shipped.** The hero's second action is now `Talk to us` →
    `cal.com/akshatagarwl/30-minutes` (`CALL` in `src/lib/site.ts`), replacing a
    duplicate "Read the docs" link.

    Why the call won: a waitlist collects addresses that then need a store, an
    outbound path and something to send — three new pieces — and it yields the
    weaker signal. With no users, the thing actually missing is knowing *who*
    wants this and what they would pay for, which a conversation gives and an
    email does not. Zero new infrastructure: the scheduler owns the form, the
    timezones and the reminders.

    It also carries the hosted service, which has no signup to link to (an
    operator sets up each tenant by hand — `docs/hosted-onboarding.md`). The
    division the site now makes: **self-hosted and local development sell
    themselves; hosted goes through a call.** The `~$14/mo` price came off the
    landing page for the same reason — it advertised a self-serve signup that
    does not exist.

    Still open: nothing captures a visitor who is not ready to book. That may be
    fine at this stage, and is worth revisiting only if the call converts badly.
    Item #44 is the related gap — the docs have no route to a person at all.
27. **Build log** — dated entries, shortest possible. It is the only honest
    signal of momentum for a project with no logos to show, and the input
    already exists as the commit history.
28. **Roadmap, public** — a filtered view of this file, not a second copy of it.
    A copy drifts, and a drifted roadmap is worse than none. Decide the
    generator before writing any prose.
29. **One page per platform / runtime / agent** — `/for/vercel`, `/for/fly`,
    `/for/railway`, `/for/claude-code`, `/for/cursor`, and so on. The SEO and
    discoverability argument is the right one: nobody searches "observability
    data layer", they search "vercel opentelemetry" or "claude code
    observability". These pages are also what an answer engine cites, since
    each one is a specific claim about a named thing.

    Two risks to settle before building any of them. First, thin content:
    twenty near-identical pages generated off a template is what Google
    classifies as doorway pages, and the penalty lands on the whole domain.
    Each page needs something genuinely specific — the actual config, the
    actual caveat. Second, honesty: `.agents/product-marketing.md` binds these pages
    too, so a page may not claim an integration that does not exist. Start with
    the three or four we can demonstrate end to end and let the set grow, in
    preference to shipping the full grid at launch.

30. **Ship a binary, not only an image** — `curl sigiro.com/install | sh`, plus
    plain downloadable binaries for linux and macOS on arm64 and x86_64. Docker
    is the only install path we publish, and it asks for a container runtime
    from a reader whose next step is `npm start` on their laptop.

    **Shipped and verified live 2026-08-04. `curl sigiro.com/install | sh`
    works.** Ran end to end against production from a clean directory: the
    script detects the target, reads `/dl/LATEST` (`0.1.0`), downloads from R2,
    verifies the SHA-256, installs, and `sigiro --version` reports `0.1.0`.
    `sigiro.com/install` returns 200 and `/dl/LATEST` returns 200. See "What
    remains" below for the three open pieces.

    Checked 2026-08-02, before writing this (the state that motivated the item;
    the `/install` fault described here is now fixed):

    - `sigiro.com/install` was **published and broken**. It 307d
      to `sigiroai/sigiro/releases/latest/download/install.sh`, which 404d.
      The short link is in the README and on the socials.
    - `ghcr.io/sigiroai/sigiro:latest` **is** anonymously pullable (verified
      with an anonymous ghcr pull token). Docker is the one install path that
      currently works.
    - The source repo is `akshatagarwl/sigiro` and is **private**. The `sigiroai`
      org exists with **0 public repos**. There is no LICENSE file anywhere,
      though `Cargo.toml` declares Apache-2.0.

    The blocker was never the build. Release assets on a private repo are not
    anonymously downloadable, so `curl | sh` cannot work off the repo as it
    stands.

    **Decided 2026-08-02: self-host on R2 behind the www Worker.** The repo
    stays private, no LICENSE decision is forced, and the published short link
    stops being a redirect to somebody else's 404. Rejected: a public
    release-only repo under `sigiroai` (a second place to keep in sync, and
    `/install` still points at `sigiroai/sigiro`), and opening the source for
    real (the right end state, but a strategy call that should not be made as a
    side effect of wanting a binary).

    `cargo-dist` was considered and not used. It generates exactly this — matrix,
    installers, checksums — but it assumes GitHub Releases, which is the one
    thing this decision rules out. Adopting it would mean fighting its hosting
    model for the sake of code we would still have to read.

    Built:

    - `www/public/install` — POSIX sh, served as a static asset at
      `sigiro.com/install`. Detects target, reads `/dl/LATEST`, verifies the
      SHA-256 against `SHA256SUMS`, installs to `~/.local/bin`.
    - `www/src/app/dl/[...key]/route.ts` — streams from a private R2 bucket
      through the Worker. `LATEST` gets 60s of shared cache; versioned keys are
      immutable for a year.
    - Bucket `sigiro-dl`, created and deliberately separate from `sigiro-prod`,
      which holds tenant telemetry. `/dl` is unauthenticated, so one bucket
      would make a key-handling bug a data leak rather than a 404.
    - `.github/workflows/release.yml` — on `v*`, four targets, checksums, R2.
    - The dead `/install` redirect in `next.config.ts` is gone.

    Verified end to end against `wrangler dev` with a seeded local R2: install
    succeeds and the binary runs; a tampered archive fails the checksum and
    installs nothing; an unknown version fails with the URL it could not reach;
    malformed keys 400.

    **v0.1.0 was published by hand, and how matters.** `wrangler r2 object put`
    authenticates with the existing Wrangler OAuth session and needs no S3
    token, so the artifacts went up without the repo secrets ever being set.
    www is deployed. Do not read the open item below as "the install path is
    blocked" — it is not, and that misreading already cost one session.

    What remains, 2026-08-04:

    1. **`x86_64-apple-darwin` is not in the bucket.** The other three targets
       are present under `0.1.0/`. An Intel Mac gets `no 0.1.0 build for
       x86_64-apple-darwin. Run the container instead` — a deliberate, honest
       error (`install:83`), not a crash. Real gap, graceful failure.
    2. **Releases are still hand-cut.** Repo secrets `R2_ACCESS_KEY_ID`,
       `R2_SECRET_ACCESS_KEY`, `R2_ACCOUNT_ID` are unset, so `release.yml`
       fails at preflight on every tag. This is an automation ticket, not an
       availability one. It is genuinely dashboard-only, but for a narrower
       reason than previously recorded: `release.yml:159-172` uses `aws s3 cp`,
       which needs AWS SigV4 keys. Wrangler has no token-minting command
       (verified 4.118.0: no `r2 token`, no `r2 credentials`), and the R2
       Temporary Credentials API only *derives* from a parent token that must
       itself be created in the dashboard. Alternative worth considering:
       switch the upload step to `wrangler r2 object put` with a
       `CLOUDFLARE_API_TOKEN`, which sidesteps SigV4 entirely.
    3. ~~Landing-page copy does not advertise the install.~~ **Done.** Verified
       against the live page 2026-08-04: `curl -fsSL https://sigiro.com/install
       | sh` renders in two places — the `INSTALL` block (`site.ts:58`) and the
       `binary` tab of the four-step terminal (`site.ts:147`). The comment at
       `site.ts:53-57` records the rule that gated it: no line ships before its
       artifact exists.

    Root `SHA256SUMS` does not exist in the bucket and does not need to: the
    script reads the versioned `${version}/SHA256SUMS` (`install:84`).

    Open sub-decisions: musl alongside gnu, which would drop the glibc floor
    entirely (gnu builds on ubuntu-22.04, so glibc 2.35 is the floor today);
    and Windows, which has no target here at all.

31. **Replace fumadocs with Blume** (useblume.dev) — **SHIPPED, and it resolved
    shape 2 rather than 1.** Verified 2026-08-04: `www/` is a Blume/Astro app
    (`blume.config.ts`, `pages/`, `layouts/`) on **one** Worker serving both
    `sigiro.com/*` and `sigiro.com/docs/*`. fumadocs and Next are gone; there is
    no second site and no duplicated header. The `docs.sigiro.com` Worker that
    existed for two days is retired — `wrangler.jsonc` records why the subdomain
    still is not routed (Cloudflare's asset router answers before the Worker, so
    the redirect middleware never fires).

    `/llms.txt` and `/llms-full.txt` are generated by Blume now, at the site
    root. The install-preference tabs still share
    `localStorage['sigiro-install']` — `INSTALL[].id` values match the docs tab
    values on purpose, so do not rename them.

    Two upstream bugs found while shipping this, both reported with the
    maintainer's fixes now in review:
    [blume#155](https://github.com/haydenbleasel/blume/issues/155) (every
    generated artifact 404s on a Cloudflare server build — `deployStaticDir`
    returns `dist/` where only `dist/client` is served; PR
    [#161](https://github.com/haydenbleasel/blume/pull/161)) and
    [blume#156](https://github.com/haydenbleasel/blume/issues/156) (Tab panels
    holding a fence *and* prose lose all padding; PR
    [#162](https://github.com/haydenbleasel/blume/pull/162)). `scripts/
    serve-agent-files.mjs` is the local workaround for the first and can be
    deleted once #161 lands in a release.

    The original deliberation, kept because it records why shape 2 won:

    Blume is a standalone Astro
    framework — `blume dev`, `blume eject` gives an Astro project — and there is
    no documented way to mount it inside a React app. Our docs are a compartment
    of the same Next page shell as the landing page: same header, same footer,
    same panel and gutter, on purpose. So this is not swapping a library, it is
    deciding whether docs become a second site.

    Three shapes, and the answer changes everything downstream:

    1. Docs move to `docs.sigiro.com`, a separate Astro build. Cleanest for
       Blume, and the site stops being one object.
    2. Docs stay at `sigiro.com/docs`, served by a second Worker on a path
       route. Keeps the URL; the header and footer become duplicated markup in
       two frameworks.
    3. Stay on fumadocs and take only what is wanted from Blume's list — the
       AI assistant and MCP are the parts fumadocs lacks, and neither needs a
       framework change.

    Two things that come along with 1 or 2: the install-preference tabs share
    `localStorage['sigiro-install']` with the landing page by matching fumadocs'
    own storage convention (see terminal.tsx), which a different docs framework
    will not honour; and `/llms.txt` and `/llms-full.txt` are generated from
    fumadocs' `source` today.

32. **Move the landing page off Next onto Astro** (added 2026-08-03) —
    **SHIPPED, folded into item 31.** Both open questions answered by what was
    built: **plain Astro via Blume**, and **one site**, not two. `pages/
    index.astro` is the landing page and the docs are `/docs/*` in the same app,
    so the shared header and footer came back and the install preference has a
    single origin. `/dl/[...key]` survived as `src/dl.ts` with the R2 binding
    intact.

    The original deliberation, kept because it measured the work honestly:

    The pull is real: two frameworks for one small property is the thing worth
    removing, Astro ships no client JS by default where Next ships a React
    runtime for a page that is mostly static, and the docs already prove the
    Cloudflare path works.

    Scope, measured rather than guessed:

    - `www/` is ~1,800 lines across the page, the copy module, the terminal and
      the Shiki helper. Small.
    - **Eight client components.** `terminal.tsx` alone is 511 lines and is the
      hard one: scroll-driven MotionValues, an XState machine, and `SkeletonSwap`
      + `TaskSteps` under it. All of it survives as one Astro island —
      `client:visible` — but the island boundary has to be drawn deliberately, not
      discovered.
    - `components/interior/*` is vendored verbatim under DESIGN.md §1b. It is
      React and must stay React and unmodified, which rules out a rewrite into
      Astro components and argues for islands.
    - `/dl/[...key]` is a real runtime route with an R2 binding, serving the
      release artifacts. Needs `output: 'server'` and an Astro endpoint.
    - Server-rendered Shiki (`lib/highlight.ts`, Effect-based) has a direct
      equivalent — Astro highlights fences at build time — but the stepped
      terminal wants pre-highlighted *lines*, which stays custom either way.

    Two questions to settle first, because they decide the shape:

    1. **Blume or plain Astro?** Blume is a docs framework. The landing page is
       not docs — it is a scrollytelling page with a bespoke design language. Its
       component overrides and React islands might carry it, or it might be
       fighting the tool. Plain Astro + the existing Tailwind theme is the
       conservative read.
    2. **One site or two?** Folding sigiro.com and docs.sigiro.com into one Astro
       project would restore the shared header/footer that item 31 gave up, and
       would give the install preference back a single origin. That is a bigger
       change than "port the landing page" and should be decided on purpose.

    Not urgent. Nothing is broken; this is consolidation.

33. **Rewrite the site copy with a humanizer pass** (added 2026-08-03) — every
    user-facing line on sigiro.com and docs.sigiro.com, run through a skill whose
    job is removing the tells that make prose read as machine-written.

    Two facts to settle before starting, because they conflict:

    - **No `humanizer` skill is installed.** The closest is `stop-slop`
      (Hardik Pandya) — "remove AI writing patterns from prose", already in
      `~/.claude/skills/`. `copy-editing` is also there but does a different job:
      it improves marketing copy, it does not hunt for machine tells. Either
      install an actual humanizer via `npx skills find`, or decide `stop-slop` is
      the tool and say so.
    - **This collides with the ASD-STE100 rule.** All user-facing text has
      followed Simplified Technical English since 2026-08-01: approved words,
      active voice, sentences under 25 words, no metaphor, no idiom, no
      contractions, no -ing forms. STE is *deliberately* machine-flavoured — that
      constraint is what makes it translate and parse cleanly. A humanizer pass
      will fight it directly, and `stop-slop`'s "vary your rhythm" rule is close
      to the opposite of STE's "keep sentences short and uniform".

    So the real decision is which register wins where, and the honest split is
    probably by audience: docs stay STE, because a reader parsing install steps
    under pressure benefits from uniform short sentences; the landing page gets
    the humanizer, because it has to persuade a person. Do not start until that
    is decided — a pass that reverses itself is worse than none.

    Source of truth for voice stays `.agents/product-marketing.md`.

#### Company (added 2026-08-03)

34. **Find investors** — TODO. Nothing decided: not the stage, not the amount,
    not the shape (angels, pre-seed fund, accelerator, or no raise at all).

    What is true today, from this file: no users, no public repo, no LICENSE, and
    and — until 2026-08-04 — the only working install path was a Docker pull.
    `curl sigiro.com/install | sh` now works (#30), so that specific gap is
    closed. Items 26–29 exist because a
    stranger cannot currently tell whether sigiro is alive. That is the same gap
    an investor looks at, so those items are the prerequisite rather than a
    parallel track — booked calls and a build log are the only evidence a
    pre-product raise can point to. (Booked calls are arguably the better
    evidence: a transcript of what ten operators said is worth more to an
    investor than a signup count, and #26 chose the call over a waitlist for
    that reason.)

    To settle before doing any outreach:

    1. **Raise or don't.** A single Rust binary with no infra bill may not need
       outside money. Decide this first; it determines whether anything below
       matters.
    2. **What the ask funds.** Runway to hosted multi-tenant with billing
       (items #15, Stripe, OAuth2 — all currently UNDECIDED) is a different
       story from runway to open-source adoption.
    3. **Open source or not** (item #30 flagged this and deliberately did not
       decide it). It changes the pitch entirely: OSS-with-hosted is a distinct
       category from closed SaaS, and investors sort by it.

    Not started. No target list, no deck, no data room.

35. **Launch on the public platforms** — X, Bluesky, Product Hunt, plus the
    aggregators that carry this category: HN (Show HN), Lobsters, r/rust,
    r/devops, r/selfhosted, Hacker Newsletter, Awesome-* lists (awesome-rust,
    awesome-observability), dev.to / Hashnode crossposts, and the OTel community
    channels. Item #17/#35 in Phase 3 already reserves "blog post + launch"; this
    is the distribution half of it.

    **Ordering constraint, and it is the whole point of this entry.** Product
    Hunt is one shot per project. HN's Show HN is close to one shot too — a
    reposted launch that flopped rarely gets a second look. Today `/install`
    the repo is private with no LICENSE, and there was
    nothing to capture anyone the traffic sends (item #26). Spending the
    single-use surfaces into that is the expensive mistake.

    **Updated 2026-08-04: both of those blockers are cleared.** `curl
    sigiro.com/install | sh` works and the landing page advertises it (#30), and
    the hero now has a `Talk to us` action (#26). So the ordering constraint that
    justified this entry is satisfied.

    What is left is a judgement call rather than a blocker: **the repo is still
    private with no LICENSE**, and a developer-tool Show HN that links no
    readable code converts badly (#36 makes the same point). That is the
    open-source decision in #30/#34, and it is the last thing worth settling
    before spending Product Hunt or Show HN.

    Free-to-repeat surfaces (X, Bluesky, dev.to, subreddits) do not carry that
    constraint and can start as a build-log-in-public feed now — which is item
    #27's content, reused.

36. **Own the social profiles** — **partly done.** Updated 2026-08-04: the site
    now links **X and LinkedIn** (`SOCIAL` in `src/lib/site.ts`, rendered in the
    footer). Still missing: **Bluesky, and any public GitHub presence.** The
    2026-08-03 note below said LinkedIn was absent; that is no longer true.
    (LinkedIn answers automated requests with HTTP 999, so whether the company
    page itself is fully set up was not verifiable from a shell — check it in a
    browser.)

    - **GitHub** is the load-bearing one and is blocked by the same decision as
      #30/#34: `akshatagarwl/sigiro` is private and the `sigiroai` org has 0
      public repos. A developer-tool launch that links no readable code converts
      badly. This is not a profile task, it is the open-source decision.
    - **Bluesky** is still unclaimed. Claim it before launch regardless of
      whether it gets used — squatting on your own name is cheaper than
      recovering it. LinkedIn is done.
    - Whatever is claimed has to be added to `SOCIAL` in `src/lib/site.ts`, not
      just created. One site now, so that is the only place.

37. **Logo / brand mark** — there is deliberately no symbol today. `www/src/
    components/site/logo.tsx` is a wordmark: the name, lowercase, set once. Its
    own comment records why the previous `sigiro[.]com` mark was removed (it was
    interior.dev's mark with our name in it, and it spent the accent colour on
    decoration).

    So this is a reversal of a considered decision, not a gap — treat it that
    way. The argument *for* a symbol is concrete and external: Product Hunt, an
    X/Bluesky avatar, a GitHub org avatar, and a favicon are all square crops
    where a wordmark either shrinks to mush or gets cropped to "si". Those
    surfaces are exactly what #35 and #36 need. `www/src/app/icon.svg` and
    `og-3.png` exist and should be audited first — the favicon problem may
    already be solved.

    Constraint to carry: whatever is designed must not re-import interior.dev's
    design language, which is the failure the wordmark comment documents. Keep
    the wordmark as the in-page mark; the symbol is for square crops only.

38. **Internationalization** — translate the site and docs. Measured
    2026-08-03, before writing this, because the cost is much lower than it
    looks:

    - `www/src/lib/site.ts` is **542 lines and is already the single source of
      truth for every string the landing page says**. Its header comment says
      so explicitly. This is the extraction step that usually dominates an i18n
      project, and it is done.
    - Docs are **4 files, ~2,800 words**. That is one translation pass, not a
      programme.
    - **No i18n anywhere yet**: no library in either `package.json`, no locale
      config in the Astro/Blume side, and `www/src/app/layout.tsx:133` hardcodes
      `lang="en"`. So nothing has to be un-wired first.

    **The real conflict is with item 33, and it has to be resolved first.**
    Docs follow ASD-STE100 (since 2026-08-01) and STE exists *precisely* to make
    text translate cleanly and parse unambiguously — controlled vocabulary, no
    idiom, no metaphor, short uniform sentences. That constraint is an i18n
    asset. Item 33 proposes running the landing page through a humanizer to
    remove exactly that uniformity, and `site.ts`'s own comment already bakes in
    "vary the rhythm" and "say you" as rules. Varied, idiomatic, second-person
    marketing prose is the most expensive kind of text to translate and the
    easiest to translate badly.

    That argues for the same audience split item 33 landed on, and for the same
    reason: **docs are the translation target, the landing page probably is
    not.** Localized docs help a reader install under pressure in their own
    language; a localized scrollytelling landing page is a brand-voice problem in
    N languages with no one to review the output.

    Open, in order:

    1. **Which locales, and why each one.** Not "all of them". Pick from actual
       traffic or an explicit market bet, and note the bet in this file.
       Machine-translated pages nobody reads are an SEO liability (thin/duplicate
       content), which is the same trap item #29 flagged for doorway pages.
    2. **Docs only, or docs + landing?** Resolve against item 33 before either.
    3. **Who reviews a translation?** Nobody here reads all target languages. An
       unreviewed locale can make a false product claim, which
       `.agents/product-marketing.md` forbids in English and does not stop
       forbidding in Japanese.
    4. **URL shape** — `/ja/docs` subpath vs `ja.sigiro.com`. This interacts with
       item 32's unresolved "one site or two", so decide 32 first or the choice
       gets made twice.

    Not urgent. Nothing is broken, and there are no users yet to be non-English
    — which is itself the argument for deferring until #26 produces evidence of
    who is actually showing up.

39. **Vary the Ask AI prompt phrasing** — the footer's "ask ai" row
    (`www/src/components/site/ask-ai.tsx`) sends the same fixed sentence to
    every provider. It is `ASK_AI_PROMPT` in `www/src/lib/site.ts`, a
    four-clause constant.

    **SHIPPED 2026-08-04. A pool of twelve one-line questions, one drawn at
    random per link on every page load.** Each covers a different use case:
    what it does, the agent loop, install, self-host versus hosted, data
    ownership, what it does *not* do, category, sending existing OTel, what
    "what changed" returns, which claims the docs support, anomaly detection,
    SQL querying.

    The goal is answer-engine coverage. A reader who opens two links gets two
    facets; a reader who refreshes gets a different set.

    **This is option 2, and it costs the footer its zero-JS property** — that
    trade was made deliberately. Random-per-refresh had nowhere else to happen:
    the page is a cached static asset at the edge, so a server-side pick means
    disabling the cache for every visitor, and a hydrated island means shipping
    React to render seven anchors. What ships instead is ~15 lines of inline
    `defer` script that rewrites the seven `href`s, reading the pool from an
    inert `application/json` tag so the prompts stay copy in `site.ts`.

    Two things make the degradation safe rather than theoretical: the
    server-rendered markup already carries **seven distinct real questions**
    (indexed into the pool, not one repeated), so with JS off, a CSP block, or a
    click faster than the parse, the links still work — they fall back to the
    previous behaviour and not to nothing. Sampling is Fisher-Yates **without
    replacement**, so two links never carry the same question on one page.

    Verified by executing the real inline script against the real built HTML in
    a DOM, six times: 7 distinct prompts per load, 0 duplicates within a page,
    6 different orderings across 6 loads.

    A per-provider variant was built first and rejected — tailoring the phrasing
    to each destination (search engine versus chat composer) is deterministic and
    keeps zero-JS, but it is not per-refresh, which was the actual requirement.

    Invariants asserted mechanically across all twelve, and worth re-running when
    one is added: every prompt names all three public URLs, none asks for a
    summary, none names a competitor (`.agents/product-marketing.md`, 2026-08-01),
    and none asks for a price.

    **Open, deliberately deferred:** the three-URL preamble is roughly twice the
    length of the question it precedes. `llms-full.txt` is load-bearing — nothing
    on the landing page links to the docs, so an assistant given only the domain
    answers from marketing prose, which is the failure this feature exists to
    prevent. The weak one is `api.sigiro.com/openapi.json`: a different host, the
    least likely to change an answer, and already discoverable through
    `/.well-known/api-catalog`. Dropping it would cut ~45 characters from every
    prompt. Left as-is for now.

    **Also fix in the same change:** the prompt's last clause asks what sigiro
    costs, but item #26 removed the hosted price from the site. An assistant is
    now asked a question the site does not answer, and the likely failure is a
    price invented from stale training data — which
    `.agents/product-marketing.md` forbids. Reword to the self-host / hosted
    split instead of a number.

    Stale in the original entry, corrected 2026-08-04: there are **seven**
    providers, not five (ChatGPT, Claude, Duck.ai, Gemini, Grok, Mistral,
    Perplexity); `docs-site/blume.config.ts` no longer exists (one Blume app at
    `www/`, and `ask` is still off there for the same reason); and the
    `/openapi.json` 404 the prompt used to name was fixed in `3603713` via
    `public/_redirects`.

    Note this is **not** Blume's `ask` widget — that is deliberately disabled in
    `www/blume.config.ts` (needs a key, a provider and a per-query bill), and
    its comment points at this prefill as the reason it is not needed.

    **The blocker is architectural, and it is why this is not a five-minute
    change.** `QUERY` is `encodeURIComponent(ASK_AI_PROMPT)` evaluated **once at
    module scope**, and the file's own comment explains why: encoding a constant
    five times per render is work with no result. Varying per visit means the
    prompt stops being a constant, which forces a choice:

    1. **Server-side pick per request.** Keeps zero client JS, which is the
       property item 32 is chasing. But the landing page is statically
       generated/cached at the edge, so "per request" is really "per cache
       entry" — needs `Vary` handling or a Worker-level pick, and it interacts
       with item 32's Astro port.
    2. **Client-side pick on mount.** Trivial to write and wrong here: the
       footer would render one prompt on the server and another after
       hydration, which is a hydration mismatch and a visible flash on a link
       users may already be hovering.
    3. **Rotate by build.** A new phrasing each deploy. Nearly free, no runtime
       cost, no hydration problem — and it is not really per-visitor, which may
       be enough depending on the goal below.

    **Settle the goal first, because it picks the option.** Two different
    motives got conflated in the request:

    - *If the point is that a repeat visitor does not see a stale identical
      string* — option 3 is sufficient and costs nothing.
    - *If the point is answer-engine coverage* — different prompts surface
      different facets and reduce the chance every assistant returns the same
      framing, which is an argument for 1 and connects this to item #29's
      answer-engine reasoning.

    **Constraint that must survive any variation.** `ASK_AI_PROMPT`'s comment
    documents the one rule that makes this feature work: never ask for a summary
    of the landing page, because an assistant that reads only the marketing page
    repeats the marketing page. Every variant must keep pointing at all three
    public URLs — the page, `/llms-full.txt`, and `/openapi.json`. A variant set
    that drops one of those quietly breaks the feature. `.agents/
    product-marketing.md` binds these strings too: N variants are N chances to
    make a claim the product cannot support.

    Also interacts with #38: a varying prompt is N strings to translate instead
    of one.

40. ~~**Document Go compile-time instrumentation as the preferred Go path**~~ —
    **SHIPPED 2026-08-04** (`9b75f07`). A `### Go: instrument the build, not the
    source` section in `www/docs/quickstart.mdx`, framing compile-time as
    preferred *for Go* while routing every other language to
    `/install-with-ai`.

    **Verified end to end against a live sigiro, not asserted.** An
    uninstrumented `net/http` service built with the `otelc go build` prefix on
    Go 1.26.5, pointed at a real server:
    `{"services":[{"name":"checkout","spanCount":25},{"name":"checkout-clean","spanCount":12}]}`
    — span counts matching request counts, full HTTP semconv attributes, plus
    `go.goroutine.count` and `go.memory.used` in `sigiro_metrics_sum`. `go.mod`
    was not modified, so no `otelc pin` step is needed.

    Three findings that changed the page:

    - **`go install` is affirmatively broken**, not merely undocumented. The
      module root has no importable command path:
      `found, but does not contain package go.opentelemetry.io/otelc/cmd/otelc`.
      Three module paths were tried.
    - **v1.0.1 does publish prebuilt binaries** for five platforms
      (`otelc-{darwin,linux}-{amd64,arm64}`, `otelc-windows-amd64.exe`), so the
      page documents a `curl` install. The original entry said clone-and-build
      was the only path.
    - **Upstream's own docs are stale**, which is where that belief came from:
      the v1.0.1 release notes and README still show only `git clone` +
      `make build` despite shipping those assets.

    Follow-up, not done: `install-with-ai.md` still says *"Go has no runtime
    agent. For Go you write a few lines of SDK setup instead"* and shows manual
    `initTracer` code. That is the weaker path this section supersedes, and the
    Go row should point here instead.

    Original entry below, kept for the reasoning:

    [`open-telemetry/opentelemetry-go-compile-instrumentation`](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation).
    Prefix the build (`otelc go build`) and the binary ships instrumented, with
    no source changes and no manual SDK wiring.

    Checked upstream 2026-08-03: **v1.0.0 tagged 2026-07-14** (v1.0.1 the same
    day), badged Status: Stable, an OpenTelemetry org project, Apache-2.0, Go
    1.25+, last push today, 397 stars, 164 open issues. It is real and it is
    released — but v1.0 is three weeks old, so verify against a live build
    before the docs call it preferred.

    Why it fits sigiro specifically: the docs' existing position is "if your
    code has no instrumentation yet, do not add it by hand"
    (`www/docs/quickstart.mdx:76`), which today routes readers to
    `/install-with-ai`. Compile-time instrumentation is the stronger version of
    the same claim for Go — deterministic instead of agent-generated, and it
    covers third-party libraries the user does not control. sigiro only needs
    the resulting OTLP, so this is a docs change, not a code change.

    One caveat to state honestly in the page: the documented install is `git
    clone` + `make build`, not `go install` of a released binary. A reader has
    to build the tool first. Check whether v1.0.1 publishes an installable
    artifact before writing the steps — if it does not, the page must show the
    clone-and-build path rather than an invented one-liner.

    Scope:

    - Add a Go section to `www/docs/quickstart.mdx` (165 lines, currently has no
      Go coverage at all — no per-language sections exist yet, so this sets the
      pattern for the others).
    - Point it at sigiro's OTLP endpoint via `OTEL_EXPORTER_OTLP_ENDPOINT`,
      consistent with the existing "no wrapper SDK" position (Phase 2 already
      rejected an npm wrapper for the same reason).
    - Relate it to `/install-with-ai` rather than contradicting it: compile-time
      is preferred **for Go**; the AI path stays the answer for languages
      without an equivalent.
    - Feeds item #29 (`/for/*` pages) — "go opentelemetry" is exactly the search
      intent that item is built around.

41. **List sigiro on [integrations.sh](https://integrations.sh/)** — an open
    registry of integration surfaces (HTTP APIs, MCP servers, GraphQL, CLIs)
    mapped to the credentials each one needs. Agent-facing discovery, which is
    the audience sigiro is built for.

    **This is self-service, not a submission.** Per
    `integrations.sh/publishing/`, you publish discovery metadata on your own
    domain and the listing is pulled from your site — no PR to merge, no queue.
    So this is a file we host, and it is the cheapest item in this section.

    Verified 2026-08-03 what sigiro.com already publishes:

    | Path | Status |
    |------|--------|
    | `/llms.txt` | 200 |
    | `/llms-full.txt` | 200 |
    | `/.well-known/api-catalog` (RFC 9727) | 200 |
    | `/.well-known/mcp/server-card.json` (SEP-1649) | 200 |
    | `/.well-known/integrations.json` | **404 — the only gap** |
    | `api.sigiro.com/openapi.json` | 200 |

    Four of the formats they read are already live, and `api-catalog` correctly
    points at both `api.sigiro.com/openapi.json` and the docs MCP server. So
    sigiro is already publishing enough to be discovered. Confirmed **not yet
    listed**: `GET integrations.sh/api/sigiro.com/discovery` returns
    `{"stored":false}` (404).

    Two things to do, in order:

    1. **Hit their detect/discover endpoint for `sigiro.com`** and read what it
       infers from the four files already published. That may be sufficient on
       its own, and it costs one request. Do this before writing anything.
    2. **Add `/.well-known/integrations.json`** (their v3 owner-declaration
       format: `{ version: 3, summary?, credentials?, surfaces? }`) only if
       step 1 comes back thin. This is the one file that lets the page reflect
       our own framing rather than an inference. Two surfaces to declare: the
       OTLP/HTTP API at `api.sigiro.com`, and the docs MCP server at
       `sigiro.com/mcp` (streamable-http, public, no auth — already described in
       the server card as `com.sigiro/sigiro-docs`).

    Credential facts are the part worth getting right, since it is what the
    registry is actually for: hosted sigiro uses TenantKeys, self-hosted Open
    mode needs none. That distinction is the useful signal to an agent deciding
    whether it can reach us without a key.

    Relates to #35/#36 as distribution, but unlike those it has no launch-timing
    constraint and no dependency on the repo being public — worth doing now
    rather than sequenced behind #30.

42. **Automate releases, or accept hand-cutting** (added 2026-08-04) —

    **Hand-cut again 2026-08-05 for `c840213`, and one new constraint came out
    of it: the remote BuildKit builder on the Hetzner box cannot build this
    image.** `docker buildx bake` there dies with `ResourceExhausted: cannot
    allocate memory` while compiling DuckDB. The box has 3.7 GiB total, ~1.4 GiB
    available and **no swap**, and it is simultaneously running production. The
    `--builder kamal-remote-ssh---root-91-98-118-235` line in
    `deploy/README.md` is therefore wrong; build locally with `--builder
    multiarch` (24 GiB, cross-compiles amd64 at native speed) and push to ghcr.

    `deploy/README.md` is stale in a second way: it says the deploy image comes
    from the on-box registry at `127.0.0.1:5555`, but
    `worldpol/config/deploy.yml` pulls `sigiro-hosted` from
    **`ghcr.io/sigiroai/sigiro:<sha>`**. ghcr is the real path. Worth correcting
    that file.

    The R2 binaries were published with `wrangler r2 object put` — no S3 token,
    same as v0.1.0 — so the credential decision below still blocks only the
    *unattended* workflow, not a release.

    `.github/workflows/release.yml` fails at preflight on every tag because the
    `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and `R2_ACCOUNT_ID` repo secrets
    are unset. v0.1.0 shipped anyway, cut by hand with `wrangler r2 object put`.
    So this is an automation ticket, not an availability one — nothing is broken
    for a user.

    **The decision, not the effort, is the whole item.** Two shapes:

    1. **Mint an R2 S3 token in the Cloudflare dashboard** and set the three
       secrets. The workflow already works as written. Verified 2026-08-04: this
       genuinely cannot be scripted — Wrangler 4.118.0 has no `r2 token` or
       `r2 credentials` command at any subcommand level, and the R2 Temporary
       Credentials API only *derives* from a parent token that must itself be
       created in the dashboard.
    2. **Change the upload step to `wrangler r2 object put`** with a
       `CLOUDFLARE_API_TOKEN`, which is the mechanism that already published
       v0.1.0 by hand and needs no SigV4 keys at all. ~10 lines of workflow diff
       against a dashboard visit that never has to happen again.

    Shape 2 is the smaller standing cost and matches how the artifacts actually
    got there. Worth noting the asymmetry that made this confusing: Wrangler's
    OAuth session can read and write R2 objects even though `wrangler whoami`
    lists no `r2` scope — only the first *SigV4* token is dashboard-bound.

    Blocks nothing. But every release stays manual until it is done, and a
    manual release is the kind of step that gets skipped under pressure.

43. **`0.1.0` in R2 was overwritten in place on 2026-08-05, and still is missing
    `x86_64-apple-darwin`.** Record this, because git and the download bucket now
    disagree about what "0.1.0" means: `LATEST` still says `0.1.0`, but the three
    archives under that prefix are built from `c840213`, not from the `v0.1.0`
    tag. Anyone who installed 0.1.0 before that date has different bytes under
    the same version string, and nothing distinguishes them.

    New checksums (the previous set is in this commit's parent, and was
    snapshotted to `/tmp/sha256sums-0.1.0-backup.txt` during the release):

    ```
    4a85bb89…  sigiro-0.1.0-aarch64-apple-darwin.tar.gz
    7a8fa596…  sigiro-0.1.0-aarch64-unknown-linux-gnu.tar.gz
    55628b1f…  sigiro-0.1.0-x86_64-unknown-linux-gnu.tar.gz
    ```

    The fix for the disagreement is to cut real version tags from here on, so a
    version means one set of bits.

    Original entry: **`x86_64-apple-darwin` is missing from the download bucket** (added
    2026-08-04) — `0.1.0/` holds three of the four targets the install script
    knows how to ask for. An Intel Mac gets `no 0.1.0 build for
    x86_64-apple-darwin. Run the container instead:
    ghcr.io/sigiroai/sigiro`, which is a deliberate honest error (`install:83`),
    not a crash.

    Cheap either way, and the choice is a positioning call rather than a
    technical one: build the fourth target (the workflow matrix already names it,
    so this closes itself the moment #42 lands), or decide Intel Mac is out of
    scope and say so in the docs. Doing neither leaves a supported-looking
    platform quietly failing.

44. ~~**The docs have no route to a person**~~ (added and **closed** 2026-08-04)
    — Blume rendered `/docs/*` with its own shell and never used
    `src/components/site/Footer.astro`, so the highest-intent reader on the
    property had no way to reach anybody.

    Fixed with a new `www/components.ts` setting Blume's `Footer` **layout
    slot** to the existing footer. Verified live: `sigiro.com/` carries the link
    twice (hero action plus footer) and every docs page carries it once.

    Worth keeping, because it is the mechanism for anything else the docs shell
    needs. Blume exposes twelve layout slots via `defineComponents({ layout })`,
    and three have **no built-in component at all** — they render nothing until
    set, which is exactly why this gap existed: `Footer` (props: `site`,
    `navigation`, `ui`), `PageHeader` and `PageFooter` (`page`, `headings`,
    `route`). The other nine replace a built-in: `Layout`, `Header`, `Logo`,
    `Search`, `Sidebar`, `MobileNav`, `Breadcrumbs`, `TableOfContents`,
    `Pagination`.

    `Header` was deliberately **not** overridden. The Blume docs header carries
    the search trigger, the sidebar toggle and the mobile-drawer wiring, so
    replacing it to match the landing bar means reimplementing those or losing
    search. The two-shell split `blume.config.ts` documents stays.

    Note this contradicts that file's "everything here is configuration, no
    custom MDX components" rule, and `components.ts` records why the exception
    is narrow.

45. ~~**A fresh data directory cannot answer a query — `json` fails to load after
    the filesystem lockdown**~~ (added 2026-08-04, **FIXED 2026-08-06**).

    **Root cause, now confirmed — it was the platform split the entry flagged as
    unverified.** `Cargo.toml` builds a different DuckDB per OS: Linux links the
    Dockerfile's `ar`-merged static archive (every fragment merged, so
    `json`/`icu`/`parquet` really are built in), macOS uses Cargo's `bundled`
    feature — DuckDB core from the vendored amalgamation, *without* those
    fragments — because DuckDB publishes no self-contained macOS static archive.
    Both `prewarm_extensions` and `lock_filesystem` asserted in comments that the
    three were always static. True on Linux, false on macOS.

    **This shipped in the published macOS binaries.** `release.yml` builds the
    two `*-apple-darwin` targets with no static-libs step (deliberately — a clone
    should build with no setup), so every downloaded macOS binary had it. The
    Linux downloads are the same artifact as the image and were always fine.

    A detail that made it look stranger than it was: `parquet` *did* land on disk
    as a transitive dependency of `ducklake`/`httpfs`, so `LOAD parquet` passed by
    luck. Deleting that file from a warmed dir still failed on `json` first, which
    is what proved none of the three were genuinely static there.

    **The fix is not "always INSTALL".** Verified against a DuckDB CLI: `INSTALL`
    on a statically linked extension is *not* a no-op — it downloads from the
    repository, writes a `.duckdb_extension`, and flips `install_mode` from
    `STATICALLY_LINKED` to `REPOSITORY`. In the Dockerfile that write lands in
    `/opt/sigiro/extensions`, read-only on the distroless image. So an
    unconditional install would have traded a working static load for a needless
    download at best and a permission failure at worst.

    Shipped instead: `storage::extensions_needing_install` asks DuckDB which of
    `LOCKDOWN_PRELOADED_EXTENSIONS` are `STATICALLY_LINKED` and caches only the
    rest, and `lock_filesystem` builds its `LOAD` batch from that same constant so
    the two cannot drift. Verified on both paths:

    | Path | Prewarm behaviour | `/v1/services` | `json`/`icu` |
    |---|---|---|---|
    | macOS (`bundled`) | caches parquet, json, icu | 200 | both work |
    | Linux image (static) | "no files needed", downloads nothing | 200 | both work |

    The Linux image's baked extension set is byte-identical to before. New guard
    test `pool::tests::lock_filesystem_loads_are_prewarmed` prewarms a temp dir
    and asserts the real `LOAD` batch succeeds — it asserts the *outcome*, so it
    stays honest on whichever platform runs it. That closes the class of bug: this
    surface had now failed three times because the prewarm set and the lockdown
    set each encoded their own belief and nothing checked they agreed.

    Original report follows.

    Found while verifying #40, and
    **reproduced twice** against `target/release/sigiro` 0.1.0 on macOS arm64.

    `sigiro fetch-extensions` succeeds on a new `SIGIRO_DATA_DIR` and reports
    success. `sigiro serve` then starts, `/health` returns 200, and
    `/v1/services` returns **400**:

    ```
    failed to preload DuckDB extensions before locking filesystem access:
    IO Error: Extension ".../extensions/v1.5.3/osx_arm64/json.duckdb_extension"
    not found. Extension "json" is an existing extension.
    Install it first using "INSTALL json".
    ```

    This contradicts the quickstart's own promise: the documented first run is
    `sigiro serve` then `curl /v1/services`, and on a clean machine the second
    step fails.

    The two halves disagree about what the binary contains.
    `prewarm_extensions` (`src/storage.rs`) deliberately installs only
    `ducklake`, `postgres`, `httpfs`, `system_stats`, `quack`, and its comment
    states that `core_functions`, `json`, `parquet`, `icu` and `autocomplete`
    "are statically linked into the binary via the prebuilt DuckDB static
    bundle, so they need no baked file." But `lock_filesystem`
    (`src/pool.rs`) runs `LOAD parquet; LOAD json; LOAD icu;` and its comment
    asserts the same premise, crediting **the Docker build** with merging every
    static fragment. At serve time `json` is looked for as a *file* and is
    absent.

    What is verified, and what is not:

    - **Verified:** the failure, the exact error, and that
      `SET extension_directory` is applied (`src/pool.rs`) before the `LOAD`
      batch runs.
    - **Not verified:** *why* `LOAD json` resolves to a file here. The obvious
      theory — that a local `cargo build --release` links fewer static
      fragments than the Docker build — was **not** confirmed; isolating it
      needs a DuckDB CLI, and `sigiro query` cannot stand in because it
      requires Tailscale and fails before reaching DuckDB. Do not fix this from
      the theory; reproduce it first.

    Fix shapes, once the cause is known: add `json`/`icu` to
    `prewarm_extensions` (cheap, works either way), or make `lock_filesystem`
    tolerate a `LOAD` failure for an extension that is genuinely built in. The
    first is the smaller change and does not depend on resolving the static
    question.

    Worth noting the same function's doc comment records a previous silent
    version of this same class of bug — prewarming a directory serve-time never
    read. This surface has now failed twice for a mismatch between what the
    prewarm caches and what serve-time wants.

46. **Deduplicate the shell commands across the site and docs** (added
    2026-08-04) — the install and OTLP lines are maintained in several places at
    once. `curl -fsSL https://sigiro.com/install | sh` appears in `site.ts`'s
    `INSTALL` block, again in its four-step terminal, and again in
    `quickstart.mdx`; the `docker run` line does the same; the
    `OTEL_EXPORTER_OTLP_ENDPOINT` exports appear in `site.ts` three times plus
    `quickstart.mdx` and `hosted-onboarding.md`.

    This is a real defect class, not tidiness: the `openapi.json` fix in
    `3603713` was exactly this — a string that drifted from what the site
    actually served.

    **The mechanism now exists on both sides, and they are different.** For the
    `.mdx` pages, `components.ts` takes an `mdx` map alongside `layout`, and
    anything registered there is available in every page with no import and
    ships no JavaScript when it is `.astro` (Blume's own islands doc says to
    prefer this over an island for static reuse). Blume has **no** `Snippet` or
    `Include` primitive — verified against all 41 components in
    `packages/blume/src/components/content/` — so the override is the idiomatic
    route.

    But an MDX override does not reach `site.ts`, which is a TypeScript module
    the landing page reads. So the shape is: the commands as exported constants
    in one module, imported by `site.ts` **and** wrapped in a thin `.astro`
    component registered in the `mdx` map. One source, two consumers.

    Do the audit before writing anything — the terminal's copies are
    *pre-highlighted per line*, so they are not a plain string swap.

47. **Score 100% on isitagentready.com** (added 2026-08-05) — goal:
    `https://isitagentready.com/sigiro.com` reports a perfect score. It is
    Cloudflare's agent-readiness scanner, and it grades a site on ~18 checks in
    five categories: Discoverability (robots.txt, sitemap, `Link` response
    headers, DNS-AID), Content Accessibility (Markdown content negotiation), Bot
    Access Control (AI bot rules, Content Signals, Web Bot Auth), API / Auth /
    MCP (API Catalog, OAuth discovery, OAuth Protected Resource, `auth.md`, MCP
    Server Card, A2A Agent Card, Agent Skills, WebMCP), and Commerce (x402, MPP,
    UCP, ACP).

    **Read this before touching the entry: measure with the API, never by hand.**
    The results page is client-rendered so `/sigiro.com` 404s to `curl`, but the
    scanner itself is scriptable:

    ```bash
    curl -s -X POST https://isitagentready.com/api/scan \
      -H 'Content-Type: application/json' \
      -d '{"url":"https://sigiro.com"}' --max-time 180
    ```

    It returns every check with a status, plus per-check `details` and a `steps`
    trace explaining *why* something failed. Hand-probing guesses at the paths
    and criteria the scanner uses and gets them wrong — that has now produced a
    false "gap" in this file **twice** (see the dnsAid note below). Use the API.

    **Measured 2026-08-06 — `level: 4`, `"Agent-Integrated"`.**

    | Category | Result |
    |---|---|
    | Discoverability | **4/4 pass** — `robotsTxt`, `sitemap`, `linkHeaders`, `dnsAid` |
    | Content Accessibility | **1/1 pass** — `markdownNegotiation` |
    | Bot Access Control | 2 pass (`robotsTxtAiRules`, `contentSignals`), `webBotAuth` neutral |
    | Discovery | 4 pass (`apiCatalog`, `mcpServerCard`, `webMcp`), 5 fail |
    | Commerce | all 5 neutral — scanner itself says "not a commerce site" |

    Recently closed: `linkHeaders` (#49) and `markdownNegotiation` (#50).

    **`webMcp` now passes, and the 2026-08-05 open question is answered.** That
    note said the failure was "suspicious — Blume ships WebMCP on by default and
    the island script is in the served HTML", and suspected the scanner was
    checking something narrower. It was not narrower, it was a different **URL**:
    Blume renders the component only from its docs layouts, so `/docs/*` had it
    and the landing page did not, and the scanner probes the root. Moving it into
    `Base.astro` (`7654089`) flipped the check to pass with **3 tools via
    `imperative_api`**. Generalises: when a check fails on a site-wide feature,
    confirm *which URL* the scanner requested before doubting the check.

    **`dnsAid` passes.** `_index._agents.sigiro.com` and `_mcp._agents.sigiro.com`
    are both published as SVCB, priority 1 (ServiceMode), `alpn=h2 port=443`,
    `AD: True` on Cloudflare and Google DoH. DNSSEC is complete — DS `2371 13 2`
    at the registry, DNSKEY at the zone, single-provider Cloudflare NS, so no
    multi-signer question. **This entry previously recorded dnsAid as failing;
    that was a bad measurement, not a real gap** — the query used the bare parent
    `_agents.sigiro.com`, which correctly returns NODATA because the records live
    one label down. Query `_index._agents`, RR type 64.

    **Correction: the MCP Server Card path in this file was wrong.** The scanner
    finds it at `/.well-known/mcp/server-card.json`, not `/.well-known/mcp.json`.
    Both exist; the latter is Blume's discovery pointer.

    **Remaining fails, in the order worth doing:**

    1. **Agent Skills** — index not found. This is item #48, and it is the only
       remaining fail that is unambiguously ours to fix and true of the product.
    2. **A2A Agent Card** — not found. Needs an actual A2A endpoint first.
    3. **`auth.md`** — not found.
    4. **OAuth discovery / OAuth Protected Resource** — both fail. sigiro.com is
       a marketing + docs site with nothing to authorize; these describe
       `api.sigiro.com`, not this origin.

    **The honest problem with "perfect score", restated against real data.**
    Commerce is **not** a gap: the scanner scores all five `neutral` and says so
    itself ("not a commerce site"), and `webBotAuth` is explicitly
    "informational only". Neutral is not failure, so those six were never
    costing anything. The genuine question is only fails 2–4: publishing an
    OAuth document or an agent card for services that do not exist would be a
    false claim, which `.agents/product-marketing.md` forbids. So the reachable
    target is **fail 1 (#48)**, after which every check that is true of this
    site passes and the rest are a product decision, not a checklist item.

    Related: #41 (integrations.sh listing) and #29's answer-engine reasoning are
    the same underlying bet — that the next visitor is an agent.

48. **Publish Agent Skills for sigiro** (added 2026-08-05) — Blume ships the
    machinery and it is one config line, but it is **not zero-config**:
    `skills: z.string().min(1).optional()` in Blume's schema has no default, so
    nothing is published until a directory exists. `/.well-known/agent-skills.json`
    404s today, and it is one of the ~18 checks #47 counts.

    Set `skills: './skills'` in `www/blume.config.ts` and write the skills. The
    build copies each subdirectory holding a `SKILL.md` under
    `/.well-known/agent-skills/` — a lone `SKILL.md` verbatim, a skill with
    supporting files as a `.tar.gz` — and emits `index.json` with a SHA-256 per
    skill, per the Agent Skills Discovery RFC.

    **The point is not to have the endpoint; it is that an agent installs a
    skill and then uses sigiro correctly without a human explaining it.** So the
    content matters more than the wiring, and each one has to be verified by
    actually running it, not by rendering it.

    Candidates, in the order they earn their place:

    - **`debug-a-production-failure`** — the loop the product exists for: call
      `/v1/anomalies` to find what changed, `/v1/investigate` for ranked
      findings, then paste the returned `drill_down_sql` into `/v1/query`. This
      is the one that turns the API into a method.
    - **`instrument-my-service`** — point OTel at sigiro; for Go, use
      compile-time instrumentation rather than editing source (see #40 and
      `/docs/install-with-ai`). This one already has verified prose to draw on.
    - **`query-my-telemetry`** — the table surface (`sigiro_spans`,
      `sigiro_logs`, `sigiro_metrics_*`) and the SQL dialect, so an agent writes
      a working query without discovering the schema by trial and error.
    - **`self-host-sigiro`** — install, run, confirm; the quickstart as a
      procedure an agent can execute.

    Do not write a skill per endpoint. `/openapi.json` already describes the
    endpoints, and a skill that restates it adds tokens without adding
    capability. A skill should encode a *workflow* that the spec cannot.

    Note the same content problem as #45: these will duplicate the shell
    commands and SQL that already live in `site.ts` and the docs. Decide the
    single source before writing four copies.

    **Updated 2026-09-05:** still unshipped (`blume.config.ts` has no `skills:`
    key; the endpoint 404s). The first candidate is now being written as one
    skill, **#94 "Debug an application with Sigiro"**, in the go-to-market
    plan below; it ships through skills.sh (#98), the Cursor plugin (#97) and
    the Claude Code plugin (#100) from a single `SKILL.md`. Set `skills:` in the
    same change.

49. ~~**Discovery `Link` headers are missing on a Cloudflare server build**~~ —
    **FIXED UPSTREAM AND LIVE 2026-08-05.** Blume PR
    [#165](https://github.com/haydenbleasel/blume/pull/165), merged as
    `b62278b`. Closes a Discoverability check in #47.

    Live now: the homepage carries the RFC 8288 `Link` header with all four
    relations, and `/.well-known/api-catalog` serves `application/linkset+json`.

    **Testing changed the fix, and this is the lesson worth keeping.** Correcting
    the `output === "static"` guard alone did nothing. `@astrojs/cloudflare`
    writes its *own* `_headers` during the build (an immutable `Cache-Control`
    for `/_astro/*`), and Blume's opt-out check was `existsSync(dist/_headers)` —
    so it read an adapter-generated file as a user opt-out and skipped silently.
    The shipped PR carries three changes: the gate, reading the opt-out from
    `public/_headers` where the doc comment always said it lived, and merging
    rather than overwriting an existing file.

    It also fixed a second bug that was never scoped: the same skipped function
    emits `Content-Type` for the extensionless well-known files, so the API
    catalog had been served with **no `Content-Type` at all**.

    Unit tests on the new predicate passed while the feature stayed broken. Only
    a real Cloudflare server build exposed it — worth remembering the next time a
    build-output fix looks obviously correct.

    Original entry: **Discovery `Link` headers are missing on a Cloudflare server
    build — fix upstream in Blume**. Verified against production: no
    `Link` header on `sigiro.com/` or any docs page, so the RFC 8288
    agent-discovery hop is absent. It is a Discoverability check in #47.

    **Not a config error — a gap in Blume.** `buildHomeLinkHeader()`
    (`ai/link-headers.ts`) builds the correct value and has exactly three
    consumers, each of which excludes this deployment:

    | Consumer | Guard | Us |
    |---|---|---|
    | `cli/commands/build.ts:156` → `_headers` | returns early unless `output === "static"` | ✗ we are `output: 'server'` |
    | `cli/commands/build.ts:285` → `.vercel/output/config.json` | Vercel adapter only | ✗ we are Cloudflare |
    | `astro/integration.ts:162` → `res.setHeader("Link", …)` | hook is `astro:server:setup` | ✗ dev server only |

    So Netlify/CF **static**, Vercel, and `blume dev` all get the header;
    **Cloudflare server** and Node server get nothing.

    The root cause is a conflation inside `emitHeaderFiles`, and its own comment
    shows the slip: *"Server adapters set the Content-Type on the Response
    directly, so this is static-only."* That is true of the **charset** half of
    that file and false of the `Link` half — no server adapter sets `Link`. One
    correct conclusion ("the charset lines are static-only") was applied to the
    whole function.

    Fix shape: emit `_headers` carrying only the `Link` line when
    `output === 'server'` and the adapter is `cloudflare`, writing to
    `deployStaticDir()` — the same function PR #161 fixed, so this is the same
    class of bug and the fix has a precedent. Evidence the mechanism works: a
    `dist/client/_headers` already exists and is served, carrying only an
    `/_astro/*` cache rule.

    Ship it as a PR to `haydenbleasel/blume` rather than a local patch. Both
    previous fixes (#161, #162) merged, so upstream is receptive, and a local
    `public/_headers` here would mask the bug for everyone else.

50. ~~**`Accept: text/markdown` negotiation does not work on our deployment**~~
    (added 2026-08-05, **CLOSED 2026-08-06**) — fixed upstream in
    [blume#166](https://github.com/haydenbleasel/blume/issues/166) (`f88d256`)
    and live on sigiro.com via the revendored tarball (`751958f`):

    ```
    curl -H 'Accept: text/markdown' https://sigiro.com/
    -> content-type: text/markdown; charset=utf-8
       vary: Accept
       x-markdown-tokens: 385
    ```

    Same for `/docs/quickstart/`. Browsers and header-less requests still get
    HTML; `/index.md` is still asset-served.

    **The maintainer's fix diverged from my proposal in one important way, and
    the difference is the lesson.** I proposed generated Astro middleware plus
    `run_worker_first`. Piece 1 was a dead end *even after* `run_worker_first`:
    `@astrojs/cloudflare`'s handler answers prerendered pages from the ASSETS
    binding (`matchStaticAsset` / `fallbackToAssets`) **before** `app.render`,
    which is the only place middleware runs, and `App.match()` excludes
    prerendered routes anyway. So a content request that *does* reach the Worker
    still never touches middleware. The negotiation had to live at the Worker
    fetch-handler level. Shipped shape:

    - `dist/server/wrangler.json` rewritten: `assets.run_worker_first` grouped
      into route globs (ours: `/`, `/docs`, `/docs/*`, `!/docs/*.md`,
      `!/docs/*.mdx`), merged with user rules and swept for redundancy —
      Wrangler rejects covered rules — within the 100-rule / 100-char limits.
    - `main` repointed at a generated `blume-worker.mjs` wrapping the adapter
      entry. It serves the prerendered `.md` mirror from the assets binding when
      the client prefers Markdown, and delegates everything else untouched, so
      only content routes pay the extra hop.
    - **Cloudflare does not apply `_headers` to worker-first routes**, so the
      wrapper re-stamps what the static layer would have added on the routes it
      takes over — the #49 homepage `Link` header, `x-markdown-tokens`, and the
      Markdown charset. Verified the `Link` header survived on the live root;
      that clobbering was the actual regression risk, not the new behaviour.

    `agent-readability.json` now advertises `contentNegotiation: text/markdown`,
    which had been a false claim on this deployment.

    **Tarball retired and then re-cut, both on 2026-08-10.** npm published
    `blume@1.4.2`, so `www` moved to a released version and `www/vendor/` was
    deleted. Removing the `run_worker_first` negatives in the same pass exposed a
    query-string regression in the replacement mechanism (blume#175), which
    upstream fixed hours later in `156d5ff` — behind a shut release gate. So the
    tarball is back as `blume-1.4.2-main-156d5ff.tgz`, this time to buy the
    deletion of those negatives. See item 69 for the measurements. Node server
    builds still have this gap upstream; that needs its own mechanism.

    **Vendoring retired for good on 2026-08-11.** npm published `blume@1.4.3`,
    which lists `156d5ff` in its changelog. Verified out of the published
    package before the upgrade: `redirectLocation` present in both `dist` and
    `src`, `describedby` present, and the pre-fix `location: redirect[0]` form
    absent. `www` now depends on the plain range `1.4.3`; `www/vendor/` and both
    `.gitignore` negations are deleted, and the `blume-release-watch` cron is
    cancelled. Re-verified on production: all six retired URLs still return 302
    with the query string intact, Markdown negotiation still returns
    `text/markdown` with `Vary: Accept`, the homepage `Link` header survives, and
    the per-page `describedby` links are live. The `run_worker_first` negatives
    stay deleted.

    Superseded detail from the original entry, kept because the negative result
    is worth not re-deriving:

    The content already exists and is correct — only the negotiation is missing:

    ```
    curl -H 'Accept: text/markdown' sigiro.com/docs/quickstart/  -> text/html
    curl                            sigiro.com/docs/quickstart.md -> text/markdown; charset=utf-8
    ```

    Third instance of the same Blume pattern as #161 and #165: negotiation has
    two implementations, `deploy/vercel-negotiation.ts` (needs
    `.vercel/output/config.json`) and the `astro:server:setup` middleware in
    `astro/integration.ts` (dev only). Cloudflare server gets neither.

    **I implemented the obvious fix and it did not work — worth remembering.**
    Generated Astro middleware, correct routes, bundled into the Worker
    (`virtual_astro_middleware.mjs`, 3.29 KiB), typecheck clean, helpers verified
    in isolation. It never executes: **the Cloudflare ASSETS binding serves
    prerendered pages before the Worker script runs.** `/docs/quickstart/` is a
    static file in `dist/client`, so middleware is never invoked. The
    discriminating test: `/docs/quickstart/` → 200 HTML (ASSETS), `/mcp` → 405
    (reaches the Worker).

    A real fix needs two coordinated pieces: the middleware, **plus**
    `assets.run_worker_first` written into the `wrangler.json` the Cloudflare
    adapter generates. It takes an array of route patterns, so it can be scoped
    to content routes instead of blanket-enabled — blanket would put the Worker
    in front of every static asset. Stopped there rather than guess at how that
    interacts with `@astrojs/cloudflare`'s own output, and said so in the issue.

    **Cloudflare's own Markdown for Agents is not an option:** their docs confirm
    it is Pro/Business/Enterprise only, and we are on a Free zone. It would also
    be worse — edge conversion of rendered HTML, versus Blume's authored `.md`
    with no nav chrome.

    A local `www/src/middleware.ts` is equally dead: ASSETS still pre-empts it,
    so it needs `run_worker_first` either way.

    Work is preserved in the `fix-cloudflare-markdown-negotiation` worktree of
    the blume clone if upstream wants it built.

51. **Prove the product claim with a benchmark** (added 2026-08-06) — the pitch
    is that sigiro helps repair software and agents in **fewer turns, fewer
    tokens, and with higher accuracy**. Today that is an assertion with no
    measurement behind it. It needs a repeatable, paired experiment that
    isolates sigiro as the only variable and reports those three numbers, so the
    claim can be cited instead of argued.

    **Decision basis:** every GTM surface already leans on this claim, so it is
    the highest-value unmeasured assertion in the product. It is also
    falsifiable — if the delta is not there, the positioning has to change, and
    that is worth knowing early rather than after a launch.

    Blocker: no substrate exists yet. Public coding benchmarks
    ([DeepSWE](https://deepswe.datacurve.ai/) and similar) are
    implement-a-feature tasks in sealed repos with no telemetry, so sigiro has
    nothing to consume and the honest result is a null one. Telemetry-in-the-loop
    repair is essentially unbenchmarked, which is the opportunity and also the
    cost.

    Prior art worth reading before designing anything: `Tura-AI/tura` publishes
    the same shape of claim on DeepSWE and concedes in its own README that no
    ablation isolates the cause — the confound to avoid. `kimjune01/deepswe-run`
    audited the substrate and found 4 of 113 reference solutions fail their own
    verifiers.

    **Researched 2026-08-06. The blocker is confirmed, and a substrate exists.**
    Decided that day: this is aimed at a **published** result, not an internal
    go/no-go.

    **The gap is real and now evidenced.** An arXiv full-text search for
    `abs:"code patch" AND abs:telemetry` returns **zero** results; so does
    `all:telemetry AND all:"program repair"`. The field splits cleanly into
    coding benchmarks with no telemetry and RCA benchmarks with no patching.
    Nothing measures code repair driven by runtime telemetry.

    **Substrate: [ORCA-bench](https://github.com/ORCA-bench/ORCA-bench)**
    (arXiv 2607.28545, Aug 2026; Apache-2.0 code, CC-BY-4.0 data; pushed
    2026-08-05). Built on the OpenTelemetry demo with k6 load and six days of
    recorded telemetry. 755 public tasks / 77 incidents over 11 flagd fault
    flags. **Correction 2026-08-07: there is no "Verified" subset** — the string
    appears nowhere in the repo, and the 40-task sizing this entry originally
    quoted was imported from SWE-bench-Verified vocabulary by mistake. The real
    split has **217 medium tasks**, which is what makes the design below viable.
    "77 incidents" is also unsourced: the repo has **42 rubrics** and a schedule
    of 126 events over 10 days; the paper says 1,079 tasks total.
    Ground truth is a JSON schema linking fault to **source file and line**,
    signed off by two SREs; grading is an LLM judge with human agreement
    κ_w = 0.90; the leaderboard publishes RCA accuracy, hallucination rate and
    whole-submission token totals. It ships a **built-in oracle agent**, which
    guards the `kimjune01/deepswe-run` failure mode.

    **Why it fits where SREGym did not.** The agent's entire telemetry surface is
    one paragraph in `harbor-template/instruction.md.template` plus a
    `GRAFANA_URL` env var, so swapping in sigiro holds agent, model, task set,
    judge, timeouts and token accounting constant by construction. And its
    published Finding #4 is the failure sigiro exists to remove: **26-40% of
    telemetry tool calls error out or return empty** because models write
    malformed PromQL/Lucene.

    **Two sigiro properties were verified as load-bearing, not assumed:**
    - `DiagnoseRequest` takes explicit `from_ts`/`to_ts` in **microseconds** with
      nothing "now"-relative (`src/diagnose/mod.rs:35-44`), so replaying
      six-day-old recorded telemetry is a first-class query. Had `diagnose` been
      `now - window`, this substrate would be unusable.
    - OTLP **profiles** are genuinely implemented (`ProfilesService` on
      `profiles::v1development`, `src/ingest.rs:24-26`), and the OTel demo ships
      an eBPF profiler whose exporter can be pointed straight at sigiro. Nobody
      else in this space has a profiles story. Caveats: `v1development` is an
      unstable proto so pin the collector digest; it is Linux-only and
      privileged; ORCA-bench's published snapshots contain **no** profiles, so
      getting them means re-collecting.

    **The work: `snapshot2otlp`.** The telemetry is historical, so there is no
    live pipeline to tee — sigiro must be **backfilled** offline once per
    snapshot (Jaeger spans out of OpenSearch via the collector's own
    `jaegertranslator`, OTel log docs near-identity, Prometheus **remote-read**
    for metrics). Then bake a derived image so each trial is just `sigiro serve`
    over a read-only data dir.

    **The four-arm / 40-task design above was GRILLED AND REJECTED on
    2026-08-07, before any money was spent.** Keep the substrate; throw out the
    design. Three findings, each measured against ORCA-bench's own source:

    - **F1 — the substrate measures diagnosis, not repair.** The deliverable is
      `/app/report.md` (Summary / Timeline / 5 Whys / Remediation prose). **No
      patch, no diff, no rerun verification.** The paper's own title is "How
      Ready Are Language Model Agents for *Oncall*". This entry already says
      "nothing measures code repair driven by runtime telemetry" and then picks
      an RCA benchmark — a reviewer reads those two sentences together and stops.
      **Not fixable by design.** Either the claim narrows to *reach a correct
      root cause*, or the substrate changes (see #62).
    - **F2 — 12 medium tasks is powered at ~1%, so the null was pre-determined.**
      The published ±3.00pp SE comes from **217** medium tasks, not 40. At 12
      tasks SE is 12.8pp. Simulated paired power at their 26.6% base rate: +15pp
      effect → **0.3%**; +40pp → 3.5%. Even total dominance needs 5 of 12 tasks
      to flip for p<0.05. **Three trials do not triple n** — they cut
      within-task variance and add no tasks.
    - **F3 — the Hard tier is at or below chance, and there is no no-telemetry
      arm.** 11 fault flags means guessing scores 9.1%; published Hard accuracy
      is Sonnet 8.96, Opus 8.46, GPT-5.5 6.47. That tier cannot discriminate any
      tool from any other. Worse, arm D was aimed one notch too shallow: the
      deeper risk is that telemetry adds nothing over **reading the source**,
      which the agent has at `/app/opentelemetry-demo`.

    **The replacement design.** Same money, ~30× the power on the contrast that
    matters:

    1. **Arm 0 kill-shot pilot first (~$50, one day):** Grafana vs *telemetry
       endpoint deleted, source only*, 20 medium tasks, 1 trial. If Arm 0 ≈ Arm
       A, the substrate cannot support any telemetry claim and the whole budget
       is saved. Nothing else runs until this passes.
    2. **Two arms, not four: C vs D across all 217 medium tasks** (434 trials,
       ~0.65B tokens — comparable to the rejected 480-trial plan). A and B become
       a later ablation. Pre-register **one** primary endpoint:
       `rca_accuracy_incident_medium`, C vs D, one-sided, α=0.05. The old
       criterion was an AND of three conditions at p<0.32 written against the
       wrong arm — passing it would have proved nothing.
    3. **Drop the Hard tier** (F3) and make the **de-labelled subset primary**,
       not optional — see the self-labelling note below.
    4. Report **turns-to-first-correct-hypothesis** and **tokens on correct
       trials only**. Episodes are wall-clock bounded (`timeout_sec = 3600`,
       **no turn cap**), so a faster backend buys *more* turns, and raw token
       means reward the arm that gives up fastest. Under that budget "fewer
       turns, fewer tokens, higher accuracy" is not three claims — it is one
       claim and two artifacts of the time limit.

    **The containment audit is broken in sigiro's favour and must be fixed
    first.** ORCA's own `classify_cmd()`, executed against sigiro-shaped calls,
    returns `other` for `/v1/diagnose`, `/v1/services`, `/v1/traces/{id}` and
    `/v1/anomalies` — only `/v1/query` matches, and only by accidental substring.
    "sigiro used 3 telemetry calls where Grafana used 13" would be substantially
    an artifact of **not counting sigiro's calls**. Extend the regex, publish the
    diff, and re-run the Grafana arms through the patched classifier.

    Also pin CPU/memory/IO per arm before running: arm A is OpenSearch (bumped to
    2 GB to stop OOM kills) + Prometheus + Jaeger over a 50 GB snapshot, arm C is
    one Rust process. Otherwise the reviewer asks whether this benchmarked a data
    architecture or a box allocation.

    **Two constraints to state rather than discover.** The leaderboard requires
    an *unmodified* dataset, so **a sigiro row is impossible**; the route is an
    independent paired study using their code, tasks, judge and metrics, with the
    variant published on the Harbor Hub as GSO does. And ORCA-bench's telemetry
    **self-labels its faults** (`feature_flag.key=…`, plus error events naming
    the flag), so any tool good at trace search is flattered relative to
    production — disclose it, and run a de-labelled subset if budget allows.

    **The judge confound was mis-located — corrected 2026-08-07.**
    `check_prediction.py` is **not** coupled to PromQL/Lucene:
    `JUDGE_PROMPT_SCORING_FOOTER` says verbatim "Scoring is based on content
    correctness only; do not penalize or reward for formatting, prose style, or
    structural polish", and the per-cluster check is `family_match` — did the
    agent cite *a query* targeting the metric family — which is query-language
    agnostic. SQL is not penalised.

    The coupling is one level up, in the **instruction template** (Section 3,
    rule 2), which *requires* "the exact PromQL query" and "the exact Lucene
    query" in the report body. That is the agent's own prompt. So this entry's
    claim that swapping `GRAFANA_URL` holds everything "constant by construction"
    is **false** — the reasoning scaffold must be edited too. Fix: rewrite that
    rule **identically in all arms** to "the exact query you ran, in your tool's
    query language", and publish both templates.

    **Self-labelling is worse than recorded, and it changes the plan.** Measured
    across all 42 rubrics: **42/42** carry the fault flag's name in the symptom
    telemetry, 26/42 reference a `feature_flag` attribute, 32/42 carry
    source_file/line. The answer is in the evidence for **every task**. Paired
    validity survives (both arms are flattered equally) but external validity
    does not — and with the answer in the telemetry *and* the 11-flag list in the
    source, the task is **retrieval, not diagnosis**, which is exactly the regime
    where arm D (search ergonomics) most likely equals arm C (data architecture).
    The de-labelled subset is therefore **primary, not budget-permitting**.

    **Check before trusting arm C:** `/v1/anomalies` must be a pure function of
    the stored window. If it computes a trailing baseline at *ingest* time, a
    six-day burst backfill gives it a different baseline than live streaming
    would. `/v1/diagnose` is already proven safe here (explicit microsecond
    windows). Minor: their compose sets `--web.enable-admin-api`, so a TSDB
    snapshot beats remote-read for fidelity.

    **Do not compete on sealed-repo coding benchmarks.** SWE-bench Verified is
    500 instances across 12 *library* repos — no service, no workload, no
    cross-process call in the failing path — so OTel auto-instrumentation emits
    approximately nothing. Worse, the "runtime evidence helps agents" result is
    already published and was won with a **debugger**: *Debug2Fix* (arXiv
    2602.18571) reports >20% improvement on GitBug-Java and SWE-bench-Live with
    ablations. On those substrates the cheap substitute for sigiro is `pdb`.

    **Ruled out:** AIOpsLab and ITBench repeat the SREGym failure (~11 of 35 and
    ~2 of 34 faults are app-level; `kubectl` substitutes for telemetry, and both
    wire telemetry into the action space in code). GSO has a real perf signal but
    `cProfile`/`py-spy` are a complete local substitute and there is no
    OTLP-profiles path for in-process Python. See [[projects/sigiro-benchmark.md]]
    for the SREGym analysis and the full survey.

62. **The empty niche: repair the agent, not the service** (added 2026-08-06) —
    the one place where being first is available. Verified 2026-08-06: active
    2026 method work on agent-failure attribution (AgentDebugX 2607.18754, FALAT
    2606.00765, StepFinder 2606.03467) **all evaluates on Who&When, TRAIL or
    MAST, which are static text-trace annotation datasets.** No execution, no
    OTLP, no repair verified by rerun. **No benchmark exists in which an agent
    reads `gen_ai.*` spans and fixes the agent that emitted them.**

    **Decision basis: sigiro already does the analysis half.** `/v1/diagnose`
    computes per-(provider, model) input/output token sums and
    `gen_ai.response.finish_reasons` distributions
    (`src/diagnose/mod.rs:436-469, 1394-1414`). This is the capability no
    infrastructure benchmark can measure, and #51's substrate cannot exercise it
    either.

    The substrate is unusually ready. The OTel demo now ships `src/agent/` — a
    LangGraph shopping agent with Traceloop instrumentation, an MCP client,
    `GRAPH_RECURSION_LIMIT`, and **VCR cassettes for deterministic LLM replay** —
    emitting `gen_ai.*` spans inside the same traces as the shop. Inject
    agent-loop pathologies (a tool returning a subtly wrong shape so the agent
    retries to the recursion limit, a prompt change that inflates output tokens,
    an MCP tool that times out) and the fault surfaces *only* in `gen_ai.*` spans
    correlated with request traces. The cassettes make it reproducible.

    Blocker/risk: this is the highest-risk item on the board, because "first" and
    "nobody cares" are hard to tell apart. Ten tasks with an oracle arm and fully
    public artifacts is the smallest credible version. Do #51 first — it
    establishes the method against a third-party baseline, and this reuses it.

63. **Sigiro monitors sigiro: self-instrumentation into an operator tenant
    replaces four of #59's six checks** (added 2026-08-07) — the product is its
    own health monitor, and the mechanism already ships. What is missing is
    three env vars, one tenant, and one instrumentation bug.

    **Production self-instrumentation is OFF, verified 2026-08-07.**
    `docker inspect linguabench-sigiro-hosted` returns **zero** `OTEL_*` vars.
    The root `env.clear` in `worldpol/config/deploy.yml` does carry
    `OTEL_EXPORTER_OTLP_ENDPOINT`, but **Kamal accessories do not inherit the
    root `env`** — which is exactly why it never reached sigiro. The app
    containers ship telemetry *to* sigiro; sigiro ships none about itself. So
    "is sigiro healthy" is answerable today only by `docker inspect`/`docker
    logs` over SSH, which is #59's gap restated.

    **Decision basis: #59's blocker is the trust boundary, and the tenant model
    already solves it.** #59 concluded the six operator checks "are not a
    migration; they are a different audience", needing a new computed
    `GET /admin/health` because `/admin/status` is a flat key/value read with no
    query path. But sigiro already has a per-tenant query path with a
    control-plane-enforced boundary. Point `OTEL_EXPORTER_OTLP_ENDPOINT` at the
    container's own loopback with a key minted for a dedicated `sigiro_ops`
    tenant, and the telemetry lands where customers cannot read it. The
    "different audience" becomes a different **tenant** — the thing the hosted
    product exists to express — not a new endpoint.

    Verified end to end on the published image `ghcr.io/sigiroai/sigiro:61fabac`
    (local, both storage modes, no production mutation): self-export
    authenticated as `sigiro_ops` and attached its own DuckLake
    (`metadata_schema=tenant_sigiro_ops`); a sibling `customer_acme` key then got
    `/v1/services` → `[]` and `/v1/diagnose service=sigiro-hosted` →
    `coverage.has_spans=false`, zero findings. **Isolation proven both ways with
    the auth that exists today.**

    With an injected fault (malformed OTLP bodies), `/v1/diagnose` returned
    HTTP 200 in 0.14 s and isolated it correctly: `otlp_http_traces` err%
    **94.08 vs a 0.0 baseline**, the exact protobuf decode error text in
    `log_patterns`, and the neighbouring operations clean.

    Four of #59's six checks are then plain `/v1/query` SQL over data sigiro
    **already emits** — `check_rate_pressure` (the `flushed batches` logs carry
    `{project_id, rows, bytes}`; returned `sigiro_ops 99.5% / customer_acme 0.5%`
    of bytes), `check_retention` (`storage maintenance enabled` carries
    `{retention_days, disk_cap_bytes, sample_secs}`; returned `7 / 0 / 30`),
    `check_storage`'s growth half, and `check_config_drift` **partially** — the
    flush thresholds and backpressure policy are read at `inserter.rs:132-142`
    and never logged, so one `tracing::info!` closes it. Unlike the deleted CLI
    version it would read the **server's** config, which that check's own doc
    comment flagged as the thing the CLI got wrong.

    Two do not survive and #59 keeps them: `check_data_dir_size` (the sampler at
    `server.rs:1487` measures bytes every 30 s but logs only on cap transitions)
    and `check_duckdb_health` (`pragma_database_size()` is never emitted). So
    **#59 shrinks from six checks behind a new endpoint to two checks plus two
    one-line log statements.**

    **Ship it in three parts.** (a) Fix `PoolInner::touch_project`
    (`pool.rs:293`): it is `#[tracing::instrument]`-ed and ticked from the
    long-lived flusher task, whose parent span never closes and is never
    exported — measured **1,793 orphan spans, all one `parent_span_id`, one
    never-ending trace, 19% of all self-telemetry**. Sigiro's own `orphan_spans`
    finding caught this, which is the pitch working; it also means every
    self-diagnosis carries a permanent false warning until fixed, and the
    finding's *explanation* ("a service stopped exporting") misattributes this
    cause. ~10 lines. (b) Two `tracing::info!` lines. (c) Deploy wiring:
    `sigiro_ops` tenant, key into SOPS, three vars on the **accessory**. ~40
    lines total, no new endpoint, no new response type, no admin client path —
    against #59's ~250 lines.

    **Blocked on a name collision and a memory cap.** `worldpol`'s
    `server/sigiro.ts:5` reads `SIGIRO_OTEL_AUTH_KEY` as *its* tenant key while
    `src/otel.rs:26` reads the same name for sigiro's self-export key; Kamal
    secrets are one flat name→value file, so rename the worldpol side first. And
    measure before enabling: idle self-telemetry is **525-566 spans/min
    (~750k/day) with zero external traffic**, idle CPU goes 0.01-0.04% → ~2-3% of
    a core, RSS reached **351 MiB** holding 20 minutes of it. Prod sits at
    **543 MiB of a 1 GiB cap** with `SIGIRO_DUCKDB_MEMORY_LIMIT: 512MB` **per
    project**, and `sigiro_ops` is another project. Raise `memory` or lower the
    DuckDB cap in the same change, or this OOMs.

    One honest limit: the **BOCPD half contributes nothing for hours**.
    Detection buckets at 5 minutes over a 7-day lookback (`anomaly.rs`), so a
    20-minute window is ~4 points and produced `anomalies=0` even against that
    94.08%-vs-0.0% shift. The deterministic half was correct and complete from
    the first minute. Also structural: `src/otel.rs` builds a tracer and a logger
    provider and **no meter provider**, so `has_metrics` is always false —
    three of four signals. Profiles do work via `SIGIRO_PYROSCOPE_ENDPOINT`
    (419 samples, real attribution into DuckDB internals).

64. **`sys_*` is reachable on the hosted service, where it describes OUR host**
    (added 2026-08-07) — **verified, and it should go before more beta tenants
    are added.** `server.rs:529` allowlists `SYS_STATS_FUNCTIONS` in a FROM
    position, and the **only** hosted-mode difference at the `/v1/query` gate is
    `allow_information_schema = !storage_mode().is_distributed()`
    (`server.rs:1861`). There is no `is_distributed` gate on `sys_*` anywhere. So
    a hosted tenant can run `SELECT * FROM sys_disk_info()` and read the mount
    points, free bytes, CPU, memory, network interfaces and OS of the machine we
    run.

    The widening was reasoned about and says so (`server.rs:368-375`: "revisit
    before exposing the quack surface to an untrusted network") — but that
    reasoning was about the **quack surface**, and the same allowlist also governs
    the multi-tenant HTTP path.

    Two consequences. It leaks our infrastructure shape to any beta tenant. And it
    is the wrong-machine trap at its worst: on hosted, `sys_*` can never answer a
    question about the tenant's own infrastructure, so **every answer it gives is
    misleading by construction.**

    **Decision basis:** gate `sys_*` on `!is_distributed()`, the same axis
    `allow_information_schema` already uses, so the self-hosted fleet case is
    untouched and hosted rejects it with a message naming `sigiro_metrics_*` as
    where host questions are answered. ~20 lines in `server.rs` plus a test beside
    `tests/federated_sys_stats_test.rs`. `www/docs/how-to/hosted-onboarding.md`
    still says "Allowed SQL: `SELECT` only, against the `sigiro_*` tables", which
    is currently incomplete — that sentence becomes true once this lands.

65. **`sigiro query` cannot answer against a local server** (added 2026-08-07) —
    verified: `main.rs:214` dispatches `Command::Query` straight to
    `federate::run_federated_query`, which resolves edges and **hard-bails on
    zero** (`federate.rs:86-89`). With no `--edge` and no `SIGIRO_EDGES` it shells
    out to `tailscale status --json` for peers tagged `tag:sigiro-edge`. So on a
    laptop running one `sigiro serve` — which opens no quack surface by default —
    `sigiro query "SELECT …"` fails. **The quickstart taught it as step 5 until
    `e0b2457`**, which rewrote the step to post to `/v1/query`.

    **Decision basis: this is a naming problem, not a docs problem.** The docs are
    now correct and the CLI is still surprising — `sigiro query` reads as "query
    sigiro" and means "fan out across a fleet". Options: (a) leave it, relying on
    the new reference page; (b) fall back to local `/v1/query` when zero edges
    resolve and no override was given (~30 lines, but one command then means two
    things by environment, which `resolve_edges`'s own comment argues against);
    (c) rename to `sigiro fleet-query` and add a local `sigiro query` (~80 lines
    plus `tests/federated_query_test.rs` and `tests/vm/fleet-e2e.sh`).
    Leaning (c): a reader who types `sigiro query` on one box wants one box.
    Not urgent — (a) already shipped as the mitigation.

66. **`POST /v1/diagnose` has no window default, and the docs assumed one**
    (added 2026-08-07) — `from_ts`/`to_ts` are `i64`, not `Option`
    (`src/diagnose/mod.rs:39,43`), and the generated spec agrees
    (`required: ["from_ts","to_ts"]`). The CLI defaults the window to the last 15
    minutes in epoch **seconds**; the HTTP body defaults nothing and takes epoch
    **microseconds**. The quickstart documented `-d '{"service":"checkout"}'`,
    which is a deserialize failure. Fixed in the docs by `e0b2457`.

    **Decision basis: the asymmetry is the defect, not the docs.** The CLI and the
    HTTP API are the same operation with different defaults and different units,
    and only one is forgiving. An agent reading `/openapi.json` is fine; an agent
    copying the quickstart was not. Make both `Option<i64>` with the CLI's
    15-minute default (~15 lines plus the `utoipa` schema; `validate_window`
    already tolerates it). `validate_micros` still catches a wrong unit loudly,
    which was the original reason they were mandatory — and that protection does
    not depend on the field being required.

67. **The `sigiro_*` table schema has no reference home** (added 2026-08-07) —
    the nine tables, their columns and the SQL rules are documented in exactly one
    place: section 4 of `www/docs/how-to/hosted-onboarding.md`, a how-to guide.
    The Diátaxis sort left it there rather than delete a hosted tenant's only
    reference, but a self-hosted reader has no reason to open the hosted guide.

    **Decision basis: this is #53's docs half and ships with it, not before.**
    Hand-writing a column table now creates a second source of truth that drifts
    from `src/schema.rs` on the next migration — exactly the failure `3603713`
    was. Once `/v1/schema` exists, `www/docs/reference/` gets a generated tables
    page beside `cli.md`, the how-to's section 4 shrinks to a link, and
    `explanation/tables.md` keeps only the family/subject distinction that is
    genuinely explanation. Blocker: #53.

68. **`bun run fmt` breaks the MDX build and reflows every doc table**
    (added 2026-08-07) — `oxfmt .` formats `.md`/`.mdx`. On `quickstart.mdx` it
    escaped an MDX comment's `{/*` to `{/\*` (the comment spanned a blank line),
    failing the parser with `MDXError: Unexpected end of file in expression`. It
    also padded every markdown table in `www/docs/` and touched `www/README.md`.
    Found the hard way during `e0b2457` and reverted.

    **Decision basis: a formatter that can break the build is worse than none for
    those files.** `.oxfmtrc.json` already carries the precedent — `DESIGN.md` is
    in `ignorePatterns`. Options: (a) add `docs/**` and `*.md` to
    `ignorePatterns`, one line, matching how these files are actually maintained
    (hand-wrapped at ~80 columns); (b) `**/*.mdx` only, accepting table reflow;
    (c) report the `{/*` escape upstream to oxfmt. Leaning (a) plus (c).

    **TODO 2026-08-16:** `vp check` currently reports 47 pre-existing formatter
    diffs. Clear that backlog only after #68 chooses the Markdown/MDX policy;
    anti-slop adoption must not silently reformat those files.

69. **Every retired `/docs` URL needs a hand-written `run_worker_first` negative
    until blume#170 ships** (added 2026-08-07) — `assets.run_worker_first`
    covering `/docs/*` makes Cloudflare bypass `_redirects` and serve **301**,
    because `@astrojs/cloudflare` declares `redirects: false` and Astro defaults
    the status. The workaround is one negative per retired prefix in
    `www/wrangler.jsonc`, and that list grew from 2 entries to 5 in `e0b2457`.
    Re-verified both directions against `wrangler dev` on the built bundle: with
    the negatives all three Diátaxis redirects served 302; with them removed all
    three served 301 while `/docs/api` held at 302.

    **Updated 2026-08-08 — do NOT remove the negatives yet, and the reason is
    not the one this entry assumed.** Blume `2e231bc` (which superseded the
    user's own merged PR #170) no longer emits negative rules at all: it bakes a
    `REDIRECTS` table into the generated `blume-worker.mjs` and answers each
    redirect with its configured status ahead of Astro. Verified in the built
    output — all 20 entries at 302.

    But a live query-string probe shows the table is currently **shadowed**:

        /docs/quickstart?probe=1 -> location: /docs/tutorials/quickstart?probe=1

    The worker replies with the bare table value and reattaches no query string,
    while production preserves it. So these redirects are still answered by the
    **asset layer via `_redirects`** — the five negatives are doing exactly what
    they were written to do, and every key in the worker's table is covered by
    one of them, so no path exists to exercise the table in production.

    **Removing them would change behaviour, not just move it:** every retired-URL
    redirect would start dropping query strings, which breaks UTM and ref
    parameters on inbound links. Either accept that, or send an upstream fix
    appending `url.search` to the worker's redirect target first. Decide
    deliberately; this is no longer a tidy-up.

    **Decision basis: a hand-edited list will eventually be forgotten, and the
    failure is silent and cached forever.** Two things: (a) when blume#170
    releases, delete the negatives and re-verify with the same two-config
    comparison; (b) until then add a check asserting every configured `redirects`
    entry returns 302 — ~40 lines, and the only thing that would catch a missing
    negative before a browser caches it.

    **(a) attempted and reverted on 2026-08-10, on blume 1.4.2.** blume#170 did
    release, and 2e231bc then replaced its negatives with a redirect table baked
    into the wrapper Worker — so the *status* is now correct either way. But the
    query-string prediction above was right, and it reproduces: the table answers
    with `headers: { location: redirect[0] }` and never appends `url.search`,
    while the negotiation branch six lines below passes `url.pathname +
    url.search`. Measured with `wrangler dev` on the real bundle, one variable
    changed:

    ```
    negatives absent : /docs/api?utm_source=x&ref=y -> /docs/reference
    negatives present: /docs/api?utm_source=x&ref=y -> /docs/reference?utm_source=x&ref=y
    ```

    Both 302; production matches the second line. So the negatives stay and still
    have to grow with every retired URL. Filed upstream as **blume#175** with the
    two-config measurement and the one-line cause.

    **(a) DONE 2026-08-10. The five negatives are deleted.** blume#175 was fixed
    the same day in `156d5ff`, taking the shape proposed in the report: the wrapper
    applies `_redirects` semantics to the `Location` header, so the request query
    is forwarded unless the destination carries its own, and a destination fragment
    stays last. Three upstream tests cover those cases.

    The release gate was shut again — `156d5ff` is the tip of `main` with one
    pending patch changeset and no `Version Packages` commit after it — so this
    ships through a **re-vendored tarball**,
    `www/vendor/blume-1.4.2-main-156d5ff.tgz`, and the `.gitignore` negation is
    back (now globbed, `!www/vendor/*.tgz`, because the filename carries the SHA).
    Verified with the negatives removed: all six retired URLs return 302 with the
    query string intact, matching what the asset layer used to produce, and the
    five `redirectLocation` edge cases pass against the function as it shipped into
    our own `dist/server/blume-worker.mjs`.

    **Superseded 2026-08-11:** `blume@1.4.3` published with `156d5ff` in it, so
    the tarball, the negations and the watcher cron are all gone. `www` tracks the
    released version. See the retirement note above.

    **Supply chain, measured rather than assumed:** `bun audit` reports 18 findings
    on our tree against 9 for published `1.4.2`, but installing the vendored
    tarball with *fresh* resolution also reports **9**. The extra findings are
    stale transitive pins in the committed `www/bun.lock` (`hono@4.12.33`,
    `nanoid@3.3.16`, `postcss@8.4.31`, all present at `HEAD` before this change),
    not the unreleased build. That backlog is worth its own item.

    New removal condition: when blume publishes above 1.4.2, drop the tarball and
    both `.gitignore` negations and restore a semver range. (b) is unchanged and
    would still be worth the ~40 lines — it is now the only guard on this
    behaviour, since the negatives that used to make it visible are gone.

70. ~~**The site serves plain HTTP with no HSTS**~~ — **CLOSED 2026-08-08.**
    Akshat enabled both settings in the Cloudflare dashboard; verified from
    outside afterwards. `http://sigiro.com/`, `/docs`, `/docs/tutorials/quickstart`
    and `/install` all now return **301** to `https://` with the query string
    preserved, and the HTTPS response carries
    `strict-transport-security: max-age=15552000` (180 days) plus
    `x-content-type-options: nosniff`.

    **`includeSubDomains` and `preload` are both deliberately absent** — confirmed
    in the live header. That was the recommendation and it is the part worth
    keeping: HSTS cannot be revoked from the server, so a browser that sees the
    header refuses plain HTTP for the full max-age whatever the setting is
    changed to afterwards. Six months without the subdomain clause is
    recoverable; twelve with subdomains and preload is not, and this origin has
    already moved its docs host twice in one week.

    Checked before enabling: all three live hostnames serve TLS
    (`sigiro.com`, `www.sigiro.com`, `api.sigiro.com`) and no other subdomain
    resolves, so the subdomain risk was contained rather than assumed away.
    `api.sigiro.com` is a separate nginx origin, already redirected HTTP→HTTPS on
    its own, and correctly sends no HSTS of its own.

    No regressions: every page still 200, retired URLs still **302** (not 301),
    and markdown negotiation still serves `text/markdown`.

    Original entry, for the record — verified:
    `http://sigiro.com/` returns **200** with the real page in cleartext, and
    `strict-transport-security` is absent from the HTTPS response. Cloudflare's
    "Always Use HTTPS" is off for the zone. Worse in combination with #69: an
    `http` request to a retired URL redirects to an `http` destination, so a
    downgrade is sticky in exactly the way a cached 301 is.

    **Decision basis:** this is a zone setting, not a code change — two toggles
    in the Cloudflare dashboard (Always Use HTTPS, then HSTS once you are sure
    every subdomain can serve TLS). No deploy, no build. Note none of the usual
    security headers ship either (`x-frame-options`, `x-content-type-options`,
    CSP, `referrer-policy`); those are a `public/_headers` change and a separate,
    lower-priority decision.

71. **"Edit this page" 404s on all 15 docs pages** (added 2026-08-07) — every
    content page links to `github.com/sigiroai/sigiro/edit/main/docs/<path>`,
    which returns **404** (verified). Two independent faults: the `sigiroai` org
    has **zero public repos** and the real remote is `akshatagarwl/sigiro`, also
    private; and the path prefix is wrong regardless, since published docs live
    at `www/docs/` while repo-root `docs/` is internal notes and plans.
    `agent-readability.json` advertises the same 404 repository URL.

    **Decision basis:** a broken affordance on every docs page is worse than no
    affordance. Three options: point it at the real repo path once the repo is
    public, correct the prefix to `www/docs/` and accept the 404 until then, or
    turn the feature off in `blume.config.ts`. Turning it off is the honest
    interim state. Roughly one config line either way.

    **CLOSED 2026-08-09 in `3e5ae8e`.** The third option shipped: the `github`
    key is deleted from `blume.config.ts`, which removed both surfaces at once.
    Verified on the live site — `sigiro.com/docs/reference` emits no
    `github.com/.../edit/...` link, and `sigiro.com/agent-readability.json`
    carries no `repository` field. `blume.config.ts` keeps a comment at the
    former key recording why it is absent and the condition to restore it.

    **Reopen condition:** the repo becomes public. Restore the key with the
    correct `www/docs/` path, not the repo-root `docs/` prefix that was wrong
    independently of visibility.

72. **The CLI is a thin layer over the API, and nothing else — make it true**
    (added 2026-08-07) — a standing principle, stated by Akshat: "the CLI should
    anyway be just a thin layer on top of the API and nothing special ever."
    Anything the CLI can do, an agent must be able to do over HTTP. Today three
    things violate that, and each violation is a different kind.

    **Decision basis: the API is the product and the agent is the reader.** A
    capability that exists only when a human types it contradicts the entire
    positioning. It is also where the defects come from — #66 existed precisely
    because the CLI owned a default the API did not, and #65 because the CLI owns
    an operation the API does not have. This is not three unrelated bugs; it is
    one missing rule, and the rule prevents the next one.

    Four of seven subcommands already comply (`status`, `anomalies`,
    `healthcheck` go through `SigiroClient`; `serve` and `fetch-extensions` are
    legitimately local because they *are* the server). Three did not. **Two are
    now closed (2026-08-07), and one remains.**

    - ~~**The window default**~~ — **CLOSED in `ae1b9f6`.** The CLI's 900-second
      constant, its own `now` and its `unwrap_or_else` are deleted. `main.rs`
      maps `Option<seconds>` → `Option<micros>` and stops; `None` travels to the
      server, and `DiagnoseRequest::window()` is now the only place the
      15-minute rule exists. `SigiroClient::diagnose`/`diagnose_multi` take
      `Option<i64>` to carry it — matching `anomalies()`, which sat directly
      below the broken code doing it correctly the whole time. Net deletion. The
      test `cli_and_api_resolve_the_same_default_window` asserts the point
      rather than the parts: the CLI with no flags and
      `POST /v1/diagnose {"service":"checkout"}` resolve the identical window.
      The seconds-flavoured `--from` flag survives, as intended.
    - ~~**`--grep` is client-side filtering**~~ — **CLOSED in `677a856`:
      deleted, not promoted.** The response is already JSON and `jq` is one pipe
      away, so a server-side filter would duplicate a tool the caller already
      has; "simplest possible architecture" won over "truer to the principle".
      The deletion spanned four `src/main.rs` sites plus a now-needless `mut`,
      **and two tracked docs pages that taught it** (`www/docs/reference/cli.md`
      row, `www/docs/tutorials/quickstart.mdx` example) — a CLI flag removal is
      a docs change too.
    - **OPEN: `sigiro query` is not `POST /v1/query`** — it is the fleet
      coordinator, with no HTTP equivalent. Tracked as #65; this entry is the
      reason #65 matters beyond the naming confusion. **It is blocked on a
      decision, not on effort:** rename it to `sigiro fleet-query` and add a real
      local `sigiro query`, or leave it. Not a quiet change either way — it
      touches `tests/vm/fleet-e2e.sh`.

    Enforcement idea worth considering: a test that asserts every subcommand
    except `serve`/`fetch-extensions` reaches its result through `SigiroClient`.
    Cheaper than remembering the rule, and it would have caught all three.

73. **Submit sigiro to `awesome-ai-startups`** (added 2026-08-09) — the list at
    `github.com/nowork-studio/awesome-ai-startups` (64 stars) accepts indie AI
    products by pull request. Sigiro is not in it yet — a grep of the README
    returns nothing.

    **Decision basis: this is a distribution channel, not a vanity link.** The
    list carries 1,393 entries across 18 categories, and 10 of them describe
    themselves as observability or tracing products, so the category is proven
    rather than assumed. Cost is one pull request.

    The mechanics, from `contributing.md`:
    - Format is exactly `- [Name](https://product.url) - One-line tagline.`
    - Append to the **end** of the section. Do not reorder or insert in the
      middle. That rule is explicit.
    - Category: `🛠 APIs, SDKs & Infrastructure` (112 entries) is the better fit
      than `📊 Analytics & Data` (66) for a self-hosted binary. Decide before the
      PR; the entry goes in one section only.
    - Inclusion criteria met: bootstrapped, live site (`https://sigiro.com/`
      returns 200, verified 2026-08-09), and an end user can download and run it.
    - No tracking parameters and no launch-only URLs. Link `https://sigiro.com`.

    Open question: the tagline. It must be one sentence and sentence-cased, and
    the current positioning is self-hosted plus local development. Write it to
    match the site's own headline rather than inventing a second description.

---

## Go-to-market plan: distribution channels and the incident campaign (added 2026-09-05)

A distribution plan written 2026-09-05, filed here and re-ordered against what
the repo actually has. It supersedes the "TODO (priority order)" list in
`docs/notes/company/gtm.md` and the "Now — Get Real Traffic" block in
`docs/notes/company/plan.md` for the next month; both predate the MCP server,
the hosted service and the binary install, and neither names a channel that a
coding agent's user actually opens.

**The premise.** Discovery should happen inside the developer's working
environment, not on a listing site. So the first channels are the ones a
Claude Code or Cursor user already has open: an editor plugin marketplace, a
skill installer, an MCP registry. Listing sites and paid reach come after, and
only to feed the same short experience.

**The one experience every channel leads to:**

> Install → run a prepared incident → ask the agent what happened → inspect the
> evidence → connect your own application.

**Checked against the code and the repo today (2026-09-05), which changed the
order the plan was written in:**

- **The MCP server exists and needs no Rust changes for any of this.** `sigiro
  mcp` (`main.rs:341`) serves stdio over `rmcp` and exposes four tools:
  `list_services`, `list_anomalies`, `diagnose_service`, `run_query`
  (`src/mcp.rs:97-164`). `SigiroMcp::new(client: SigiroClient)`
  (`src/mcp.rs:90`) is constructor injection of the shared HTTP client, and it
  is the only DI boundary in the path (flagged per AGENTS.md; it is one
  constructor argument with one production caller, `main.rs:349`, so it is the
  right amount). Every plugin below wraps the *binary* as a stdio command; none
  touches the struct. The plan's "reuse that dependency injection boundary for
  the integrations" is therefore already true, with nothing to build.
- **This repo is private with no LICENSE** (`gh repo view`, 2026-09-05; same
  state #35 recorded on 2026-08-04). Cursor's marketplace requires a public
  repository. That forces the **public integration repo (#93)** to be a
  separate repository, which also keeps the open-source decision in #30/#34
  untouched.
- **No skill exists yet.** `www/blume.config.ts` has no `skills:` key and
  `sigiro.com/.well-known/agent-skills.json` returns 404, exactly as #48
  recorded on 2026-08-05. So the skill in #94 is net-new content, and it is the
  shared payload for #97, #98, #100, #48 and #57. Write it once.
- **No prepared incident or example application exists.** `dev/` holds the
  Lima VM only; `scripts/` holds benchmarks and audits. Every recording,
  tutorial, landing page and ad in this plan depends on one, so it is P0 (#95).
- **Local storage runs `Open` auth** (`src/auth.rs:35`): no per-request auth,
  protected by the network perimeter. A public Railway template would expose
  ingest and queries to the internet, so Railway is deferred.
- **Hosted tenants are provisioned by hand** (`www/docs/how-to/
  hosted-onboarding.md:3`, and #92's third gap: `/admin/tenants` and
  `/admin/keys` are loopback-only). The funnel's last step, pay, cannot happen
  without a conversation until #106 ships.
- **The proposed headline breaks the brand register.** "Find why your API
  slowed down—from Claude Code." uses the `claim — em-dash — refinement`
  pattern that `.agents/product-marketing.md` names as the thing to remove from
  older copy. The second sentence is fine as written. Rewrite the first before
  it reaches the site; one STE-compliant candidate: "Find out why your API
  slowed down, from Claude Code."

**External requirements quoted below** (Cursor's submission checklist, the
skills.sh FAQ, Claude's plugin publishing docs, Smithery's local bundles, the
MCP Registry's checksum metadata, AlternativeTo's $5 review, DEV's canonical
link, Reddit community targeting, Coolify's star gate) are as stated in the
plan on 2026-09-05. Re-read the source page on the day of each submission;
none of them was re-verified while filing this entry.

### The promise

Start with developers who use Claude Code or Cursor and already have
OpenTelemetry. One concrete promise everywhere:

> Find out why your API slowed down, from Claude Code.
> Sigiro gives your agent ranked findings, the suspected deploy, and the
> queries behind the evidence.

The first campaign: **"Why did checkout get slow after this deploy?"** Record a
reproducible incident and the actual investigation. Include what Sigiro found
and what still required judgment. That honesty is the differentiator, and
`.agents/product-marketing.md` requires it anyway (no invented proof points).

### Priority order

Dependency first, then the plan's channel ranking. Estimates are the plan's.

#### P0 — week 1, the pieces every channel consumes

93. **Public integration repo** (added 2026-09-05) — one public repository that
    every channel points at. It holds: the skill (#94), the Cursor plugin
    manifest (#97), the Claude Code plugin manifest (#100), the example
    application and prepared incidents (#95, #102), and install docs that
    reference `curl sigiro.com/install | sh` (#30) rather than duplicate it.

    **Why a separate repo:** this repo is private with no LICENSE and Cursor
    requires a public one; the alternative is settling #30/#34 (open-sourcing
    the core) first, which is a bigger decision than a plugin deserves. The
    integration repo needs its own LICENSE on day one to be useful.

    **Not decided:** the repository name and owner (personal `akshatagarwl` vs
    an org), and whether it also becomes the agent-plugins.org directory #57
    describes. Decide the name once; every install command below embeds it.

94. **The one skill: "Debug an application with Sigiro"** (added 2026-09-05) —
    a single skill that checks the setup, discovers services, diagnoses a
    failure and reads the supporting evidence. It must also handle the two
    failure modes a fresh user hits: installation, and missing telemetry (the
    empty-tenant silence #92 documents).

    This is #48's first candidate, `debug-a-production-failure`, plus the
    setup-check half of `self-host-sigiro`, written as **one** skill rather than
    four. The plan is right that one focused skill beats a set: the agent
    installs it and then uses sigiro correctly without a human explaining it.
    Package the knowledge that helps the agent complete the task, not a restatement
    of `/openapi.json` (#48's rule).

    **Verification is the deliverable.** Run it in a memoryless Claude Code
    session and in Cursor against #95's incident, the way #92 was found, and fix
    the skill until the run completes without prompting. Rendering it is not
    testing it.

    **One source, four outputs:** the same `SKILL.md` ships in the integration
    repo (#93), through `npx skills add` (#98), under
    `/.well-known/agent-skills/` via Blume (#48), and inside the plugin
    directories (#97, #100, #57). Do not fork the text per channel.

95. **Prepared incident #1 and the shared example application** (added
    2026-09-05) — a runnable example app with a reproducible failure, a deploy
    marker, and actual Sigiro output. Incident #1 is the campaign's: checkout
    gets slow after a deploy, shaped as a FastAPI timeout for tutorial #1 (#99).
    The same app carries incident #2, a Node.js latency regression (#102), and
    the later error spike and slow dependency.

    **This is the asset under everything:** the short recording Cursor's
    submission needs (#97), the 60–90 second tutorial recordings (#99, #102),
    the landing page's "Try this incident" button, the ad creative (#105) and
    the weekly clips (#103). Build it once, in the integration repo.

    Engineering verifies each incident end to end before anything is published
    about it. The lesson from the external docs review (see "Gaps found by an
    external review", 2026-08-09) applies: the published surface must not run
    ahead of the product.

96. **Track the funnel from the first visit** (added 2026-09-05) — referral
    visits and installation clicks immediately, on every page the channels
    land on. For hosted users: first telemetry, first diagnosis, return usage,
    payment.

    The progression to measure: visit → start example → complete diagnosis →
    connect own application → return → pay. Local downloads are only a proxy
    for usage; keep that limitation visible when judging results.

    **Weekly review per channel, and the action each reading triggers:**

    | Reading | Action |
    |---|---|
    | Visits without attempts | Inspect the promise, the prerequisites, the landing page |
    | Repeated setup failures | Pause promotion; fix that step first |
    | Demo success, no own-application use | Ask whether the example addresses a recurring need |
    | Own-application use and returns | Publish more around that problem; increase distribution cautiously |

    Ship this in week 1 because #105 must not spend money into a page that
    cannot report what happened, and #99's landing page is the first thing it
    instruments.

#### P1 — week 1–2, first submissions

97. **Cursor plugin submission, and Cursor's MCP install button on the site**
    (added 2026-09-05; plan's channel #1; 2–3 days including a fresh-install
    test) — package `sigiro mcp` with the skill (#94) and submit through
    Cursor's marketplace publish flow. Cursor's stated requirements: a public
    repository (#93), a valid manifest, documentation, and local testing.
    Include the short recording of the complete workflow, cut from #95.

    While the review is pending, add Cursor's MCP install button to the site:
    it works without marketplace approval, and it is the first channel that
    puts discovery inside the developer's working environment.

    Test the plugin from a clean machine before submitting. The install path
    the manifest points at is `curl sigiro.com/install | sh`, verified live on
    2026-08-04 (#30), so the plugin does not need to ship the binary itself.

98. **Publish the same skill through skills.sh** (added 2026-09-05; plan's
    channel #2; half a day incremental on #94/#97) — distribute via
    `npx skills add <public-repo>`. Per the skills.sh FAQ, skills appear
    automatically as users install them; there is no manual leaderboard
    submission, so initial discovery still needs entry points we control.

    Put the install command in the integration repo's README, at the top of
    both tutorials (#99, #102), and on the agent setup pages
    (`www/docs/how-to/install-with-ai.md`, `agent-telemetry.md`). Publish the
    same file at `/.well-known/agent-skills/` in the same change, since #48 is
    one config line once the skill exists.

99. **Tutorial #1 and its landing page: "Debug a FastAPI timeout with Claude
    Code and OpenTelemetry"** (added 2026-09-05; plan's channel #5, first half;
    2–3 days sharing #95's app) — a runnable example, a reproducible failure,
    actual Sigiro output, and a 60–90 second recording. Written walkthrough on
    the site, recording on YouTube, both linking directly to installation.

    **The landing page continues the demonstration.** Someone who clicks the
    checkout-timeout video lands on that exact example: the video,
    reproduction steps, prerequisites, expected result, and one primary button,
    **"Try this incident."** After success, offer **"Connect your
    application."** For hosted, show pricing and provide signup and checkout,
    which is #106; until then the hosted path is the call (#26).

    This page is the first honest `/for/claude-code` page #29 asked for: it
    has the actual config and the actual caveat, so it is not a doorway page.
    Adjacent positioning around agent-driven production debugging already
    exists in the market; that supports testing this audience, and it does not
    establish search volume or likely conversions. Publish while the
    marketplace submissions are in review.

#### P2 — week 2–4, incremental channels off the same repo

100. **Claude Code plugin submission** (added 2026-09-05; plan's channel #3;
     half to one day incremental) — add Claude's plugin configuration to the
     integration repo, validate it, and submit through Anthropic's submission
     form. Qualification: submissions go to the reviewed community
     marketplace, which users must add themselves; inclusion in the
     automatically registered official marketplace is discretionary with no
     application process. So provide the exact installation steps on a
     dedicated Claude Code page, and do not describe it as "in the
     marketplace" without that qualifier.

     Decide here whether the plugin directory doubles as the agent-plugins.org
     directory in #57. Same `SKILL.md`, one more manifest.

101. **Smithery local MCP bundle, then the official MCP Registry** (added
     2026-09-05; plan's channel #4; 1–2 days packaging plus a few hours
     registration) — Smithery accepts local MCP bundles, so the existing stdio
     server distributes there as-is: the prebuilt executable, its endpoint
     configuration, and clear local setup instructions. Reuse that bundle for
     the official MCP Registry, which requires release-hosted artifacts and
     checksum metadata. Treat the registry as distribution infrastructure that
     feeds downstream directories, not as a page users read.

     **Dependency:** checksummed, release-hosted artifacts are #42 (automate
     releases) and #52 (point `v0.1.0` at the shipped bits). Do not hand-cut a
     release for the registry; that is the exact failure #43 recorded.

     Start with the local experience. Hosted access requires manual
     provisioning (#92) until #106, so the bundle's default configuration
     points at `localhost`.

     Related, already filed: #41 (integrations.sh self-service listing) and the
     server card at `/.well-known/mcp/server-card.json` (#47). The registry
     entry, the server card and #57's `mcp.json` should describe the same
     server the same way.

102. **Tutorial #2: "Find a Node.js latency regression from Cursor"** (added
     2026-09-05; plan's channel #5, second half) — same shape as #99, same
     example app (#95), the Cursor half of the audience. Publish it the week
     after #99, as the second turn of #103's cycle.

103. **Turn one incident into a week of content, and repeat** (added
     2026-09-05) — each incident produces four assets, and every piece ends
     with the same invitation: **"Run this incident yourself."**

     | Asset | Where | What it does |
     |---|---|---|
     | 45–60 second demonstration | X and LinkedIn | Shows the failure and the evidence quickly |
     | 4–6 minute walkthrough | YouTube | Explains the investigation and the reproduction |
     | Complete technical tutorial | sigiro.com, cross-posted to DEV | Answers a specific debugging question |
     | Runnable example | The public integration repo (#93) | Lets someone try the exact workflow |

     DEV supports cross-posting company articles with a canonical link to the
     original. The next week's failure is a timeout, an error spike, or a slow
     dependency, so the collection grows into useful examples rather than
     announcements.

     **First month's deliverables:** four reproducible incidents, four
     tutorials, four walkthroughs, eight short clips, one capped advertising
     experiment (#105). One person owns the weekly cycle; engineering verifies
     each demonstration before publication.

     This is the free-to-repeat half of #35 (X, DEV, subreddits, no
     one-shot surfaces) and the content #27's build log was waiting for. Product
     Hunt and Show HN stay reserved until the repo decision in #30/#34.

104. **One AlternativeTo submission** (added 2026-09-05; plan's channel #6;
     about an hour) — accurate platform, licensing and observability
     capabilities, with one concrete screenshot of a diagnosis and its
     evidence. The free queue currently takes months; the optional $5 priority
     review usually takes 1–2 business days, longer when busy. Payment buys a
     review, not approval or ranking. A small secondary experiment; do it once
     #99's page exists so the listing has somewhere to send people.

#### P3 — paid reach, only after #96 and #99 exist

105. **A capped Reddit experiment: $200 over seven days** (added 2026-09-05) —
     a learning budget, not an expected acquisition cost. One captioned demo
     video (#95), one audience. Check in Ads Manager whether r/ClaudeCode,
     r/cursor or r/devops are eligible; Reddit's community targeting follows
     engaged users across Reddit and does not guarantee placement inside those
     communities.

     Review after the first $100. Spend the remainder only if the results
     justify continuing or testing one specific improvement. Google Search
     comes afterwards, and only once Keyword Planner shows real demand for
     phrases such as "OpenTelemetry MCP server"; that volume is unverified.

#### Blocker track — code

106. **Automatic hosted signup, key issuance and checkout** (added 2026-09-05)
     — to turn adoption into paying customers without conversations. Today an
     operator sets up each tenant by hand (`hosted-onboarding.md:3`), the
     admin endpoints are loopback-only by design (#92, gap 3), the landing page
     routes hosted interest to a call (#26), and pricing plus billing are
     PROPOSED in #15 (Stripe Meters + Zitadel).

     This reverses #26's "hosted goes through a call" decision, and it should
     do so only when #96 shows own-application usage and returns. Until then
     the call is the right instrument, because it yields the stronger signal
     with no users. Schedule it the week the funnel earns it, not before; but
     file it now so the paid step of the funnel is known to be missing rather
     than discovered.

### Deferred, with reasons

- **Railway template.** Local storage uses `Open` authentication
  (`src/auth.rs:35`), so a public deployment would expose ingest and queries.
  Revisit when a public deployment protects both. This sequences the Railway
  parts of #6 and #14 behind auth, and it does not affect Fly (#23) where the
  sidecar sits on a private network.
- **OpenTelemetry directory submissions.** The ecosystem lists are frozen;
  nothing to submit to.
- **Coolify's catalog.** Currently requires 1,000 GitHub stars, which a private
  core repo cannot accrue. Revisit with #30/#34.

### First week, committed

The public integration repo (#93), the skill (#94) and incident #1 (#95), the
Cursor submission (#97) with the install button live, skills.sh distribution
(#98), one working tutorial (#99), and tracking (#96) switched on before the
first link is shared.

## Gaps found by an external review of the published docs (added 2026-08-09)

A reviewer read only `sigiro.com` and `/docs` and wrote up what the product does
not quantify or explain. Fourteen items. Each was then checked against the real
code rather than accepted, which changed several verdicts — two were wrong about
the product, and two were worse than the reviewer could see from outside.

**The headline finding is that the site claimed two things the code cannot do.**
Both are fixed in the same commit as this entry. The rest are filed below.

The review is also a standing lesson: **the published surface was ahead of the
product in exactly the places nobody re-read.** Both defects were in the oldest,
most-polished copy — the hero description and the five-step loop — which had been
written once and never re-checked against the code that grew underneath it.

74. **Incident lifecycle: shifts vanish, they do not close** (added 2026-08-09) —
    **the copy defect is fixed; the product gap under it is open and is the more
    interesting half.**

    The site's "Verify" step used to print `recovered 19:07`, `no open shifts`
    and `inc_4a1c closed · 34 min open`. None of those values can exist.
    `src/anomaly.rs:199` keeps a shift only when `mean_after > mean_before`, and
    the doc comment at `:175` says it outright: *"Improvements never persist."*
    `src/schema.rs:216-240` has ten columns and no status. `src/responses.rs:43-80`
    exposes no lifecycle field. There is no duration: `detected_at` is the pass
    wall-clock, rewritten every five minutes.

    **The real behaviour is worse than the missing feature.** `persist` runs
    `DELETE FROM sigiro_anomalies` then re-inserts the whole table
    (`src/anomaly.rs:475-498`). When a service recovers, BOCPD stops finding the
    worsening shift, so the row **silently disappears on the next pass**. A user
    who saw an anomaly at 14:00 has no way to learn what became of it. That
    destroys the evidence trail the rest of the product sells, and it is the part
    worth fixing regardless of what the homepage says.

    Fixed today in copy only: the frame now prints an empty result and the
    caption names the absence, which is what actually happens. Two smaller
    fabrications in the same section went with it — `+48σ` (no sigma is computed
    anywhere; the real magnitude is the unit-free ratio at `src/server.rs:2099`,
    now printed as `+0.96`/`+0.99` and checkable against the columns beside it)
    and `inc_4a1c` (real IDs are `{service}@{iso8601}`, `src/server.rs:2141`).

    **To make the original claim true:** persist improving shifts with a
    direction, add a status column, and stop the full-table wipe from erasing
    history. ~300-500 lines. Until then no lifecycle word goes back on the site.

75. **Publish the BOCPD priors — "threshold-free" is not "assumption-free"**
    (added 2026-08-09) — docs only, ~1 page, and the code comes out of it well.

    The reviewer inferred hidden tunable knobs. There are none: every value is a
    library default and nothing is configurable, so the threshold-free claim
    survives. But the assumptions are real and invisible. Hazard rate 250.0, so
    H = 1/250 and the expected run length is ~20.8h at 5-min buckets — which
    implies ~8 expected changepoints per 7-day series. Gaussian likelihood,
    `NormalGamma(0,1,1,1)` prior (`augurs-changepoint-0.10.2/src/lib.rs:81`),
    run-length pruning at 1e-6, CDF threshold 1e-3
    (`changepoint-0.14.2/src/bocpd_truncated.rs:81-82`), MAP run-length argmax
    with no probability threshold (`utils.rs:165`), z-score pre-transform
    (`src/anomaly.rs:151-159`), index-0 suppression (`:168`).

    Publish the table and say plainly that you cannot tune them and why. The
    three doc claims that were checked all hold: 5-minute buckets, 7-day history,
    worsening-only.

76. **No seasonality model** (added 2026-08-09) — REAL GAP, confirmed by a global
    grep: no `day_of_week`, `seasonal` or `hour_of_day` anywhere in `src/`. The
    only normalization is a global z-score over the whole series
    (`src/anomaly.rs:151-159`) against a flat 7-day window (`:57`).

    A regime detector and a seasonal detector answer different questions. Weekday
    morning spikes, midnight crons, Monday batch jobs, weekend drops and
    month-end billing all look like regime changes to this detector. ~300-600
    lines plus new series construction. **Note the site does not claim seasonality
    handling** — this is a product gap, not a copy defect.

77. **Cross-service correlation — and the dependency graph nobody knows we ship**
    (added 2026-08-09) — the reviewer said sigiro has no service dependency graph.
    **That is wrong, and the docs are why he thought so.**

    `src/diagnose/mod.rs:1758-1791` builds `dependencies` and `dependents` per
    service with span counts and error rates, top 20. It ships today and is
    documented nowhere. **Claim it in the diagnose reference — it is the cheapest
    win on this list.** Two honest caveats to state with it: it is derived from
    trace co-occurrence on `span_kind IN (3,4)`, not from `parent_span_id`, so it
    cannot tell A→B from A→C→B; and it is computed per request and thrown away.

    The correlation half of the claim stands and is **the same gap as #61**, seen
    from outside. `correlate_shifts` (`src/anomaly.rs:86-100`) groups on `ts_shift`
    alone, and the caller partitions by service first (`src/server.rs:2123-2130`),
    so cross-service merging is structurally impossible. Redis→catalog→checkout
    surfaces as three unrelated diagnoses of equal weight. ~400-800 lines — and
    the expensive part, deriving the graph, already exists. This is persistence
    and wiring, not new analytics. **Read #61 before starting: it has the SQL
    fan-out route already measured.**

78. **Sampling-aware statistics** (added 2026-08-09) — REAL GAP. `trace_state` is
    stored (`src/otlp.rs:83,188-191,541`) and then only ever displayed
    (`src/server.rs:2185,2291`). Nothing parses it. Zero hits for `adjusted_count`
    or a `th:` threshold anywhere in `src/`.

    So error rate, p95, operation frequency and every trace correlation are
    computed over whatever spans survived sampling, with no reweighting. At 1%
    head sampling those numbers are biased, and a customer running adaptive
    sampling gets a different bias per route. ~200-400 lines, but it touches every
    aggregate in `diagnose/mod.rs` and `anomaly.rs`, so it is wider than it is
    deep. The product is only as correct as the population it analyses, and right
    now it assumes that population is complete.

79. **Operator docs for the machinery that already exists** (added 2026-08-09) —
    mostly a docs gap, and again the code is better than the docs suggest.

    Already shipped and undocumented: retention (`src/config.rs:283`,
    `src/inserter.rs:693-719`), compaction (`:758-759`), 30s checkpoint (`:836`),
    WAL recovery (`:452`), backpressure (`src/config.rs:241`,
    `src/inserter.rs:30-46`). **Call out that the disk cap defaults to `0`, which
    means disabled** (`src/config.rs:241`) — an operator needs to know that.

    Genuinely missing and worth their own decisions: no secondary index
    (`src/schema.rs:243-246`, hence the timestamp-bound rule already documented),
    no schema migrations, no backup or restore, no replication, single-writer
    (`src/pool.rs:25`). No published sizing at any spans/sec. `benches/` exists
    and could produce real numbers — that is ~200 lines of bench scale-up, and
    real numbers would answer the reviewer's sharpest unanswered question.

80. **Auth beyond a bearer token** (added 2026-08-09) — REAL GAP, with one thing
    the reviewer missed. Self-hosted is open by default and says so
    (`src/auth.rs:88-94`, `AuthMode::Open` with an explicit warning); hosted has
    bearer plus tenant isolation (`:107-122`). Undocumented third tier: a
    `SIGIRO_ADMIN_TOKEN` gate (`src/config.rs:360`, `src/server.rs:2550`) and a
    read-only SQL firewall (`src/server.rs:766-800`) — both worth documenting.

    Absent entirely: RBAC, read-vs-ingest scopes, audit log, SSO/SAML, SCIM, PII
    redaction, data residency. ~400-800 lines for scopes and audit, ~200-400 for
    redaction at ingest. This matters more than the line counts suggest, because
    traces and logs carry customer data and agent telemetry carries more.
    Related: #15 already pairs RBAC with billing.

81. **The benchmark the review keeps pointing at** (added 2026-08-09) — the
    reviewer's closing argument is the one already on the priority list, arrived
    at independently: no published accuracy numbers of any kind — precision,
    recall, false-positive rate, top-1 root-cause accuracy, time-to-detection —
    and no comparison against a naive baseline such as rolling z-score, EWMA,
    Prometheus alerts or plain OTel.

    His proposed shape is worth taking seriously because it measures the claim
    that actually matters: 20-50 reproducible bugs in real microservices, each
    failure injected, then run a coding agent **with sigiro vs raw OTel vs logs
    only vs an existing observability MCP**, and measure root-cause accuracy,
    repair rate, wall time, tokens and number of investigation steps.

    That is the difference between "we built an architecture" and "this makes an
    agent measurably better at fixing things", and nothing published today
    establishes the second. See the benchmark work already tracked separately.

82. ~~**`test_otlp_json_payloads_ingest_all_signal_types` fails under machine
    load**~~ — **CLOSED 2026-08-11** by the `SIGIRO_SHUTDOWN_TIMEOUT_SECS` envar.
    Production keeps the 5s default; `just check` and `just check-release` set
    60s. The timeout message now reports the real budget and names the knob.

    Taken as filed, with one correction: the entry offered "a parameter on
    `shutdown_and_wait`, or a test-only constant". A parameter is the worse of
    the two — there are **17 call sites** across six test files plus
    `src/server.rs:1382`, so it churns all of them and puts the flake fix in
    every future test author's hands. An `Envar` beside the other flusher knobs
    changes zero call sites and is a genuine operator control: draining a large
    buffer onto slow disk hits exactly the same wall.

    It blocked a commit on 2026-08-11 before it was fixed, which is the entry's
    own prediction coming true.

    Verified: default 5s reproduced the failure (1 of 3 runs) on an idle-ish
    machine; 60s passed 5 of 5, and 3 of 3 under three concurrent release
    builds — the loaded condition from the table below.

    Original diagnosis, kept because the measurement is the argument:

    `InsertRouter::shutdown_and_wait` gives each project's flusher a fixed
    `Duration::from_secs(5)` to drain (`src/inserter.rs:422`). That budget is
    wall-clock, so it competes with everything else on the box. Measured
    2026-08-10, same commit, same 16-test file:

    | Condition | Result | File runtime |
    | --- | --- | --- |
    | 3 concurrent `cargo` builds in other worktrees, disk at 97% | **2 of 3 runs FAILED** | 17–19 s |
    | idle machine, worktrees removed | 3 of 3 passed | ~12 s |
    | the test alone (`--test ... <name>`) | passed | 1.9 s |

    Bisected across every commit in that session — `e8fff16`, `eeeaebe`,
    `36e3f6f` each pass 3/3 in a clean worktree — so no code change caused it.
    The failing test never touches the paths those commits changed.

    **The fix is not a longer timeout.** Five seconds is a reasonable
    *production* shutdown budget and raising it to hide test flake makes real
    shutdown hangs slower to detect. Better options, cheapest first: let the test
    harness override the budget (a parameter on `shutdown_and_wait`, or a
    test-only constant) so production keeps 5 s while the suite tolerates a
    loaded CI box; or make the wait deadline-free and rely on the existing
    `handle.abort()` path plus an outer test timeout.

    Worth doing because it is a **false negative in the other direction**: a gate
    that fails for reasons unrelated to the change teaches people to re-run it
    until it passes, which is how a real failure gets waved through.

83. ~~**Profiles are write-only through `/v1/query`**~~ — **CLOSED 2026-08-11.**
    `sigiro_profiles.stack_trace` is a `VARCHAR[]`, but the scalar allowlist
    (`src/server.rs`) carried no list or array functions: `unnest`,
    `list_filter`, `list_extract` and `array_to_string` were all rejected. So
    sigiro ingested and stored profiles nobody could aggregate by frame — no
    "which function is hot", no flamegraph — through the only public query API.

    Collateral damage from sourcing the allowlist before profiles were
    queryable, not a deliberate gate; `LEN` was already admitted for the same
    array type. Fixed by adding the read-only list family, plus a test proving a
    lambda body cannot smuggle `getenv`/`current_setting`/`read_csv` past the
    scalar allowlist.

    Found by pointing one sigiro's Pyroscope agent at another and then being
    unable to group the result by stack frame — the dogfood found a real defect
    on its first run.

84. ~~**Boot is ~2.3 CPU-seconds and dominates everything else sigiro does**~~
    — **RETRACTED the same day it was filed. See the correction below.**
    (added 2026-08-11) — measured with two local sigiro
    processes: B booting with `SIGIRO_PYROSCOPE_ENDPOINT` pointed at A, then
    querying B's own boot profile back out of A over `/v1/query`.

    | 10s flush window | CPU-seconds | samples |
    | --- | --- | --- |
    | first (covers boot) | **2.29** | 175 |
    | second (idle server) | 0.04 | 4 |

    **RETRACTED 2026-08-11, same day, by three parallel investigations. The
    headline claim was measured on a DEBUG binary against a FRESH catalog under
    concurrent build load, and does not describe the shipped artifact.**

    Three independent errors, recorded because each is a trap worth not
    repeating:

    1. **The dev profile compiles DuckDB's C++ at `-O0` with assertions on.**
       `libduckdb-sys` builds the amalgamation through the `cc` crate, so it
       inherits Cargo's `OPT_LEVEL`. Measured: `LOAD json` 173.6ms dev vs 18.7ms
       release (9.3×); `LOAD ducklake` 260.8ms vs 34.8ms (7.5×). A Rust
       performance number about DuckDB work is meaningless without naming the
       profile.
    2. **The phase table charged DuckLake DDL to `lock_filesystem`.** The 1.489s
       "extension preload + lockdown" window was bracketed by two log lines that
       live in *different modules*, and `schema::create_tables` — nine
       `CREATE TABLE IF NOT EXISTS` transactions — runs between them at
       `pool.rs:347`. That is what the `BindCreateTableInfo` / `Catalog::GetSchema`
       frames were. On the production Linux image that phase contains **no
       extension file loading at all**: `parquet`, `json` and `icu` are absent
       from the image yet `LOAD` succeeds, because they are statically linked.
    3. **"Warm" was not warm.** `create_tables` costs 128-157ms on a fresh
       DuckLake catalog vs 21-35ms warm.

    Re-measured on the same machine, same warm catalog, five runs each:

    | build | warm boot to `/health` |
    | --- | --- |
    | **release** | **0.19-0.45s** |
    | debug | 0.75-1.54s |

    Against the 3.6-3.9s originally filed. Production Linux image: 140-216ms.

    **A hot frame in a profile means it is on the stack, not that it owns the
    time.** `ComputeSHA256FileSegment` was real and was over-attributed;
    signature verification is 196-340ms for all 176MB of extensions, not
    seconds.

    What survives is in #85 and #86. The concurrent-spawn work earlier the same
    day still stands on its own measurement (12% on one test file), but it was
    justified partly by this now-retracted budget.

    Kept, not deleted, because the dual-sigiro dogfood that produced it also
    found a real defect (#83) on its first run, and the retraction is more
    useful than the claim was.

85. ~~**Statically link `parquet` and `json` on macOS**~~ — **CLOSED 2026-08-11**
    (`fed64d1`). The one free win from the #84 investigation, and the measured
    saving beat the estimate.

    Release build, same warm catalog, 7 runs each:

    | | boot to `/health` |
    | --- | --- |
    | before | 0.191-0.214s (median **0.203**) |
    | after | 0.133-0.147s (median **0.141**) |

    **−30%**, with a tighter spread. Build cost was 3m15s, not the ~9 minutes
    estimated.

    **No code changed.** `extensions_needing_install` (`src/storage.rs`) already
    asks DuckDB at runtime rather than hardcoding which extensions are static —
    it was written for exactly this platform split after an earlier bug where
    hardcoding broke macOS. Proof the linking is real: after the change
    `sigiro fetch-extensions` stops caching `parquet.duckdb_extension` and
    `json.duckdb_extension` entirely (39 MB of files gone), while
    `pool::tests::lock_filesystem_loads_are_prewarmed` still passes.

    Both features resolve to `bundled`, already in use on this target, so
    `Cargo.lock` is unchanged and no new build backend appears. **`icu` was
    excluded**: the duckdb crate routes it through `bundled-cmake`, a different
    backend that adds a cmake dependency. It stays a cached file on macOS.

    Original estimate, kept for the reasoning: −47ms of a 122ms logged
    release boot, with **zero security cost** and no upstream work: the
    extensions become part of the binary, so there is no file to hash and no
    `dlopen`.

    This only brings macOS in line with Linux, which already statically links
    them — verified by exporting the production image and finding no
    `parquet`/`json`/`icu` files while `LOAD` still succeeds.

    Where extension loading *is* a file load, it is **80-90% signature
    verification**, isolated cleanly with `allow_unsigned_extensions` (which
    gates exactly one branch, `extension_load.cpp:483`) across eight extensions.
    The residual 1-3.7ms is `dlopen` + init + catalog registration.

    **Do NOT reach for `allow_unsigned_extensions`.** On the production image its
    ceiling is ~30ms, and DuckDB's `INSTALL` URLs are plain HTTP
    (`CORE_REPOSITORY_URL = "http://extensions.duckdb.org"`), so the signature is
    the only integrity control on that download. Also note it cannot be `SET`
    post-open; it must be a `Config` flag at `open_in_memory_with_flags`.

    Not droppable, each checked: `parquet` is DuckLake's storage format,
    `json` is used by `src/diagnose/mod.rs`, `icu` was added deliberately in
    `c9ba4f8` for timezone handling under lockdown.

86. **Two unreported upstream DuckDB defects** (added 2026-08-11) — both found
    while attributing #84, both still present on DuckDB `main`, neither with any
    issue, PR or discussion filed by anyone.

    **(a) FILED 2026-08-11 as [duckdb/duckdb#24686](https://github.com/duckdb/duckdb/issues/24686).**
    The vendored mbedtls has no ARMv8 SHA-2 acceleration. DuckDB's
    `third_party/mbedtls/include/mbedtls/mbedtls_config.h` defines
    `MBEDTLS_SHA256_C` but not
    `MBEDTLS_SHA256_USE_ARMV8_A_CRYPTO_IF_PRESENT`, so the accelerated path in
    `third_party/mbedtls/library/sha256.cpp:65` is dead code. Measured on Apple
    M4: **85.6 MB/s as shipped vs 371.7 MB/s with the one `#define` added — a
    4.34× regression.** `git log -S "MBEDTLS_SHA256_USE_ARMV8" --all` returns
    zero commits.

    This is a one-line change with runtime feature detection (`_IF_PRESENT`
    no-ops on non-crypto ARM and on x86). It affects every DuckDB user on Apple
    Silicon and ARM servers, and reaches beyond extension loading to the
    `sha256()` SQL function and httpfs request signing.

    Re-verified independently before filing, against upstream `main` rather
    than the crate's vendored copy — the subagent's 4.34× was understated:

    | build | throughput (5 interleaved runs) |
    | --- | --- |
    | as shipped | 140-213 MB/s (median 193) |
    | + the `#define` | 964-1248 MB/s (median 1068) |

    The issue leads with an **instruction count, not a timing**, because that is
    reproducible in one command and cannot be attributed to my machine:
    `otool -tV sha256.o \| grep -cE 'sha256h\.\|sha256su'` gives **0** stock and
    **200** with the define.

    Two objections pre-empted in the filing, both tested rather than argued:
    the header's "beware of changes here" cipher-table warning (all 22 sources
    in the `duckdb_mbedtls` CMake target compile clean with the define,
    including `cipher_wrap.cpp`), and correctness (FIPS 180-4 known-answer
    vectors including the one-million-`'a'` long-message case both produce
    identical, correct digests).

    **The fix lands in `duckdb/duckdb`, not upstream mbedtls.** The tree is a
    hand-maintained vendored copy: `inline_mbedtls.py` only overwrites files
    that already exist and are not marked `// dummy file`, and
    `inline_mbedtls.diff` does not touch the config — so an edit survives the
    next version bump. It will not reach sigiro until DuckDB releases it and
    `libduckdb-sys` vendors that release; the pin is v1.5.3.

    **(b) Extension signature verification has no cache of any kind.** The whole
    binary is re-SHA256'd on every process start, in every process. The `.info`
    sidecar carries only an ETag, no hash field. Dedupe exists only within a
    single `DatabaseInstance`.

    Worth less than it sounds — the measured cost is 196-340ms for 176MB — and
    a cache keyed on filesystem metadata is forgeable by exactly the attacker
    who matters, so upstream would likely push back. File (a), not (b).

    Also established: the verification code is **byte-identical** between the
    pinned v1.5.3 and current `main` apart from a typo fix, so upgrading gains
    nothing. `LoadStaticCAPIExtension` landed on `main` but is unreleased
    (absent from v1.5.3, v1.5.4, v1.5.5).

88. **Pyroscope profile ingest is shipped, advertised nowhere, and structurally
    absent from the API reference** (added 2026-08-20) — **high priority.** The
    code is already paid for; what is missing is prose plus one routing decision.

    Four routes work today, all handled by `otlp_http_pyroscope`
    (`src/ingest.rs:896`):

    | route | note |
    |---|---|
    | `/` | the Pyroscope RS SDK posts to the root |
    | `/push.v1.PusherService/Push` | pyroscope-rs SDK push path |
    | `/ingest` | |
    | `/ingest/pyroscope` | |

    Storage has been ready for longer. `sigiro_profiles.stack_trace` is a
    `VARCHAR[]`, and #83 admitted the 15 read-only list functions so profiles can
    be aggregated by frame rather than merely stored.

    **Three separate defects, and the third is the one worth the item.**

    (a) *Priority list said this was unbuilt.* Item #8 listed "Pyroscope SDK
    profiles — HTTP :4040 /ingest, ~150 lines" under Build Next as future work.
    Corrected in this commit.

    (b) *Nothing advertises it.* Measured 2026-08-20: **zero** `www/docs/` pages
    mention Pyroscope, **zero** mentions in `www/src/lib/` site copy. The word
    "profile" does not appear in the landing copy at all. A working capability
    with no route to a reader.

    (c) *It cannot appear in `/docs/reference` even in principle.* The Pyroscope
    routes live on a **separate composable Router with its own auth and
    `DefaultBodyLimit`** (`src/server.rs:651`). utoipa only sees annotated
    handlers on the main router, so the generated OpenAPI spec has no knowledge
    of these four paths. The reference page therefore presents itself as the
    complete API while omitting an entire ingest protocol. That is the structural
    half: fixing (b) with a docs page leaves the reference still lying.

    **Decision this item needs:** either annotate the composable router into the
    spec so `/docs/reference` is complete by construction, or accept that
    ingest-protocol endpoints live outside the spec and document them by hand in
    a how-to. The first keeps one source of truth and costs utoipa plumbing
    against a router deliberately kept separate for auth reasons; the second is
    cheap now and guarantees the same drift recurs for the next non-OTLP
    protocol.

    Docs shape, once decided: a `www/docs/how-to/send-profiles.md` beside
    `agent-telemetry.md`. How-to is the Diátaxis mode, since the reader arrives
    wanting to ship profiles. The 2026-08-08 no-internals ruling still applies,
    so the page names endpoints and SDK config and never the storage engine.
    Naming Pyroscope is permitted: the no-competitor-named rule
    (`product-marketing.md:116`) is scoped to cost claims, and this is a wire
    compatibility statement.

    Note #13 also lists Pyroscope under a future Grafana-protocol compat item;
    check whether that is now partly redundant.

87. ~~**`HealthResponse::Starting` is unreachable in production**~~ — **CLOSED
    2026-08-11** (`0fc766d`). `run_servers` now flips `init_complete` from
    `Handle::listening()`, which resolves when `axum_server` has bound the
    socket — the first instant a probe can arrive, so it is both the earliest
    correct point and the latest safe one. `run_serve` no longer flips it at
    all.

    Spawned rather than awaited: `listening()` only resolves once the server
    future starts being polled, which cannot happen while `run_servers` is still
    building the remaining servers. Awaiting it inline deadlocks.

    **Proven, not assumed.** A temporary post-bind delay made the endpoint
    return `503 {"status":"starting"}` at t=6.087s and `200 {"status":"ready"}`
    at t=9.084s. Before the fix `starting` was never observable at any polling
    rate. The probe was removed afterwards.

    **Why the existing tests missed it, and the lesson.** Both health tests
    construct `AppState` directly and set the flag by hand, so they pass no
    matter how production wires it — they pinned the handler, never the wiring.
    The new test asserts the wiring itself (`src/main.rs` must contain no
    `init_complete.store`, and `run_servers` must gate on `listening().await`)
    and was verified red by reintroducing the defect. **A test that constructs
    its own state cannot catch a defect in how production assembles that
    state.**

    Original diagnosis:

    `init_complete` flips at `src/main.rs:522`, but the HTTP listener does not
    bind until later inside `run_servers`. So no external client can ever observe
    `init_complete == false`: **zero 503s across 18 instrumented boots**, every
    first successful probe returned `200 {"status":"ready"}`. The intended
    three-state health contract is silently a two-state one.

    The comment at `src/server.rs:1654-1657` asserts the *opposite* — that the
    listener is up before init finishes and probes must see 503. It is backwards.

    Harmless today, but it degrades the moment anything that can block or fail is
    added before the HTTP spawn — and `pool.clone_connection("default")`, which
    can do an S3-backed distributed attach, already sits there. Then `/health`
    reports ready for a server that is not serving.

    `AppState::mark_init_complete()` at `src/server.rs:252` already exists for
    exactly this. The fix is ~2 lines plus the stale comment.

    (Also measured and closed: the "0.3-0.6s `/health` lag" was 87ms of quack
    setup between the flag flip and the listener bind, amplified by concurrent
    build load. No reclaimable time — nothing can connect during that window.
    And the "0.352s before any DuckDB work" is `dyld` resolving **63,647 weak
    external symbols** in the debug binary; the release binary exports 5 and
    starts in 5-8ms. Both were debug-build artifacts, like #84.)

---

52. **Point `v0.1.0` at the shipped bits and push the tag** (added 2026-08-06) —
    the only unfinished step of the `c0f0606` re-cut. R2 and ghcr both carry
    `c0f0606`, but the git tag does not: local `v0.1.0` is still `631b71d`, and
    so is `origin`'s. The published artifacts and the tag that is supposed to
    name them disagree.

    **Decision basis:** anyone bisecting or building from `v0.1.0` gets the
    binary with the #45 extension bug, which is exactly the thing the re-cut
    existed to remove. Two commands, and it closes the release cleanly.

    Blocker: none — `main` is now pushed (`origin/main..main` is empty), which was
    the reason the tag push was deliberately held back on 2026-08-06. Note the
    earlier local force-move did **not** survive; there is a
    `backup/v0.1.0-pre-overwrite-927a13f` tag pointing at a different commit than
    the session recorded, so re-verify `git rev-parse v0.1.0` before and after
    rather than trusting the backup name. The force-push rewrites a published tag —
    flag it, then do it with the backup tag as the recovery path.

    **Re-verified 2026-08-09, and the shape changed: this is a create, not a
    rewrite.** `v0.1.0` no longer exists at all — `git rev-parse v0.1.0` fails
    locally, and `git ls-remote --tags origin` returns only
    `backup/v0.1.0-pre-overwrite-927a13f` (→ `927a13f`). The tag was deleted, so
    the paragraph above describing a force-push over a published tag no longer
    applies. Nothing is rewritten and no consumer is broken; the risk that
    justified the flag is gone.

    What is left is two commands — `git tag -a v0.1.0 c0f0606` and
    `git push origin v0.1.0` — and one thing to know before running them:
    **tagging publishes nothing.** Actions is off repo-wide, so `release.yml`
    does not fire and R2 is unaffected. The tag is a name for bits that are
    already published, which is exactly what this item wants. Confirm `c0f0606`
    is still the SHA behind the R2 and ghcr artifacts before tagging, since the
    entry records that from 2026-08-06.

53. **`/v1/schema` — a schema-description surface** (added 2026-08-06, carried
    over from the fleet-stats thread where it was assessed as the highest-value
    item and never filed here) — there is no schema description anywhere today:
    no `COMMENT ON`, no `/v1/schema`. Agents hitting `/v1/query` discover tables
    by guessing or by reading `llms-full.txt`.

    **Decision basis:** this is a silent wrong-answer bug, not a convenience gap.
    An agent asks about a remote service's disk, `sys_*` answers with the sigiro
    host's numbers, and it confidently reports the wrong machine. The distinction
    that has to be machine-readable is not "self vs remote query modes" but:
    `sys_*` = live point-in-time reading of **the machine sigiro itself runs on**
    (not telemetry, not historical, no time range applies), versus
    `sigiro_metrics_*` = **historical telemetry from whatever sent OTLP**,
    including remote hosts — which is where "is service X's disk full" is
    actually answered.

    Shape: `COMMENT ON` in `schema.rs` surfaced through `duckdb_tables()`, exposed
    as `/v1/schema`. DuckDB already carries the comments, so this is plumbing
    rather than new machinery.

54. **Delete the unit tests wearing an integration filename** (added 2026-08-06)
    — `tests/federated_sys_stats_test.rs` calls `validate_read_only()` directly
    (7 call sites), violating AGENTS.md's "E2E real user tests, never unit tests."
    Shipped to `main` in that state.

    **Decision basis:** the real coverage already exists and is better — fleet
    query 4 in `fleet-e2e.sh` rejects the same hostile SQL over the wire, and the
    quack self-test runs at boot. So this is deletion, not replacement: ~3 tests
    out, no coverage lost. Verify the `fleet-e2e.sh` case still covers each one
    before removing it.

55. **`src/` is 3.4x over the AGENTS.md line ceiling** (added 2026-08-06) —
    17,211 lines against a stated 5,000-line ceiling, `server.rs` alone at ~2,600.
    Pre-existing and growing; the pre-push slop audit does not gate on it.

    **Decision basis:** none yet, and that is the honest status — the ceiling is
    either a real constraint or it should be raised in AGENTS.md. Extracting
    crates has never been started, and doing it unprompted would be a large
    untargeted refactor of a working system. The cheap first move is deciding
    which of the two the number means.

56. **Resource tags for the API reference sidebar** (added 2026-08-06, deferred
    with a condition) — the generated reference groups every operation under
    Blume's untagged fallback heading, the literal word "Operations".
    Re-litigated 2026-08-04 from prior art rather than from prior decisions: the
    standard **is** resource tags (Resend tags by resource noun — `Emails`,
    `Domains`, `API Keys` — never by transport or verb), and sigiro's paths
    already name the resources: Services, Anomalies, Traces, Investigations,
    Query.

    **Decision basis / why it is still deferred:** five endpoints across five
    resource tags gives one page per group — a heading per item, the same
    "grouped nothing" problem multiplied. Resource tags are the correct fix and
    should land **when any one group would hold several operations**. That is the
    trigger to watch, not a date. Note the earlier "drop tags" call was right for
    the wrong reason: tagging was not the error, *transport* tags were.

    Related and separately open from the same session: **the reference page titles
    are too large** — a theme/heading-CSS concern in `www/`, unrelated to the
    sidebar labels, and not yet touched.

57. **Ship sigiro as an Agent Plugin** (added 2026-08-06) — support
    [agent-plugins.org](https://agent-plugins.org/), the v1.0.0 portable package
    format for agent extensions. Spec and schemas:
    [`agentplugins/agent-plugins-spec`](https://github.com/agentplugins/agent-plugins-spec)
    (`spec/1.0.0.md`, `schemas/1.0.0/{plugin,mcp}.schema.json`).

    **Decision basis:** the Technical Steering Committee is Amazon, Cursor,
    Microsoft, OpenAI and Vercel, so this is the interoperability floor the
    clients sigiro's users actually run are converging on. Publishing one plugin
    directory means every compatible client can load sigiro's skills without a
    per-client repackage. It also **shares its entire payload with #48** — same
    `SKILL.md` files — so the marginal cost on top of that work is a manifest and
    a directory move, not a second body of content.

    What the format requires (read from the normative schema, not the marketing
    page — the docs subpages are client-rendered and fetch empty):

    - A plugin is a plain **directory**, deliberately not an archive or a
      registry bundle, so it stays inspectable with `ls`/`cat` and diffable in
      git. Required: `plugin.json` at the root, whose only mandatory fields are
      `$schema` and `name` (lowercase, `^[a-z0-9][a-z0-9.-]*[a-z0-9]$`, no `--`
      or `..`). `version`, `description`, `author`, `homepage`, `repository`,
      `license`, `keywords` are optional metadata worth filling in anyway.
    - `skills/` — each **immediate** child directory containing a `SKILL.md`
      is one skill; clients MUST NOT search deeper. The skill format itself is
      delegated to the [Agent Skills specification](https://agentskills.io/specification),
      which is the same format #48 already targets.
    - `mcp.json` — optional, describes `stdio` / `streamable-http` / legacy `sse`
      servers. Only relevant if sigiro ever ships an MCP server; note the site
      already serves a server card at `/.well-known/mcp/server-card.json` (#47),
      so decide whether these two should describe the same thing.
    - Plugin-relative paths MUST start with `./` and MUST resolve inside the
      plugin root — no symlink escapes. Clients set `PLUGIN_ROOT` and
      `PLUGIN_DATA` for stdio subprocesses.

    **Sequencing: this is downstream of #48, not parallel to it.** Write the
    skills once, then expose them twice — under `/.well-known/agent-skills/` for
    the discovery RFC, and inside a plugin directory for this spec. Doing #57
    first would mean authoring skills with no way to serve them.

    Not decided: whether the plugin lives in this repo or its own, whether it
    carries an `mcp.json` at all, and where it gets listed (the site tracks
    "Compatible Clients", which sigiro is not — sigiro would be a plugin author).
    Open question worth checking before investing: the spec is young (v1.0.0,
    repo created 2026-04) and defines no distribution or install mechanism, so
    "support it" currently means "publish a conforming directory", with discovery
    still an unsolved problem the ecosystem has not answered.

    **Updated 2026-09-05:** the Cursor (#97) and Claude Code (#100) plugin
    submissions in the go-to-market plan produce the plugin directory this entry
    describes, in the public integration repo (#93). Whether that directory also
    conforms to agent-plugins.org is decided in #100; `mcp.json` should describe
    `sigiro mcp` the same way the MCP Registry entry (#101) and the server card
    do.

58. **Rename `investigate` to `diagnose`, and drop the current `diagnose`**
    (added 2026-08-06) — decided, not open. One verb, one surface, callable by
    anyone: agents over HTTP, humans over the CLI. Today the codebase has two
    things wearing near-synonymous names, and the smaller one is a consumer of
    the larger.

    **Decision basis: the public label already says `diagnose`.** `server.rs`
    (~:2395) gives `/v1/investigate` the OpenAPI summary **"Diagnose a service"**
    — and since Blume renders `summary` as both sidebar label and page `<h1>`
    (see #56), the docs have been saying "Diagnose a service" while every
    identifier says investigate. The rename removes an existing contradiction
    rather than imposing a preference. "Diagnose" is also the plainer word for
    what it returns: a verdict with evidence, not an open-ended inquiry.

    **What `investigate` is** (the thing being renamed, and it is the big one):
    `src/investigate/` — `mod.rs` 1,713 lines + `analysis.rs` 234 — plus
    `POST /v1/investigate` returning `InvestigationBlock` with ranked findings
    and `drill_down_sql`, `sigiro investigate <service>` on the CLI, and
    `tests/investigate_test.rs` (2,699 lines) + `benches/investigate_bench.rs`.
    **114 references across 13 files in `src/`** (`anomaly`, `client`,
    `investigate/*`, `lib`, `main`, `pool`, `responses`, `schema`, `server`,
    `storage`, `types`, and `diagnose` itself). 23 mentions of `/v1/investigate`
    in `docs/` and `www/`.

    **What today's `diagnose` is** (the thing being dropped): `src/diagnose.rs`,
    1,229 lines, a **21-check CLI report** (`check_errors`, `check_slow_spans`,
    `check_ingest_rate`, `check_storage`, `check_config_drift`,
    `check_duplicate_spans`, `check_orphan_spans`, `check_rate_pressure`,
    `check_duckdb_health`, …) that runs entirely through the public HTTP API —
    `/v1/query`, `/v1/investigate`, `/v1/traces/{id}` — with no direct DB access.
    Its own header states the intent: "self-diagnosis via sigiro's own telemetry
    — never tail Docker logs." It even contains `check_investigate_deep_dive` and
    an `investigate_with_retry` helper, so **it is a caller of the endpoint being
    renamed, not a sibling of it.**

    **The trap that makes this more than a `sed`:** `main.rs` (~:497) calls
    `diagnose::ebpf_status` and `diagnose::write_ebpf_status` at `serve`
    startup — a read-only "could this host run eBPF auto-instrumentation" probe
    written to `instance_status`, with nothing to do with the CLI report and its
    own `tests/ebpf_test.rs`. `EbpfStatus`, `check_obi_available` and
    `Suggestion` all live in `diagnose.rs` too. **Rehome the eBPF probe before
    deleting the module**, or `serve` loses a startup capability check. A
    module-wide delete would take it silently.

    Sequencing, so the two halves do not collide: rehome eBPF → delete the CLI
    report and its `Command::Diagnose` arm → *then* rename `investigate` →
    `diagnose` across code, tests, benches, the route, docs and `www/`. Renaming
    first would mean editing 21 checks that are about to be deleted.

    **Decided 2026-08-06: clean break, no alias and no redirect.** `/v1/diagnose`
    replaces `/v1/investigate` outright. Akshat's call — "this is a greenfield
    project" — and it matches AGENTS.md's standing no-backwards-compat rule, so
    there is no deprecation window to design and no dual-route tests to carry.
    Note what the break costs, since it is real and not merely theoretical:
    `/v1/investigate` is in the **published spec on the live box** (verified
    2026-08-06 — `curl https://api.sigiro.com/openapi.json` lists it under
    `info.title: 'sigiro API Reference'`), so any pinned client breaks with a 404
    the moment the server deploys. Acceptable pre-1.0 with no external consumers;
    revisit this reasoning the moment there is one.

    Still open, and it decides the size of the job: does the 21-check report's
    *value* survive its deletion, or should the useful checks migrate into the
    renamed `diagnose`'s findings rather than being dropped outright? That is the
    difference between a ~1,200-line deletion and a partial migration. Worth
    noting the checks are not redundant with `investigate` today — they cover
    install health (`check_config_drift`, `check_storage`, `check_retention`,
    `check_data_dir_size`, `check_duplicate_spans`, `check_orphan_spans`) that no
    endpoint currently reports, so a straight delete does lose real coverage.

    Because the rename lands on the server, remember the **two-deploy chain**
    that spec-sourced labels need: the server deploy publishes the new
    `/v1/diagnose` path and its `summary`, and only then does a `www` deploy pick
    them up in the rendered reference (see the API-reference work of 2026-08-04/05).

    Note the architecture table lower in this file lists `investigate.rs | ~200`
    lines; it is a **directory module of ~1,950 lines** and `diagnose.rs` is
    absent from that table entirely. Fix both while here.

    **Implemented 2026-08-06 on branch `letta/diagnose-rename`, not yet merged.**
    Ten commits. `src/investigate/` → `src/diagnose/`, the eBPF startup probe
    rehomed to `src/ebpf.rs` *before* any deletion, the 1,229-line CLI report
    deleted, and three checks migrated to findings. Verified: full `cargo test`
    green (35 suites, 0 failures), `POST /v1/investigate` → **404** and
    `/v1/diagnose` → 200, the generated `/openapi.json` carries zero `investigat`
    matches, `sigiro investigate` → `unrecognized subcommand`, and the eBPF probe
    still writes `ebpf_status` to `instance_status` at `serve` startup. Each
    migrated finding's `drill_down_sql` was pasted into `POST /v1/query` and
    returned the offending rows — finding and drill both proven over HTTP.
    **Blocked from pushing by the `just slop` pre-push gate — see #60.**

    Two brief premises turned out to be undercounts, worth knowing before the
    next rename of this size: the blast radius was **243 matches across 20 Rust
    files**, not 114 across 13; and **8 executable E2E harnesses**
    (`tests/e2e/docker|k8s/run.sh`, `tests/vm/lima-e2e*.sh`,
    `dev/lima/sigiro-*-test.sh`, `scripts/ci-compare.sh`,
    `scripts/self_improve_loop.py`) call the route or the subcommand and are
    **not run by `cargo test`** — they would have silently started 404ing.

59. **Operator-scoped install-health endpoint — the six checks the CLI
    `diagnose` deletion dropped** (added 2026-08-06) — deferred by decision, not
    oversight. #58 migrated the three tenant-safe checks
    (`check_duplicate_spans`, `check_orphan_spans`,
    `check_span_kind_distribution`) into `/v1/diagnose` findings as
    `duplicate_spans` / `orphan_spans` / `missing_client_spans`. Six did not
    move, and they share one blocker.

    **Decision basis: the trust boundary, not the difficulty.**
    `check_config_drift` reads `SIGIRO_FLUSH_INTERVAL_SECS` /
    `FLUSH_ROW_THRESHOLD` / `FLUSH_BYTE_THRESHOLD` / `BACKPRESSURE_POLICY`,
    `check_retention` reads `RETENTION_DAYS`, `check_data_dir_size` walks the
    data dir and reads free disk, `check_storage` counts rows across every table
    plus growth since last run, `check_duckdb_health` reads
    `pragma_database_size()` headroom, and `check_rate_pressure` ranks services
    by share of the last minute's spans **across all tenants**. On the shared
    hosted box, putting any of these on the tenant-facing `/v1/diagnose` hands an
    authenticated tenant the operator's flush tuning, retention policy, disk
    usage, and other tenants' relative volume. They are not a migration; they are
    a different audience.

    **The blocker: there is no computed operator surface.** `/admin/status`
    exists behind `require_admin_token`, but it is a flat key/value read out of
    `instance_status` in admin SQLite — no query path, no per-tenant fan-out, no
    place to hang a check that walks a directory or aggregates five tables.
    Designing `GET /admin/health` (or widening `/admin/status` to carry computed
    findings) is the actual work; it also needs a decision on whether the CLI
    grows an admin-token-carrying client, since `SigiroClient` deliberately sends
    no bearer token to `/admin/*` — the reason `check_ebpf_status` degraded to a
    warning in hosted mode, and why #58 deleted the now-dead
    `SigiroClient::admin_status`.

    ~250 lines (endpoint + response type + six checks re-derived against
    server-local state rather than the CLI process's env, which
    `check_config_drift`'s own doc comment flagged as wrong) plus an
    admin-authenticated client path. Two of the six are cheap enough to be
    tempting alone — `check_retention` and `check_duckdb_health` are one envar
    and one pragma — but shipping two checks behind a new endpoint costs the same
    design as shipping six, so do the surface once.

    Note `check_storage`'s growth delta persisted to
    `data_dir/.diagnose_storage_state.json` between CLI runs. A server-side
    version should use `instance_status` rather than reintroduce a state file.

60. **`slop_audit.py` erosion ceiling: 0.55 is now unreachable, and the metric
    is the reason** (added 2026-08-06) — one-line decision, needs a human. This
    currently **blocks pushing #58's branch**.

    **Decision basis: #58 made the ratio worse by making the code better.**
    `erosion = sum(mass) over CC>10 functions / sum(mass) over all`. Deleting the
    1,229-line CLI report removed ~20 small, low-CC functions — almost pure
    denominator. Measured, and reproduced independently in the parent session:

    | | hot mass | total mass | erosion |
    |---|---|---|---|
    | before (`392c7bc`) | 6242.2 | 11537.6 | 0.541 |
    | after | 6138.6 | 10978.6 | **0.559** |

    Absolute complexity **fell** by 103.6. Against the old denominator the new
    tree reads 0.532 — better than the baseline it "regressed" from. `just slop`
    is a pre-push gate and fails; `just check` (pre-commit) is green, which is why
    all ten commits landed.

    Three options, pick one: **(a)** raise `EROSION_CEILING` to 0.57 with this
    arithmetic as the comment — cheapest, and honest about what the number
    measures; **(b)** earn 0.55 back by extracting `diagnose_one`'s
    incident-correlation loop (CC 24 / 289 SLOC; the `for group in
    correlate_shifts` block is ~100 lines and most of its branching) — the only
    option that improves the code, ~1 hour, medium regression risk on the ranking
    tests; **(c)** make erosion slope-relative so deleting simple code cannot fail
    the gate — the real fix, but it needs a stored baseline, which the script's
    own docstring rejects ("a baseline file would drift and need its own review").

    `CLONE_CEILING` is settled: #58 ratcheted it 0.16 → 0.13 (measured 0.122),
    since the 0.16 had been raised specifically for the 19 uniformly-shaped
    checks that deletion removed. The real erosion wins are unrelated to this
    work: `federate.rs:plan_query` (mass 771, CC 41), `server.rs:run_servers`
    (637, CC 39), `main.rs:main` (584, CC 41).

61. **Diagnose across services, not one at a time — `diagnose_one`'s
    single-service boundary** (added 2026-08-06) — understand it first, then
    decide whether to change it. Filed as a question Akshat wants answered, not a
    committed change.

    **Correct the premise before designing anything: multi-service already
    works.** `DiagnoseRequest` takes `service: Option<String>` **or**
    `services: Option<Vec<String>>` (max `MAX_SERVICES = 10`), `diagnose()`
    resolves either into a list, loops, and returns
    `DiagnoseResponse { results: BTreeMap<service, DiagnosisBlock>, errors }`
    with per-service partial failure — one broken service does not discard the
    others. So the API is not limited to one service.

    **What is actually single-service is the analysis, not the request.**
    `diagnose_one` is "ONE DuckDB round-trip per service" — a stack of CTEs
    collapsed into one JSON document, with the service bound as a query parameter
    (`params![service, from_ts, to_ts, baseline_from, from_ts]`) and literal
    `WHERE service_name = '{svc}'` filters in the log-template and recent-error
    sub-queries. Ten services means **ten independent round-trips whose results
    are keyed side by side and never compared.** The gap is therefore:

    - **No cross-service reasoning.** Nothing correlates service A's latency
      shift with service B's error spike in the same window, which is the actual
      question during an incident ("what broke, and did it break upstream?").
      Ranked findings are computed per service, so a root cause in a dependency
      and its symptom in a caller surface as two unrelated diagnoses of equal
      weight.
    - **The data for it is already there.** `diagnose_one` has a `deps` CTE that
      reads cross-service parent/child span relationships, and #58's
      `orphan_spans` anti-join deliberately runs **across all services** (scoping
      it per service would have flagged every normal cross-service child as an
      orphan). So the span graph needed to rank a cause above a symptom is
      already being read — it is just being read once per service and thrown away
      at the block boundary.
    - **Cost is linear in services.** 10 services = 10 full CTE stacks. A single
      pass binding a service *list* would share the window scans; whether DuckDB
      actually wins there needs measuring, not assuming
      (`benches/diagnose_bench.rs` exists for this).

    **The SQL is already shaped for a fan-out — measured 2026-08-06, and this is
    the cheapest route in.** `build_diagnosis_sql` opens with
    `WITH args AS (SELECT ?::VARCHAR AS service, make_timestamp(?)…)` — a
    **one-row** parameter table — and all **20** downstream sections join against
    it as `WHERE x.service_name = a.service`, a column reference rather than a
    baked-in literal. Widening `args` from one row to N is therefore the entry
    point: most sections fan out without being rewritten. What the change actually
    costs:

    - **A `GROUP BY` sweep across the 20 sections.** Each returns one row for one
      service today; with N in `args` each must group by service and the JSON
      assembly must key by it. Mechanical but not a find-and-replace.
    - **The partial-failure property is lost and must be re-derived.**
      `diagnose()` currently catches a per-service error and still returns the
      rest (`results` + `errors` maps). One query means one failure kills all N.
      Either accept that or detect which services came back empty.
    - **Measure before committing.** Ten round-trips over the same window may
      already share DuckDB's cache, so the win might be correlation rather than
      speed. `benches/diagnose_bench.rs` settles it.

    **Not a blocker, contrary to first reading:** lines ~763/772/834 interpolate
    `service_name = '{svc}'` with `format!`, which looks like per-service SQL that
    would have to be unified. They build **`drill_down_sql` strings** — text handed
    to the caller to paste into `/v1/query` — assembled in Rust *after* the single
    `query_row` returns. They are not executed by `diagnose_one`, cost no
    round-trip, and **must stay per-service**, because a drill for one finding has
    to name one service. A fan-out keeps them as they are.

    **Decision basis for looking at it at all:** the product claim is repair in
    fewer turns (#51). An agent that must call `/v1/diagnose` per service and do
    the correlation itself spends turns and tokens on work the query engine could
    do in one pass — so this is one of the few items that feeds the benchmark
    claim directly rather than being adjacent to it.

    Not decided, and each changes the size: whether this is (a) a
    `MAX_SERVICES`-wide single-pass rewrite of `build_diagnosis_sql`, (b) a new
    cross-service correlation section added to the existing per-service blocks, or
    (c) a separate `/v1/diagnose` mode that takes a *trace* or a time window
    instead of a service list and reports the graph. Also open: whether
    `MAX_SERVICES = 10` is a real limit or an arbitrary guard — it has no stated
    justification in the code.

    Answer the "why" before writing code. `diagnose_one` is also the function
    named in **#60** as the extraction candidate for the erosion ceiling (CC 24 /
    289 SLOC), so restructuring it may resolve both — check #60 before starting
    so the two do not conflict.

### Deferred (revisit when needed)
- **OpenUI generative UI** — [[architecture/investigation.md|investigation]] results as self-rendering interactive components (charts, tables, sparklines) instead of raw JSON. Two integration points: (1) `/investigate/ui` endpoint that feeds SQL results to a cheap LLM → OpenUI Lang output → streamed to frontend, (2) HTTP/agent responses return OpenUI components instead of text dumps. Sigiro-specific component library: `SigiroLatencyChart`, `SpanWaterfall`, `LogTable`, `MetricSparkline`, `DeployTimeline`. 67% fewer tokens than JSON-render. Works with React/RN/Vue/ShadCN. See [openui.com](https://www.openui.com/) + [OpenUI Lang spec](https://www.openui.com/docs/openui-lang). Explored 2026-04-12.
- DB visibility — raw `db.*` span attrs already queryable; build first-class views when correlating hosted DB platform data (PlanetScale, Neon, Supabase)
- OTel [[architecture/data-model.md|Arrow]] zero-copy — no scale need yet
- [[architecture/ducklake-integration.md|DuckLake]] compaction tuning — premature optimization
- Fly pollers/scrapers — agent scrapes live instead
- Fly metadata enrichment — OTel SDK handles it
- OAuth2 / hosted web UI — revisit when hosted multi-tenant has users
- **Log parser upgrade** — Migrate hand-rolled DRAIN (`src/drain.rs`) to a proper Rust crate similar to [`axiomhq/drain3`](https://github.com/axiomhq/drain3) (Go) or port a mature implementation. Current DRAIN is ~100 lines, works, but lacks masking, persistence, and configurability. A production-grade Rust log template extraction crate would benefit the wider community; consider publishing `sigiro-drain` or contributing to `drainrs` once it matures.

### Key Decisions (this review session)
- **[[designs/future/smart-sre.md|Smart SRE]]**: Sigiro is the agent's full debugging toolkit. One binary: app telemetry + OS state ([[designs/future/smart-sre.md|osquery]]) + kernel tracing (aya eBPF) + escape hatch (bpftrace). See docs/designs/[[designs/future/smart-sre.md|smart-sre]].md
- **Agent scrapes live**: Sigiro does NOT poll platform APIs. The agent reads Fly/Vercel/Railway/PlanetScale APIs during [[architecture/investigation.md|investigation]]. agents.txt teaches it how.
- **On-demand fan-out**: Fleet queries are live fan-out (arbitrary [[designs/future/smart-sre.md|osquery]]/[[designs/future/ebpf-gpu-position.md|eBPF]]), NOT pre-synced background tables. Agent decides what to ask, not us.
- **[[designs/future/three-layer-debugging.md|Three-layer]] debugging**: Layer 1 (Sigiro telemetry) → Layer 2 (platform APIs) → Layer 3 ([[designs/future/smart-sre.md|osquery]] + eBPF). See docs/designs/[[designs/future/three-layer-debugging.md|three-layer-debugging]].md
- **[[designs/future/perses-compat.md|Perses]] over custom UI**: Protocol compat (Prometheus/Tempo/Loki/[[architecture/profiles.md|Pyroscope]] APIs) instead of custom flamegraph/log viewer endpoints
- **Quack for federation (updated 2026-07-11 — supersedes the original Spice.ai plan, which was never built)**: Not custom peer routing, not duckherder alone, and not Spice.ai. Each self-hosted node optionally serves its own DuckLake read-only over quack (`SIGIRO_QUERY_SURFACE=quack`); the `sigiro query` CLI is a client-side coordinator — no central node, no worker registry. See [[architecture/distributed-query.md|distributed-query]].md and [[guides/self-host-edge-federation.md|the operator guide]].
- **Deployment events as OTel spans**: No custom table. Ingest CI/CD pipelines (Jenkins, GitHub Actions, Keptn) as standard CICD semantic convention spans. Webhook adapter for non-OTel tools.
+ **Fleet observability via [[architecture/data-model.md|DuckDB]] extensions**: system_stats, fsquery, hostfs loaded by default. Agents query fleet state with SQL. No SSH, no agents on hosts.
- **instance_status in admin.sqlite**: Queryable key-value table for collector capabilities, [[designs/future/ebpf-gpu-position.md|eBPF]] [[plans/STATUS.md|status]], missing permissions. Not a JSON file.
- **[[designs/pricing.md|Pricing]]**: Free locally. Hosted pricing TBD. See docs/designs/pricing.md (PROPOSED)
- **Auth**: Open mode for self-hosted deployments; hosted TenantKeys through the shared Postgres control plane
- **Single binary**: [[designs/future/smart-sre.md|osquery]] + bpftrace embedded via include_bytes!(), [[designs/future/ebpf-gpu-position.md|eBPF]] probes compiled in via aya. One download, one `sigiro setup`.

---

## Architecture

One binary: `sigiro`. Receives [[architecture/write-path.md|OTLP]], stores in [[architecture/data-model.md|DuckDB]], serves [[architecture/investigation.md|investigation]] queries.

```
sigiro (Rust binary, ~4800 lines)
├─ :4317 gRPC  — OTLP traces/logs/metrics
├─ :4318 HTTP  — OTLP HTTP (protobuf + JSON)
├─ :9999 HTTP  — /v1/ (query, investigate, services), /admin/ (key mgmt), /health
├─ DuckDB embedded (duckdb-rs, in-process)
├─ otlp2records (upstream, OTLP → Arrow)
├─ Per-project DuckDB pool (multi-tenant data isolation)
├─ Admin SQLite (API key storage, separate from project data)
└─ Local disk default, S3 opt-in
```

Not an extension. Not two processes. One binary that IS the product.

---

## MVP — Build Now

Single Rust binary. Receive [[architecture/write-path.md|OTLP]]. Store in [[architecture/data-model.md|DuckDB]]. Query via SQL + [[architecture/investigation.md|InvestigationBlock]].

| Component | What | Lines |
|-----------|------|-------|
| server.rs | tonic gRPC :4317 + axum HTTP :4318/:9999 + admin endpoints | ~200 |
| ingest.rs | OTLP handler → tokio::spawn_blocking transform → per-project bounded channel | ~280 |
| inserter.rs | Per-project flusher threads, buffered flush (10k rows/1s timer) | ~300 |
| investigate.rs | SQL queries + span tree assembly → InvestigationBlock JSON | ~200 |
| schema.rs | CREATE TABLE IF NOT EXISTS, auto-create on first data | ~80 |
| main.rs | CLI: sigiro [serve\|query\|investigate\|status] | ~80 |
| auth.rs | Open/TenantKeys request authentication | ~150 |
| pool.rs | Per-project DuckDB connection pool (DashMap, LRU) | ~150 |
| client.rs | HTTP client for CLI → server (investigate, query, admin) | ~100 |
| **Total** | | **~4800** |

**Schema:** otlp2records native. Separate metric tables (gauge/sum/histogram/exp_histogram) for zero-copy insert.

**User experience:**
```bash
sigiro              # zero config, local storage
docker run sigiro   # container
sigiro investigate checkout  # InvestigationBlock JSON
```

---

## Phase 1 — Harden (post-MVP)

| Item | What | Lines | Decision basis |
|------|------|-------|---------------|
| ~~Multi-tenant auth~~ | ~~Tenant keys, per-tenant storage, hosted control plane~~ | ~~470~~ | ~~Done (83 tests)~~ |
| ~~WAL~~ | ~~Decided against — DuckDB has its own WAL. Custom WAL only protected ms-level window DuckDB already covers. See docs/decisions/001-wal-not-needed.md~~ | ~~0~~ | ~~Rejected after analysis~~ |
| ~~Anomaly view~~ | ~~v1 done (flat thresholds). v2 planned: relative 3σ baselines, cross-signal (see docs/designs/anomaly-view.md)~~ | ~~20 SQL~~ | ~~v1 done, v2 pending~~ |
| ~~Log template dedup~~ | ~~DRAIN algorithm at insert time. template_id column + sigiro_log_templates lookup. investigate does GROUP BY template_id JOIN.~~ | ~~~200 Rust~~ | ~~Done (23 DRAIN tests + e2e)~~ |
| ~~OTLP profiles~~ | ~~ProfilesService gRPC + dictionary resolution + sigiro_profiles table + investigate queries. SDK validation with Pyroscope Go/Python/Node/Java.~~ | ~~~200~~ | ~~Done~~ |
| **Phase 1 total** | | **~210** | |

---

## Phase 2 — Reach

| Item | What | Lines | Decision basis |
|------|------|-------|---------------|
| Vercel Marketplace | Wire vercel.rs routes, mount in server.rs | ~50 | Code exists, not wired. Blocked on having a compelling story beyond server traces |
| Docker + PaaS deploy | Dockerfile, Railway template, Render Blueprint, Fly.io | ~50 config | 8/8 consensus |
| ~~Web Vitals + client errors~~ | ~~DEPRIORITIZED: platforms give this free (Vercel Speed Insights, Cloudflare Analytics). Agent reads platform APIs instead. See docs/designs/platform-correlation.md~~ | ~~0~~ | ~~Agent correlates~~ |
| ~~DB visibility~~ | ~~Extract db.statement/db.system from spans, slow query grouping, N+1 detection~~ | ~~100~~ | ~~MOVED TO BACKLOG — raw span attrs queryable today; build first-class views when correlating hosted DB platform data (PlanetScale, Neon, Supabase). See docs/designs/db-visibility.md~~ |
| Deploy event correlation | Ingest CI/CD as OTel spans (Jenkins OTel Plugin, GitHub Actions OTel, Keptn). Webhook adapter for ArgoCD/Flux via POST /v1/events. Agent queries: `SELECT ... FROM sigiro_spans WHERE span_name LIKE 'cicd.pipeline%'` | ~50 | See docs/architecture/distributed-query.md — deployment events section |
| ~~npm @sigiro/nextjs~~ | ~~NO WRAPPER — @vercel/otel + OTEL_EXPORTER_OTLP_ENDPOINT env var is enough. Just docs.~~ | ~~0~~ | ~~Reuse existing OTel SDKs~~ |
| OAuth2 signup flow | GitHub/Google OAuth → create hosted tenant + tenant key. Only needed for hosted multi-tenant + web UI. | ~250 | UNDECIDED — revisit when hosted/UI is planned |
| Stripe billing | Usage metering via Stripe Meters API. Same as OAuth — only needed for hosted. | ~300 | UNDECIDED — revisit when hosted is planned |
| S3 storage (ops only) | DuckLake already writes to S3 — just configure bucket + catalog env vars | 0 | Code done, needs test + docs |
| Prometheus remote_write | axum route, parse protobuf, map to sigiro_metrics. Nice-to-have — OTel Collector bridges Prometheus→OTLP already. | ~150 | LOW — OTLP covers this, only 5/8 consensus |
| Pyroscope SDK profiles | HTTP :4040 /ingest — pprof + JFR + folded stacks parser | ~150 | 6/8 consensus |
| **Phase 2 total** | | **~1000+** | |

---

## Phase 3 — Optimize (when scale demands)

| Item | What | Lines | Decision basis |
|------|------|-------|---------------|
| ~~OTel Arrow zero-copy~~ | ~~Register ArrowTracesService alongside TraceService on :4317~~ | ~~150~~ | ~~DEMOTED to Phase 4+ — optimization for scale we don't have~~ |
| Perses/Grafana protocol compat | Prometheus + Tempo + Loki + Pyroscope HTTP API endpoints backed by DuckDB SQL. Replaces custom flamegraph endpoint. See docs/designs/perses-compat.md | ~600 | Replaces flamegraph endpoint. Parser crates: promql-parser, logql, traceql |
| ~~DuckLake compaction tuning~~ | ~~Query result caching, materialized views~~ | ~~100~~ | ~~REMOVED — premature optimization. DuckLake handles compaction. Revisit when perf is a real problem.~~ |
| Agent-system metric correlation | OTel Collector hostmetrics receiver sends /proc data to Sigiro. Add correlation query to InvestigationBlock: join gen_ai.* spans with system metrics by timestamp. "MemAvailable dropped 40MB at tool call #17". Sigiro does NOT read /proc itself. | ~30 SQL + docs | Edera article insight. Simplified: OTel Collector reads /proc, Sigiro correlates. |
| Helm chart | Package Sigiro for K8s deployment. Map namespace → project. | ~50 config | From long-term plan. Event ingestion (K8s, Fly, Vercel, Railway) covered by docs/designs/platform-correlation.md — agent correlates now, Sigiro ingests later. |
| Platform marketplace integrations | Provisioning webhooks for Netlify, Railway, Render marketplaces. Each: receive webhook → admin API create project + API key → return OTLP endpoint. Vercel already in Phase 2. Platform handles auth + billing (rev share). | ~50/platform | Unblocked — hosted Sigiro on Fly + multi-tenant auth exist. |
| Usage-based pricing + RBAC | Stripe Meters for direct users (~50 lines). Marketplace users: platform bills. Hosted TenantKeys for auth (no Zitadel). See docs/designs/pricing.md + docs/designs/auth-billing.md | ~100 | Free locally; hosted pricing TBD |
| Blog post + launch | "I replaced Datadog with DuckDB" → HN, X, Reddit | 0 | 8/8 consensus |
| **Phase 3 total** | | **~900** | |

---

## Phase 4 — Fly.io First-Class Support (long-term)

Why: [[company/gtm.md|Fly.io]] has massive observability gaps — no tracing, no profiling, lossy ephemeral
logs, no alerting, and zero visibility into scale-to-zero lifecycle. Their users are
exactly Sigiro's target (indie devs, small teams shipping fast). Sigiro already deploys
on [[company/gtm.md|Fly.io]] — dogfooding this is free.

### Fly.io Observability Gaps Today

| Gap | What Fly gives you | What's missing |
|-----|-------------------|----------------|
| Logs | Ephemeral NATS stream, `flyctl logs` | Persistent storage, search, lossy under load |
| Tracing | Nothing | No OTLP, no collector, no distributed tracing |
| Profiling | Nothing | No continuous profiling |
| Alerting | Nothing | No webhooks, no thresholds, no notifications |
| Metrics | Managed Prometheus (`api.fly.io/prometheus/<org>/`) | Limited retention, no correlation, basic dashboard |
| Scale-to-zero | Proxy triggers autostart | Cold start invisible, no lifecycle event stream |
| Multi-region | Metrics labeled by region | No cross-region comparison, replay routing opaque |

### Platform Context Sigiro Can Ingest

[[company/gtm.md|Fly.io]] exposes rich metadata that no observability tool correlates today:

**Runtime env vars (auto-detect inside Machine):**
- `FLY_APP_NAME`, `FLY_MACHINE_ID`, `FLY_ALLOC_ID`, `FLY_REGION`
- `FLY_PUBLIC_IP`, `FLY_IMAGE_REF`, `FLY_MACHINE_VERSION`
- `FLY_PROCESS_GROUP`, `PRIMARY_REGION`, `FLY_VM_MEMORY_MB`

**Machines API (poll or webhook when available):**
- Machine states: created → started → stopping → stopped → destroyed
- Machine events: launch, start, stop, exit (with exit code + signal)
- Health check [[plans/STATUS.md|status]] per check name (passing/warning/critical)
- Machine config: CPU kind/count, memory, image, services

**Proxy headers (from Fly edge):**
- `Fly-Region`, `Fly-Client-IP`, `fly-replay` (cross-region routing)
- Edge HTTP metrics: response times + [[plans/STATUS.md|status]] codes per region

**Prometheus endpoint (`api.fly.io/prometheus/<org>/`):**
- `fly_instance_up`, `fly_instance_cpu`, `fly_instance_memory_*`
- `fly_instance_disk_*`, `fly_instance_net_*`, `fly_instance_load_average`
- `fly_edge_http_*`, `fly_app_concurrency`, `fly_app_http_response_time`
- Custom app metrics (if exposed via `[metrics]` in fly.toml)

### Decision: Agent Scrapes Live, Sigiro Doesn't Poll

All platform data (Fly events, metrics, machine state) is scraped live by the
agent during investigations via platform APIs. Sigiro does NOT build pollers.
Rationale: freshest data, no credentials in Sigiro, no sync lag, works with
any new platform instantly (just add to agents.txt). Historical retention is
the platform's problem — most debugging is recent. See docs/designs/[[designs/future/three-layer-debugging.md|three-layer-debugging]].md.

### Implementation

| Item | What | Status |
|------|------|--------|
| ~~Fly metadata enrichment~~ | ~~Auto-detect FLY_* env vars~~ | ~~DEFERRED — OTel SDK handles via OTEL_RESOURCE_ATTRIBUTES~~ |
| ~~Fly Machines API poller~~ | ~~Poll machine states + events~~ | ~~REMOVED — agent scrapes Fly API live (agents.txt)~~ |
| ~~Fly Prometheus scraper~~ | ~~Scrape api.fly.io/prometheus/~~ | ~~REMOVED — agent reads Fly Prometheus API live~~ |
| ~~Fly Log Shipper replacement~~ | ~~Accept Fly NATS stream~~ | ~~REMOVED — app sends OTLP logs directly to Sigiro~~ |
| ~~Cold start tracking~~ | ~~Measure proxy-autostart latency~~ | ~~REMOVED — agent correlates machine events + spans live~~ |
| ~~Cross-region InvestigationBlock~~ | ~~Correlate fly-replay header~~ | ~~REMOVED — agent reads fly-replay from span attributes~~ |
| `sigiro.com/for/fly` | Landing page: "Replace Fly's broken logging with Sigiro" | GTM — keep |
| `fly.toml` one-liner | Sigiro as sidecar or co-located Machine with OTLP endpoint | Docs/config — keep |

---

## Decided Against (with reasoning)

| Item | Decision | Why |
|------|----------|-----|
| sigiro_sentinel extension | REPLACED by SQL view + cron/agent poll | 8/8: a timer + SQL + curl is not an extension |
| sigiro as DuckDB extension | REPLACED by standalone binary embedding DuckDB | 8/8: simpler build, no !Send/!Sync, no -unsigned |
| Dependency signal (Plan 20) | SKIP — query-time extraction | 7/8: `WHERE span_kind=3` + extract peer.service |
| eBPF auto-instrumentation | SKIP — keep separate | 8/8: don't add privileged requirement to sigiro |
| File compaction (Plan 09) | SKIP — DuckLake handles it | 7/8: if DuckLake can't compact, switch storage strategy |
| Local hot tier (Plan 10) | SKIP — DuckLake inlined data IS the hot tier | 6/8: test DuckLake before building custom |
| Trigram log index (Plan 12) | SKIP — DuckDB LIKE/FTS extension sufficient | 8/8: custom index for `<1%` of users |
| k8s attribute enrichment | SKIP — document downward API + OTel Collector gateway | 8/8: don't rebuild k8sattributesprocessor |
| Tail sampling | SKIP — store everything, delegate to OTel Collector | 8/8: $150/month at 1000 services without sampling |
| Loki push ingest | SKIP — OTel logs covers it | 7/8 |
| Parca agent support | SKIP — tell users to use OTel eBPF profiler | 6/8: same eBPF, speaks OTLP |
| Unified sigiro_metrics table | REPLACED by 4 separate metric tables | 8/8: zero-copy insert, no schema projection |
| httpserver extension | REPLACED by axum routes on :9999 | 8/8: already running axum |
| Parquet-native storage | REPLACED by DuckLake | DuckLake handles compaction, S3, catalog — no custom Parquet writer needed |
| Logto + Unkey + Lago | REPLACED by Open mode + hosted TenantKeys and planned direct Stripe | Too many moving parts. |

---

## Historical Plans (Go era — archived)

Plans 01-07, 19 were implemented in Go and deleted during the Go→Rust migration.
See `docs/architecture/rust-migration-gaps.md` for the 22 behaviors preserved.

Plans 08-23 are preserved in `docs/plans/` as historical context. The decisions
above supersede them.
