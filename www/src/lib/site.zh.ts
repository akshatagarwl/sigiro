import type { LandingCopy } from "./site.en";

/**
 * Simplified Chinese (zh) landing-page copy.
 *
 * Typed against `site.en.ts`: this file implements the `LandingCopy` type
 * derived from `EN`, so every key, every array length and every ordering here
 * is fixed by the English source. Add a field there and this file is a compile
 * error until it carries the field too — which is the point, because a missing
 * translated string renders as `undefined` and nobody here reads Chinese well
 * enough to catch that by looking.
 *
 * Translated from `site.en.ts`, whose comments explain WHY each string is
 * worded as it is. Those constraints bind this file. The ones that bite:
 *
 * 1. Say 你. Prose with nobody in it reads as machine-generated in Chinese too.
 * 2. Technical register, mainland conventions, for a developer reading under
 *    incident pressure. Not marketing prose.
 * 3. Never translate `sigiro` (always lowercase, never transliterated),
 *    OpenTelemetry, OTLP, SQL, or any product, brand, command, URL, HTTP verb
 *    or path. Those live in `site.shared.ts` where a translation cannot reach
 *    them; the few that appear here are copied byte-for-byte.
 * 4. Never mention storage engines or runtime internals. They are
 *    implementation details.
 *
 * **WARNING — the causation rule.** English says "suspect deploy", never
 * "cause". sigiro correlates a deploy in time with a shift, and correlation in
 * time is not cause. In Chinese that means 可疑 and never 原因, 导致 or 证明.
 * The only place a conclusion word is allowed is `loop.learn`, which is about a
 * conclusion the USER reached after reading the evidence, and the `cause` line
 * in `LOOP_SESSIONS.learn` (site.shared.ts) that it sits under.
 *
 * **WARNING — the cost rule.** No competitor is named anywhere, and no price
 * or multiplier is quoted. The page sells the self-hosted product: free, and
 * running in five minutes. Hosted is an invited beta an operator sets up by
 * hand, so it gets a conversation, never a number and never a signup.
 *
 * **WARNING — the frame.** 软件工厂 ("software factory") was considered and
 * rejected on 2026-08-01. The frame is 自我修复的软件, "software that repairs
 * itself". Do not reintroduce the factory frame in any string here.
 */

