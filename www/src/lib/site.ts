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
  title: 'sigiro — observability data layer for software that repairs itself',
  /** The h1: the category claim. */
  tagline: 'The data layer for software that repairs itself.',
  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters or search results cut it mid-sentence. */
  description:
    'Send OpenTelemetry to sigiro. When a service fails, you get the cause and the query that proves it.',
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

/**
 * Connect, ask, correlate, prove — four shell sessions you step through.
 *
 * These were four numbered sections with one static code block each. A block
 * shows the input and never the answer, so a reader had to take the answer on
 * trust. One stepped terminal shows both, in the register the loop section above
 * it already set: a shell you watch, not a picture of a payload.
 *
 * Prove replaced a bespoke staged demo that expanded a panel to reveal the query
 * and then its rows. The session is a stronger proof for the same claim: you
 * watch `jq` lift `drill_down_sql` out of the answer, redirect it to a file, and
 * run that file. The query is not merely printed next to the answer — it is
 * visibly taken from it. `operation_summaries[]` and `--sql-file` are both real
 * (src/responses.rs, src/main.rs); an earlier draft of this step said
 * `.operations[0]`, which does not exist.
 *
 * `run` is a list because a step is not always one command. Connect starts a
 * container, sets two variables, starts your app, and asks sigiro what it sees.
 * A block whose `out` is empty prints nothing, and the terminal draws its prompt
 * line alone.
 *
 * The incident is the one the rest of the page reports: service `checkout`,
 * deploy 9f2c1ab, 246 timeouts on POST /api/orders. The investigate payload and
 * the correlation SQL are the artifacts the old sections printed, carried over
 * rather than rewritten. The 246 in the Correlate result is the same 246: one
 * model call per failed order.
 *
 * Every line has to fit the well, which holds about 86 characters at 12.5px and
 * clips the rest. That is why the two environment variables are two `export`
 * lines, and why the query is read from a file: neither fits beside a prompt on
 * one line. The docker line is INSTALL[0] with `-d` added — one duplicated
 * string, marked here for review if a third copy appears.
 */
export const HOW = [
  {
    title: 'Connect',
    say: 'sigiro uses standard OTLP on the standard ports.',
    run: [
      {
        cmd: 'docker run -d -p 4317:4317 -p 4318:4318 -p 9999:9999 ghcr.io/sigiroai/sigiro',
        out: 'a3f1c9e2d7b4',
      },
      { cmd: 'export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318', out: '' },
      { cmd: 'export OTEL_SERVICE_NAME=checkout', out: '' },
      { cmd: 'npm start &', out: '[1] 41823\ncheckout listening on :3000' },
      {
        cmd: 'sigiro status',
        out: 'ready · OTLP 4317 and 4318 · service checkout · 18,402 spans in 5 min',
      },
    ],
  },
  {
    title: 'Ask',
    say: 'sigiro ranks the results and names the deploy that caused the failure.',
    run: [
      {
        cmd: `curl -sX POST localhost:9999/v1/investigate -d '{"service":"checkout"}'`,
        out: `{
  "summary": "two signals moved together on checkout at 18:35",
  "suspect": "deploy 9f2c1ab, landed 18:33",
  "failing": "POST /api/orders",
  "how": { "timeout": 246, "server_error": 6 },
  "errors_total": 1284,
  "query": "SELECT * FROM spans WHERE service = 'checkout' AND ..."
}`,
      },
    ],
  },
  {
    title: 'Correlate',
    say: 'An agent is a service that calls a model. One trace holds both.',
    run: [
      {
        cmd: 'cat models.sql',
        out: `SELECT model, count(*) AS calls, max(duration) AS slowest
FROM spans JOIN logs USING (trace_id)
WHERE service = 'checkout' AND status = 'error'
GROUP BY model ORDER BY slowest DESC`,
      },
      {
        cmd: 'sigiro query --sql-file models.sql',
        out: `MODEL           CALLS  SLOWEST
gpt-5.2-mini      246    30.0s
claude-haiku-4.6   31     2.1s`,
      },
    ],
  },
  {
    title: 'Prove',
    say: 'Run the query again. Make the query wider. Show that the result is wrong.',
    run: [
      {
        cmd: `curl -s localhost:9999/v1/investigate -d '{"service":"checkout"}' > answer.json`,
        out: '',
      },
      {
        cmd: 'jq .operation_summaries[0] answer.json',
        out: `{
  "operation": "POST /api/orders",
  "errors": 246,
  "error_rate": 0.19,
  "drill_down_sql": "SELECT trace_id, duration, status_message FROM …"
}`,
      },
      {
        cmd: 'jq -r .operation_summaries[0].drill_down_sql answer.json > drill.sql',
        out: '',
      },
      {
        cmd: 'sigiro query --sql-file drill.sql',
        out: `TRACE_ID       DURATION  STATUS_MESSAGE
9f2c1ab4e77d      30012  context deadline exceeded
3b81fe0a2c56      30004  context deadline exceeded
c40d9e73aa18      29997  context deadline exceeded`,
      },
    ],
  },
] as const;

