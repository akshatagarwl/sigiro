/**
 * One source of truth for everything the site says about itself.
 *
 * Positioning, voice and the five claims live in .agents/product-marketing.md —
 * read that before editing copy here. Three rules from it that bite hardest:
 *
 * 1. Say "you". Prose with nobody in it reads as machine-generated no matter how
 *    accurate it is, and that was the diagnosis on the previous draft of this file.
 * 2. Vary the rhythm. The old copy hinged eleven sentences on an em-dash, which is
 *    what made every line sound like the same line. Budget for the page is two.
 * 3. Never mention Parquet, DuckDB, S3 or eBPF. Users care about outcomes; the
 *    storage engine is an implementation detail.
 *
 * Cost claims name no competitor and quote no multiplier: "free if you run it,
 * ~$14/mo if we do". The reader does their own arithmetic against a bill they
 * already know. Internal cost work stays in docs/notes/company/cost.md.
 */

export const SITE = {
  name: 'sigiro',
  url: 'https://sigiro.com',
  /** Where the hosted API answers. The reference builds its curl examples against
      this, so a reader can paste one and have it reach something real. */
  apiUrl: 'https://api.sigiro.com',
  /** Keeps "observability" in the title: the h1 is a category bet with no search
      intent behind it, and this is the one place the category word still earns
      its keep for search and for citation. */
  title: 'sigiro — observability data layer for self-improving software',
  /** The h1: the category claim. */
  tagline: 'The data layer for self-improving software and agents.',
  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters or search results cut it mid-sentence. */
  description:
    'Point OpenTelemetry at it. When something breaks, you get an answer with the query that proves it, not a chart to read.',
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
    say: 'Standard OTLP, standard ports. Nothing in your app changes.',
    artifact: {
      title: 'your shell',
      meta: 'two variables, five minutes',
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
    say: 'What moved together, which deploy looks guilty, how it fails.',
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
    rest: 'Your services and your agents in one question.',
    say: 'An agent is a service that calls a model. Same trace, one query.',
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
    say: 'Run the query it came with. Widen it. Try to disprove it.',
    artifact: {
      title: 'POST /v1/investigate',
      meta: 'one finding, abridged',
      lang: 'json' as const,
      code: `{
  "operation": "POST /api/orders",
  "errors": 246,
  "error_rate": 0.19,
  "drill_down_sql": "SELECT trace_id, duration, status_message FROM sigiro_spans WHERE service_name = 'checkout' AND span_name = 'POST /api/orders' AND status_code = 2 ORDER BY duration DESC LIMIT 3"
}`,
    },
  },
  {
    n: '05',
    lead: 'Discover.',
    rest: 'Your agent learns the whole surface by reading it.',
    say: 'One public document describes every endpoint. That’s the integration.',
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

/**
 * Stages two and three of the 04 Prove demo. Stage one is that section's own
 * `artifact` above — the finding — so the finding lives with every other artifact
 * on the page rather than being duplicated here.
 *
 * `sql` must stay character-identical to the finding's `drill_down_sql`, modulo
 * line breaks. The entire claim is that the query you are handed is the query you
 * run; if these two drift, the demo quietly disproves the section it illustrates.
 *
 * `rows` are the timeouts that section 02 already reported (`"timeout": 246`), so
 * a reader scrolling down meets the same incident twice and the numbers line up.
 */
export const PROOF = {
  sql: `SELECT trace_id, duration, status_message
FROM sigiro_spans
WHERE service_name = 'checkout'
  AND span_name = 'POST /api/orders'
  AND status_code = 2
ORDER BY duration DESC
LIMIT 3`,
  rows: [
    `{ "trace_id": "9f2c1ab4e77d", "duration": 30012, "status_message": "context deadline exceeded" }`,
    `{ "trace_id": "3b81fe0a2c56", "duration": 30004, "status_message": "context deadline exceeded" }`,
    `{ "trace_id": "c40d9e73aa18", "duration": 29997, "status_message": "context deadline exceeded" }`,
  ],
} as const;

/**
 * The factory loop, and which stage belongs to whom.
 *
 * The positioning in one object: a software factory is not a factory until every
 * stage is covered, and sigiro covers two of them plus the evidence for a third.
 * Saying which stages are *not* ours is the point. A claim to the whole loop is
 * the claim every dashboard vendor already makes, and it is why an agent driving
 * one gets a screenshot instead of an answer.
 *
 * `owner` is rendered verbatim under each stage, so it has to name a real thing a
 * reader already runs.
 *
 * `cmd` and `out` are one continuous terminal session, read top to bottom: the
 * same checkout incident that sections 02 and 04 already report, carried from
 * detection to recovery. Stages three and five deliberately run something that
 * is not sigiro, so "we are two of these five" is a thing you watch rather than
 * a thing you are told.
 *
 * The register here is the CLI, not HTTP. Every operation has both surfaces
 * (`sigiro anomalies` and `GET /v1/anomalies` are the same call) and this section
 * used to name the HTTP one, on the grounds that its reader is an agent. A
 * terminal session settles it the other way: you cannot show a shell and label it
 * with a verb-and-path. The endpoints are named in 01–05 and in the reference —
 * still do not mix the two within one page.
 *
 * Numbers were dropped from these stages when the 01–05 spine below kept its own.
 * Two `01–05` sequences ran back to back with identical numerals, and a reader
 * going "…05 Learn" → "01 Connect" had no way to see a new sequence had started.
 */
export const LOOP = [
  {
    title: 'Detect',
    owner: 'sigiro',
    ours: true,
    say: 'Measured against the service’s own history, not a threshold you set.',
    cmd: 'sigiro anomalies',
    out: `SERVICE   SIGNAL       BASELINE   NOW      SHIFT    INCIDENT
checkout  error_rate   0.4%       19.2%    +48σ     inc_4a1c
checkout  latency_p95  180ms      30.0s    +166×    inc_4a1c

2 shifts · 1 incident · since 18:35`,
  },
  {
    title: 'Diagnose',
    owner: 'sigiro',
    ours: true,
    say: 'Ranked findings, the suspect deploy, and the query proving each one.',
    cmd: 'sigiro investigate checkout',
    out: `suspect        deploy 9f2c1ab, landed 18:33
failing        POST /api/orders
failure mode   timeout 246 · server_error 6
errors_total   1284
drill_down_sql SELECT trace_id, duration, status_message
               FROM sigiro_spans WHERE service_name = 'checkout' …`,
  },
  {
    title: 'Fix',
    owner: 'your coding agent',
    ours: false,
    say: 'Whichever agent already has commit rights. This stage was solved first.',
    cmd: 'claude "checkout times out since 9f2c1ab"',
    out: `read   src/orders/upstream.ts
edit   src/orders/upstream.ts   +4 −1
test   42 passed
push   fix: bound the orders upstream call at 5s`,
  },
  {
    title: 'Verify',
    owner: 'your pipeline, on our evidence',
    ours: 'partly',
    say: 'Tests passing is not the incident closing. The baseline says which.',
    cmd: 'sigiro anomalies --service checkout',
    out: `no open shifts

error_rate    19.2%  →  0.5%    recovered 19:07
latency_p95   30.0s  →  210ms   recovered 19:07

inc_4a1c closed · 34 min open`,
  },
  {
    title: 'Learn',
    owner: 'your memory',
    ours: false,
    say: 'The fix, kept beside the evidence, for the next time this shape appears.',
    cmd: 'cat playbooks/checkout-upstream-timeout.md',
    out: `## checkout · upstream timeout

detected   error_rate +48σ, latency_p95 +166×
cause      unbounded upstream call, shipped in 9f2c1ab
fix        5s deadline on the orders client
watch      the same shape on any service calling orders`,
  },
] as const;

/**
 * The five things users actually care about, plus the one that proves we mean it.
 *
 * Order is deliberate: proof first (it is the differentiator), then the three
 * cheap-easy-quiet promises, then privacy, then dogfooding as evidence. Six tiles
 * rather than five because the grid is three columns wide and a hanging cell reads
 * as a rendering bug.
 */
export const PROMISES = [
  {
    title: 'It proves itself',
    body: 'Every finding carries the query that produced it.',
  },
  {
    title: 'Nothing to maintain',
    body: 'No thresholds, no rules, no panels to keep in sync.',
  },
  {
    title: 'Five minutes, once',
    body: 'One container, two environment variables, no code changes.',
  },
  {
    title: 'Free if you run it',
    body: 'No per-host, per-seat or ingest fees. Hosted is ~$14 a month.',
  },
  {
    title: 'It never phones home',
    body: 'You run it. Your telemetry never leaves your infrastructure.',
  },
  {
    title: 'It watches itself',
    body: 'sigiro sends its own telemetry to sigiro. That is how we find our own bugs.',
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
    /**
     * Services and agents together, deliberately. An agent is a service that
     * calls a model instead of a database — section 03's whole point — so
     * splitting them into two rows would argue against the page. Claude Code
     * appears here and under "Read by" because it genuinely does both: it emits
     * its own OpenTelemetry (docs/install-with-ai) and it reads the answers.
     */
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
      { name: 'Claude Code', icon: 'SiClaude' },
      { name: 'Codex', icon: null },
      { name: 'Vercel AI SDK', icon: 'SiVercel' },
      { name: 'LangChain', icon: 'SiLangchain' },
      { name: 'Ollama', icon: 'SiOllama' },
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
 *
 * "Where does my data go?" says plainly that hosted holds your telemetry. Privacy
 * is one of the five claims, so a reader who goes looking for the seam between
 * self-hosted and hosted has to find an honest sentence rather than a dodge.
 */
export const FAQ = [
  {
    q: 'What is sigiro, in one sentence?',
    a: 'An observability backend built to be read by an agent. You send it OpenTelemetry, ask in SQL or in one investigate call, and get back a diagnosis instead of a dashboard.',
  },
  {
    q: 'Does it replace my current observability stack?',
    a: 'It replaces what you were using them for when the reader is a machine. If a person needs to look at a chart, keep the dashboard. This is for everything downstream of that.',
  },
  {
    q: 'Do I have to change my code?',
    a: 'No. It speaks standard OTLP on ports 4317 and 4318, so any OpenTelemetry SDK or collector you already run points at it unchanged.',
  },
  {
    q: 'Where does my data go?',
    a: 'Nowhere you did not put it. You run sigiro on your own infrastructure and it writes to your own storage. It does not phone home and there is no account to create. Hosted exists if you would rather not run anything, and that one does hold your telemetry for you.',
  },
  {
    q: 'Does it page me?',
    a: 'No, deliberately. Paging exists to interrupt someone who cannot watch continuously. What you get back is a table something else reads on a loop, so the interruption is the part we deleted.',
  },
  {
    q: 'How does it know what is worth reporting without thresholds?',
    a: 'Each service is compared against its own recent history instead of against a number you picked. A service that is chronically slow but steady reports nothing, because nothing changed. A service that quietly doubled reports, even though no rule existed for it.',
  },
  {
    q: 'What about sampling?',
    a: 'There is none. The request you need is kept, not one in a hundred. Sampling was a bargain struck because nobody was ever going to read them all. Something will.',
  },
  {
    q: 'What query language do I need to learn?',
    a: 'None. You ask in plain SQL, which your agent already writes fluently. There is no proprietary query language between you and your own telemetry, and every answer hands you the query that produced it.',
  },
  {
    q: 'Can my coding agent use it without me?',
    a: 'Yes, that is the design. It reads the API description itself at /openapi.json, with no key and no hand-written tool definition, then queries in SQL it already knows.',
  },
  {
    q: 'What does it cost?',
    a: 'Running it yourself is free and the storage is yours, so there is no per-host fee, no per-seat fee and no ingest bill. Hosted is about $14 a month and currently invite-only.',
  },
  {
    q: 'What does it not do?',
    a: 'It does not write your patches, run your pipeline, or evaluate alert rules. There is no dashboard builder and no full-text log index. Questions are structured, which is how an agent asks them.',
  },
] as const;

export const NAV = [
  { href: '/docs', label: 'Docs' },
  { href: '/docs/api', label: 'API' },
] as const;

export const SOCIAL = [
  { href: 'https://x.com/SigiroAI', label: 'X' },
  { href: 'https://www.linkedin.com/company/sigiro', label: 'LinkedIn' },
] as const;
