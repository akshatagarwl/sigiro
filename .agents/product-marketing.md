# Product Marketing Context

**Document version:** v4
**Last updated:** 2026-08-01

## Product Overview

**One-liner:** The data layer for software that repairs itself.

**The frame we own:** *software that repairs itself*. Chosen 2026-08-01 over "software factory"
(it describes our process, not the reader's outcome) and "recursive self-improvement" (too
insider). Every section head, docs intro and title tag hangs off this phrase.

The frame was *self-improving software* for most of that day. ASD-STE100 ended it: the standard
bans -ing forms outside technical names, and "self-improving" is one. Keeping it as an approved
exception was tried and rejected, because a brand frame that appears on the landing page in one
form and in the docs in another is worse than either form alone. The claim is unchanged; only
the grammar moved.

**What it does:** You send OpenTelemetry to sigiro. sigiro compares each service with its own
history. When a service changes, one call returns what moved, which deploy caused it, how the
service fails, and the query behind each claim. Your agent reads the result and writes the fix.
The same baseline shows whether the fix worked. One binary, five minutes, your own storage.

**Product category:** Observability backend. The shelf we want does not exist yet:
*instrumentation for software that repairs itself*. We sit on the "observability" shelf for
search and citation, and we argue for the new shelf in the copy.

**Product type:** Open-source self-hosted binary + hosted API (invited beta).

**Business model:** Self-hosting is free forever, no billing path at all — storage is the
user's own bucket. Hosted is usage-based for people who don't want to run anything.
Reference point: ~$14/mo hosted at a workload where Datadog Pro is ~$680/mo.

## Target Audience

**Target companies:** 1–10 engineers. Early SaaS, indie products, agencies, AI-native teams.
Anyone whose observability bill would be a noticeable fraction of their infra bill. Explicitly
including one-person teams and people who would not call themselves ops.

**Decision-makers:** The developer. There is no procurement, no security review, no committee —
the person who feels the pain also installs the thing, on the same afternoon.

**Primary use case:** Something is wrong in production and the agent that can fix it has no
way to find out what, or to confirm the fix worked.

**Jobs to be done:**
- Tell me when production changed, without me configuring what "changed" means.
- Hand my coding agent evidence it can act on, not a chart it has to interpret.
- Do not turn me into an observability administrator.

**Use cases:**
- Coding agent in a loop: detect → diagnose → patch → confirm recovery.
- 3am incident where the on-call "person" is a scheduled agent.
- LLM app debugging: which model call stalled the request, in the same trace as the request.
- Cost escape: replacing a Datadog or Grafana Cloud bill that outgrew the product.

## Personas

| Persona | Cares about | Challenge | Value we promise |
|---------|-------------|-----------|------------------|
| Solo founder / indie dev | Price, and not spending a weekend on setup | Can't justify $500/mo for three services; ships alone | Free, five minutes, nothing to maintain |
| AI-native builder | Their agent being able to close its own loop | Agent writes code blind; no way to feed it production truth | An API an agent reads by itself, with proof on every row |
| Small platform team | Getting hours back | Hours a week maintaining dashboards nobody reads | No thresholds, no panels, no dashboard rot |
| Privacy-conscious builder | Telemetry not leaving their infrastructure | Vendors want customer data shipped to a third party | Self-hosted; your bucket; open format; leaving costs nothing |

## Problems & Pain Points

**Core problem:** The tools that watch production were built for a person looking at a screen.
The thing that now fixes production is an agent, and it cannot read a screen. So the loop
breaks upstream of the fix: the agent can write the patch but cannot find out what broke, and
cannot tell whether its patch worked.

**Why alternatives fall short:**
- Priced for companies with a procurement department, not for a product with three services.
- Setup measured in weeks, and the setup never really ends — thresholds, rules, panels, retention.
- Optimized for a 50ms chart refresh, not for a coherent answer to one question.
- The AI features summarize a dashboard you still had to pick, rather than answering a question.
- Your telemetry lives in their store, in their format, and leaving is a project.

**What it costs them:** Money that should have gone to the product. Hours a week of maintenance
that produces nothing shippable. And incidents that stay open longer than they need to because
the fastest thing in the loop — an agent that could patch in seconds — is waiting on a human to
go and look.

**Emotional tension:** Not wanting to become the monitoring person. Quiet dread about the bill.
The specific helplessness of knowing something is wrong and having to guess where.

## Competitive Landscape

**Direct:** Datadog, Grafana Cloud, Honeycomb, Axiom — same job, priced and shaped for a human
reader and a bigger company. Their AI layer sits on top of a dashboard rather than replacing it.

**Secondary:** `console.log` plus Sentry; platform-native logs (Vercel, Railway, Fly). Free and
already there, but they answer "what does this one request look like" and never "what changed."

**Indirect:** Doing nothing and reading logs by hand when a customer complains. This is the real
incumbent for our audience, and the honest competitor to beat.

## Differentiation

**Key differentiators — the five things users actually care about:**

