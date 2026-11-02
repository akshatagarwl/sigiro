import { Data, Effect } from "effect";
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import bash from "shiki/langs/bash.mjs";
import json from "shiki/langs/json.mjs";
import shellsession from "shiki/langs/shellsession.mjs";
import sql from "shiki/langs/sql.mjs";
import ts from "shiki/langs/typescript.mjs";
import yaml from "shiki/langs/yaml.mjs";
import githubDark from "shiki/themes/github-dark-default.mjs";
import githubLight from "shiki/themes/github-light-default.mjs";

export type Lang =
  | "json"
  | "sql"
  | "bash"
  | "typescript"
  | "yaml"
  | "shellsession";

/**
 * The highlighter could not be built. This is the failure this file's whole
 * shape exists to avoid — see the note on `started` — so it is worth a name of
 * its own rather than being folded into "highlighting failed".
 *
 * Recoverable: `degrade` turns it into plain text. Colour is the only thing the
 * reader loses.
 */
class ShikiUnavailable extends Data.TaggedError("ShikiUnavailable")<{
  readonly cause: unknown;
}> {}

/** One string, in one grammar, that Shiki refused. Recoverable, as above. */
class HighlightFailed extends Data.TaggedError("HighlightFailed")<{
  readonly lang: Lang;
  readonly cause: unknown;
}> {}

/**
 * A transcript came back with a different number of lines than it went in with.
 *
 * The one failure here that is NOT recovered, deliberately. The other two are
 * conditions — a runtime that will not run the WASM, a grammar that chokes on a
 * string — and the honest response to a condition is to do less and keep going.
 * This one is a broken invariant: `SteppedTerminal` pairs `text` with `html`
 * positionally and sizes its frame from the count, so a transcript one line
 * short does not look broken, it looks like a different transcript. There is no
 * degraded version of "the output is silently wrong". It stays in
 * `highlightSession`'s error channel, `Effect.runPromise` rethrows it, and the
 * page 500s — which is the loud failure the project rule asks for and, since
 * every transcript is a literal in `lib/site.ts`, it is a failure that can only
 * be caused by a commit rather than by a visitor.
 */
class TranscriptMismatch extends Data.TaggedError("TranscriptMismatch")<{
  readonly reason: string;
}> {}

/**
 * Shiki, built for the Workers runtime.
 *
 * The convenient `codeToHtml` from `shiki` pulls the oniguruma WASM engine, which
 * throws in Cloudflare Workers — the page 500s there while rendering fine under
 * Node. `createJavaScriptRegexEngine` uses the platform's own RegExp instead, and
 * the fine-grained imports keep only the three grammars and one theme we use
 * rather than every language Shiki ships.
 *
 * Note this runs per request, not at build: vinext reports these routes as
 * static but does not emit HTML files, so the Worker renders them. Hence the
 * module-level singleton — the highlighter is built once per isolate, not once
 * per request.
 */
const started: Promise<HighlighterCore> = createHighlighterCore({
  themes: [githubLight, githubDark],
  langs: [json, sql, bash, ts, yaml, shellsession],
  engine: createJavaScriptRegexEngine(),
});

/**
 * The singleton, as an effect. Awaiting a settled promise repeatedly is free, so
 * this stays one build per isolate while giving every highlight below a typed
 * `ShikiUnavailable` channel instead of a rejection nobody named.
 */
const core: Effect.Effect<HighlighterCore, ShikiUnavailable> =
  Effect.tryPromise({
    try: () => started,
    catch: (cause) => new ShikiUnavailable({ cause }),
  });

/**
 * Two of the light theme's token colours land just under AA at our code size —
 * the comment grey at 4.0:1 and the keyword red at 4.49:1 against 4.5 required.
 * `colorReplacements` is Shiki's own hook for this, so the theme is adjusted
 * rather than the output patched afterwards. Also used by the docs, via
 * source.config.ts.
 *
 * Keys must be lowercase. Shiki lowercases a token's colour before looking it
 * up, so a replacement written the way the theme writes it — `#6E7781` — is
 * silently never applied.
 */
export const COLOR_REPLACEMENTS = {
  "#6e7781": "#5a6169",
  "#cf222e": "#b3121f",
};

/**
 * Two themes and `defaultColor: false`, so Shiki writes each token's colour as a
 * `--shiki-light` / `--shiki-dark` pair instead of an inline `color`. globals.css
 * reads whichever side matches the material the block is cut into — with a single
 * theme the colour is inline, wins over any stylesheet, and a dark well ends up
 * wearing light-mode syntax.
 */
