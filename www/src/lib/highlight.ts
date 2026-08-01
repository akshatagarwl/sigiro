import { createHighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import bash from 'shiki/langs/bash.mjs';
import shellsession from 'shiki/langs/shellsession.mjs';
import ts from 'shiki/langs/typescript.mjs';
import yaml from 'shiki/langs/yaml.mjs';
import json from 'shiki/langs/json.mjs';
import sql from 'shiki/langs/sql.mjs';
import githubDark from 'shiki/themes/github-dark-default.mjs';
import githubLight from 'shiki/themes/github-light-default.mjs';

export type Lang = 'json' | 'sql' | 'bash' | 'typescript' | 'yaml' | 'shellsession';

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
const highlighter = createHighlighterCore({
  themes: [githubLight, githubDark],
  langs: [json, sql, bash, ts, yaml, shellsession],
  engine: createJavaScriptRegexEngine(),
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
export const COLOR_REPLACEMENTS = { '#6e7781': '#5a6169', '#cf222e': '#b3121f' };

/**
 * Two themes and `defaultColor: false`, so Shiki writes each token's colour as a
 * `--shiki-light` / `--shiki-dark` pair instead of an inline `color`. globals.css
 * reads whichever side matches the material the block is cut into — with a single
 * theme the colour is inline, wins over any stylesheet, and a dark well ends up
 * wearing light-mode syntax.
 */
const THEMES = { light: 'github-light-default', dark: 'github-dark-default' } as const;

export async function highlight(code: string, lang: Lang): Promise<string> {
  const shiki = await highlighter;
  return shiki.codeToHtml(code, {
    lang,
    themes: THEMES,
    defaultColor: false,
    colorReplacements: COLOR_REPLACEMENTS,
  });
}

/** One line of a shell session: its plain text, and its highlighted markup. */
export type SessionLine = { text: string; html: string };

/**
 * `structure: 'inline'` drops the `pre`/`code`/`.line` wrappers and separates
 * lines with `<br>`, which is the one Shiki structure a component can hand out
 * one line at a time. The caller puts each line in its own box.
 */
async function inlineLines(
  code: string,
  lang: Lang,
  colorReplacements: Record<string, string>,
): Promise<string[]> {
  const shiki = await highlighter;
  const html = shiki.codeToHtml(code, {
    lang,
    themes: THEMES,
    defaultColor: false,
    structure: 'inline',
    colorReplacements,
  });
  return html.split('<br>');
}

/**
 * shellsession paints every output line one colour — `meta.output` — and the
 * theme's is a loud blue. Output is not syntax and should not read as any, so
 * both themes' output colour is remapped onto the ink the well already used for
 * it (§3: no decorative colour). Nested `var()` is fine: globals.css resolves
 * `color: var(--shiki-light)` to `var(--ink-2)`, which flips with the theme by
 * itself, so one value serves both sides.
 */
const OUTPUT_INK = { '#0550ae': 'var(--ink-2)', '#79c0ff': 'var(--ink-2)' };

/** The prompt, in the muted ink the stacked fallback has always given it. */
const PROMPT = '<span style="--shiki-light:var(--ink-3);--shiki-dark:var(--ink-3)">$ </span>';

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
 */
export async function highlightSession(cmd: string, out: string): Promise<SessionLine[]> {
  const texts = out.split('\n');
  const [cmdHtml, outHtml] = await Promise.all([
    inlineLines(cmd, 'bash', COLOR_REPLACEMENTS),
    inlineLines(out, 'shellsession', { ...COLOR_REPLACEMENTS, ...OUTPUT_INK }),
  ]);

  // The terminal pairs `text` with `html` positionally and sizes its frame from
  // the count, so a line that went missing would silently shift the transcript.
  if (cmdHtml.length !== 1) throw new Error(`transcript command is not one line: ${cmd}`);
  if (outHtml.length !== texts.length) {
    throw new Error(`highlighted ${outHtml.length} of ${texts.length} output lines`);
  }

  return [
    { text: `$ ${cmd}`, html: PROMPT + cmdHtml[0] },
    ...texts.map((text, i) => ({ text, html: outHtml[i] })),
  ];
}