1. **It does what it says.** Every finding ships with the query that produced it. You can run it,
   widen it, disprove it. A claim you can check is a different kind of claim.
2. **Nothing to maintain.** No thresholds, no alert rules, no panels. Each service is judged
   against its own history, so "normal" is a property of the data rather than something you tune.
3. **Setup is five minutes.** One `docker run`, two environment variables, no code changes, no
   agent to install. Whatever OpenTelemetry you already emit works unchanged.
4. **It's cheap, and free if you run it.** No per-host fee, no per-seat fee, no ingest bill.
   Self-hosting costs the bucket you already pay for.
5. **It's private.** You run it, so your telemetry never leaves your infrastructure. It doesn't
   phone home. Hosted exists for people who would rather not run anything, and it is described
   as exactly that — the convenience option, not the default story.

**Cost claims — publish ours only.** Decided 2026-08-01: no competitor named, no multiplier, no
comparison table. "Free if you run it. ~$14/mo if we do." The reader does their own arithmetic
against a bill they already know, which is more convincing than a number we picked and safer
than comparative advertising. Internal cost work stays in `docs/notes/company/cost.md`.

**Privacy story — self-hosted is the default.** Decided 2026-08-01. Lead with "runs in your
infrastructure, writes to your storage, never phones home." Do not lead with hosted, and never
imply hosted has the same property. If a reader goes looking for the seam, they should find an
honest sentence, not a dodge.

**How we do it differently:** One process, one query engine, no pre-aggregation. Questions are
plain SQL, so there is no query language between a user and their own telemetry — and an agent
already writes SQL fluently. The API describes itself at `/openapi.json` with no key, so an
agent integrates without anyone hand-writing a tool definition.

**Why that's better:** The whole loop closes without a person in the middle of it.

**Why customers choose us:** Because it is free, it took five minutes, and their agent could use
it without being taught how.

## Objections

| Objection | Response |
|-----------|----------|
| "Another thing to self-host." | One container, one port, no database to provision. If it creates work, we failed. |
| "I already pay for Datadog." | Keep it for the humans. This is for everything downstream of a person looking. |
| "No dashboards? I want to see it." | Fair — we're the layer under a dashboard, not a replacement for wanting one. Bring your own. |
| "Can I trust an automated diagnosis?" | You don't have to. Every finding hands you the query behind it; check it yourself. |
| "Is a one-person open-source project safe to depend on?" | Your data is in your bucket in an open format. If we disappear, your telemetry doesn't. |

**Anti-persona:** Large enterprise with a compliance-driven monitoring mandate, an SRE team that
wants a chart wall, or anyone who needs full-text log search or sub-second live tail. We don't
do those, deliberately.

## Switching Dynamics

**Push:** A bill that grew faster than the product. Hours lost to dashboard upkeep. Having a
coding agent that can fix things and no way to tell it what's wrong.

**Pull:** Free. Five minutes. An answer instead of a panel. An agent that closes its own loop.

**Habit:** The dashboard is already open in a tab, and someone already built it. Muscle memory
for looking rather than asking.

**Anxiety:** Trusting a diagnosis they didn't derive. Self-hosting one more thing. Depending on
a young project. All three are answered by the same fact: the proof and the data are both theirs.

## Customer Language

**How they describe the problem:**
- "I don't know what broke, I just know it's broken."
- "I can't justify $500 a month for three services."
- "I don't want to become the monitoring guy."
- "My agent wrote the fix but it was guessing."

**How they describe us:**
- "I asked it what broke and it told me."
- "It was running in five minutes."

**Words to use:** what broke, prove it, check it yourself, one call, your bucket, free, five
minutes, nothing to maintain, fixes itself, closes the loop, plain SQL.

**Words to avoid:**
- Architecture nouns users don't care about: Parquet, DuckDB, S3, R2, eBPF, DuckLake.
- Enterprise filler: platform, unified, seamless, leverage, robust, empower, end-to-end.
- Our own jargon that reads as mechanical: "presentation layer," "data layer underneath,"
  "ranked findings" as a standalone phrase, "surface" as a verb, "affordance."
- Stacked em-dash clauses. The current site copy leans on them until every sentence sounds
  like the same sentence.

**Glossary:**
| Term | Meaning |
|------|---------|
| Software factory | An agent loop that writes, tests, ships and repairs code with a person supervising rather than typing. |
| The five stages | Detect, diagnose, fix, verify, learn. sigiro is detect + diagnose, plus the evidence verify needs. |
| Investigate | One API call that returns a whole diagnosis instead of a row of numbers. |
| Baseline | A service's own recent history, which is what "abnormal" is measured against. |

## Brand Voice

**Register — ASD-STE100 Simplified Technical English, second person.** Decided 2026-08-01. This
replaces the earlier register, which asked for varied sentence rhythm and one dry aside per
page. All docs and all site copy follow ASD-STE100. Ten rules:

1. **Write in the active voice.** The reader or sigiro does the action, and the sentence says
   which one.
2. **One instruction or one idea per sentence.** Keep a procedural sentence to 20 words. Keep a
   descriptive sentence to 25 words.
3. **No -ing forms.** Write "sample rates", not "sampling". Write "agent", not "coding agent".
   The only exception is a technical name that contains an -ing form.
4. **No contractions.** Write "do not", not "don't".
5. **No metaphor, no idiom, no figurative language.** Write "sends no data to us", not "phones
   home". Write "caused the failure", not "looks guilty". Write "connects to", not "plugs
   into".
6. **Use the articles "the" and "a"** wherever grammar allows them.
7. **One word for one thing.** Use the same word for the same thing every time. Never change a
   word for style.
8. **Simple present tense** where possible. Avoid the perfect tenses.
9. **Six sentences per paragraph, at most.**
10. **Say "you."** The reader stays in the sentence. Prose with nobody in it reads as
    machine-generated, however accurate it is.

**No exceptions to rule 3.** "self-improving software" was tried as one and dropped: the phrase
appeared on the landing page in one form and in the docs in another, and a split brand frame
costs more than the grammar saved. Use "software that repairs itself". Do not invent new
exceptions either — the value of a controlled language is that it is controlled.

**Style:** Short sentences. Concrete nouns. Say the useful thing first. The copy reads like a
good README, not like a press release. No jokes, and no snark at a competitor: the argument is
strong enough without a sneer.

**Personality:** Honest, unbothered, precise, generous, quietly opinionated. STE removes the
rhythm tricks, so the precision has to carry the personality.

**Why STE:** two audiences read the same words. A solo developer reads under time pressure
during an incident, and an agent parses the page as input. STE serves both. It also fixes the
older failure directly, because the old copy was *mechanical*: every sentence built as
`claim — em-dash — refinement`, abstractions ("layer," "surface," "affordance") where a plain
verb does the job, and no first or second person anywhere.

**What to fix in older copy:** delete every contraction. Replace every -ing form. Split every
sentence that carries two ideas. Replace every metaphor with the literal fact. Break the
`claim — em-dash — refinement` pattern into two sentences. Leave code blocks, commands, SQL and
JSON byte-identical, because STE governs prose only.

## Proof Points

**Metrics:**
- Setup: one `docker run` plus two env vars, under five minutes.
- Cost, as published: free self-hosted; ~$14/mo hosted. No competitor named, no multiplier.
- Investigation: one call, ~7s, replacing what used to be 13 round trips and ~40s.
- Payload: a full diagnosis is tens of KB of structured evidence, not a raw span dump.

**Customers:** None public yet. We dogfood: linguabench/worldpol runs in production against
hosted sigiro, which is where the honest stories come from.

**Testimonials:** None yet. Do not invent any.

**Value themes:**
| Theme | Proof |
|-------|-------|
| It does what it says | Every finding carries a runnable query; `/openapi.json` is public |
| Nothing to maintain | No thresholds or rules exist to configure; baselines are statistical |
| Easy setup | Standard OTLP on standard ports; nothing in your code changes |
| Cheap | Free self-hosted; ~$14/mo hosted |
| Private | Runs in your infra, writes to your storage, never phones home |
| It watches itself | A stalled writer, a leaked connection and a dropped channel were each caught by sigiro watching sigiro |

## Goals

**Business goal:** Become the default instrumentation for agent-driven development loops before
the incumbents notice the reader changed.

**Conversion action:** Run the container. Secondary: read the docs. Tertiary: join hosted beta.

**Current metrics:** Pre-launch. No public traffic baseline yet.

## Changelog
*Newest first. One line per revision: what changed and why.*
- v4 (2026-08-01) — Frame moved from "self-improving software" to "software that repairs itself", and the -ing exception was withdrawn. The landing page had already shipped the new phrasing under STE rule 3 while the docs and this file kept the old one, so the brand frame read differently in two places. The claim did not change; the grammar did.
- v3 (2026-08-01) — Brand Voice replaced: the register is now ASD-STE100 Simplified Technical English in the second person. The old "vary the rhythm, one dry aside per page" advice conflicted with STE and is gone. Reason: an agent parses the same page a developer reads under incident pressure, and STE serves both. `www/content/docs/*` rewritten to match.
- v2 (2026-08-01) — Locked four decisions that drive all copy: frame = "self-improving software"; privacy story leads self-hosted (hosted is the convenience option, never claimed equivalent); voice = second person, dry, no jokes, because the absence of "you" is what made the old copy mechanical; cost claims publish ours only with no competitor named.
- v1 (2026-08-01) — Initial context. Drafted from repo (`docs/notes/company/*`, `www/src/lib/site.ts`) against shipped reality, correcting stale eBPF/DaemonSet/B2/profiles claims in `value-proposition.md` and `user-personas.md`. Positioning centred on self-improving software; five user-facing promises (works, no maintenance, easy, cheap, private) made the differentiation spine.