const THEMES = {
  light: "github-light-default",
  dark: "github-dark-default",
} as const;

const ESCAPES = new Map([
  ["&", "&amp;"],
  ["<", "&lt;"],
  [">", "&gt;"],
  ['"', "&quot;"],
  ["'", "&#39;"],
]);

/**
 * HTML-escape a string bound for `dangerouslySetInnerHTML`.
 *
 * Everything this file highlights today is a literal committed to this repo —
 * `lib/site.ts`, `openapi.json`, an assembled curl line — so nothing here is
 * attacker-controlled and this function has no work to do. It escapes anyway,
 * because the fallback below takes the exact same route into the DOM that
 * Shiki's output does, and Shiki escapes its own token text. Highlighting is
 * what makes the highlighted path safe; without this, the *unhighlighted* path
 * would be the one that renders a `<` as markup. The day someone renders a
 * `code` prop that came off a request, the difference between the two paths is
 * a stored XSS, and "the inputs happen to be literals" is not a property the
 * compiler is checking for us.
 */
function escapeHtml(text: string): string {
  return text.replace(
    /[&<>"']/g,
    (character) => ESCAPES.get(character) ?? character
  );
}

/**
 * Shiki's `pre` carries `--shiki-light` / `--shiki-dark` for the theme's default
 * foreground, and globals.css resolves `color: var(--shiki-light)` off it. A
 * shell without them computes an invalid colour, so the plain shells declare the
 * pair too — pointed at our own ink, which is what code with no syntax in it
 * should be in either material. Nested `var()` is fine; `OUTPUT_INK` already
 * relies on it.
 */
const PLAIN_INK =
  "--shiki-light:var(--foreground);--shiki-dark:var(--foreground)";

/**
 * `highlight`'s output, minus the colour: the same `pre.shiki > code > span.line`
 * Shiki emits, so `CodeBlock`, the docs' fence styling and the copy cap all keep
 * working on it and the block does not move on the page.
 */
function plainBlock(code: string): string {
  const lines = code
    .split("\n")
    .map((line) => `<span class="line">${escapeHtml(line)}</span>`)
    .join("\n");
  return `<pre class="shiki shiki-themes" style="${PLAIN_INK}" tabindex="0"><code>${lines}</code></pre>`;
}

/** `inlineLines`' output, minus the colour: one escaped span per line. */
function plainInline(code: string): string[] {
  return code
    .split("\n")
    .map((line) => `<span style="${PLAIN_INK}">${escapeHtml(line)}</span>`);
}

/**
 * Recovery, in one place.
 *
 * Both producers below run through this, so no server component repeats a catch
 * and — more to the point — none can forget one: after this, `ShikiUnavailable`
 * and `HighlightFailed` are not in any exported signature, so there is nothing
 * left at the five boundaries to handle. A highlighter that will not start is a
 * page rendered in one colour, not a 500.
 *
 * It is not silent. `Effect.logError` puts the tagged error, cause and all, on
 * the Worker's stderr, so a degraded render is something you find in the logs
 * rather than something you notice months later because the site looks flat.
 */
function degrade<A>(
  effect: Effect.Effect<A, ShikiUnavailable | HighlightFailed>,
  plain: () => A,
  what: string
): Effect.Effect<A> {
  return Effect.catch(effect, (error) =>
    Effect.as(
      Effect.logError(
        `highlight: falling back to plain text for ${what}`,
        error
      ),
      plain()
    )
  );
}

/**
 * One block of code, highlighted — or, if Shiki cannot, the same block in plain
 * escaped text inside the same shell.
 *
 * This returns a description rather than a promise: nothing runs until a server
 * component hands it to `Effect.runPromise`. The error channel is now empty by
 * construction, which is the point of `degrade` — the two things that can go
 * wrong are named, caught and logged here, once, instead of at five call sites
 * or, as before, nowhere.
 */
export function highlight(code: string, lang: Lang): Effect.Effect<string> {
  const shiki = Effect.flatMap(core, (shiki) =>
    Effect.try({
      try: () =>
        shiki.codeToHtml(code, {
          lang,
          themes: THEMES,
          defaultColor: false,
          colorReplacements: COLOR_REPLACEMENTS,
        }),
      catch: (cause) => new HighlightFailed({ lang, cause }),
    })
  );
  return degrade(shiki, () => plainBlock(code), `a ${lang} block`);
}

/** One line of a shell session: its plain text, and its highlighted markup. */
export type SessionLine = { text: string; html: string };

/**
 * `structure: 'inline'` drops the `pre`/`code`/`.line` wrappers and separates
 * lines with `<br>`, which is the one Shiki structure a component can hand out
 * one line at a time. The caller puts each line in its own box.
 *
 * Degrades here rather than in `highlightSession`, and that placement is what
 * keeps the two kinds of failure apart: `plainInline` splits the same string on
 * the same newline, so a fallback returns exactly the line count the caller
 * counted. A `TranscriptMismatch` therefore still means what it says — the
 * transcript really is a different shape — and never just "Shiki was down".
 */
function inlineLines(
  code: string,
  lang: Lang,
  colorReplacements: Record<string, string>
): Effect.Effect<string[]> {
  const shiki = Effect.flatMap(core, (shiki) =>
    Effect.try({
      try: () =>
        shiki
          .codeToHtml(code, {
            lang,
            themes: THEMES,
            defaultColor: false,
            structure: "inline",
            colorReplacements,
          })
          .split("<br>"),
      catch: (cause) => new HighlightFailed({ lang, cause }),
    })
  );
  return degrade(shiki, () => plainInline(code), `a ${lang} transcript line`);
}

/**
 * shellsession paints every output line one colour — `meta.output` — and the
 * theme's is a loud blue. Output is not syntax and should not read as any, so
 * both themes' output colour is remapped onto the ink the well already used for
 * it. Nested `var()` is fine: globals.css resolves
 * `color: var(--shiki-light)` to `var(--muted-foreground)`, which flips with the theme by
 * itself, so one value serves both sides.
 */
const OUTPUT_INK = {
  "#0550ae": "var(--muted-foreground)",
  "#79c0ff": "var(--muted-foreground)",
};

/** The prompt, in the muted ink the stacked fallback has always given it. */
const PROMPT =
  '<span style="--shiki-light:var(--muted-foreground);--shiki-dark:var(--muted-foreground)">$ </span>';

/**
 * What a continuation row of a multi-line command starts with: blank space the
 * width of the prompt, so the command's own indentation lines up under it.
 */
const CONTINUATION = "<span>  </span>";

/**
 * A `$ command` + output transcript, highlighted line by line.
 *
 * Two grammars rather than the one shellsession looks like it should be. Its
 * command rule hands the text after the prompt to `source.shell` through a
 * *capture*, and shell's statement patterns are `^`-anchored, so they cannot
 * match at a non-zero offset — the whole command comes back as a single
 * unstyled token. (Confirmed under both regex engines; it is the grammar, not
 * the engine.) So the command goes through `bash`, which scopes the command
 * name, its options and its arguments, and only the output goes through
 * shellsession, which is the thing shellsession does get right: every line is
 * output, whatever it happens to look like.
 *
 * The only fallible thing left on the site. `TranscriptMismatch` stays in the
 * channel on purpose — see the note on the class for why this one failure is not
 * degraded — so a caller still has to run it somewhere that can reject.
 */
export function highlightSession(
  cmd: string,
  out: string
): Effect.Effect<SessionLine[], TranscriptMismatch> {
  return Effect.gen(function* () {
    // A long command is written across lines (a `\` continuation, or a quoted
    // SQL string broken at its clauses), so its payoff is on screen instead of
    // past the right edge of the well. Each row gets its own line here.
    const cmdTexts = cmd.split("\n");
    const texts = out.split("\n");
    const [cmdHtml, outHtml] = yield* Effect.all(
      [
        inlineLines(cmd, "bash", COLOR_REPLACEMENTS),
        inlineLines(out, "shellsession", {
          ...COLOR_REPLACEMENTS,
          ...OUTPUT_INK,
        }),
      ],
      { concurrency: "unbounded" }
    );

    // The terminal pairs `text` with `html` positionally and sizes its frame from
    // the count, so a line that went missing would silently shift the transcript.
    // A typed failure rather than a `throw`: it still takes the render down, which
    // is the point, but the shape of the mistake is now in the function's type.
    if (cmdHtml.length !== cmdTexts.length) {
      return yield* new TranscriptMismatch({
        reason: `highlighted ${cmdHtml.length} of ${cmdTexts.length} command lines`,
      });
    }
    if (outHtml.length !== texts.length) {
      return yield* new TranscriptMismatch({
        reason: `highlighted ${outHtml.length} of ${texts.length} output lines`,
      });
    }

    return [
      ...cmdTexts.map((text, i) =>
        i === 0
          ? { text: `$ ${text}`, html: PROMPT + cmdHtml[i] }
          : { text: `  ${text}`, html: CONTINUATION + cmdHtml[i] }
      ),
      ...texts.map((text, i) => ({ text, html: outHtml[i] })),
    ];
  });
}
