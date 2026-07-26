import { createHighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import bash from 'shiki/langs/bash.mjs';
import json from 'shiki/langs/json.mjs';
import sql from 'shiki/langs/sql.mjs';
import githubLight from 'shiki/themes/github-light-default.mjs';

export type Lang = 'json' | 'sql' | 'bash';

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
  themes: [githubLight],
  langs: [json, sql, bash],
  engine: createJavaScriptRegexEngine(),
});

export async function highlight(code: string, lang: Lang): Promise<string> {
  const shiki = await highlighter;
  return shiki.codeToHtml(code, { lang, theme: 'github-light-default' });
}
