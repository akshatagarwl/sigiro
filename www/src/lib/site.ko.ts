import type { LandingCopy } from './site.en';

/**
 * Korean (ko) landing-page copy.
 *
 * Typed against `site.en.ts`: this file implements the `LandingCopy` type
 * derived from `EN`, so English stays the definition and this translation is
 * checked against it by the compiler. Read `site.en.ts` first — its comments
 * explain why each English string is worded the way it is, and those reasons
 * bind the Korean just as hard. Positioning and the five claims live in
 * .agents/product-marketing.md.
 *
 * Register: formal 합니다체, technical documentation voice. Imperatives are
 * 하십시오체. The reader stays in the sentence — Korean drops the subject far
 * more freely than English, but prose with nobody in it reads as
 * machine-generated in either language, so direct address is kept.
 *
 * **Terms that are never translated.** sigiro (always lowercase, never 시기로),
 * OpenTelemetry, OTLP, SQL, Docker, Kubernetes, every product and brand name,
 * every command, every URL, and every HTTP verb or path such as
 * `POST /api/orders` or `GET /openapi.json`. Brands are written verbatim in
 * Korean technical documentation, so translating them would be wrong even if it
 * were safe.
 *
 * **WARNING — the claim boundary, which is the thing nobody here can review.**
 * A deploy is 의심되는 배포 ("suspect deploy"), never 원인 ("cause"). sigiro
 * correlates a deploy in time with a shift, and correlation in time is not
 * cause — see docs/explanation/evidence.md, "What this design refuses to do".
 * Do not put 원인, 증명 or 입증 into any string on this page. The only place the
 * word "cause" appears anywhere in this feature is the `learn` playbook in
 * site.shared.ts, and that is a file the *reader* wrote after reaching their own
 * conclusion. What sigiro can claim is that every result carries the query that
 * made it, so the reader can check the result or disprove it. That is why
 * "Prove it yourself" is 직접 확인하십시오 ("check it yourself") rather than
 * anything built on 증명.
 *
 * **Cost claims name no competitor and quote no multiplier**, in any language.
 * The page sells the self-hosted product — free, running in five minutes — and
 * quotes **no hosted price at all**: hosted is an invited beta that an operator
 * sets up by hand, so a number there promises a self-serve signup that does not
 * exist. `promises[3]` therefore ends in an invitation to talk, not a price and
 * not a signup.
 *
 * **Never write 소프트웨어 공장 ("software factory").** That frame was considered
 * and rejected on 2026-08-01 because it describes our process and not the
 * reader's outcome. The frame is "software that repairs itself", rendered
 * throughout as 스스로 복구하는 소프트웨어.
 *
 * Commands, terminal output, icon slugs, URLs and the `ours` flags are NOT here.
 * They are in `site.shared.ts`, where a translation cannot reach them.
 */