/**
 * What is left of the spine: one capability, one plain sentence, one artifact.
 *
 * It was 01–05. Connect, Ask, Correlate and then Prove all left for the stepped
 * terminal above, so Discover is the only one that stayed. It keeps a numeral
 * because it is still the last beat of a sequence the reader has been walking,
 * and dropping to a bare heading would read as a different kind of section.
 */
export const SECTIONS = [
  {
    n: '01',
    lead: 'Discover.',
    rest: 'Your agent reads the API description without help.',
    say: 'One public document describes all of the endpoints.',
    artifact: {
      title: 'GET /openapi.json',
      meta: 'public, no key',
      lang: 'json' as const,
      /* The real path set, checked against www/openapi.json. This used to list
         /v1/health, which the spec does not publish, and omitted /v1/anomalies,
         which it does — a section whose whole claim is "the document describes
         every endpoint" was printing a document that did neither. */
      code: `[
  "/v1/anomalies",
  "/v1/investigate",
  "/v1/query",
  "/v1/services",
  "/v1/traces/{trace_id}"
]`,
    },
  },
] as const;

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
 * same checkout incident that `HOW` and the Prove section report, carried from
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
 * Numbers were dropped from these stages when the numbered spine below kept its
 * own. Two `01–05` sequences ran back to back with identical numerals, and a
 * reader going "…05 Learn" → "01 Connect" had no way to see a new sequence had
 * started. `HOW` drops them for the same reason: a stepped terminal counts its
 * own steps in the caption, and a second numeral outside the frame competes.
 */
export const LOOP = [
  {
    title: 'Detect',
    owner: 'sigiro',
    ours: true,
    say: 'sigiro compares each service with its own history. You set no thresholds.',
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
    say: 'One call gives ranked results, the suspect deploy, and the query for each result.',
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
    owner: 'your agent',
    ours: false,
    say: 'Your agent writes the patch. sigiro does not do this step.',
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
    say: 'A test that passes does not close the incident. The baseline shows the recovery.',
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
    say: 'Keep the fix with the evidence. Use it when the same failure occurs again.',
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
    title: 'Proof with each result',
    body: 'Each result contains the query that made it.',
  },
  {
    title: 'Nothing to maintain',
    body: 'You set no thresholds and no rules. sigiro uses the history of each service.',
  },
  {
    title: 'Installation in five minutes',
    body: 'One container and two environment variables. You change no code.',
  },
  {
    title: 'Free when you install it',
    body: 'No fees for hosts, seats or data. The hosted service costs approximately $14 each month.',
  },
  {
    title: 'Your data stays with you',
    body: 'You install sigiro on your servers. sigiro sends no data to us.',
  },
  {
    title: 'sigiro monitors sigiro',
    body: 'sigiro sends its telemetry to sigiro. This is how we find our own failures.',
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
    q: 'What is sigiro?',
    a: 'sigiro is an observability backend for agents. You send OpenTelemetry to sigiro. You ask a question in SQL or with one investigate call. sigiro gives you a diagnosis, not a dashboard.',
  },
  {
    q: 'Does sigiro replace my current observability stack?',
    a: 'sigiro replaces them when a machine reads the data. If a person must look at a chart, keep the dashboard. sigiro is for the steps after that.',
  },
  {
    q: 'Must I change my code?',
    a: 'No. sigiro uses standard OTLP on ports 4317 and 4318. Your OpenTelemetry SDK or collector sends to sigiro without a change.',
  },
  {
    q: 'Where does my data go?',
    a: 'You install sigiro on your servers. sigiro writes to your storage and sends no data to us. You do not make an account. The hosted service is different: it keeps your telemetry for you.',
  },
  {
    q: 'Does sigiro page me?',
    a: 'No. A page interrupts a person who cannot watch continuously. sigiro gives a table that a program reads in a loop.',
  },
  {
    q: 'How does sigiro know what to report without thresholds?',
    a: 'sigiro compares each service with its own recent history. A service that is always slow but steady reports nothing, because nothing changed. A service that becomes two times slower reports, because no rule was necessary.',
  },
  {
    q: 'Does sigiro sample the data?',
    a: 'No. sigiro keeps each request. Sample rates were necessary when no one could read all of the data. A machine can read all of the data.',
  },
  {
    q: 'Which query language must I learn?',
    a: 'None. You ask in SQL. Your agent writes SQL correctly. Each answer gives you the query that made it.',
  },
  {
    q: 'Can my agent use sigiro without me?',
    a: 'Yes. The agent reads the API description at /openapi.json. It needs no key and no tool definition. Then it asks in SQL.',
  },
  {
    q: 'What does sigiro cost?',
    a: 'sigiro is free when you install it, and the storage is yours. There are no fees for hosts, seats or data. The hosted service costs approximately $14 each month. Access is by invitation.',
  },
  {
    q: 'What does sigiro not do?',
    a: 'sigiro does not write your patches, run your pipeline, or evaluate alert rules. sigiro has no dashboard builder and no full-text log index. Questions are structured, because that is how an agent asks.',
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
