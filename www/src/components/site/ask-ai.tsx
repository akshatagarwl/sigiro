/**
 * `sigiro-icons` is `simple-icons` under an alias, and the alias is load-bearing.
 *
 * Blume depends on `simple-icons@^13` for the language marks on its code fences,
 * and this page needs 16 — `siClaude`, `siGooglegemini` and `siPerplexity` are
 * brands that did not exist in 13. Two majors of one package in one project.
 *
 * Bun installs both correctly (16 at the root, 13 nested under `blume/`), but
 * Vite resolves Blume's own `simple-icons` import to the hoisted root copy
 * regardless, and 16 renamed `siCss3` to `siCss` — so Blume's fences failed to
 * build. Aliasing our side means the bare specifier belongs to Blume alone and
 * each gets the major it was written against.
 */
import { siClaude, siDuckduckgo, siGooglegemini, siMistralai, siPerplexity } from 'sigiro-icons';
import { ASK_AI_PROMPT, SITE } from '@/lib/site';

/**
 * Seven links that open the reader's own assistant with one question already
 * typed, pointed at our docs.
 *
 * This is not a chat widget. A chat widget is a model we host, a bill we pay, a
 * prompt we can tune until it stops saying the inconvenient thing, and a reader
 * who discounts every word of it for exactly that reason. These links spend
 * nothing and are worth more: the answer arrives from a party with no stake in
 * it, in a thread the reader can keep interrogating after they leave.
 *
 * The question is `ASK_AI_PROMPT` in site.ts, since it is copy.
 *
 * Every provider takes the prompt in the query string. The parameter name is the
 * only thing that differs, so the whole integration is one `encodeURIComponent`
 * and a template — there is no SDK here and there should never be one.
 *
 * Gemini has no first-party prefill endpoint. `google.com/search?udm=50` is
 * Google's own AI Mode, which is where a prefilled Gemini query actually lands.
 *
 * A provider earns a place here only if the prefilled URL answers for a reader
 * who is not signed in. A link that spends the reader's click on a signup form,
 * or that opens an empty composer, costs us the trust this row exists to earn.
 * Two groups fail that test and were measured failing it, so read this before
 * adding either back:
 *
 * - No prefill at any parameter name. `chat.z.ai` (GLM) drops the query string
 *   on redirect; `kimi.com`, `chat.qwen.ai` and `agent.minimax.io` keep it and
 *   ignore it. `q`, `prompt` and `query` were each tried against all four.
 * - A sign-in wall. `exa.ai/search` and `platform.parallel.ai/play/search` both
 *   redirect to one. `meta.ai/?prompt=` is the near miss — it does fill the
 *   composer, then asks for an account the moment you send.
 */
const MARKS = {
  /**
   * OpenAI and xAI are not in `simple-icons`: both marks were removed from the
   * set after the trademark holders asked, so there is nothing to import and a
   * version bump will not bring them back. The paths are inlined from each
   * company's own published mark, unmodified and monochrome, and they name the
   * destination of the link — the same use as a "Sign in with Google" button.
   *
   * Each keeps its native viewBox. Rescaling a path by hand to reach a shared
   * 24×24 grid is how a logo ends up subtly wrong.
   */
  chatgpt: {
    viewBox: '0 0 256 260',
    d: [
      'M239.184 106.203a64.716 64.716 0 0 0-5.576-53.103C219.452 28.459 191 15.784 163.213 21.74A65.586 65.586 0 0 0 52.096 45.22a64.716 64.716 0 0 0-43.23 31.36c-14.31 24.602-11.061 55.634 8.033 76.74a64.665 64.665 0 0 0 5.525 53.102c14.174 24.65 42.644 37.324 70.446 31.36a64.72 64.72 0 0 0 48.754 21.744c28.481.025 53.714-18.361 62.414-45.481a64.767 64.767 0 0 0 43.229-31.36c14.137-24.558 10.875-55.423-8.083-76.483Zm-97.56 136.338a48.397 48.397 0 0 1-31.105-11.255l1.535-.87 51.67-29.825a8.595 8.595 0 0 0 4.247-7.367v-72.85l21.845 12.636c.218.111.37.32.409.563v60.367c-.056 26.818-21.783 48.545-48.601 48.601Zm-104.466-44.61a48.345 48.345 0 0 1-5.781-32.589l1.534.921 51.722 29.826a8.339 8.339 0 0 0 8.441 0l63.181-36.425v25.221a.87.87 0 0 1-.358.665l-52.335 30.184c-23.257 13.398-52.97 5.431-66.404-17.803ZM23.549 85.38a48.499 48.499 0 0 1 25.58-21.333v61.39a8.288 8.288 0 0 0 4.195 7.316l62.874 36.272-21.845 12.636a.819.819 0 0 1-.767 0L41.353 151.53c-23.211-13.454-31.171-43.144-17.804-66.405v.256Zm179.466 41.695-63.08-36.63L161.73 77.86a.819.819 0 0 1 .768 0l52.233 30.184a48.6 48.6 0 0 1-7.316 87.635v-61.391a8.544 8.544 0 0 0-4.4-7.213Zm21.742-32.69-1.535-.922-51.619-30.081a8.39 8.39 0 0 0-8.492 0L99.98 99.808V74.587a.716.716 0 0 1 .307-.665l52.233-30.133a48.652 48.652 0 0 1 72.236 50.391v.205ZM88.061 139.097l-21.845-12.585a.87.87 0 0 1-.41-.614V65.685a48.652 48.652 0 0 1 79.757-37.346l-1.535.87-51.67 29.825a8.595 8.595 0 0 0-4.246 7.367l-.051 72.697Zm11.868-25.58 28.138-16.217 28.188 16.218v32.434l-28.086 16.218-28.188-16.218-.052-32.434Z',
    ],
  },
  grok: {
    viewBox: '0 0 1024 1024',
    d: [
      'M395.479 633.828L735.91 381.105C752.599 368.715 776.454 373.548 784.406 392.792C826.26 494.285 807.561 616.253 724.288 699.996C641.016 783.739 525.151 802.104 419.247 760.277L303.556 814.143C469.49 928.202 670.987 899.995 796.901 773.282C896.776 672.843 927.708 535.937 898.785 412.476L899.047 412.739C857.105 231.37 909.358 158.874 1016.4 10.6326C1018.93 7.11771 1021.47 3.60279 1024 0L883.144 141.651V141.212L395.392 633.916',
      'M325.226 695.251C206.128 580.84 226.662 403.776 328.285 301.668C403.431 226.097 526.549 195.254 634.026 240.596L749.454 186.994C728.657 171.88 702.007 155.623 671.424 144.2C533.19 86.9942 367.693 115.465 255.323 228.382C147.234 337.081 113.244 504.215 171.613 646.833C215.216 753.423 143.739 828.818 71.7385 904.916C46.2237 931.893 20.6216 958.87 0 987.429L325.139 695.339',
    ],
  },
  claude: { viewBox: '0 0 24 24', d: [siClaude.path] },
  duckai: { viewBox: '0 0 24 24', d: [siDuckduckgo.path] },
  gemini: { viewBox: '0 0 24 24', d: [siGooglegemini.path] },
  mistral: { viewBox: '0 0 24 24', d: [siMistralai.path] },
  perplexity: { viewBox: '0 0 24 24', d: [siPerplexity.path] },
} as const;

