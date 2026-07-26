/**
 * One source of truth for everything the site says about itself.
 *
 * Copy rule, from docs/notes/company/taglines.md: never mention Parquet, DuckDB,
 * S3 or eBPF in customer-facing text. Users care about outcomes; the storage
 * engine is an implementation detail. The hero line is the one that file already
 * chose. The thesis is docs/notes/company/pitch.md: the consumer of observability
 * data changed, and it is no longer a person staring at a dashboard.
 */

export const SITE = {
  name: 'sigiro',
  url: 'https://sigiro.com',
  /** Keeps "observability" in the title: the h1 is a category bet with no search
      intent behind it, and this is the one place the category word still earns
      its keep for search and for citation. */
  title: 'sigiro — observability data layer for self-improving software',
  /** The h1: the category claim. */
  tagline: 'The data layer for self-improving software and agents.',
  description:
    'Dashboards are a presentation layer. sigiro is the data layer underneath: OpenTelemetry in, ranked findings out, each one carrying the query that proves it. Built for agents reading at 3am, not people reading charts.',
} as const;

export const INSTALL = [
  {
    id: 'self-host',
    label: 'Self-host',
    code: 'docker run -p 4317:4317 -p 4318:4318 -p 9999:9999 ghcr.io/sigiroai/sigiro',
  },
  {
    id: 'point-at-it',
    label: 'Point your app at it',
    code: 'OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 OTEL_SERVICE_NAME=checkout npm start',
  },
] as const;

/** The 01–05 spine: one capability, one plain sentence, one artifact. */
export const SECTIONS = [
  {
    n: '01',
    lead: 'Connect.',
    rest: 'Point OpenTelemetry at it and nothing else changes.',
    say: 'Standard OTLP on the standard ports. Whatever SDK or collector you already run keeps working — no agent to install, no code to touch, no migration.',
    artifact: {
      title: 'your shell',
      meta: 'under five minutes, two variables',
      lang: 'bash' as const,
      code: `docker run -p 4317:4317 -p 4318:4318 -p 9999:9999 ghcr.io/sigiroai/sigiro

OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \\
OTEL_SERVICE_NAME=checkout npm start`,
    },
  },
  {
    n: '02',
    lead: 'Ask.',
    rest: 'One question, one answer, already ranked.',
    say: 'Not a panel to interpret and not a firehose to wade through. What moved together, which deploy is the suspect, and how the thing actually fails — in one response something else can act on.',
    artifact: {
      title: 'POST /v1/investigate',
      meta: '{"service":"checkout"}',
      lang: 'json' as const,
      code: `{
  "summary": "two signals moved together on checkout at 18:35",
  "suspect": "deploy 9f2c1ab, landed 18:33",
  "failing": "POST /api/orders",
  "how": { "timeout": 246, "server_error": 6 },
  "errors_total": 1284,
  "query": "SELECT * FROM spans WHERE service = 'checkout' AND ..."
}`,
    },
  },
  {
    n: '03',
    lead: 'Correlate.',
    rest: 'Your services and your agents in the same question.',
    say: 'An agent is a service that calls a model instead of a database. Its reasoning and your request path land in the same place, joined by the same trace — so “which model call stalled the checkout” is one query rather than two products.',
    artifact: {
      title: 'POST /v1/query',
      meta: 'no query language to learn',
      lang: 'sql' as const,
      code: `SELECT model, count(*) AS calls, max(duration) AS slowest
FROM spans JOIN logs USING (trace_id)
WHERE service = 'checkout' AND status = 'error'
GROUP BY model ORDER BY slowest DESC`,
    },
  },
  {
    n: '04',
    lead: 'Prove.',
    rest: 'Every answer carries the query behind it.',
    say: 'Nothing has to be taken on faith. The agent widens, narrows or pivots any finding by editing a query it was handed, never by guessing at a shape it has not seen.',
    artifact: null,
  },
  {
    n: '05',
    lead: 'Discover.',
    rest: 'Your agent learns the whole surface by reading it.',
    say: 'No tool description to hand-write and no onboarding for a machine. One unauthenticated document describes every endpoint, and that is the integration.',
    artifact: {
      title: 'GET /openapi.json',
      meta: 'public, no key',
      lang: 'json' as const,
      code: `[
  "/v1/investigate",
  "/v1/query",
  "/v1/services",
  "/v1/traces/{trace_id}",
  "/v1/health"
]`,
    },
  },
] as const;

