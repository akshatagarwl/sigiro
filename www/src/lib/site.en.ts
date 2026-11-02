/**
 * English landing-page copy — the source every translation is made from.
 *
 * Positioning, voice and the five claims live in .agents/product-marketing.md —
 * read that before editing copy here. Three rules from it that bite hardest:
 *
 * 1. Say "you". Prose with nobody in it reads as machine-generated no matter how
 *    accurate it is, and that was the diagnosis on the previous draft.
 * 2. Vary the rhythm. The old copy hinged eleven sentences on an em-dash, which
 *    is what made every line sound like the same line. Budget for the page is two.
 * 3. Never mention storage engines or runtime internals. Users care about
 *    outcomes, not implementation details.
 *
 * Cost claims name no competitor and quote no multiplier. The page sells the
 * self-hosted product — free, and running in five minutes — and quotes **no
 * hosted price at all**: hosted is an invited beta that an operator sets up by
 * hand, so a number there promises a self-serve signup that does not exist.
 *
 * **This file is the contract for `site.zh.ts`, `site.ja.ts` and `site.ko.ts`.**
 * They implement the same `LandingCopy` type, so adding a field here is a
 * compile error in all three until they carry it too — which is the point. A
 * silently-missing translated string would render as `undefined`, and nobody
 * here reads all three languages well enough to catch that by looking.
 *
 * Commands, terminal output, icon slugs, URLs and the `ours` flags are NOT here.
 * They are in `site.shared.ts`, where a translation cannot reach them.
 */

/**
 * Widen string literals to `string`, recursively, while preserving the key
 * structure and every array length.
 *
 * Needed because `EN` is `as const`, which makes each value a *literal* type:
 * `title` is not `string` but `'sigiro — observability data layer …'`. A type
 * derived directly from it would therefore reject every translation — the only
 * assignable value would be the English one, byte for byte. The first version of
 * this contract did exactly that and produced 177 `TS2322` errors the moment a
 * locale file was translated; the Japanese pass caught it.
 *
 * Tuples are mapped elementwise rather than collapsed to `T[]`, so the array
 * lengths stay part of the contract: a locale that ships 5 promises instead of 6,
 * or 12 Ask-AI prompts instead of 13, is still a compile error. That is the
 * property worth keeping — a missing entry would otherwise render as `undefined`
 * on a page nobody here can proofread.
 */
type Widen<T> = T extends string
  ? string
  : T extends readonly unknown[]
    ? { readonly [K in keyof T]: Widen<T[K]> }
    : T extends object
      ? { readonly [K in keyof T]: Widen<T[K]> }
      : T;

/**
 * The shape every locale must satisfy.
 *
 * Derived from this file rather than declared separately, so English stays the
 * definition and the translations are checked against it by the compiler: add a
 * field here and all three locale files fail to compile until they carry it too.
 */
export type LandingCopy = Widen<typeof EN>;

