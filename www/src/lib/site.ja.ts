import type { LandingCopy } from "./site.en";

/**
 * Japanese landing-page copy — a translation of `site.en.ts`.
 *
 * English is the source. This file is typed as `LandingCopy`, the shape derived
 * from `site.en.ts`, so a field added there is a compile error here until this
 * file carries it too. A silently-missing string would render as `undefined`,
 * and nobody here reads Japanese well enough to catch that by looking.
 *
 * Register: polite です／ます, technical documentation style. The English says
 * "you" in almost every sentence; Japanese drops the pronoun, which is the
 * natural equivalent. あなた appears only in the `loop` owner labels, where the
 * whole point of the label is whose stage it is.
 *
 * Three constraints from `.agents/product-marketing.md` survive translation and
 * are the ones a reviewer here cannot check:
 *
 * 1. **疑わしいデプロイ, never 原因.** sigiro correlates a deploy in time with a
 *    shift, and correlation in time is not cause. 原因 and 証明 appear nowhere in
 *    this file. The one conclusion the page allows is the user's own, in
 *    `loop.learn`.
 * 2. **No competitor is named and no price or multiplier is quoted.** The page
 *    sells the self-hosted product, which is free and runs in five minutes.
 *    Hosted is an invited beta an operator sets up by hand, so `promises[3]`
 *    offers a conversation rather than a signup that does not exist.
 * 3. **The frame is 自らを修復するソフトウェア** ("software that repairs itself"),
 *    used verbatim in `title` and `tagline`. ソフトウェア工場 ("software factory")
 *    was considered and rejected on 2026-08-01, because it describes our process
 *    and not the reader's outcome. Do not reintroduce it.
 *
 * Product names, commands, URLs, HTTP verbs and paths are never translated:
 * brands are written verbatim in Japanese technical documentation. The ones that
 * carry real risk are not in this file at all — commands, terminal output, icon
 * slugs, URLs and the `ours` flags live in `site.shared.ts`, where a translation
 * cannot reach them.
 */
