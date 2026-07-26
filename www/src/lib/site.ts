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

/** The thesis: five steps, and who owns each one. One line each. */
export const LOOP = [
  { step: 'Detect', owner: 'sigiro', ours: true, body: 'What changed, without an alert rule.' },
  { step: 'Diagnose', owner: 'sigiro', ours: true, body: 'Why, as one block of evidence.' },
  { step: 'Fix', owner: 'your coding agent', ours: false, body: 'Claude Code, Codex, whatever writes your patches.' },
  { step: 'Verify', owner: 'your pipeline', ours: false, body: 'Did the signal recover after the deploy.' },
  { step: 'Learn', owner: 'your agent’s memory', ours: false, body: 'So the same incident is faster next time.' },
] as const;

/**
 * Visible FAQ *and* FAQPage JSON-LD. Every answer here is rendered in the HTML;
 * schema whose text does not appear on the page is the fastest way to get
 * structured data ignored.
 */
export const FAQ = [
  {
    q: 'What is a software factory?',
    a: 'A loop where agents carry work from symptom to shipped fix: detect what changed, diagnose why, fix it, verify the fix, learn from it. Automate four of the five and a human is still in the loop — that is a faster way to page someone, not a factory.',
  },
  {
    q: 'What does sigiro do?',
    a: 'Detect and diagnose. It ingests OpenTelemetry, finds shifts against each service’s own baseline, and returns one structured evidence block per service with runnable SQL on every row. It does not write your patches and does not run your pipeline.',
  },
  {
    q: 'How does an AI agent debug a production incident?',
    a: 'It asks for the current anomalies, picks the largest shift, calls investigate on that service and window, then runs the SQL that came attached to the suspicious row. Every step is an HTTP request returning structured data, so no human has to read a chart.',
  },
  {
    q: 'Is sigiro OpenTelemetry compatible?',
    a: 'Yes — OTLP on 4317 (gRPC) and 4318 (HTTP) for traces, logs, metrics and profiles, plus Pyroscope-format profiles. Point OTEL_EXPORTER_OTLP_ENDPOINT at sigiro and any existing SDK or collector works unchanged.',
  },
  {
    q: 'Do I still need an OpenTelemetry Collector?',
    a: 'Not for a few services — SDKs export straight to sigiro, which is the collector. For a fleet, keep a collector gateway in front to batch, scrub, retry, and hold your key as one secret.',
  },
  {
    q: 'How is sigiro different from a traditional observability platform?',
    a: 'They are excellent human interfaces with an API bolted on. sigiro inverts it: the primary interface returns ranked findings plus the SQL behind them, over DuckDB and Parquet you can query directly. If a person is reading, keep your dashboard.',
  },
  {
    q: 'How does sigiro detect anomalies without thresholds?',
    a: 'Bayesian changepoint detection over each service’s own recent series — error rate, p95 latency, log volume, error log rate, profile cost — keeping only shifts that made things worse. Chronically slow but stable is not an anomaly, because nothing changed.',
  },
  {
    q: 'Can sigiro tell me which deploy caused an incident?',
    a: 'It names the suspect. Deploys are derived from your telemetry onto the same timeline as the shift, so correlated shifts arrive as one incident with the deploy that lines up attached. A correlation, not a confession.',
  },
  {
    q: 'What query language does sigiro use?',
    a: 'SQL. Your telemetry is DuckDB tables backed by Parquet, and every finding carries the exact query that produced it, so an agent can widen or pivot any answer by editing SQL it was handed.',
  },
  {
    q: 'Can I self-host sigiro?',
    a: 'Yes, that is the default: one container, no sidecars, Parquet on local disk or S3. Self-hosted runs in open mode with no per-request auth, so keep it on a private network. Hosted is an invited beta.',
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