const PROVIDERS = [
  { name: 'ChatGPT', mark: MARKS.chatgpt, url: (q: string) => `https://chatgpt.com/?prompt=${q}` },
  { name: 'Claude', mark: MARKS.claude, url: (q: string) => `https://claude.ai/new?q=${q}` },
  /* DuckDuckGo's own mark, because Duck.ai has none of its own. It is the one
     provider here that answers without an account. */
  { name: 'Duck.ai', mark: MARKS.duckai, url: (q: string) => `https://duck.ai/?q=${q}` },
  {
    name: 'Gemini',
    mark: MARKS.gemini,
    url: (q: string) => `https://www.google.com/search?udm=50&q=${q}`,
  },
  { name: 'Grok', mark: MARKS.grok, url: (q: string) => `https://grok.com/?q=${q}` },
  /* Le Chat, named for the company: the mark is Mistral's and the product name
     means nothing to a reader who has only heard of the model. */
  {
    name: 'Mistral',
    mark: MARKS.mistral,
    url: (q: string) => `https://chat.mistral.ai/chat?q=${q}`,
  },
  {
    name: 'Perplexity',
    mark: MARKS.perplexity,
    url: (q: string) => `https://www.perplexity.ai/search?q=${q}`,
  },
] as const;

/**
 * Encoded once, at module scope. The prompt is a constant, and encoding it once
 * per provider per render to get seven identical strings is work with no result.
 */
const QUERY = encodeURIComponent(ASK_AI_PROMPT);

export function AskAI() {
  return (
    <div className="flex items-center gap-2">
      {/* `.meta` and not `.label`: this sits beside the footer's own text links,
          which are all `.meta` lowercase. An uppercase 600 section label here
          would announce itself louder than the nav it stands next to. */}
      <span className="meta text-ink-3">ask ai</span>
      <ul className="flex items-center gap-0.5">
        {PROVIDERS.map((provider) => (
          <li key={provider.name}>
            <a
              href={provider.url(QUERY)}
              target="_blank"
              rel="noopener"
              /* The accessible name says what the link does, not who it goes to:
                 "ChatGPT" alone leaves a screen reader user to guess whether
                 this is a logo, a partner, or a link off the site.

                 Sized as a 24px box around a 14px mark, matching the hit area
                 the footer's text links already get. */
              aria-label={`Ask ${provider.name} about ${SITE.name}`}
              title={`Ask ${provider.name} about ${SITE.name}`}
              className="press inline-flex size-6 items-center justify-center rounded-[5px] text-ink-3 hover:text-ink"
            >
              <svg
                viewBox={provider.mark.viewBox}
                width="14"
                height="14"
                fill="currentColor"
                aria-hidden
              >
                {provider.mark.d.map((d) => (
                  <path key={d} d={d} />
                ))}
              </svg>
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