export const EN = {
  /** Keeps "observability" in the title: the h1 is a category bet with no search
      intent behind it, and this is the one place the category word still earns
      its keep for search and for citation. */
  title: "sigiro — observability data layer for software that repairs itself",

  /** The h1: the category claim. */
  tagline: "The data layer for software that repairs itself.",

  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters or search results cut it mid-sentence.

      It says "suspect deploy" and not "cause". sigiro correlates a deploy in
      time with a shift, and correlation in time is not cause — see
      docs/explanation/evidence.md, "What this design refuses to do". The claim
      the product can keep is that each result carries the query that made it,
      so you can check the result or disprove it. Do not put "cause", "proves"
      or "proven" back in this string. */
  description:
    "Send OpenTelemetry to sigiro. When a service fails, you get ranked results, the suspect deploy, and the query for each result.",

  /** Nav labels. The hrefs are in site.shared.ts.

      "API" rather than the "API Reference" that Stripe and Resend use in their
      sidebars: this nav is lowercase `.meta` links in a footer row, so it takes
      the shortest honest word. */
  nav: {
    docs: "Docs",
    reference: "API",
    dashboard: "Dashboard",
  },

  /** The hero's second action. "Talk to us", not "book a demo": there is no
      demo, since the product is a binary you run in one command, and "demo"
      promises a sales call that nobody here is staffed to give. */
  callLabel: "Talk to us",

  /**
   * Strings that were hardcoded in the markup before the landing page was
   * localized.
   *
   * They were found by extracting every text node from `pages/index.astro` and
   * its components after the first split, which is worth recording: a copy
   * inventory taken from `site.ts` alone missed the hero's primary button and
   * every section heading — the most visible text on the page. Anything a
   * reader sees belongs here, not in the markup.
   */
  ui: {
    /** The header's way back in for a reader who already has an account. */
    signIn: "Sign in",
    signUp: "Sign up",
    installCli: "Install CLI",
    /** The line under the hero loop. Restates the two claims that decide
        whether a reader keeps reading: it is free self-hosted, and nothing
        leaves their infrastructure. */
    heroNote:
      "Installed on your servers: free, and sigiro sends no data to us. Signed up: the hosted beta, which stores your telemetry on servers we run.",
    /** The stepped-terminal section. `stepsAria` labels the step list for
        screen readers; `shellAria` names the transcript region. */
    howLabel: "how you use it",
    howTitle: "Four steps.",
    howTail: "You connect, ask, correlate, and prove.",
    howLede:
      "You send OpenTelemetry to sigiro. You ask one question. Each step below is one shell session.",
    stepsAria: "The four steps",
    shellAria: "your shell",
    /** The integrations section. "No migration" is a claim: sigiro speaks OTLP,
        so the reader keeps the instrumentation they already have. */
    integrationsLabel: "what you get",
    integrationsTitle: "No migration.",
    integrationsTail: "sigiro connects to the tools you use.",
  },

  /**
   * Connect, ask, correlate, prove — the four steps of the stepped terminal.
   *
   * `title` is the step name in the caption; `say` is the sentence above the
   * transcript. The transcripts themselves are in site.shared.ts.
   */
  how: {
    connect: {
      title: "Connect",
      say: "sigiro uses standard OTLP on the standard ports.",
    },
    ask: {
      title: "Ask",
      /* "the suspect deploy", not "the deploy that caused the failure". The
         payload says `"suspect"`, and the prose has to say what the payload
         says. sigiro finds a deploy near the shift in time; a person or an
         agent decides whether that deploy is the cause. */
      say: "sigiro ranks the results and names the suspect deploy.",
    },
    correlate: {
      title: "Correlate",
      say: "An agent is a service that calls a model. One trace holds both.",
    },
    prove: {
      title: "Prove",
      say: "Run the query again. Make the query wider. Show that the result is wrong.",
    },
  },

  /**
   * What is left of the spine: one capability, one plain sentence, one artifact.
   *
   * It was 01–05. Connect, Ask, Correlate and Prove all left for the stepped
   * terminal, so Discover is the only one that stayed. It keeps a numeral
   * because it is still the last beat of a sequence the reader has been walking.
   */
  discover: {
    n: "01",
    lead: "Discover.",
    rest: "Your agent reads the API description without help.",
    say: "One public document describes all of the endpoints.",
    artifactTitle: "GET /openapi.json",
    artifactMeta: "public, no key",
  },

  /**
   * The factory loop. `owner` is rendered verbatim under each stage, so it has
   * to name a real thing a reader already runs.
   *
   * The positioning in one object: a software factory is not a factory until
   * every stage is covered, and sigiro covers two of them plus the evidence for
   * a third. Saying which stages are *not* ours is the point.
   */
  loop: {
    detect: {
      title: "Detect",
      owner: "sigiro",
      say: "sigiro compares each service with its own history. You set no thresholds.",
    },
    diagnose: {
      title: "Diagnose",
      owner: "sigiro",
      say: "One call gives ranked results, the suspect deploy, and the query for each result.",
    },
    fix: {
      title: "Fix",
      owner: "your agent",
      say: "Your agent writes the patch. sigiro does not do this step.",
    },
    verify: {
      title: "Verify",
      owner: "your pipeline, on our evidence",
      say: "A test that passes is not the same as a service that recovers. Ask again, and read what is gone.",
    },
    learn: {
      title: "Learn",
      owner: "your memory",
      say: "Keep the fix with the evidence. Use it when the same failure occurs again.",
    },
  },

  /**
   * The five things users actually care about, plus the one that proves we mean
   * it. Order is deliberate: proof first (it is the differentiator), then the
   * three cheap-easy-quiet promises, then privacy, then dogfooding as evidence.
   * Six tiles rather than five because the grid is three columns wide and a
   * hanging cell reads as a rendering bug.
   */
  promises: [
    {
      /* "Prove it yourself", not "Proof with each result". The old title put the
         proof on us, and sigiro proves nothing: it measures a shift and names a
         deploy near it in time. What you get is a result you can check, and the
         check is yours to run. */
      title: "Prove it yourself",
      body: "Each result contains the query that made it.",
    },
    {
      title: "Nothing to maintain",
      body: "You set no thresholds and no rules. sigiro uses the history of each service.",
    },
    {
      /* "One command", not "one container": the binary is the lead install path
         now that `/install` works. */
      title: "Installation in five minutes",
      body: "One command and two environment variables. You change no code.",
    },
    {
      /* The self-hosted product is free. Hosted is a beta with a real sign-up
         now (the hero's "Sign up") but no published price, so the body says
         exactly that and points to a conversation. Do not quote a number here
         until one is decided. */
      title: "Free when you install it",
      body: "No fees for hosts, seats or data when you run it. The hosted beta has no published price yet; talk to us.",
    },
    {
      title: "Your data stays with you",
      body: "You install sigiro on your servers. sigiro sends no data to us. The hosted beta is the exception: it stores your telemetry with us.",
    },
    {
      title: "sigiro monitors sigiro",
      body: "sigiro sends its telemetry to sigiro. This is how we find our own failures.",
    },
  ],

  /**
   * The three integration group labels, plus the one prose item that closes the
   * "Read by" group. Every other entry in those groups is a product name and
   * lives in site.shared.ts.
   */
  integrations: {
    emitLabel: "Send telemetry from",
    runLabel: "Run it on",
    readLabel: "Read by",
    readAnySql: "Anything that speaks SQL",
  },

  /**
   * The pool of questions handed to the reader's own assistant. One is drawn at
   * random per link, per page load.
   *
   * **One use case each, one line each, and each in the reader's voice.** These
   * are not questions about sigiro — they are the situation the reader arrived
   * with. A reader who opens two gets two facets rather than the same paragraph
   * twice.
   *
   * The situations are not invented here. They are the use cases and personas in
   * `.agents/product-marketing.md`. Adding a line means finding the persona it
   * serves, not thinking of a new question.
   *
   * Do not write "software factory". That frame was considered and **rejected**
   * on 2026-08-01 because it describes our process and not the reader's outcome;
   * the frame is "software that repairs itself".
   *
   * Two rules bind every entry, and a variant that breaks either one quietly
   * breaks the feature:
   *
   * 1. The three public URLs are prepended by `site.ts` from the shared
   *    `READ_ALL_THREE`, so they are never retyped here or in a translation.
   * 2. Claim nothing the product does not do — including naming no competitor,
   *    which is why none of these ask for a comparison against a named rival.
   *    Ask questions; do not assert features.
   *
   * On price: this list DOES contain one cost question ("I ship alone and
   * cannot justify a per-seat observability bill. What does sigiro cost me?").
   * An earlier version of this comment claimed there was deliberately none,
   * which the entry directly below it contradicted — caught during the Korean
   * translation pass, when the translator asked which of the two to follow.
   *
   * The entry is kept, and the comment corrected to match, because the question
   * is safe in the form it takes: it asks what sigiro costs *the reader* rather
   * than requesting a price list, and the three documents it is sent with answer
   * it truthfully (self-hosted is free; hosted is a call). What must never
   * appear is a prompt that asks for a *number* or a comparison against a named
   * rival, since either invites an answer from training data rather than from
   * our own documents.
   *
   * If this entry is ever removed, remove it from all four locale files in the
   * same commit — the array length is part of the `LandingCopy` contract.
   */
  askAi: [
    // Coding agent in a loop — the primary use case in product-marketing.md.
    "My coding agent writes patches blind because it cannot see production. Can sigiro close that loop?",
    "I want my agent to detect a fault, patch it, and confirm the fix worked. How does sigiro fit that loop?",
    "What evidence does sigiro hand an agent that a dashboard cannot?",

    // Language and stack entry points — the "I have an X app" shape.
    "I have a Go service with no instrumentation. What do I do to debug it with sigiro?",
    "I have a Node app that already sends OpenTelemetry somewhere else. What changes if I point it at sigiro?",
    "I run a few services on one box. Is sigiro too much or about right?",

    // LLM app debugging — a named use case.
    "My LLM app stalls and I cannot tell which model call did it. Does sigiro show that?",

    // 3am incident with an agent on call.
    "Production broke at 3am and nobody was awake. What would sigiro have told a scheduled agent?",

    // Solo founder persona: price and setup time.
    "I ship alone and cannot justify a per-seat observability bill. What does sigiro cost me?",
    "How long does sigiro take to set up, and what do I have to maintain afterwards?",

    // Privacy-conscious persona: data ownership.
    "I do not want customer telemetry leaving my own infrastructure. Where does sigiro store it?",

    // Small platform team: dashboard rot.
    "I spend hours a week on dashboards nobody reads. What does sigiro replace?",

    // The honest one. A reader who asks this and gets a straight answer trusts
    // the rest.
    "Where is sigiro the wrong choice, and what does it not do yet?",
  ],
} as const;
