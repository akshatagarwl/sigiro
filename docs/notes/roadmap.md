---
title: Roadmap
created: 2026-03-16
updated: 2026-09-11
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'data-model', 'decision']
---

# Roadmap

Single Rust binary embedding [[architecture/data-model.md|DuckDB]]. Simplest possible architecture.

Execution-ready work lives in [GitHub Issues](https://github.com/akshatagarwl/sigiro/issues). This file contains only unfiled or deferred roadmap ideas.

---

## Priority Order (reviewed 2026-04-11)

### Build Now (Phase 1 — unblocked, high value)
- **#89 — Small Parquet files: two levers that work, and two that don't. BLOCKED on upstream DuckLake.** Quiet projects emit ~11 KiB Parquet files. Two attempts to fix it are recorded here so neither gets retried.

  **Lever 1, flush interval: tried, disproved, reverted.** `SIGIRO_FLUSH_INTERVAL_SECS` went 1 to 60 and shipped on 2026-08-24. Production over a 15-hour window: file rate fell from 612-910/day to **407/day** and average file size moved **11 KiB to 12 KiB**. The premise was wrong. At ~0.5 rows/s telemetry arrives in bursts every few minutes and the flusher skips when the buffer is empty, so file count tracks the arrival pattern, not the timer. Reverted the same day. Measure file *size*, not count, before touching this again.

  Two methodology notes, both worth keeping. A synthetic ladder that generates one file per simulated second does **not** model a bursty 0.5 rows/s workload, and it produced a projection 140x off. And counting `ducklake_data_file` joined to `ducklake_snapshot` silently drops every file whose creating snapshot retention has expired: 522 of 1844 rows here, 28%. Count on `begin_snapshot` id ranges instead.

  **Lever 2, DuckLake data inlining: right mechanism, wrong trigger, and blocked.** `data_inlining_row_limit` defaults to 10 against a measured 77-112 row flush, so nothing ever inlines. Raising it puts small appends in the catalog and keeps them queryable there. Measured locally at limit 200, 60 inserts of 112 rows: 3 Parquet files averaging 204 KiB versus 61 averaging 10.8 KiB, identical row counts, and rows queryable while still inlined (112 in, 112 visible, 0 files on disk). The limit also self-selects, a 112-row insert inlines and a 10,000-row insert writes Parquet directly.

  That local result **does not transfer** by itself. Production now checkpoints
  hourly, while the experiment checkpointed every 30 **inserts**. A fixed timer
  still cannot express the desired rule: materialize once enough rows/bytes
  have accumulated, with a max-age bound. Inlining also remains blocked for
  multi-writer tables by ducklake#1215, reproduced unchanged on DuckDB/DuckLake
  1.5.5 on 2026-09-10: both cross-store deletes committed and 29 deleted rows
  reappeared.

  **Safety correction closed 2026-09-10 ([#102](https://github.com/akshatagarwl/sigiro/pull/102)).**
  Sigiro now persists `data_inlining_row_limit=0` for `sigiro_profiles` on every
  attach. This disables inlining in fresh catalogs and replaces the unsafe
  persisted 1,000-row value in existing catalogs while retaining DuckLake's
  native `auto_compact` and hourly `CHECKPOINT`. A migration regression seeds
  the old value, reattaches the catalog, and verifies the resulting policy.
  The small-file item itself remains blocked on the upstream correctness gate.

  **Upstream issues, verified against the current DuckDB/DuckLake 1.5.5 artifact:**
  - [ducklake#1335](https://github.com/duckdb/ducklake/issues/1335) (**open upstream, no longer reproduces**) — the exact direct→inline→direct sequence now commits 201 rows with 201 distinct IDs. The stale `next_file_id` collision reproduced on 1.5.3 but is not a current blocker.
  - [ducklake#1215](https://github.com/duckdb/ducklake/issues/1215) (**open, still reproduces**) — concurrent deletes straddling `data_inlining_row_limit` both commit, so deleted rows can reappear. Touches our retention `DELETE` path.
  - [ducklake#1067](https://github.com/duckdb/ducklake/issues/1067) (closed) — inline cleanup; confirm whether `expire_snapshots` + `cleanup_old_files` now reclaims dead inline rows.
  - [ducklake#1401](https://github.com/duckdb/ducklake/discussions/1401) — predicates are not pushed into the inline catalog table, so the hot set is transferred and filtered in DuckDB. Fine for 30s of inline data, not for hours.

  So: do not enable inlining until those gates pass on a proven artifact. Pick the limit from per-table append p95/p99 **rows and bytes**, not an average, because the decision is per-table and row-count-only while a sub-200-row log or profile batch can approach the 8 MiB body cap.

  **Interim lever that is not blocked:** scheduled [`ducklake_merge_adjacent_files`](https://ducklake.select/docs/stable/duckdb/maintenance/merge_adjacent_files) with a 1s flush. Costs rewrite traffic and temporary R2 objects, but it does not turn the constrained catalog Postgres into a telemetry staging store.
### Build Next (Phase 2 — reach)
5. **S3 storage** — [[architecture/ducklake-integration.md|DuckLake]] config + test + docs, 0 new lines
6. **Docker + PaaS deploy** — Dockerfile, Railway/Render/Fly templates, ~50 config
7. **Vercel Marketplace** — wire existing code, ~50 lines
### Build Later (Phase 3 — Smart SRE + optimize)
9. **sysquery module** — procfs/sysinfo Rust-native OS state (processes, memory, network, disk). ~500 lines. Fallback when [[designs/future/smart-sre.md|osquery]] unavailable (macOS, containers).
10. **Embedded [[designs/future/smart-sre.md|osquery]]** — Bundle osqueryd static binary (~8MB compressed) via include_bytes!(). Extract on `sigiro setup`. Full 200+ tables. ~100 lines (subprocess manager + osquery extension API).

12. **Embedded bpftrace** — Bundle static binary (~3MB) for escape hatch. Agent writes arbitrary bpftrace/bpfquery SQL when pre-built probes aren't enough. ~50 lines.
13. **Perses/[[designs/future/perses-compat.md|Grafana]] protocol compat** — Prometheus + Tempo + Loki + [[architecture/profiles.md|Pyroscope]] HTTP APIs, ~600 lines. See docs/designs/[[designs/future/perses-compat.md|perses-compat]].md
14. **Platform marketplace integrations** — Netlify/Railway/Render webhooks, ~50/platform
15. **Usage-based [[designs/pricing.md|pricing]] + RBAC** — Stripe Meters + [[designs/auth-billing.md|Zitadel]], ~100 lines. See docs/designs/pricing.md + docs/designs/[[designs/auth-billing.md|auth-billing]].md
16. **Helm chart** — K8s packaging, ~50 config
17. **Blog post + launch** — "Your agent debugs like a senior SRE" → HN, X, Reddit

### Build Eventually (Phase 5 — fleet)

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

39. **Shorten the Ask AI prompt preamble** — the shipped prompt variants repeat
    three public URLs before every question. `llms-full.txt` is load-bearing, but
    `sigiro.com/openapi.json` is the least likely to change an answer and is
    already discoverable through `/.well-known/api-catalog`. Decide whether to
    drop that URL to shorten every prompt by about 45 characters.

40. **Update the install-with-ai Go guidance** — the guide still says Go has no
    runtime agent and recommends handwritten SDK setup. Point its Go path to the
    shipped compile-time `otelc go build` guidance instead.

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
      `profiles::v1development`, `src/ingest.rs:24-26`). The protocol is unstable,
      so pin the producer digest. ORCA-bench's published snapshots contain **no**
      profiles, so getting them means re-collecting.

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
    At the time, the former Worldpol root `env.clear` carried
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
    `/v1/services` → `[]` and `/v1/diagnose services=[sigiro-hosted]` →
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
    `server/sigiro.ts:5` reads `SIGIRO_OTEL_AUTH_KEY` as *its* OAuth access token while
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
    `healthcheck` go through `SigiroClient`; `serve` is legitimately local
    because it *is* the server). Three did not. **Two are
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
      `POST /v1/diagnose {"services":["checkout"]}` resolve the identical window.
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
    except `serve` reaches its result through `SigiroClient`.
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

## Go-to-market plan: distribution channels (added 2026-09-05)

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

**The current experience every channel leads to:**

> Install → connect your own application → ask the agent what happened → inspect
> the evidence.

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
- **The skill and its public distribution exist.** `debug-with-sigiro` is the
  shared payload for #97, #98, #100, #48 and #57. Keep its public repository,
  `npx skills add` installation, generated endpoint and live endpoint byte
  identical rather than creating channel-specific copies.
- **No prepared incident or example application exists, and one is not needed
  for the current distribution work.** `dev/` holds the Lima VM only; `scripts/`
  holds benchmarks and audits. The incident campaign is deferred (#95, #99,
  #102, #103); plugin and skill distribution must not wait for it.
- **Local storage runs `Open` auth** (`src/auth.rs:35`): no per-request auth,
  protected by the network perimeter. A public Railway template would expose
  ingest and queries to the internet, so Railway is deferred.
- **Hosted tenants are provisioned by hand** (`www/docs/how-to/
  hosted-onboarding.md:3`. The hosted authentication integration depends on the Better Auth/OAuth API; Sigiro exposes no provisioning UI.
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

Prepared-incident demonstrations are deferred. Current distribution should
teach users to connect their own application and should make no campaign claim
that needs a recorded example to substantiate it.

### Deferred distribution experiments

95. **Prepared incidents and shared example application — DEFERRED 2026-09-11.**
    We do not need to display an incident for now. This is not a prerequisite
    for the skill, plugin submissions, tracking, or other distribution work.

    If demand later justifies a demonstration campaign, build one runnable app
    with a reproducible failure, a deploy marker and actual Sigiro evidence in
    the public integration repo. Until then, do not create the FastAPI timeout,
    Node.js regression, recordings, "Try this incident" surface, or supporting
    campaign infrastructure.

99. **FastAPI incident tutorial and landing page — DEFERRED 2026-09-11.** The
    runnable example, recording and interactive "Try this incident" surface are
    not needed for the current distribution phase. Revisit only after real
    users show that a prepared demonstration would remove a specific adoption
    barrier. `/for/claude-code` can be built independently when it has real,
    platform-specific setup material and does not need an incident campaign.

102. **Node.js incident tutorial — DEFERRED 2026-09-11.** This was the second
     prepared-incident demonstration and is deferred with #95/#99.

103. **Weekly incident-to-content cycle — DEFERRED 2026-09-11.** Do not commit
     to recurring demonstrations, walkthroughs, tutorials or runnable incidents
     before the first prepared incident is justified. The general build log in
     #27 can continue independently.

105. **A capped Reddit experiment: $200 over seven days** (added 2026-09-05) —
     a learning budget, not an expected acquisition cost. Creative is undecided
     while the prepared-incident campaign is deferred. Check in Ads Manager
     whether r/ClaudeCode,
     r/cursor or r/devops are eligible; Reddit's community targeting follows
     engaged users across Reddit and does not guarantee placement inside those
     communities.

     Review after the first $100. Spend the remainder only if the results
     justify continuing or testing one specific improvement. Google Search
     comes afterwards, and only once Keyword Planner shows real demand for
     phrases such as "OpenTelemetry MCP server"; that volume is unverified.

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

81. **Benchmark Sigiro against raw telemetry and existing observability tools** —
    no published result establishes that Sigiro improves root-cause accuracy,
    repair rate, wall time, tokens, or investigation steps. Compare reproducible
    failures with Sigiro, raw OTel, logs only, and an existing observability MCP.
    This is unbuilt; the research and experiment design remain in item #51.

116. **OpenTelemetry semantic contract and destructive storage v2** (added
     2026-08-17) — design before building. Prompted by Juraci Paixão Kröhling's
     ["OTLP support is not OpenTelemetry support"](https://telemetrydrops.com/blog/otlp-is-not-otel-native/)
     (Telemetry Drops, 2026-05-25): accepting the wire format is the easy part;
     preserving its resource model, semantic-convention provenance and attribute
     names is the product contract.

     **Current verdict, checked against `main`: two yes, two partial, one no.**
     Attribute names survive ingest unchanged in the JSON bags
     (`src/otlp.rs:730-742`), and a query can use the name the SDK emitted. Spans,
     logs and metrics also carry the same promoted `service.name`,
     `service.namespace`, `service.instance.id` fields plus the full resource bag
     (`src/schema.rs:20-31,49-59,88-95`). The gaps are structural:

     - `ResourceSpans`/`ScopeSpans`, `ResourceLogs`/`ScopeLogs` and
      `ResourceMetrics`/`ScopeMetrics` reach the decoder, but both wrapper-level
      `schema_url` values are discarded (`src/otlp.rs:349-360,390-401,437-448`).
     - Resource identity is duplicated per record and never checked for agreement
      across signals. Profiles have a narrower envelope than the other signals.
     - Resource validation has already drifted between builders: spans and logs
      reject missing or `unknown_service*` names, while metric builders append an
      empty `service_name` (`src/otlp.rs:1130-1131`, with the same pattern in the
      other metric shapes).
     - `/v1/services` scans `sigiro_spans` alone (`src/server.rs:1339-1346`), so a
      logs-only or metrics-only service is invisible even though its data exists.
     - Product-owned analysis hardcodes one convention generation — for example
      `http.response.status_code` and current `gen_ai.*` names — with no version
      provenance or explicit alias policy (`src/diagnose/mod.rs:1245-1251,
      1455-1474`). An SDK upgrade can therefore make one section silently empty.

     **Proposed direction: one decoded OTel envelope, separate physical signal
     tables.** Do not merge spans, logs, four metric shapes and profiles into one
     nullable event table. Their shapes are irreducibly different, and the current
     four-table metric split avoids insert-time projection for that reason
     (`www/docs/explanation/tables.md:84-98`). Physical co-location would not make
     the resource model shared. Give every signal table the same envelope instead:

     ```text
     resource_fingerprint
     resource_schema_url
     resource_attributes
     resource_dropped_attributes_count
     resource_entity_refs

     scope_fingerprint
     scope_schema_url
     scope_name
     scope_version
     scope_attributes
     scope_dropped_attributes_count

     service_name
     service_namespace
     service_instance_id
     ```

     The fingerprints identify the exact, type-preserving resource/scope snapshot;
     they are not stable entity IDs. A full resource bag can contain volatile
     process and pod attributes, so hashing it and calling the result an entity
     would create false identity. Keep the original `service.*` keys inside
     `resource_attributes` when promoting them: the promoted columns are scan
     accelerators, not replacements for the OTel vocabulary.

     **One decoder owns the contract.** Introduce a shared `OtelContext` /
     `ResourceContext` before any signal-specific builder. It validates
     `service.name`, preserves both schema URLs, serializes attributes, computes
     fingerprints and carries dropped counts/entity references once. Every span,
     log, metric and profile builder receives that context instead of extracting
     the resource independently. This deletes the duplicated policy that let
     metrics and traces disagree; it is also the DI seam — signal builders depend
     on one decoded context rather than on the whole OTLP wrapper and their own
     extraction rules.

     Invalid records use one policy across all signals: accept valid siblings,
     reject invalid records explicitly, return a standards-compliant OTLP
     partial-success response and retain rejection counts/reasons as operator
     evidence. Never synthesize a service name or silently skip a record, because
     either choice changes the meaning of the payload.

     **Compatibility happens at query time, over untouched raw data.** Maintain a
     small, pinned registry only for concepts sigiro itself interprets, for example
     `http.request.method` ← `http.method`, `http.response.status_code` ←
     `http.status_code`, `db.system.name` ← `db.system` and
     `deployment.environment.name` ← `deployment.environment`. The diagnosis uses
     the ordered aliases and reports the raw key that matched. It does not rewrite
     ingest, hide the source under a canonical-only view or claim to understand
     every convention whose attributes it preserves. A missing `schema_url` is
     unknown provenance, not a failure; real SDKs often leave it empty, so drift
     detection also compares observed keys and instrumentation scope versions.

     Add a `telemetry_contract` section to `/v1/diagnose`, with runnable SQL behind
     every finding. Candidate kinds: `semantic_convention_drift`,
     `mixed_semantic_conventions`, `resource_identity_mismatch`,
     `cross_signal_resource_mismatch`, `dropped_attributes` and
     `ingest_rejections`. A canary legitimately emits two convention generations,
     so mixed data is evidence rather than an automatic instruction to "fix" it.

     **Two metadata surfaces, with different jobs.** Keep #53 `/v1/schema` as the
     static description of tables and columns. Add a separately bounded
     `/v1/attributes?service=...&from_ts=...&to_ts=...` surface for keys actually
     observed, returning location (resource/scope/span/log/metric), signal, JSON
     type, prevalence, first/last seen and scope name/version. Do not return example
     values by default; attributes can contain customer data. Measure whether this
     is better served by a batch-aggregated `sigiro_attribute_observations` table
     or by bounded JSON scans before choosing storage. A companion
     `sigiro_resource_observations` table may record one resource envelope per OTLP
     batch so cross-signal consistency does not require scanning every telemetry
     record; it earns its place only if the measured query justifies it.

     Rebuild `/v1/services` across every signal and return signal-specific
     first/last-seen times plus whether namespace, instance and resource
     fingerprints agree. Do this before inventing a canonical `resource_id`: the
     report measures whether real telemetry has an identity problem and which
     attributes cause it.

     **Migration policy: a clean storage-format boundary.** This project is
     greenfield and Akshat explicitly allows destructive data migration for this
     design. Treat it as storage format v2: detect v1 at boot, fail with a precise
     reset/migration instruction, drop and recreate the telemetry tables, and
     re-ingest fixtures/dogfood data. Do not dual-write v1/v2, add compatibility
     columns or preserve the old builder paths. The version check remains useful
     after this one destructive move because another `CREATE TABLE IF NOT EXISTS`
     cannot silently leave an old catalog behind.

     **Do not build:** one table for all signals; an EAV row for every attribute;
     ingest-time renaming; a stable entity ID derived from the full resource bag;
     automatic rewriting of user SQL; or support for every semantic convention
     before a sigiro analysis consumes it. Each adds machinery without improving
     the first failing contract above.

     **First experiment, before the architecture is accepted:** on a fresh data
     directory, preserve resource/scope schema URLs for spans and route one service
     through two real HTTP semantic-convention generations. Prove SDK → Collector
     → sigiro → SQL round trips for exact dotted keys, arrays, bytes, nested values
     and resource identity; then prove `/v1/diagnose` reports which old/current key
     matched and supplies reconciling SQL. Record schema-URL population, DuckLake
     storage support (`JSON` versus the current JSON-compatible `VARCHAR`), ingest
     overhead of fingerprinting and the prevalence/cardinality of observed keys.
     Those four measurements decide whether the full envelope, observation tables
     and dynamic attribute endpoint survive the spike.

---

54. **Delete the unit tests wearing an integration filename** —
    `tests/federated_sys_stats_test.rs` still calls `validate_read_only()`
    directly. Remove those unit-shaped cases only after verifying that the real
    fleet E2E covers the same hostile SQL over the wire.

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
    no bearer token to `/admin/*`, which is why #58 deleted the now-dead
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

61. **Diagnose across services, not one at a time — `diagnose_one`'s
    single-service boundary** (added 2026-08-06) — understand it first, then
    decide whether to change it. Filed as a question Akshat wants answered, not a
    committed change.

    **Correct the premise before designing anything: multi-service already
    works.** `DiagnoseRequest` takes the required
    `services: Vec<String>` field (max `MAX_SERVICES = 10`), and `diagnose()`
    validates the list, loops, and returns
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
- **[[designs/future/smart-sre.md|Smart SRE]]**: Sigiro is the agent's debugging toolkit: app telemetry plus OS state through osquery.
- **Agent scrapes live**: Sigiro does NOT poll platform APIs. The agent reads Fly/Vercel/Railway/PlanetScale APIs during [[architecture/investigation.md|investigation]]. agents.txt teaches it how.
- **On-demand fan-out**: Fleet queries are live osquery fan-out, NOT pre-synced background tables. Agent decides what to ask, not us.
- **[[designs/future/perses-compat.md|Perses]] over custom UI**: Protocol compat (Prometheus/Tempo/Loki/[[architecture/profiles.md|Pyroscope]] APIs) instead of custom flamegraph/log viewer endpoints
- **Quack for federation (updated 2026-07-11 — supersedes the original Spice.ai plan, which was never built)**: Not custom peer routing, not duckherder alone, and not Spice.ai. Each self-hosted node optionally serves its own DuckLake read-only over quack (`SIGIRO_QUERY_SURFACE=quack`); the `sigiro query` CLI is a client-side coordinator — no central node, no worker registry. See [[architecture/distributed-query.md|distributed-query]].md and [[guides/self-host-edge-federation.md|the operator guide]].
- **Deployment events as OTel spans**: No custom table. Ingest CI/CD pipelines (Jenkins, GitHub Actions, Keptn) as standard CICD semantic convention spans. Webhook adapter for non-OTel tools.
- **instance_status in admin.sqlite**: Queryable key-value table for runtime status. Not a JSON file.
- **[[designs/pricing.md|Pricing]]**: Free locally. Hosted pricing TBD. See docs/designs/pricing.md (PROPOSED)
- **Auth**: Open mode for self-hosted deployments; hosted Better Auth OAuth/JWT through the shared Postgres Better Auth/OAuth API


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
| auth.rs | Open/Better Auth OAuth/JWT request authentication | ~150 |
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
| ~~Multi-tenant auth~~ | ~~OAuth access tokens, per-tenant storage, Better Auth/OAuth API~~ | ~~470~~ | ~~Done (83 tests)~~ |
| ~~WAL~~ | ~~Decided against — DuckDB has its own WAL. Custom WAL only protected ms-level window DuckDB already covers. See docs/decisions/001-wal-not-needed.md~~ | ~~0~~ | ~~Rejected after analysis~~ |
| ~~Anomaly view~~ | ~~v1 done (flat thresholds). v2 planned: relative 3σ baselines, cross-signal (see docs/designs/anomaly-view.md)~~ | ~~20 SQL~~ | ~~v1 done, v2 pending~~ |
| ~~Log template dedup~~ | ~~DRAIN algorithm at insert time. Classification is stored on each log row; diagnosis groups inline template text without a lookup join.~~ | ~~~200 Rust~~ | ~~Done (23 DRAIN tests + e2e)~~ |
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
| OAuth2 signup flow | GitHub/Google OAuth → create hosted tenant + OAuth access token. Only needed for hosted multi-tenant + web UI. | ~250 | UNDECIDED — revisit when hosted/UI is planned |
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
| Usage-based pricing + RBAC | Stripe Meters for direct users (~50 lines). Marketplace users: platform bills. Hosted Better Auth OAuth/JWT for auth (no Zitadel). See docs/designs/pricing.md + docs/designs/auth-billing.md | ~100 | Free locally; hosted pricing TBD |
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
the platform's problem — most debugging is recent.

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

| File compaction (Plan 09) | SKIP — DuckLake handles it | 7/8: if DuckLake can't compact, switch storage strategy |
| Local hot tier (Plan 10) | SKIP — DuckLake inlined data IS the hot tier | 6/8: test DuckLake before building custom |
| Trigram log index (Plan 12) | SKIP — DuckDB LIKE/FTS extension sufficient | 8/8: custom index for `<1%` of users |
| k8s attribute enrichment | SKIP — document downward API + OTel Collector gateway | 8/8: don't rebuild k8sattributesprocessor |
| Tail sampling | SKIP — store everything, delegate to OTel Collector | 8/8: $150/month at 1000 services without sampling |
| Loki push ingest | SKIP — OTel logs covers it | 7/8 |
| Parca agent support | SKIP — use a supported Pyroscope SDK | Parca uses its own ingestion protocol |
| Unified sigiro_metrics table | REPLACED by 4 separate metric tables | 8/8: zero-copy insert, no schema projection |
| httpserver extension | REPLACED by axum routes on :9999 | 8/8: already running axum |
| Parquet-native storage | REPLACED by DuckLake | DuckLake handles compaction, S3, catalog — no custom Parquet writer needed |
| Logto + Unkey + Lago | REPLACED by Open mode + hosted Better Auth OAuth/JWT and planned direct Stripe | Too many moving parts. |

---

## Historical Plans (Go era — archived)

Plans 01-07, 19 were implemented in Go and deleted during the Go→Rust migration.
See `docs/architecture/rust-migration-gaps.md` for the 22 behaviors preserved.

Plans 08-23 are preserved in `docs/plans/` as historical context. The decisions
above supersede them.
