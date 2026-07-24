/**
 * One source of truth for everything the site says about itself.
 *
 * Metadata, JSON-LD, the sitemap, /llms.txt and the visible copy all read from
 * here — so the structured data can never drift from what a human sees, which
 * is exactly what search and answer engines check for.
 */

export const SITE = {
  name: 'sigiro',
  url: 'https://sigiro.com',
  title: 'sigiro — the detect and diagnose layer for software factory agents',
  tagline: 'The detect and diagnose layer for self-operating software',
  description:
    'sigiro is observability for agents, not dashboards. OpenTelemetry in; ranked anomalies and one structured evidence block out, each row carrying the SQL to go deeper. The detect and diagnose half of a detect → diagnose → fix → verify → learn loop.',
  install: 'docker run -p 4317:4317 -p 4318:4318 -p 9999:9999 ghcr.io/sigiroai/sigiro',
} as const;

/** The thesis: five steps, and who owns each one. */
export const LOOP = [
  {
    step: 'Detect',
    owner: 'sigiro',
    ours: true,
    body: 'Something changed and nobody was watching. A loop needs a signal it can poll — not an alert rule somebody has to write first. `sigiro anomalies` returns regime shifts found in each service’s own history, ranked, grouped into incidents across signals.',
  },
  {
    step: 'Diagnose',
    owner: 'sigiro',
    ours: true,
    body: 'Knowing a thing broke is not knowing why. `POST /v1/investigate` returns one structured evidence block per service — ranked findings, failure modes by error type, the slow and failing operations, log patterns, sampled spans — with a runnable SQL query on every row.',
  },
  {
    step: 'Fix',
    owner: 'your coding agent',
    ours: false,
    body: 'Claude Code, Codex, or whatever writes your patches. It is already good at this — as long as something hands it evidence instead of a dashboard screenshot.',
  },
  {
    step: 'Verify',
    owner: 'your pipeline, plus sigiro',
    ours: false,
    body: 'A fix is a hypothesis until the signal recovers. Deploys show up as derived events in the same timeline as the shift, so “did it actually get better after that deploy” is a query, not a vibe.',
  },
  {
    step: 'Learn',
    owner: 'your agent’s memory',
    ours: false,
    body: 'The same incident twice is a process bug. Whatever you use for agent memory — playbooks, a notes directory, a skill — is what turns one resolved incident into the next one being resolved faster.',
  },
] as const;

/**
 * Visible FAQ *and* FAQPage JSON-LD. Every answer here is rendered in the HTML;
 * schema whose text does not appear on the page is the fastest way to get
 * structured data ignored.
 */
export const FAQ = [
  {
    q: 'What is a software factory?',
    a: 'A software factory is a development loop where agents, not people, carry the work from symptom to shipped fix: detect what changed, diagnose why, fix it, verify the fix, and learn from it. Each step has to be automated and connected to the next. Automate four of the five and you still have a human in the loop, which means you do not have a factory — you have a faster way to page someone.',
  },
  {
    q: 'What does sigiro do?',
    a: 'sigiro covers the first two steps: detect and diagnose. It ingests OpenTelemetry over OTLP, finds shifts against each service’s own baseline, and returns one structured evidence block per service — ranked findings, error breakdowns, slow operations, log patterns and sampled spans — with a ready-to-run SQL query attached to every row. It does not write your patches and it does not run your pipeline.',
  },
  {
    q: 'How does an AI agent debug a production incident?',
    a: 'It needs evidence in one call, not a dashboard to look at. An agent asks sigiro for the current anomalies, picks the incident with the largest shift, calls investigate on that service and window, reads the ranked findings, and runs the SQL that came attached to whichever row looked suspicious. Every one of those steps is an HTTP request returning structured data, so the agent can act on it without a human interpreting a chart.',
  },
  {
    q: 'Is sigiro OpenTelemetry compatible?',
    a: 'Yes. sigiro speaks OTLP natively on port 4317 (gRPC) and 4318 (HTTP) for traces, logs, metrics and profiles, and also accepts Pyroscope-format profiles. Any OpenTelemetry SDK or collector you already run works unchanged — point OTEL_EXPORTER_OTLP_ENDPOINT at sigiro and you are done.',
  },
  {
    q: 'Do I still need an OpenTelemetry Collector?',
    a: 'Not for a handful of services — SDKs export straight to sigiro, which is the collector. For a fleet, keep a collector gateway in front: it batches, scrubs, retries, and holds your key as one secret, then forwards OTLP onward. sigiro does not care which of the two you choose.',
  },
  {
    q: 'How is sigiro different from a traditional observability platform?',
    a: 'Those products are excellent human interfaces: dashboards, charts, and a query language you learn. Their agent story is a dashboard with an API bolted on. sigiro inverts it — the primary interface is an endpoint that returns ranked findings plus the SQL to go deeper, and the storage underneath is DuckDB and Parquet you can query directly. If a person is the one reading, use a dashboard. If an agent is reading, the shape of the answer matters more than the shape of the chart.',
  },
  {
    q: 'How does sigiro detect anomalies without me configuring thresholds?',
    a: 'There are no thresholds to configure. sigiro runs Bayesian changepoint detection over each service’s own recent series — error rate, p95 latency, log volume, error log rate and profile cost — and records only shifts that made things worse. A service that is chronically slow but stable produces no anomaly, because nothing changed. Shifts landing in the same or adjacent detection bucket are grouped under one incident instead of arriving as five separate pages.',
  },
  {
    q: 'Can sigiro tell me which deploy caused an incident?',
    a: 'It names the suspect. Deploys are derived from your telemetry as events on the same timeline as the shift, so when several signals move together sigiro groups them into one incident and attaches the deploy that lines up. That is a correlation, not a confession — but it is the correlation you would have gone looking for by hand.',
  },
  {
    q: 'What query language does sigiro use?',
    a: 'SQL. Your telemetry is DuckDB tables backed by Parquet, so there is no DSL to learn and no proprietary query layer between you and your data. Every finding sigiro returns carries the exact SQL that produced it, which means an agent can widen, narrow, or pivot any answer by editing a query it was handed.',
  },
  {
    q: 'Can I self-host sigiro?',
    a: 'Yes — that is the default. One container, no sidecars and no external dependencies, storing data as Parquet on local disk or S3-compatible object storage. A self-hosted server runs in open mode with no per-request auth, so put it on a private network or behind a proxy. A hosted multi-tenant service exists as an invited beta if you would rather not run anything.',
  },
] as const;

export const NAV = [
  { href: '/docs', label: 'Docs' },
  { href: '/docs/quickstart', label: 'Quickstart' },
] as const;

export const SOCIAL = [
  { href: 'https://x.com/SigiroAI', label: 'X' },
  { href: 'https://www.linkedin.com/company/sigiro', label: 'LinkedIn' },
] as const;