export const JA: LandingCopy = {
  /** Keeps the category word in the title: the h1 is a category bet with no
      search intent behind it, and this is the one place オブザーバビリティ still
      earns its keep for search and for citation. */
  title: "sigiro — SQL でクエリできる OpenTelemetry オブザーバビリティ",

  /** The h1: what the product does. See site.en.ts. */
  tagline: "OpenTelemetry を送り、SQL でクエリする。",

  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters. Names only what ships; the ranked-results
      and suspect-deploy design is retired. See site.en.ts. */
  description:
    "sigiro はトレース、ログ、メトリクス、プロファイルを保存します。1 つの HTTP API で SQL を使ってクエリでき、エージェントからは CLI と MCP で、人はダッシュボードで読めます。",

  /** Nav labels. The hrefs are in site.shared.ts.

      "API" rather than API リファレンス: this nav is lowercase `.meta` links in a
      footer row, so it takes the shortest honest word. */
  nav: {
    docs: "ドキュメント",
    reference: "API",
    dashboard: "ダッシュボード",
  },

  /** The hero's second action. 相談する ("talk to us"), not デモを予約 ("book a
      demo"): there is no demo, since the product is a binary you run in one
      command, and "demo" promises a sales call that nobody here is staffed to
      give. */
  callLabel: "相談する",

  /**
   * Strings that were hardcoded in the markup before the landing page was
   * localized — the hero's primary button and every section heading.
   *
   * Added after the Japanese translation pass had already been dispatched, so
   * this block was written separately, in the same です／ます register the rest of
   * the file uses.
   */
  ui: {
    /** The header's way back in for a reader who already has an account. */
    signIn: "ログイン",
    signedInAs: "ログイン中:",
    signUp: "登録する",
    installCli: "CLI をインストール",
    /** The line under the hero loop. Restates the two claims that decide
        whether a reader keeps reading: it is free self-hosted, and nothing
        leaves their infrastructure. Must not imply a hosted signup — there is
        none, so no 無料トライアル and no number. */
    heroNote:
      "ご自身のサーバーで動かせば無料で、sigiro がデータを当社に送ることはありません。サインアップするとホスト版ベータになり、テレメトリーは当社が運用するサーバーに保存されます。",
    /** The stepped-terminal section. `howTail` repeats the four `how.*` titles
        in order (接続・質問・相関・検証), so the two must stay in step.
        `stepsAria` labels the step list for screen readers; `shellAria` names
        the transcript region. */
    howLabel: "使い方",
    howTitle: "4 つのステップ。",
    howTail: "接続し、質問し、相関させ、掘り下げます。",
    howLede:
      "OpenTelemetry のエクスポーターを sigiro に向け、SQL で質問します。以下の各ステップは 1 つのシェルセッションです。",
    stepsAria: "4 つのステップ",
    shellAria: "あなたのシェル",
    /** The integrations section. "No migration" is a claim: sigiro speaks OTLP,
        so the reader keeps the instrumentation they already have. */
    integrationsLabel: "得られるもの",
    integrationsTitle: "移行は不要です。",
    integrationsTail: "sigiro は、あなたが使っているツールに接続します。",
  },

  /**
   * Connect, ask, correlate, prove — the four steps of the stepped terminal.
   *
   * `title` is the step name in the caption; `say` is the sentence above the
   * transcript. The transcripts themselves are in site.shared.ts.
   */
  how: {
    connect: {
      title: "接続",
      say: "sigiro は標準の OTLP を標準のポートで受け取ります。",
    },
    ask: {
      title: "質問",
      say: "HTTP 経由で SQL を送ると、答えが JSON の行として返ります。",
    },
    correlate: {
      title: "相関",
      say: "エージェントとは、モデルを呼び出すサービスです。1 つのトレースがその両方を収めます。",
    },
    prove: {
      title: "掘り下げ",
      say: "失敗したリクエストを一覧にします。各行の trace_id を使えば、/v1/traces からトレース全体を取得できます。",
    },
  },

  /**
   * What is left of the spine: one capability, one plain sentence, one artifact.
   *
   * It was 01–05. Connect, Ask, Correlate and Prove all left for the stepped
   * terminal, so Discover is the only one that stayed. It keeps a numeral
   * because it is still the last beat of a sequence the reader has been walking.
   *
   * `lead` and `rest` render as one heading, `lead` followed by `rest` in a
   * lighter colour, so `lead` closes with 。 and `rest` is a full sentence.
   */
  discover: {
    n: "01",
    lead: "発見。",
    rest: "エージェントは手助けなしで API の記述を読み取ります。",
    say: "1 つの公開ドキュメントが、すべてのエンドポイントを記述しています。",
    artifactTitle: "GET /openapi.json",
    artifactMeta: "公開、キー不要",
  },

  /**
   * The five-stage loop. `owner` is rendered verbatim under each stage, so it
   * has to name a real thing a reader already runs.
   *
   * The positioning in one object: the loop is not closed until every stage is
   * covered, and sigiro covers two of them plus the evidence a third needs.
   * Saying which stages are *not* ours is the point, which is why あなたの stays
   * in three of these labels even though the rest of the file drops the pronoun.
   */
  loop: {
    detect: {
      title: "検出",
      owner: "sigiro",
      say: "エージェントが SQL クエリ 1 つで失敗したスパンを数えます。",
    },
    diagnose: {
      title: "診断",
      owner: "sigiro",
      say: "2 つ目のクエリで、どの処理がどれだけ失敗しているかが分かります。",
    },
    fix: {
      title: "修正",
      owner: "あなたのエージェント",
      say: "パッチを書くのはエージェントです。sigiro はこのステップを行いません。",
    },
    verify: {
      title: "確認",
      owner: "あなたのパイプライン（sigiro の根拠を使用）",
      say: "テストが通ることと、サービスが回復することは同じではありません。クエリをもう一度実行し、エラーが消えたことを確かめます。",
    },
    learn: {
      title: "学習",
      owner: "あなたのメモリ",
      say: "修正内容を根拠とあわせて残します。同じ障害がまた起きたときに、それを使います。",
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
      title: "答えは再実行できます",
      body: "すべての答えは SQL クエリです。ご自身でも、エージェントでも、チームの誰でも同じクエリを再実行できます。",
    },
    {
      title: "すべてのシグナルを 1 か所に",
      body: "トレース、ログ、メトリクス、プロファイルはテーブルとして保存され、1 つのクエリで結合できます。",
    },
    {
      /* コマンド 1 つ, not "one container": the binary is the lead install path
         now that `/install` works. */
      title: "5 分で導入できます",
      body: "コマンド 1 つと環境変数 2 つだけです。コードは変更しません。",
    },
    {
      /* The site sells the self-hosted product. Hosted is a hand-built beta, so
         it gets a conversation rather than a price: quoting a monthly figure
         invited a reader to sign up for something with no signup. Do not add a
         number or a 無料トライアル to this tile. */
      title: "自分で動かせば無料です",
      body: "ホスト単位、ユーザー単位、データ量のいずれにも料金はかかりません。ホスト版ベータの料金はまだ公開していません。ご相談ください。",
    },
    {
      title: "データは手元に残ります",
      body: "sigiro はご自身のサーバーにインストールします。sigiro が開発元へデータを送ることはありません。例外はホスト版ベータで、テレメトリーは当社に保存されます。",
    },
    {
      title: "sigiro は sigiro を監視します",
      body: "sigiro は自身のテレメトリを sigiro に送ります。私たちはこの方法で自分たちの障害を見つけています。",
    },
  ],

  /**
   * The three integration group labels, plus the one prose item that closes the
   * "Read by" group. Every other entry in those groups is a product name and
   * lives in site.shared.ts.
   */
  integrations: {
    emitLabel: "テレメトリの送信元",
    runLabel: "実行環境",
    readLabel: "読み取るツール",
    readAnySql: "SQL を扱えるものすべて",
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
   * Do not write ソフトウェア工場 ("software factory"). That frame was considered
   * and **rejected** on 2026-08-01 because it describes our process and not the
   * reader's outcome; the frame is 自らを修復するソフトウェア.
   *
   * Two rules bind every entry, and a variant that breaks either one quietly
   * breaks the feature:
   *
   * 1. The three public URLs are prepended by `site.ts` from the shared
   *    `READ_ALL_THREE`, so they are never retyped here or in a translation. An
   *    assistant reads a URL regardless of the language of the sentence around
   *    it, so no URL belongs in this array.
   * 2. Claim nothing the product does not do — including naming no competitor,
   *    which is why none of these ask for a comparison against a named rival.
   *    Ask questions; do not assert features.
   *
   * No entry quotes a price. Hosted is a call rather than a signup, so a number
   * here would invite an assistant to repeat one from its training data.
   */
  askAi: [
    // Coding agent in a loop — the primary use case in product-marketing.md.
    "コーディングエージェントが本番環境を見られないため、手探りでパッチを書いています。sigiro はこのループを閉じられますか？",
    "エージェントに障害を検出させ、パッチを当てさせ、修正が効いたことまで確認させたいです。sigiro はこのループのどこに入りますか？",
    "エージェントは sigiro にどうやってクエリし、何が返ってきますか？",

    // Language and stack entry points — the "I have an X app" shape.
    "計装をまったく入れていない Go のサービスがあります。sigiro でデバッグするには何をすればよいですか？",
    "すでに OpenTelemetry を別の宛先へ送っている Node のアプリがあります。送信先を sigiro に向けると何が変わりますか？",
    "1 台のサーバーでサービスをいくつか動かしています。sigiro は大げさすぎますか、それともちょうどよいですか？",

    // LLM app debugging — a named use case.
    "LLM アプリの処理が止まるのですが、どのモデル呼び出しで止まったのか分かりません。sigiro はそれを見せてくれますか？",

    // 3am incident with an agent on call.
    "深夜 3 時に本番環境が壊れましたが、誰も起きていませんでした。sigiro があれば、スケジュール実行のエージェントに何を伝えられましたか？",

    // Solo founder persona: price and setup time.
    "一人で開発していて、ユーザー単位で課金されるオブザーバビリティの請求は正当化できません。sigiro だといくらかかりますか？",
    "sigiro のセットアップにはどれくらい時間がかかり、その後は何を維持する必要がありますか？",

    // Privacy-conscious persona: data ownership.
    "顧客のテレメトリを自分のインフラの外に出したくありません。sigiro はデータをどこに保存しますか？",

    // Small platform team: dashboard rot.
    "誰も見ないダッシュボードの手入れに毎週何時間も使っています。sigiro は何を置き換えてくれますか？",

    // The honest one. A reader who asks this and gets a straight answer trusts
    // the rest.
    "sigiro が向かないのはどんな場合で、まだできないことは何ですか？",
  ],
} as const;
