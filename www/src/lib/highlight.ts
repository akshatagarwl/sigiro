import { createHighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import bash from 'shiki/langs/bash.mjs';
import ts from 'shiki/langs/typescript.mjs';
import yaml from 'shiki/langs/yaml.mjs';
import json from 'shiki/langs/json.mjs';
import sql from 'shiki/langs/sql.mjs';
import githubDark from 'shiki/themes/github-dark-default.mjs';
import githubLight from 'shiki/themes/github-light-default.mjs';

export type Lang = 'json' | 'sql' | 'bash' | 'typescript' | 'yaml';

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
  langs: [json, sql, bash, ts, yaml],
  engine: createJavaScriptRegexEngine(),
});

/**
 * Two of the light theme's token colours land just under AA at our code size —
 * the comment grey at 4.0:1 and the keyword red at 4.49:1 against 4.5 required.
 * `colorReplacements` is Shiki's own hook for this, so the theme is adjusted
 * rather than the output patched afterwards. Also used by the docs, via
 * source.config.ts.
 */
export const COLOR_REPLACEMENTS = { '#6e7781': '#5a6169', '#cf222e': '#b3121f' };

/**
 * Two themes and `defaultColor: false`, so Shiki writes each token's colour as a
 * `--shiki-light` / `--shiki-dark` pair instead of an inline `color`. globals.css
 * reads whichever side matches the material the block is cut into — with a single
 * theme the colour is inline, wins over any stylesheet, and a dark well ends up
 * wearing light-mode syntax.
 */
export async function highlight(code: string, lang: Lang): Promise<string> {
  const shiki = await highlighter;
  return shiki.codeToHtml(code, {
    lang,
    themes: { light: 'github-light-default', dark: 'github-dark-default' },
    defaultColor: false,
    colorReplacements: COLOR_REPLACEMENTS,
  });
}