export const KO: LandingCopy = {
  /** Keeps "observability" in the title: the h1 is a category bet with no search
      intent behind it, and this is the one place the category word still earns
      its keep for search and for citation. 옵저버빌리티 is the form Korean
      developers search for; 관측 가능성 is the calque and gets no traffic. */
  title: 'sigiro — 스스로 복구하는 소프트웨어를 위한 옵저버빌리티 데이터 레이어',

  /** The h1: the category claim. */
  tagline: '스스로 복구하는 소프트웨어를 위한 데이터 레이어.',

  /** Also the meta description, the OG description and the Twitter card, so it
      has to stay under ~155 characters or search results cut it mid-sentence.

      It says 의심되는 배포 ("suspect deploy") and not 원인 ("cause"). sigiro
      correlates a deploy in time with a shift, and correlation in time is not
      cause — see docs/explanation/evidence.md, "What this design refuses to do".
      The claim the product can keep is that each result carries the query that
      made it, so you can check the result or disprove it. Do not put 원인, 증명
      or 입증 back into this string. */
  description:
    'OpenTelemetry를 sigiro로 보내십시오. 서비스에 장애가 발생하면 순위가 매겨진 결과와 의심되는 배포, 각 결과의 쿼리를 받습니다.',

  /** Nav labels. The hrefs are in site.shared.ts.

      "API" stays in Latin script, as it does in Korean technical documentation.
      It is also the shortest honest word: this nav is lowercase `.meta` links in
      a footer row, so "API 레퍼런스" would not fit the register or the row. */
  nav: {
    docs: '문서',
    reference: 'API',
  },

  /** The hero's second action. 문의하기 ("get in touch"), not 데모 신청
      ("book a demo"): there is no demo, since the product is a binary you run in
      one command, and "demo" promises a sales call that nobody here is staffed
      to give. 상담 was rejected for the same reason — it implies a sales desk. */
  callLabel: '문의하기',

  /**
   * Strings that were hardcoded in the markup before the landing page was
   * localized.
   *
   * They were found by extracting every text node from `pages/index.astro` and
   * its components after the first split, which is worth recording: a copy
   * inventory taken from `site.ts` alone missed the hero's primary button and
   * every section heading — the most visible text on the page. Anything a
   * reader sees belongs here, not in the markup.
   *
   * `howTail` lists the four step names, so it has to use the same four words as
   * `how.*.title` below: 연결, 질문, 상관관계, 확인. If a step title changes,
   * change it here too.
   */
  ui: {
    /** The hero's primary action, and the same label in the header. */
    getStarted: '시작하기',
    signUp: '가입하기',
    installCli: 'CLI 설치',
    /** The line under the hero loop. Restates the two claims that decide
        whether a reader keeps reading: it is free self-hosted, and nothing
        leaves their infrastructure. Wording matches `promises[3]`, and it
        promises no signup — there isn't one. */
    heroNote: '직접 설치하면 무료입니다. sigiro는 저희에게 어떤 데이터도 보내지 않습니다.',
    /** The stepped-terminal section. `stepsAria` labels the step list for
        screen readers; `shellAria` names the transcript region. */
    howLabel: '사용 방법',
    howTitle: '네 단계.',
    howTail: '연결하고, 질문하고, 상관관계를 보고, 확인합니다.',
    howLede:
      'OpenTelemetry를 sigiro로 보냅니다. 질문 하나를 던집니다. 아래의 각 단계는 셸 세션 하나입니다.',
    stepsAria: '네 단계',
    shellAria: '사용자의 셸',
    /** The integrations section. 마이그레이션 불필요 ("No migration") is a claim:
        sigiro speaks OTLP, so the reader keeps the instrumentation they already
        have. */
    integrationsLabel: '얻는 것',
    integrationsTitle: '마이그레이션 불필요.',
    integrationsTail: 'sigiro는 이미 사용 중인 도구에 연결됩니다.',
  },

  /**
   * Connect, ask, correlate, prove — the four steps of the stepped terminal.
   *
   * `title` is the step name in the caption; `say` is the sentence above the
   * transcript. The transcripts themselves are in site.shared.ts.
   */
  how: {
    connect: {
      title: '연결',
      say: 'sigiro는 표준 포트에서 표준 OTLP를 사용합니다.',
    },
    ask: {
      title: '질문',
      /* 의심되는 배포 ("the suspect deploy"), not 장애를 일으킨 배포 ("the
         deploy that caused the failure"). The payload says `"suspect"`, and the
         prose has to say what the payload says. sigiro finds a deploy near the
         shift in time; a person or an agent decides whether that deploy is the
         cause. 지목합니다 ("points to") is deliberate — it names without
         concluding. */
      say: 'sigiro는 결과의 순위를 매기고 의심되는 배포를 지목합니다.',
    },
    correlate: {
      title: '상관관계',
      say: '에이전트는 모델을 호출하는 서비스입니다. 트레이스 하나에 둘 다 들어 있습니다.',
    },
    prove: {
      /* 확인 ("check"), not 증명 ("prove"). The English title is "Prove", but
         the sentence under it is about the reader running, widening and
         breaking the query — the reader's check, not our proof. 검증 is reserved
         for `loop.verify`, which is the pipeline stage of that name. */
      title: '확인',
      say: '쿼리를 다시 실행하십시오. 범위를 넓히십시오. 결과가 틀렸음을 보이십시오.',
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
    n: '01',
    lead: '탐색.',
    rest: '에이전트가 도움 없이 API 설명을 읽습니다.',
    say: '공개 문서 하나가 모든 엔드포인트를 설명합니다.',
    artifactTitle: 'GET /openapi.json',
    artifactMeta: '공개, 키 불필요',
  },

  /**
   * The factory loop. `owner` is rendered verbatim under each stage, so it has
   * to name a real thing a reader already runs.
   *
   * The positioning in one object: the loop is not closed until every stage is
   * covered, and sigiro covers two of them plus the evidence for a third. Saying
   * which stages are *not* ours is the point. Do not label this section
   * 소프트웨어 공장 anywhere it is rendered — see the file header.
   *
   * The five stage names are the canonical Korean set: 탐지, 진단, 수정, 검증,
   * 학습.
   */
  loop: {
    detect: {
      title: '탐지',
      owner: 'sigiro',
      say: 'sigiro는 각 서비스를 그 서비스의 이력과 비교합니다. 임계값은 설정하지 않아도 됩니다.',
    },
    diagnose: {
      title: '진단',
      owner: 'sigiro',
      say: '한 번의 호출로 순위가 매겨진 결과와 의심되는 배포, 각 결과의 쿼리를 받습니다.',
    },
    fix: {
      title: '수정',
      owner: '사용자의 에이전트',
      say: '패치는 사용자의 에이전트가 작성합니다. sigiro는 이 단계를 수행하지 않습니다.',
    },
    verify: {
      title: '검증',
      owner: '사용자의 파이프라인, sigiro의 증거로',
      say: '테스트 통과는 서비스 회복과 같지 않습니다. 다시 질문하고, 무엇이 사라졌는지 읽으십시오.',
    },
    learn: {
      title: '학습',
      owner: '사용자의 메모리',
      say: '수정 내용을 증거와 함께 보관하십시오. 같은 장애가 다시 발생하면 그 기록을 사용하십시오.',
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
      /* 직접 확인하십시오 ("check it yourself"), not 결과마다 증명 ("proof with
         each result"). The old English title put the proof on us, and sigiro
         proves nothing: it measures a shift and names a deploy near it in time.
         What you get is a result you can check, and the check is yours to run. */
      title: '직접 확인하십시오',
      body: '모든 결과에는 그 결과를 만든 쿼리가 들어 있습니다.',
    },
    {
      title: '유지 관리할 것이 없습니다',
      body: '임계값도 규칙도 설정하지 않습니다. sigiro는 각 서비스의 이력을 사용합니다.',
    },
    {
      /* 명령어 하나 ("one command"), not 컨테이너 하나 ("one container"): the
         binary is the lead install path now that `/install` works. */
      title: '5분이면 설치가 끝납니다',
      body: '명령어 하나와 환경 변수 두 개면 됩니다. 코드는 바꾸지 않습니다.',
    },
    {
      /* The site sells the self-hosted product. Hosted is a hand-built beta, so
         it gets a conversation rather than a price: quoting a monthly figure
         invited a reader to sign up for something with no signup. Keep the
         second sentence an invitation to talk — no number, no 가입, no 무료 체험. */
      title: '직접 설치하면 무료입니다',
      body: '호스트 수, 사용자 수, 데이터 양에 따른 요금이 없습니다. 저희가 대신 운영하기를 원하시면 문의해 주십시오.',
    },
    {
      title: '데이터는 사용자 환경에 남습니다',
      body: 'sigiro는 사용자의 서버에 설치합니다. sigiro는 저희에게 어떤 데이터도 보내지 않습니다.',
    },
    {
      title: 'sigiro가 sigiro를 모니터링합니다',
      body: 'sigiro는 자체 텔레메트리를 sigiro로 보냅니다. 저희는 이렇게 저희 장애를 찾습니다.',
    },
  ],

  /**
   * The three integration group labels, plus the one prose item that closes the
   * "Read by" group. Every other entry in those groups is a product name and
   * lives in site.shared.ts.
   *
   * The three labels are parallel by construction — 보내는 곳 / 실행하는 곳 /
   * 읽는 곳 — because they sit in a row above three grids of the same shape.
   */
  integrations: {
    emitLabel: '텔레메트리를 보내는 곳',
    runLabel: '실행하는 곳',
    readLabel: '결과를 읽는 곳',
    readAnySql: 'SQL을 쓰는 모든 도구',
  },

  /**
   * The pool of questions handed to the reader's own assistant. One is drawn at
   * random per link, per page load.
   *
   * **One use case each, one line each, and each in the reader's voice.** These
   * are not questions about sigiro written by us — they are the situation the
   * reader arrived with, so they are first person (저/제) and they end in a
   * question. A reader who opens two gets two facets rather than the same
   * paragraph twice.
   *
   * The situations are not invented here. They are the use cases and personas in
   * `.agents/product-marketing.md`. Adding a line means finding the persona it
   * serves, not thinking of a new question.
   *
   * Do not write 소프트웨어 공장 ("software factory"). That frame was considered
   * and **rejected** on 2026-08-01 because it describes our process and not the
   * reader's outcome; the frame is 스스로 복구하는 소프트웨어.
   *
   * Three rules bind every entry, and a variant that breaks any one of them
   * quietly breaks the feature:
   *
   * 1. The three public URLs are prepended by `site.ts` from the shared
   *    `READ_ALL_THREE`, so they are never retyped here or in a translation. No
   *    entry below contains a URL.
   * 2. Claim nothing the product does not do — including naming no competitor,
   *    which is why none of these ask for a comparison against a named rival.
   *    Ask questions; do not assert features. A statement that slipped in here
   *    becomes a claim the assistant repeats as ours.
   * 3. Quote no price. The one entry that raises cost asks what sigiro costs; it
   *    does not answer. Hosted is a call rather than a signup, so a number here
   *    would invite the assistant to fill in one from training data.
   */
  askAi: [
    // Coding agent in a loop — the primary use case in product-marketing.md.
    '제 코딩 에이전트는 프로덕션을 볼 수 없어서 짐작으로 패치를 작성합니다. sigiro가 그 루프를 닫아 줄 수 있습니까?',
    '저는 에이전트가 장애를 탐지하고, 패치하고, 수정이 효과가 있었는지 확인하기를 원합니다. sigiro는 그 루프의 어디에 들어갑니까?',
    '대시보드가 줄 수 없는 어떤 증거를 sigiro는 에이전트에게 건네줍니까?',

    // Language and stack entry points — the "I have an X app" shape.
    '계측이 전혀 없는 Go 서비스가 있습니다. sigiro로 디버깅하려면 무엇을 해야 합니까?',
    '이미 OpenTelemetry를 다른 곳으로 보내고 있는 Node 앱이 있습니다. 이것을 sigiro로 돌리면 무엇이 달라집니까?',
    '서버 한 대에서 서비스를 몇 개 운영합니다. sigiro는 과한 편입니까, 아니면 적당한 편입니까?',

    // LLM app debugging — a named use case.
    '제 LLM 앱이 멈추는데 어떤 모델 호출 때문인지 알 수 없습니다. sigiro가 그것을 보여 줍니까?',

    // 3am incident with an agent on call.
    '새벽 3시에 프로덕션이 망가졌는데 깨어 있는 사람이 없었습니다. sigiro라면 예약 실행된 에이전트에게 무엇을 알려 주었겠습니까?',

    // Solo founder persona: price and setup time.
    '저는 혼자 개발하고 배포해서 사용자 수 기준 옵저버빌리티 요금을 감당할 근거가 없습니다. sigiro는 저에게 비용이 얼마나 듭니까?',
    'sigiro를 설정하는 데 얼마나 걸리고, 그 뒤에는 무엇을 유지 관리해야 합니까?',

    // Privacy-conscious persona: data ownership.
    '고객 텔레메트리가 제 인프라 밖으로 나가지 않기를 바랍니다. sigiro는 그것을 어디에 저장합니까?',

    // Small platform team: dashboard rot.
    '아무도 보지 않는 대시보드에 매주 몇 시간씩 씁니다. sigiro는 무엇을 대체합니까?',

    // The honest one. A reader who asks this and gets a straight answer trusts
    // the rest.
    'sigiro가 맞지 않는 경우는 언제이고, 아직 하지 못하는 일은 무엇입니까?',
  ],
} as const;