/** The three promises under 04 — each one a thing we deliberately do not do. */
export const PROMISES = [
  {
    title: 'Nothing to configure',
    body: 'No thresholds, no rules, no panels to keep in sync. Each service is judged against its own history, which is a property of the data rather than a thing you set up.',
  },
  {
    title: 'Nothing sampled away',
    body: 'The request you need is kept, not one in a hundred. Sampling was a bargain struck because nobody was ever going to read them all — something will.',
  },
  {
    title: 'Nothing locked in',
    body: 'Your telemetry stays in storage you already own, in a format anything can read. Leaving costs you nothing, so staying is a choice.',
  },
] as const;

/**
 * Three honest groups: what emits the telemetry, where sigiro runs, and what
 * reads the answers. Icon names are simple-icons slugs from
 * @icons-pack/react-simple-icons — `icon: null` means the brand has no mark in
 * that set, so the tile is the label alone rather than a wrong logo.
 */
export const INTEGRATIONS = [
  {
    label: 'Send telemetry from',
    items: [
      { name: 'OpenTelemetry', icon: 'SiOpentelemetry' },
      { name: 'Node.js', icon: 'SiNodedotjs' },
      { name: 'Python', icon: 'SiPython' },
      { name: 'Go', icon: 'SiGo' },
      { name: 'Java', icon: 'SiOpenjdk' },
      { name: '.NET', icon: 'SiDotnet' },
      { name: 'Ruby', icon: 'SiRuby' },
      { name: 'Rust', icon: 'SiRust' },
    ],
  },
  {
    label: 'Run it on',
    items: [
      { name: 'Docker', icon: 'SiDocker' },
      { name: 'Kubernetes', icon: 'SiKubernetes' },
      { name: 'Fly.io', icon: 'SiFlydotio' },
      { name: 'Railway', icon: 'SiRailway' },
      { name: 'Render', icon: 'SiRender' },
      { name: 'Cloudflare', icon: 'SiCloudflare' },
    ],
  },
  {
    label: 'Read by',
    items: [
      { name: 'Claude Code', icon: 'SiClaude' },
      { name: 'Cursor', icon: 'SiCursor' },
      { name: 'Codex', icon: null },
      { name: 'Anything that speaks SQL', icon: null },
    ],
  },
] as const;

/**
 * Visible FAQ *and* FAQPage JSON-LD. Every answer here is rendered in the HTML;
 * schema whose text does not appear on the page gets ignored.
 */
export const FAQ = [
  {
    q: 'What is sigiro, in one sentence?',
    a: 'An observability backend whose primary reader is an agent: send it OpenTelemetry, ask in SQL or in one investigate call, and get back a diagnosis rather than a dashboard.',
  },
  {
    q: 'Does it replace my current observability stack?',
    a: 'It replaces what you were using them for when the reader is a machine. If a person needs to look at a chart, keep the dashboard — this is for everything downstream of that.',
  },
  {
    q: 'Do I have to change my code?',
    a: 'No. It speaks standard OTLP on ports 4317 and 4318, so any OpenTelemetry SDK or collector you already run points at it unchanged.',
  },
  {
    q: 'Does it page me?',
    a: 'No, and that is deliberate. Paging exists to interrupt someone who cannot watch continuously. What sigiro returns is a table something else reads on a loop, so the interruption is the part we deleted.',
  },
  {
    q: 'How does it know what is worth reporting without thresholds?',
    a: 'Each service is compared against its own recent history rather than against a number you picked. A service that is chronically slow but steady reports nothing, because nothing changed — and a service that quietly doubled reports even though no rule existed for it.',
  },
  {
    q: 'What query language do I need to learn?',
    a: 'None. Questions are plain SQL, which your agent already writes fluently. There is no proprietary query language between you and your telemetry, and every answer hands you the query that produced it.',
  },
  {
    q: 'Can my coding agent use it without me?',
    a: 'Yes — that is the design. It reads the API description itself at /openapi.json, with no key and no hand-written tool definition, then queries in SQL it already knows.',
  },
  {
    q: 'What does it cost?',
    a: 'Self-hosting is free and the storage is yours, so there is no per-host fee, no per-seat fee and no ingest bill. A hosted service exists as an invited beta if you would rather not run anything.',
  },
  {
    q: 'What does it not do?',
    a: 'It does not write your patches, run your pipeline, or evaluate alert rules. It also has no dashboard builder and no full-text log index — questions are structured, which is how an agent asks them.',
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