export const ZH: LandingCopy = {
  /** Keeps "observability" (可观测性) in the title: the h1 is a category bet
      with no search intent behind it, and this is the one place the category
      word still earns its keep for search and for citation. */
  title: "sigiro — 用 SQL 查询的 OpenTelemetry 可观测性",

  /** The h1: what the product does. See site.en.ts. */
  tagline: "发送 OpenTelemetry，用 SQL 查询。",

  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters. Names only what ships; the ranked-results
      and suspect-deploy design is retired. See site.en.ts. */
  description:
    "sigiro 存储你的链路、日志、指标和性能剖析数据。通过一个 HTTP API 用 SQL 查询：智能体走 CLI 和 MCP，人看仪表盘。",

  /** Nav labels. The hrefs are in site.shared.ts.

      "API" is left in English rather than translated: it is a lowercase `.meta`
      link in a footer row, and 接口文档 would be both longer and vaguer than
      the word every Chinese-speaking developer already uses. */
  nav: {
    docs: "文档",
    reference: "API",
    dashboard: "仪表板",
  },

  /** The hero's second action. "Talk to us", not "book a demo": there is no
      demo, since the product is a binary you run in one command, and 预约演示
      promises a sales call that nobody here is staffed to give. */
  callLabel: "联系我们",

  /**
   * Strings that were hardcoded in the markup before the landing page was
   * localized: the hero's primary button and every section heading. Anything a
   * reader sees belongs here, not in the markup.
   */
  ui: {
    /** The header's way back in for a reader who already has an account. */
    signIn: "登录",
    signedInAs: "已登录：",
    signUp: "注册",
    installCli: "安装 CLI",
    /** The line under the hero loop. Restates the two claims that decide
        whether a reader keeps reading: it is free self-hosted, and nothing
        leaves their infrastructure. Same wording as `promises[3]`, and it must
        not imply a hosted signup — there is none. */
    heroNote:
      "部署在你自己的服务器上就免费，sigiro 不会把任何数据发给我们。注册则使用托管测试版，遥测数据存放在我们运行的服务器上。",
    /** The stepped-terminal section. `stepsAria` labels the step list for
        screen readers; `shellAria` names the transcript region. `howTail`
        repeats the four `how.*` titles in order, so the two must stay in
        step. */
    howLabel: "怎么用",
    howTitle: "四个步骤。",
    howTail: "你依次完成接入、提问、关联和深入排查。",
    howLede:
      "把 OpenTelemetry 导出器指向 sigiro，然后用 SQL 提问。下面每一步都是一次终端会话。",
    stepsAria: "四个步骤",
    shellAria: "你的终端",
    /** The integrations section. 不用迁移 is a claim: sigiro speaks OTLP, so
        the reader keeps the instrumentation they already have. */
    integrationsLabel: "你能得到什么",
    integrationsTitle: "不用迁移。",
    integrationsTail: "sigiro 能对接你正在用的工具。",
  },

  /**
   * Connect, ask, correlate, prove — the four steps of the stepped terminal.
   *
   * `title` is the step name in the caption; `say` is the sentence above the
   * transcript. The transcripts themselves are in site.shared.ts.
   */
  how: {
    connect: {
      title: "接入",
      say: "sigiro 使用标准 OTLP，监听标准端口。",
    },
    ask: {
      title: "提问",
      say: "通过 HTTP 发送 SQL，答案以 JSON 行的形式返回。",
    },
    correlate: {
      title: "关联",
      say: "智能体就是一个会调用模型的服务。同一条链路里两者都在。",
    },
    prove: {
      title: "深入排查",
      say: "列出失败的请求。每一行都带有 trace_id，可以从 /v1/traces 取回完整链路。",
    },
  },

  /**
   * What is left of the spine: one capability, one plain sentence, one artifact.
   *
   * It was 01–05. Connect, Ask, Correlate and Prove all left for the stepped
   * terminal, so Discover is the only one that stayed. It keeps a numeral
   * because it is still the last beat of a sequence the reader has been walking.
   *
   * `n` and `artifactTitle` are not copy: '01' is a numeral, and
   * `GET /openapi.json` is a real request line.
   */
  discover: {
    n: "01",
    lead: "自动发现。",
    rest: "你的智能体不用人帮忙就能读懂 API 描述。",
    say: "一份公开文档描述了全部接口。",
    artifactTitle: "GET /openapi.json",
    artifactMeta: "公开，无需密钥",
  },

  /**
   * The five-stage loop. `owner` is rendered verbatim under each stage, so it
   * has to name a real thing a reader already runs.
   *
   * The positioning in one object: sigiro covers two of the five stages plus
   * the evidence a third one needs. Saying which stages are *not* ours is the
   * point, so `fix` and `learn` name the reader's own tools.
   */
  loop: {
    detect: {
      title: "检测",
      owner: "sigiro",
      say: "你的智能体用一条 SQL 查询统计失败的 span。",
    },
    diagnose: {
      title: "诊断",
      owner: "sigiro",
      say: "第二条查询显示哪些操作在失败、失败了多少次。",
    },
    fix: {
      title: "修复",
      owner: "你的智能体",
      say: "补丁由你的智能体来写。这一步 sigiro 不做。",
    },
    verify: {
      title: "验证",
      owner: "你的流水线，用我们的证据",
      say: "测试通过不等于服务恢复。把查询再跑一遍，确认错误已经消失。",
    },
    learn: {
      /* The one stage where a conclusion word is allowed: the playbook is a
         file YOU wrote after you read the evidence and fixed the service, and
         `LOOP_SESSIONS.learn` prints a `cause` line to match. Do not copy that
         register into any string that describes sigiro's own output. */
      title: "沉淀",
      owner: "你的记忆库",
      say: "把修复方案和证据一起存下来。同样的故障再出现时就直接拿来用。",
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
      title: "答案可以重跑",
      body: "每个答案都是一条 SQL 查询。你、你的智能体或同事都可以再跑一遍。",
    },
    {
      title: "所有信号在一处",
      body: "链路、日志、指标和性能剖析都是表，一条查询就能关联起来。",
    },
    {
      /* 一条命令, not 一个容器: the binary is the lead install path now that
         `/install` works. */
      title: "五分钟装好",
      body: "一条命令，两个环境变量。代码一行都不用改。",
    },
    {
      /* The site sells the self-hosted product. Hosted is a hand-built invited
         beta, so it gets a conversation rather than a price: a number here
         would promise a self-serve signup that does not exist. Do not add one,
         and do not write 注册 or 免费试用 in this tile. */
      title: "自己部署就免费",
      body: "不按主机、席位或数据量收费。托管测试版尚未公布价格，欢迎来聊。",
    },
    {
      title: "数据留在你手里",
      body: "你把 sigiro 装在自己的服务器上。sigiro 不会把任何数据发给我们。托管测试版例外：遥测数据存放在我们这里。",
    },
    {
      title: "sigiro 监控 sigiro",
      body: "sigiro 把自己的遥测数据发给 sigiro。我们就是这样发现自己的故障的。",
    },
  ],

  /**
   * The three integration group labels, plus the one prose item that closes the
   * "Read by" group. Every other entry in those groups is a product name and
   * lives in site.shared.ts, untranslated.
   */
  integrations: {
    emitLabel: "从这些地方发送遥测数据",
    runLabel: "在这些平台上运行",
    readLabel: "这些工具可以读取",
    readAnySql: "任何会说 SQL 的工具",
  },

  /**
   * The pool of questions handed to the reader's own assistant. One is drawn at
   * random per link, per page load.
   *
   * **One use case each, one line each, and each in the reader's voice.** These
   * are not questions about sigiro in the abstract — they are the situation the
   * reader arrived with, written as that reader would write it in Chinese. A
   * reader who opens two gets two facets rather than the same paragraph twice.
   *
   * The situations are not invented here. They are the use cases and personas in
   * the positioning brief. Adding a line means finding the persona it
   * serves, not thinking of a new question.
   *
   * Do not write 软件工厂. That frame was considered and **rejected** on
   * 2026-08-01 because it describes our process and not the reader's outcome;
   * the frame is 自我修复的软件.
   *
   * Two rules bind every entry, and a variant that breaks either one quietly
   * breaks the feature:
   *
   * 1. The three public URLs are prepended by `site.ts` from the shared
   *    `READ_ALL_THREE`. Never retype a URL here — a corrupted path inside
   *    Chinese text fails silently and no reviewer here would spot it.
   * 2. Claim nothing the product does not do — including naming no competitor,
   *    which is why none of these ask for a comparison against a named rival.
   *    Ask questions; do not assert features.
   *
   * No entry quotes a price. Hosted is a call rather than a signup, so a number
   * in one of these invites an assistant to repeat it as fact.
   */
  askAi: [
    // Coding agent in a loop — the primary use case in product-marketing.md.
    "我的编程智能体看不到生产环境，只能盲写补丁。sigiro 能把这个闭环补上吗？",
    "我想让我的智能体发现故障、打上补丁，再确认修复是否生效。sigiro 在这个闭环里能做哪一段？",
    "我的智能体怎样查询 sigiro？会拿到什么？",

    // Language and stack entry points — the "I have an X app" shape.
    "我有一个 Go 服务，完全没做埋点。想用 sigiro 排查它，我要先做什么？",
    "我的 Node 应用已经把 OpenTelemetry 发到别的地方了。改成发给 sigiro 会有什么变化？",
    "我在一台机器上跑着几个服务。用 sigiro 是不是太重了，还是正合适？",

    // LLM app debugging — a named use case.
    "我的大模型应用会卡住，但我看不出是哪一次模型调用拖住了它。sigiro 能看出来吗？",

    // 3am incident with an agent on call.
    "生产环境凌晨三点出了问题，当时没人醒着。sigiro 会告诉一个定时运行的智能体什么？",

    // Solo founder persona: price and setup time.
    "我一个人做产品，实在没法承担按席位计费的可观测性账单。用 sigiro 我要花多少钱？",
    "sigiro 装起来要多久？装完之后我还要维护什么？",

    // Privacy-conscious persona: data ownership.
    "我不想让客户的遥测数据离开我自己的基础设施。sigiro 会把数据存在哪里？",

    // Small platform team: dashboard rot.
    "我每周要花好几个小时维护根本没人看的仪表盘。sigiro 能替掉其中的哪些？",

    // The honest one. A reader who asks this and gets a straight answer trusts
    // the rest.
    "sigiro 在什么情况下并不合适？它目前还做不到什么？",
  ],
} as const;
